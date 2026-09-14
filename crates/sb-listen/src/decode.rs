//! Decode bytes into the `sb topic listen` JSON envelope.
//!
//! Two paths:
//!
//! - [`decode_frame`] — Header-probe-only. Lifts `schema` and tags
//!   `encoding: "protobuf"` for any Header-leading `*Stamped` payload,
//!   but always renders the body as hex. Used when the caller has no
//!   FileDescriptorPool handy.
//! - [`decode_frame_dynamic`] — same shape, but consumes a
//!   [`prost_reflect::DescriptorPool`] (typically built from the vault's
//!   FileDescriptorSet) and decodes the payload into a JSON object using
//!   the schema named by the Header probe. Falls back to hex on any
//!   miss (schema unknown to the pool, decode error, etc.) so the CLI
//!   surface never silently lies about what's on the wire.
//!
//! `--raw` short-circuits BOTH paths to hex + null schema.

use prost_reflect::{DescriptorPool, DynamicMessage};
use sb_discover::probe_header;
use serde::Serialize;
use serde_json::Value;

/// One frame as the CLI hands it to stdout. Field order is pinned —
/// snapshot tests assert against it, and downstream tooling (swarmctl)
/// will too.
#[derive(Debug, Clone, Serialize)]
pub struct DecodedFrame<'a> {
    pub transport: &'a str,
    pub topic: &'a str,
    pub schema: Option<String>,
    pub hash: Option<String>,
    pub encoding: Option<&'static str>,
    pub payload: FramePayload,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FramePayload {
    Hex { bytes: String },
    Json { json: Value },
}

/// Decode `bytes` into a [`DecodedFrame`]. `raw=true` short-circuits the
/// Header probe so the schema column reports `null` even for sb-generated
/// frames (matches `sb topic listen --raw`).
pub fn decode_frame<'a>(
    transport: &'a str,
    topic: &'a str,
    bytes: &[u8],
    raw: bool,
) -> DecodedFrame<'a> {
    let schema = if raw {
        None
    } else {
        probe_header(bytes)
            .map(|m| m.msg_type)
            .filter(|s| !s.is_empty())
    };
    DecodedFrame {
        transport,
        topic,
        schema: schema.clone(),
        hash: None,
        encoding: if schema.is_some() {
            Some("protobuf")
        } else {
            None
        },
        payload: FramePayload::Hex {
            bytes: hex_string(bytes),
        },
    }
}

/// Decode + dynamically deserialize the payload if the pool knows the
/// schema. Falls back to [`decode_frame`]'s hex when:
///   - `raw=true` (forced hex),
///   - Header probe failed (foreign payload),
///   - the schema name doesn't resolve in the pool,
///   - or the dynamic decode failed (corrupt / wrong-schema bytes).
pub fn decode_frame_dynamic<'a>(
    transport: &'a str,
    topic: &'a str,
    bytes: &[u8],
    raw: bool,
    pool: &DescriptorPool,
) -> DecodedFrame<'a> {
    if raw {
        return decode_frame(transport, topic, bytes, raw);
    }
    let Some(schema) = probe_header(bytes)
        .map(|m| m.msg_type)
        .filter(|s| !s.is_empty())
    else {
        return decode_frame(transport, topic, bytes, raw);
    };
    let Some(desc) = protobuf_name_candidates(&schema)
        .iter()
        .find_map(|n| pool.get_message_by_name(n))
    else {
        // Schema known to publisher but not in the consumer's pool —
        // fall through to hex with a populated schema column so the
        // user knows what they're missing.
        return DecodedFrame {
            transport,
            topic,
            schema: Some(schema),
            hash: None,
            encoding: Some("protobuf"),
            payload: FramePayload::Hex {
                bytes: hex_string(bytes),
            },
        };
    };
    let json = match DynamicMessage::decode(desc, bytes) {
        Ok(msg) => match serde_json::to_value(&msg) {
            Ok(v) => v,
            Err(_) => return hex_with_schema(transport, topic, &schema, bytes),
        },
        Err(_) => return hex_with_schema(transport, topic, &schema, bytes),
    };
    DecodedFrame {
        transport,
        topic,
        schema: Some(schema),
        hash: None,
        encoding: Some("protobuf"),
        payload: FramePayload::Json { json },
    }
}

/// Candidate protobuf full names for a vault message name, most qualified
/// first. The caller takes the first that resolves in the pool.
///
/// A vault name is `<style>/<namespace>/<Leaf>` (older publishers emit the
/// 2-segment `<namespace>/<Leaf>`). Two things make a single mechanical
/// mapping impossible, and getting either wrong sends every frame to the
/// hex fallback:
///
///   - **The style is not part of the protobuf package.** `ros2/std/Header`
///     lives in package `swarmbotix.std`, so the leading segment is dropped.
///     Taking the last two segments handles the 2- and 3-segment forms alike.
///   - **Two package conventions ship.** `std/` and the whole `swarmbotix`
///     style use `swarmbotix.<ns>`; the ROS2 mirror uses a bare `<ns>`
///     (`sensor_msgs`, `geometry_msgs`, … — 130 of the 138 ros2 protos).
///
/// Known limit: the package encodes no style, so two styles defining the
/// same `<ns>/<Leaf>` are indistinguishable here and the pool's entry wins.
fn protobuf_name_candidates(schema: &str) -> Vec<String> {
    let parts: Vec<&str> = schema.split('/').filter(|s| !s.is_empty()).collect();
    match parts.as_slice() {
        [.., ns, leaf] => vec![format!("swarmbotix.{ns}.{leaf}"), format!("{ns}.{leaf}")],
        [leaf] => vec![(*leaf).to_string()],
        [] => vec![schema.to_string()],
    }
}

