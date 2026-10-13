//! `TopicName`: parser/validator for
//! `<device>/<workspace>/<module>/<transport>/<topic>`.
//!
//! The canonical form has no leading `/` and is byte-identical to the
//! wire key (a Zenoh key expression or an iceoryx2 service name). A
//! single leading `/` is still accepted on input and dropped, so older
//! slashed spellings keep resolving to the same topic.
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
    /// Parse `<device>/<workspace>/<module>/<transport>/<topic>`.
    ///
    /// One leading `/` is tolerated and dropped, so
    /// `/dev01/ws/cam/iox2/image_raw` and `dev01/ws/cam/iox2/image_raw`
    /// parse to the same value.
    pub fn parse(s: &str) -> Result<Self, TopicNameError> {
        let rest = s.strip_prefix('/').unwrap_or(s);
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
            "{device}/{workspace}/{module}/{transport}/{topic}"
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

    /// Canonical wire form: `device/workspace/module/transport/topic`
    /// (no leading `/`).
    pub fn as_full(&self) -> String {
        format!(
            "{}/{}/{}/{}/{}",
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
        let t = TopicName::parse("dev01/perception/camera/iox2/image_raw").unwrap();
        assert_eq!(t.device(), "dev01");
        assert_eq!(t.workspace(), "perception");
        assert_eq!(t.module(), "camera");
        assert_eq!(t.transport(), "iox2");
        assert_eq!(t.topic(), "image_raw");
        assert_eq!(t.as_full(), "dev01/perception/camera/iox2/image_raw");
    }

    #[test]
    fn accepts_both_slashed_and_slashless() {
        let slashed = TopicName::parse("/dev01/ws/mod/iox2/topic").unwrap();
        let bare = TopicName::parse("dev01/ws/mod/iox2/topic").unwrap();
        assert_eq!(slashed, bare);
        assert_eq!(slashed.as_full(), "dev01/ws/mod/iox2/topic");
        assert_eq!(bare.as_full(), "dev01/ws/mod/iox2/topic");
        // Empty input: split yields [""], one segment.
        assert_eq!(
            TopicName::parse(""),
            Err(TopicNameError::WrongSegmentCount { got: 1 })
        );
    }
}
