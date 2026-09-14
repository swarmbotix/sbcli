//! Header probe — try to lift `metadata.{msg_type, msg_freq_desired}` out
//! of any Zenoh sample without knowing the wrapping `*Stamped` type.
//!
//! The trick: every `<Type>Stamped` message in this repo (see
//! `message_definitions/std/StringStamped.proto` etc.) carries
//! `std.Header header = 1;` as its first field, so the outer message's
//! wire layout always starts with tag `0x0a` (field 1, length-delimited)
//! followed by the embedded Header bytes. We decode the outer payload as
//! [`StampedProbe`] — prost silently skips unknown fields, so the rest of
//! the `*Stamped` payload (`data`, `image`, etc.) gets ignored — and read
//! the metadata out of the embedded Header.
//!
//! Defined locally rather than imported from `sb-vault`'s generated
//! bindings so this crate stays trivial (no build script, no proto deps).

use prost::Message;

/// Minimal Header wire layout — must match `message_definitions/std/Header.proto`.
#[derive(Clone, PartialEq, Message)]
pub struct HeaderProbe {
    #[prost(uint64, tag = "1")]
    pub timestamp_ns: u64,
    #[prost(string, tag = "2")]
    pub frame_id: String,
    #[prost(message, optional, tag = "3")]
    pub metadata: Option<MetadataProbe>,
}

#[derive(Clone, PartialEq, Message)]
pub struct MetadataProbe {
    #[prost(string, tag = "1")]
    pub topic_full_name: String,
    #[prost(string, tag = "2")]
    pub msg_type: String,
    #[prost(double, tag = "3")]
    pub msg_freq_desired: f64,
}

/// Generic `*Stamped` probe — any message that carries `Header` as field 1.
/// Other fields of the outer message are silently skipped by prost.
#[derive(Clone, PartialEq, Message)]
pub struct StampedProbe {
    #[prost(message, optional, tag = "1")]
    pub header: Option<HeaderProbe>,
}

/// Try to lift the Header metadata out of an arbitrary `*Stamped` payload.
/// Returns `None` for foreign / un-Stamped bytes (failed decode, no
/// `header` field, no nested `metadata`). The success path is the common
/// one for sb-generated publishers.
pub fn probe_header(payload: &[u8]) -> Option<MetadataProbe> {
    StampedProbe::decode(payload)
        .ok()
        .and_then(|sp| sp.header)
        .and_then(|h| h.metadata)
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    /// Encode a synthetic `*Stamped`-shaped payload (Header as field 1,
    /// plus a trailing "fake payload" field at tag 2 that we expect prost
    /// to ignore) and verify the probe round-trips.
    #[test]
    fn lifts_msg_type_and_rate_from_synthetic_stamped() {
        let header = HeaderProbe {
            timestamp_ns: 1_700_000_000_000_000_000,
            frame_id: "world".into(),
            metadata: Some(MetadataProbe {
                topic_full_name: "/dev01/ws/mod/zenoh/chatter".into(),
                msg_type: "std/StringStamped".into(),
                msg_freq_desired: 10.0,
            }),
        };

        // Build the outer `*Stamped` payload by hand:
        //   field 1 (length-delimited) = Header bytes
        //   field 2 (length-delimited) = arbitrary unknown payload (e.g. the
        //                                String payload of StringStamped)
        let mut wire = Vec::new();
        // Field 1 — Header
        let mut header_buf = Vec::new();
        header.encode(&mut header_buf).unwrap();
        wire.push(0x0a); // tag 1 << 3 | wire type 2 (length-delimited)
        prost::encoding::encode_varint(header_buf.len() as u64, &mut wire);
        wire.extend_from_slice(&header_buf);
        // Field 2 — "unknown" payload prost should skip
        wire.push(0x12); // tag 2 << 3 | wire type 2
        let body = b"hello";
        prost::encoding::encode_varint(body.len() as u64, &mut wire);
        wire.extend_from_slice(body);

        let meta = probe_header(&wire).expect("probe should succeed on Stamped wire");
        assert_eq!(meta.msg_type, "std/StringStamped");
        assert_eq!(meta.msg_freq_desired, 10.0);
        assert_eq!(meta.topic_full_name, "/dev01/ws/mod/zenoh/chatter");
    }

    #[test]
    fn returns_none_for_foreign_bytes() {
        // Random bytes that won't parse as protobuf — empty case is the
        // honest failure path that keeps the schema / rate columns blank.
        assert!(probe_header(b"HELLO_WORLD_RAW").is_none());
    }

    #[test]
    fn returns_none_when_header_lacks_metadata() {
        // Header field present but without nested Metadata — the probe
        // correctly bails (no msg_type / rate to report).
        let payload = StampedProbe {
            header: Some(HeaderProbe {
                timestamp_ns: 0,
                frame_id: String::new(),
                metadata: None,
            }),
        };
        let mut buf = Vec::new();
        payload.encode(&mut buf).unwrap();
        assert!(probe_header(&buf).is_none());
    }
}
