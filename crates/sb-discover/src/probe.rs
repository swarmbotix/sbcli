//! Header probe: try to lift `metadata.{msg_type, msg_freq_desired}` out
//! of any Zenoh sample without knowing the wrapping message type.
//!
//! The trick: every Header-carrying message in the vault (the ros2
//! `std/*Stamped` types, the swarmbotix `images/*` and `sensors/*` types)
//! has its Header as field 1, so the outer message's wire layout always
//! starts with tag `0x0a` (field 1, length-delimited) followed by the
//! embedded Header bytes. We decode the outer payload as [`StampedProbe`]
//! (prost silently skips unknown fields, so the rest of the payload, such
//! as `data` or `image`, gets ignored) and read the metadata out of the
//! embedded Header.
//!
//! The two styles' Headers disagree on where `metadata` lives; see
//! [`HeaderProbe`] for how both are read.
//!
//! Defined locally rather than imported from `sb-vault`'s generated
//! bindings so this crate stays trivial (no build script, no proto deps).
//!
//! [`parse_wire_tag`] reads the other source of a sample's type: the Zenoh
//! encoding that generated publishers stamp
//! (`application/protobuf;<style>/<ns>/<Leaf>`, the FlatBuffers form, or a
//! raw entry's MIME type). `sb topic list` and `sb topic listen` share it so
//! they agree on what a topic carries.

use prost::bytes::{Buf, BufMut};
use prost::encoding::{self, DecodeContext, WireType};
use prost::{DecodeError, Message};

/// Header wire layout of both vault styles. They agree on fields 1 and 2
/// and disagree from field 3 on:
///
/// | tag | ros2 `std/Header` | swarmbotix `header/Header` |
/// |-----|-------------------|----------------------------|
/// | 1   | `timestamp_ns`    | `timestamp_ns`             |
/// | 2   | `frame_id`        | `frame_id`                 |
/// | 3   | `metadata`        | `seq` (`uint64`)           |
/// | 4   | (none)            | `metadata`                 |
///
/// Tag 3 is told apart by its wire type: length-delimited is the ros2
/// `metadata`, varint is the swarmbotix `seq`. A `#[derive(Message)]`
/// struct cannot do that, because prost fails the whole decode on a wire
/// type that disagrees with the declared field, so a swarmbotix Header
/// with a non-zero `seq` would never probe. [`Message`] is therefore
/// implemented by hand. [`HeaderProbe::into_metadata`] prefers tag 4.
#[derive(Clone, PartialEq, Default, Debug)]
pub struct HeaderProbe {
    pub timestamp_ns: u64,
    pub frame_id: String,
    /// swarmbotix `header/Header.seq` (tag 3, varint). `0` for the ros2
    /// layout, and for a swarmbotix publisher that leaves it unset.
    pub seq: u64,
    /// ros2 `std/Header.metadata` (tag 3, length-delimited).
    pub ros2_metadata: Option<MetadataProbe>,
    /// swarmbotix `header/Header.metadata` (tag 4).
    pub swarmbotix_metadata: Option<MetadataProbe>,
}

impl HeaderProbe {
    /// The Header's metadata, whichever layout carried it: tag 4 when
    /// present, else tag 3.
    pub fn into_metadata(self) -> Option<MetadataProbe> {
        self.swarmbotix_metadata.or(self.ros2_metadata)
    }
}

impl Message for HeaderProbe {
    fn encode_raw(&self, buf: &mut impl BufMut) {
        if self.timestamp_ns != 0 {
            encoding::uint64::encode(1, &self.timestamp_ns, buf);
        }
        if !self.frame_id.is_empty() {
            encoding::string::encode(2, &self.frame_id, buf);
        }
        if self.seq != 0 {
            encoding::uint64::encode(3, &self.seq, buf);
        }
        if let Some(meta) = &self.ros2_metadata {
            encoding::message::encode(3, meta, buf);
        }
        if let Some(meta) = &self.swarmbotix_metadata {
            encoding::message::encode(4, meta, buf);
        }
    }

