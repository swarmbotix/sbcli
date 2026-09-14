//! `TopicName` — parser/validator for
//! `/<device>/<workspace>/<module>/<transport>/<topic>`.
//!
//! `sb` enforces fully-qualified topic names on the wire. Users type bare
//! names (`image_raw`); CLI resolves them via `sb.dev.yml` + workspace
//! `flow.yaml`. The `<transport>` segment is the literal `iox2` or `zenoh`,
//! picked from the entry's `--iox2`/`--zenoh` flag. Embedding it in the
//! key lets the same bare name coexist on both transports for the same
//! module without collision.
//!
//! This module is the canonical validator both code paths eventually feed
//! into.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Fully-qualified swarmbotix topic.
///
/// Invariant: `device`, `workspace`, `module`, `topic` each match
/// `[A-Za-z0-9_]+`; `transport` is exactly `"iox2"` or `"zenoh"`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TopicName {
    device: String,
    workspace: String,
    module: String,
    transport: String,
    topic: String,
}

const ALLOWED_TRANSPORTS: [&str; 2] = ["iox2", "zenoh"];

impl TopicName {
    /// Parse `/<device>/<workspace>/<module>/<transport>/<topic>`.
    pub fn parse(s: &str) -> Result<Self, TopicNameError> {
        if !s.starts_with('/') {
            return Err(TopicNameError::MissingLeadingSlash);
        }
        let rest = &s[1..];
        let segments: Vec<&str> = rest.split('/').collect();
        if segments.len() != 5 {
            return Err(TopicNameError::WrongSegmentCount {
                got: segments.len(),
            });
        }
        for (idx, seg) in segments.iter().enumerate() {
            if seg.is_empty() {
                return Err(TopicNameError::EmptySegment { index: idx });
            }
            if let Some(bad) = seg.chars().find(|c| !is_valid_segment_char(*c)) {
                return Err(TopicNameError::IllegalChar {
                    index: idx,
                    ch: bad,
                });
            }
        }
        if !ALLOWED_TRANSPORTS.contains(&segments[3]) {
            return Err(TopicNameError::InvalidTransport {
                got: segments[3].to_owned(),
            });
        }
        Ok(Self {
            device: segments[0].to_owned(),
            workspace: segments[1].to_owned(),
            module: segments[2].to_owned(),
            transport: segments[3].to_owned(),
            topic: segments[4].to_owned(),
        })
    }

    pub fn from_parts(
        device: &str,
        workspace: &str,
        module: &str,
        transport: &str,
        topic: &str,
    ) -> Result<Self, TopicNameError> {
        Self::parse(&format!(
            "/{device}/{workspace}/{module}/{transport}/{topic}"
        ))
    }

    pub fn device(&self) -> &str {
        &self.device
    }
    pub fn workspace(&self) -> &str {
        &self.workspace
    }
    pub fn module(&self) -> &str {
        &self.module
    }
    pub fn transport(&self) -> &str {
        &self.transport
    }
    pub fn topic(&self) -> &str {
        &self.topic
    }

    /// Wire form: `/device/workspace/module/transport/topic`.
    pub fn as_full(&self) -> String {
        format!(
            "/{}/{}/{}/{}/{}",
            self.device, self.workspace, self.module, self.transport, self.topic
        )
    }
}

fn is_valid_segment_char(c: char) -> bool {
    // `-` is allowed so swarm-instance modules like `controller-2` (set
    // at construction time on the generated publisher/subscriber) parse
    // as valid segments. Module *identifiers* (the `module:` field in
    // `sb.dev.yml`) still reject `-` via `validate_identifier`; this
    // relaxation only affects the wire-path segments.
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// Validate an instance ID. Instance IDs are the suffix appended to a
/// module name on the wire (`<module>-<id>` → `cam_gige_ht-left`), so
/// they must fit a wire segment: non-empty and only chars accepted by
/// [`is_valid_segment_char`] (`[A-Za-z0-9_-]`). Unlike
/// [`validate_identifier`](crate::validate_identifier), `-` is allowed,
/// so nested IDs like `front-left` and numeric forms like `1` both pass.
pub fn validate_instance_id(s: &str) -> Result<(), InstanceIdError> {
    if s.is_empty() {
        return Err(InstanceIdError::Empty);
    }
    if let Some(c) = s.chars().find(|c| !is_valid_segment_char(*c)) {
        return Err(InstanceIdError::IllegalChar { found: c });
    }
    Ok(())
}

pub fn is_valid_instance_id(s: &str) -> bool {
    validate_instance_id(s).is_ok()
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InstanceIdError {
    #[error("instance id is empty")]
    Empty,
    #[error("instance id may only contain ASCII letters, digits, `_`, and `-`; found {found:?}")]
    IllegalChar { found: char },
}

impl fmt::Display for TopicName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_full())
    }
}