fn hex_with_schema<'a>(
    transport: &'a str,
    topic: &'a str,
    schema: &str,
    bytes: &[u8],
) -> DecodedFrame<'a> {
    DecodedFrame {
        transport,
        topic,
        schema: Some(schema.to_string()),
        hash: None,
        encoding: Some("protobuf"),
        payload: FramePayload::Hex {
            bytes: hex_string(bytes),
        },
    }
}

/// One JSON line, no trailing newline. The caller writes its own `\n`
/// so the output stream stays platform-consistent.
pub fn render_decoded_json_line(frame: &DecodedFrame<'_>) -> String {
    serde_json::to_string(frame).unwrap_or_else(|_| "{}".into())
}

fn hex_string(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;
    use sb_discover::{HeaderProbe, MetadataProbe, StampedProbe};

    fn synthetic_stamped(msg_type: &str, rate: f64) -> Vec<u8> {
        let probe = StampedProbe {
            header: Some(HeaderProbe {
                timestamp_ns: 0,
                frame_id: String::new(),
                metadata: Some(MetadataProbe {
                    topic_full_name: String::new(),
                    msg_type: msg_type.into(),
                    msg_freq_desired: rate,
                }),
            }),
        };
        let mut buf = Vec::new();
        probe.encode(&mut buf).unwrap();
        buf
    }

    #[test]
    fn header_probe_populates_schema() {
        let bytes = synthetic_stamped("std/StringStamped", 5.0);
        let frame = decode_frame("zenoh", "/x", &bytes, false);
        assert_eq!(frame.schema.as_deref(), Some("std/StringStamped"));
        assert_eq!(frame.encoding, Some("protobuf"));
    }

    #[test]
    fn raw_skips_probe_and_returns_null_schema() {
        let bytes = synthetic_stamped("std/StringStamped", 5.0);
        let frame = decode_frame("zenoh", "/x", &bytes, true);
        assert!(frame.schema.is_none());
        assert!(frame.encoding.is_none());
    }

    #[test]
    fn json_line_is_compact_single_line() {
        let frame = decode_frame("zenoh", "/x", b"foo", false);
        let line = render_decoded_json_line(&frame);
        assert!(
            !line.contains('\n'),
            "json line must not contain newlines: {line}"
        );
        assert!(line.contains("\"transport\":\"zenoh\""));
        assert!(line.contains("\"payload\":{\"kind\":\"hex\",\"bytes\":\"666f6f\"}"));
    }

    /// The style segment is dropped: it is part of the vault identity but
    /// never of the protobuf package. Before this, a 3-segment name produced
    /// `swarmbotix.ros2.std/StringStamped`, which resolves in no pool — so
    /// `sb topic listen` fell back to hex for every sb-generated frame.
    #[test]
    fn fully_qualified_name_drops_the_style_segment() {
        let c = protobuf_name_candidates("ros2/std/StringStamped");
        assert!(
            c.contains(&"swarmbotix.std.StringStamped".to_string()),
            "style must be dropped, got {c:?}"
        );
        assert!(
            c.iter().all(|n| !n.contains('/')),
            "no candidate may keep a path separator: {c:?}"
        );
    }

    /// The 2-segment form older publishers emit maps to the same candidates.
    #[test]
    fn legacy_two_segment_name_maps_the_same_way() {
        assert_eq!(
            protobuf_name_candidates("std/StringStamped"),
            protobuf_name_candidates("ros2/std/StringStamped"),
        );
    }

    /// Two package conventions ship. `swarmbotix.<ns>` covers `std/` and the
    /// swarmbotix style; the ROS2 mirror uses a bare `<ns>`. Hard-coding the
    /// prefix broke 130 of the 138 ros2 messages, so both must be offered.
    #[test]
    fn both_package_conventions_are_offered() {
        let c = protobuf_name_candidates("ros2/nav_msgs/Path");
        assert!(
            c.contains(&"nav_msgs.Path".to_string()),
            "bare ROS2 package: {c:?}"
        );
        assert!(
            c.contains(&"swarmbotix.nav_msgs.Path".to_string()),
            "swarmbotix-prefixed package: {c:?}"
        );
    }

    #[test]
    fn degenerate_names_do_not_panic() {
        assert_eq!(protobuf_name_candidates("Leaf"), vec!["Leaf".to_string()]);
        assert!(!protobuf_name_candidates("").is_empty());
        assert!(!protobuf_name_candidates("///").is_empty());
    }
}