    fn merge_field(
        &mut self,
        tag: u32,
        wire_type: WireType,
        buf: &mut impl Buf,
        ctx: DecodeContext,
    ) -> Result<(), DecodeError> {
        match (tag, wire_type) {
            (1, _) => encoding::uint64::merge(wire_type, &mut self.timestamp_ns, buf, ctx),
            (2, _) => encoding::string::merge(wire_type, &mut self.frame_id, buf, ctx),
            (3, WireType::LengthDelimited) => encoding::message::merge(
                wire_type,
                self.ros2_metadata
                    .get_or_insert_with(MetadataProbe::default),
                buf,
                ctx,
            ),
            (3, _) => encoding::uint64::merge(wire_type, &mut self.seq, buf, ctx),
            (4, _) => encoding::message::merge(
                wire_type,
                self.swarmbotix_metadata
                    .get_or_insert_with(MetadataProbe::default),
                buf,
                ctx,
            ),
            _ => encoding::skip_field(wire_type, tag, buf, ctx),
        }
    }

    fn encoded_len(&self) -> usize {
        let mut len = 0;
        if self.timestamp_ns != 0 {
            len += encoding::uint64::encoded_len(1, &self.timestamp_ns);
        }
        if !self.frame_id.is_empty() {
            len += encoding::string::encoded_len(2, &self.frame_id);
        }
        if self.seq != 0 {
            len += encoding::uint64::encoded_len(3, &self.seq);
        }
        if let Some(meta) = &self.ros2_metadata {
            len += encoding::message::encoded_len(3, meta);
        }
        if let Some(meta) = &self.swarmbotix_metadata {
            len += encoding::message::encoded_len(4, meta);
        }
        len
    }

    fn clear(&mut self) {
        *self = Self::default();
    }
}

/// `Header.Metadata`, identical in both styles.
#[derive(Clone, PartialEq, Message)]
pub struct MetadataProbe {
    #[prost(string, tag = "1")]
    pub topic_full_name: String,
    #[prost(string, tag = "2")]
    pub msg_type: String,
    #[prost(double, tag = "3")]
    pub msg_freq_desired: f64,
}

/// Generic Header-leading probe: any message that carries `Header` as
/// field 1. Other fields of the outer message are silently skipped by prost.
#[derive(Clone, PartialEq, Message)]
pub struct StampedProbe {
    #[prost(message, optional, tag = "1")]
    pub header: Option<HeaderProbe>,
}

/// Try to lift the Header metadata out of an arbitrary Header-leading
/// payload, in either style's layout. Returns `None` for foreign /
/// un-Stamped bytes (failed decode, no `header` field, no nested
/// `metadata`). The success path is the common one for sb-generated
/// publishers.
pub fn probe_header(payload: &[u8]) -> Option<MetadataProbe> {
    StampedProbe::decode(payload).ok()?.header?.into_metadata()
}

/// MIME type of a protobuf wire tag (`application/protobuf;<vault name>`).
pub const MIME_PROTOBUF: &str = "application/protobuf";
/// MIME type of a FlatBuffers wire tag (`application/flatbuffers;<vault name>`).
pub const MIME_FLATBUFFERS: &str = "application/flatbuffers";
/// MIME type of an opaque byte payload.
pub const MIME_OCTET_STREAM: &str = "application/octet-stream";

/// What a Zenoh sample's encoding says about its payload. See
/// [`parse_wire_tag`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum WireTag {
    /// `application/protobuf;<name>`: protobuf bytes of vault message
    /// `<name>` (`<style>/<ns>/<Leaf>`).
    Protobuf(String),
    /// `application/flatbuffers;<name>`: FlatBuffers bytes of vault message
    /// `<name>`.
    Flatbuffers(String),
    /// `text/<subtype>` or `application/json`: human-readable text. Holds
    /// the whole encoding string, parameters included
    /// (`text/plain;charset=utf-8`).
    Text(String),
    /// `application/octet-stream`: opaque bytes, declared as such.
    Octets,
    /// Anything else: Zenoh's default `zenoh/bytes` (an untagged sample),
    /// another MIME type, or a protobuf / FlatBuffers MIME type with no
    /// message name after the `;`.
    Unknown,
}