impl FromStr for TopicName {
    type Err = TopicNameError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl Serialize for TopicName {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.as_full())
    }
}

impl<'de> Deserialize<'de> for TopicName {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TopicNameError {
    #[error("topic must start with '/' (got no leading slash)")]
    MissingLeadingSlash,
    #[error(
        "topic must have exactly 5 segments \
         (device/workspace/module/transport/topic), got {got}"
    )]
    WrongSegmentCount { got: usize },
    #[error("segment {index} is empty")]
    EmptySegment { index: usize },
    #[error("segment {index} contains illegal character {ch:?} (allowed: [A-Za-z0-9_])")]
    IllegalChar { index: usize, ch: char },
    #[error("transport segment must be \"iox2\" or \"zenoh\", got {got:?}")]
    InvalidTransport { got: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_canonical_form() {
        let t = TopicName::parse("/dev01/perception/camera/iox2/image_raw").unwrap();
        assert_eq!(t.device(), "dev01");
        assert_eq!(t.workspace(), "perception");
        assert_eq!(t.module(), "camera");
        assert_eq!(t.transport(), "iox2");
        assert_eq!(t.topic(), "image_raw");
        assert_eq!(t.as_full(), "/dev01/perception/camera/iox2/image_raw");
    }

    #[test]
    fn parses_zenoh_transport() {
        let t = TopicName::parse("/dev01/ws/mod/zenoh/topic").unwrap();
        assert_eq!(t.transport(), "zenoh");
    }

    #[test]
    fn parses_underscore_and_digits() {
        assert!(TopicName::parse("/d_1/w_1/m_1/iox2/t_1").is_ok());
        assert!(TopicName::parse("/abc/def/ghi/zenoh/jkl").is_ok());
        assert!(TopicName::parse("/A/B/C/iox2/D").is_ok());
        assert!(TopicName::parse("/host01/ws_alpha/mod_42/zenoh/topic_v2").is_ok());
    }

    #[test]
    fn rejects_missing_leading_slash() {
        assert_eq!(
            TopicName::parse("dev01/ws/mod/iox2/topic"),
            Err(TopicNameError::MissingLeadingSlash)
        );
        assert_eq!(
            TopicName::parse(""),
            Err(TopicNameError::MissingLeadingSlash)
        );
    }

    #[test]
    fn rejects_wrong_segment_count() {
        // Too few — old 4-segment form
        match TopicName::parse("/a/b/c/d").unwrap_err() {
            TopicNameError::WrongSegmentCount { got: 4 } => {}
            e => panic!("unexpected: {e:?}"),
        }
        match TopicName::parse("/a/b/c").unwrap_err() {
            TopicNameError::WrongSegmentCount { got: 3 } => {}
            e => panic!("unexpected: {e:?}"),
        }
        // Too many
        match TopicName::parse("/a/b/c/iox2/d/e").unwrap_err() {
            TopicNameError::WrongSegmentCount { got: 6 } => {}
            e => panic!("unexpected: {e:?}"),
        }
        // Single slash: rest = "" → split → [""] → 1 segment
        match TopicName::parse("/").unwrap_err() {
            TopicNameError::WrongSegmentCount { got: 1 } => {}
            e => panic!("unexpected: {e:?}"),
        }
    }

    #[test]
    fn rejects_empty_segment() {
        match TopicName::parse("//ws/mod/iox2/topic").unwrap_err() {
            TopicNameError::EmptySegment { index: 0 } => {}
            e => panic!("unexpected: {e:?}"),
        }
        match TopicName::parse("/dev//mod/iox2/topic").unwrap_err() {
            TopicNameError::EmptySegment { index: 1 } => {}
            e => panic!("unexpected: {e:?}"),
        }
        match TopicName::parse("/dev/ws//iox2/topic").unwrap_err() {
            TopicNameError::EmptySegment { index: 2 } => {}
            e => panic!("unexpected: {e:?}"),
        }
        match TopicName::parse("/dev/ws/mod//topic").unwrap_err() {
            TopicNameError::EmptySegment { index: 3 } => {}
            e => panic!("unexpected: {e:?}"),
        }
        match TopicName::parse("/dev/ws/mod/iox2/").unwrap_err() {
            TopicNameError::EmptySegment { index: 4 } => {}
            e => panic!("unexpected: {e:?}"),
        }
    }

    #[test]
    fn accepts_hyphen_in_segments() {
        // Required for runtime-instance overrides (e.g., a swarm sim
        // launching N copies of `controller` with `module=controller-2`).
        assert!(TopicName::parse("/dev01/ws/controller-2/iox2/state").is_ok());
        assert!(TopicName::parse("/host-1/ws/mod/zenoh/topic").is_ok());
    }

    #[test]
    fn rejects_illegal_chars() {
        match TopicName::parse("/dev/ws/mod/iox2/topic.raw").unwrap_err() {
            TopicNameError::IllegalChar { index: 4, ch: '.' } => {}
            e => panic!("unexpected: {e:?}"),
        }
        match TopicName::parse("/dev/ws/m d/iox2/topic").unwrap_err() {
            TopicNameError::IllegalChar { index: 2, ch: ' ' } => {}
            e => panic!("unexpected: {e:?}"),
        }
        match TopicName::parse("/dev/ws:1/mod/iox2/topic").unwrap_err() {
            TopicNameError::IllegalChar { index: 1, ch: ':' } => {}
            e => panic!("unexpected: {e:?}"),
        }
    }

    #[test]
    fn rejects_unknown_transport() {
        match TopicName::parse("/dev/ws/mod/tcp/topic").unwrap_err() {
            TopicNameError::InvalidTransport { got } => assert_eq!(got, "tcp"),
            e => panic!("unexpected: {e:?}"),
        }
        // "iceoryx2" is the YAML form, not the path-segment form.
        match TopicName::parse("/dev/ws/mod/iceoryx2/topic").unwrap_err() {
            TopicNameError::InvalidTransport { got } => assert_eq!(got, "iceoryx2"),
            e => panic!("unexpected: {e:?}"),
        }
    }

    #[test]
    fn display_round_trip() {
        let s = "/dev01/ws/mod/zenoh/topic_v2";
        assert_eq!(TopicName::parse(s).unwrap().to_string(), s);
    }

    #[test]
    fn from_parts_builds_correct_name() {
        let t = TopicName::from_parts("dev01", "ws", "mod", "iox2", "topic").unwrap();
        assert_eq!(t.as_full(), "/dev01/ws/mod/iox2/topic");
    }

    #[test]
    fn from_parts_rejects_empty() {
        assert!(matches!(
            TopicName::from_parts("dev", "", "mod", "iox2", "topic"),
            Err(TopicNameError::EmptySegment { index: 1 })
        ));
    }

    #[test]
    fn from_parts_rejects_unknown_transport() {
        assert!(matches!(
            TopicName::from_parts("dev", "ws", "mod", "tcp", "topic"),
            Err(TopicNameError::InvalidTransport { .. })
        ));
    }

    #[test]
    fn serde_round_trip_via_yaml() {
        let t = TopicName::parse("/dev01/ws/cam/iox2/image_raw").unwrap();
        let y = serde_yaml::to_string(&t).unwrap();
        let back: TopicName = serde_yaml::from_str(&y).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn instance_id_accepts_typical_forms() {
        for s in [
            "left",
            "right",
            "1",
            "front_left",
            "cam-01",
            "_internal",
            "A1",
        ] {
            assert!(is_valid_instance_id(s), "{s:?} should be valid");
        }
    }

    #[test]
    fn instance_id_rejects_separators_and_whitespace() {
        assert!(matches!(
            validate_instance_id(""),
            Err(InstanceIdError::Empty)
        ));
        for s in ["a/b", "a b", "a.b", "a:b", "a,b"] {
            assert!(
                matches!(
                    validate_instance_id(s),
                    Err(InstanceIdError::IllegalChar { .. })
                ),
                "{s:?} should be invalid"
            );
        }
    }
}