impl WireTag {
    /// The `sb topic list` type column for this tag: the vault name of a
    /// typed payload, the MIME type of a text one, `None` otherwise.
    pub fn type_label(&self) -> Option<&str> {
        match self {
            Self::Protobuf(name) | Self::Flatbuffers(name) => Some(name),
            Self::Text(mime) => Some(mime),
            Self::Octets | Self::Unknown => None,
        }
    }
}

/// Classify a rendered Zenoh encoding (`sample.encoding().to_string()`).
///
/// The MIME half is everything before the first `;`, compared without
/// case; the rest is the schema. Rules, first match wins:
///
/// | encoding                          | tag                    |
/// |-----------------------------------|------------------------|
/// | `application/protobuf;<name>`     | `Protobuf(<name>)`     |
/// | `application/flatbuffers;<name>`  | `Flatbuffers(<name>)`  |
/// | `text/<anything>`                 | `Text(<whole string>)` |
/// | `application/json`                | `Text(<whole string>)` |
/// | `application/octet-stream`        | `Octets`               |
/// | anything else                     | `Unknown`              |
///
/// A protobuf or FlatBuffers MIME type with an empty name is `Unknown`:
/// it names no message. Works on the `Display` string because
/// `Encoding::schema()` sits behind zenoh's `internal` feature.
pub fn parse_wire_tag(encoding: &str) -> WireTag {
    let encoding = encoding.trim();
    let (mime, schema) = match encoding.split_once(';') {
        Some((mime, schema)) => (mime.trim(), schema.trim()),
        None => (encoding, ""),
    };
    let named = |make: fn(String) -> WireTag| {
        if schema.is_empty() {
            WireTag::Unknown
        } else {
            make(schema.to_owned())
        }
    };
    let is_text = mime
        .split_once('/')
        .is_some_and(|(ty, sub)| ty.eq_ignore_ascii_case("text") && !sub.is_empty());
    if mime.eq_ignore_ascii_case(MIME_PROTOBUF) {
        named(WireTag::Protobuf)
    } else if mime.eq_ignore_ascii_case(MIME_FLATBUFFERS) {
        named(WireTag::Flatbuffers)
    } else if is_text || mime.eq_ignore_ascii_case("application/json") {
        WireTag::Text(encoding.to_owned())
    } else if mime.eq_ignore_ascii_case(MIME_OCTET_STREAM) {
        WireTag::Octets
    } else {
        WireTag::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    /// Wrap `header` as field 1 of an outer message, followed by an
    /// "unknown" field 2 (e.g. the `data` of `StringStamped`) that prost
    /// must skip.
    fn stamped_wire(header: &[u8]) -> Vec<u8> {
        let mut wire = vec![0x0a]; // tag 1 << 3 | wire type 2 (length-delimited)
        prost::encoding::encode_varint(header.len() as u64, &mut wire);
        wire.extend_from_slice(header);
        wire.push(0x12); // tag 2 << 3 | wire type 2
        let body = b"hello";
        prost::encoding::encode_varint(body.len() as u64, &mut wire);
        wire.extend_from_slice(body);
        wire
    }

    /// Encode synthetic Header-leading payloads in both styles' layouts and
    /// verify the probe lifts the metadata out of each.
    #[test]
    fn lifts_msg_type_and_rate_from_synthetic_stamped() {
        // ros2 `std/Header`: metadata at tag 3.
        let header = HeaderProbe {
            timestamp_ns: 1_700_000_000_000_000_000,
            frame_id: "world".into(),
            ros2_metadata: Some(MetadataProbe {
                topic_full_name: "dev01/ws/mod/zenoh/chatter".into(),
                msg_type: "std/StringStamped".into(),
                msg_freq_desired: 10.0,
            }),
            ..Default::default()
        };
        let mut header_buf = Vec::new();
        header.encode(&mut header_buf).unwrap();

        let meta =
            probe_header(&stamped_wire(&header_buf)).expect("probe should succeed on Stamped wire");
        assert_eq!(meta.msg_type, "std/StringStamped");
        assert_eq!(meta.msg_freq_desired, 10.0);
        assert_eq!(meta.topic_full_name, "dev01/ws/mod/zenoh/chatter");

        // swarmbotix `header/Header`: `seq` (varint) at tag 3, metadata at
        // tag 4. Built byte by byte from the .proto, with a non-zero `seq`
        // so tag 3 is on the wire with the type the ros2 layout does not use.
        let metadata = MetadataProbe {
            topic_full_name: "dev01/ws/cam/iox2/image_raw".into(),
            msg_type: "swarmbotix/images/Image480pMono8".into(),
            msg_freq_desired: 30.0,
        };
        let mut swarmbotix = vec![0x08]; // tag 1, varint: timestamp_ns
        prost::encoding::encode_varint(1_700_000_000_000_000_000, &mut swarmbotix);
        swarmbotix.extend_from_slice(&[0x12, 5]); // tag 2, length-delimited: frame_id
        swarmbotix.extend_from_slice(b"world");
        swarmbotix.extend_from_slice(&[0x18, 42]); // tag 3, varint: seq = 42
        swarmbotix.push(0x22); // tag 4, length-delimited: metadata
        prost::encoding::encode_varint(metadata.encoded_len() as u64, &mut swarmbotix);
        metadata.encode(&mut swarmbotix).unwrap();

        let meta = probe_header(&stamped_wire(&swarmbotix))
            .expect("probe should succeed on the swarmbotix Header layout");
        assert_eq!(meta, metadata);
        // The encoder writes the same bytes for that shape.
        let probe = HeaderProbe {
            timestamp_ns: 1_700_000_000_000_000_000,
            frame_id: "world".into(),
            seq: 42,
            swarmbotix_metadata: Some(metadata.clone()),
            ..Default::default()
        };
        assert_eq!(probe.encode_to_vec(), swarmbotix);

        // Both present: tag 4 wins.
        let both = HeaderProbe {
            ros2_metadata: Some(MetadataProbe::default()),
            ..probe
        };
        assert_eq!(both.into_metadata(), Some(metadata));
    }

    #[test]
    fn parse_wire_tag_table() {
        let cases: &[(&str, WireTag)] = &[
            (
                "application/protobuf;ros2/std/String",
                WireTag::Protobuf("ros2/std/String".into()),
            ),
            (
                "application/flatbuffers;swarmbotix/images/Image",
                WireTag::Flatbuffers("swarmbotix/images/Image".into()),
            ),
            ("text/plain", WireTag::Text("text/plain".into())),
            (
                "text/plain;charset=utf-8",
                WireTag::Text("text/plain;charset=utf-8".into()),
            ),
            ("text/csv", WireTag::Text("text/csv".into())),
            ("application/json", WireTag::Text("application/json".into())),
            ("Application/JSON", WireTag::Text("Application/JSON".into())),
            ("application/octet-stream", WireTag::Octets),
            // No name after the `;` names no message.
            ("application/protobuf", WireTag::Unknown),
            ("application/protobuf;", WireTag::Unknown),
            ("application/flatbuffers", WireTag::Unknown),
            // Zenoh's default for an untagged sample, and other MIME types.
            ("zenoh/bytes", WireTag::Unknown),
            ("image/png", WireTag::Unknown),
            ("text/", WireTag::Unknown),
            ("", WireTag::Unknown),
        ];
        for (encoding, expected) in cases {
            assert_eq!(&parse_wire_tag(encoding), expected, "{encoding:?}");
        }
    }

    #[test]
    fn returns_none_for_foreign_bytes() {
        // Random bytes that won't parse as protobuf — empty case is the
        // honest failure path that keeps the schema / rate columns blank.
        assert!(probe_header(b"HELLO_WORLD_RAW").is_none());
    }
}
