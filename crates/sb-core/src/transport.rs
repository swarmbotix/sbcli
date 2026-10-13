//! `Transport`: pick a wire (Zenoh / Iceoryx2) and derive the per-transport
//! key or service name from a fully-qualified `TopicName`.
//!
//! Both transports share the same wire form, which is the canonical topic
//! name itself (`device/workspace/module/transport/topic`, no leading
//! `/`). Zenoh `KeyExpr` rejects leading slashes; iceoryx2 `ServiceName`
//! accepts internal `/` (see the iceoryx2 0.9 docs example
//! `ServiceName::new("My/Funk/ServiceName")`). The `iox2`/`zenoh` path
//! segment is what differentiates the two.

use serde::{Deserialize, Serialize};

use crate::topic::TopicName;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    Zenoh,
    Iceoryx2,
}

impl Transport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Zenoh => "zenoh",
            Self::Iceoryx2 => "iceoryx2",
        }
    }

    /// Literal path segment used in the canonical topic form
    /// `<device>/<workspace>/<module>/<transport>/<topic>`. Differs
    /// from `as_str()` for iceoryx2: the wire-key segment is `iox2`
    /// (matches the `--iox2` CLI flag), not `iceoryx2`.
    pub fn path_segment(self) -> &'static str {
        match self {
            Self::Zenoh => "zenoh",
            Self::Iceoryx2 => "iox2",
        }
    }

    /// Inverse of [`Self::path_segment`]: the transport a fully-qualified
    /// topic's `<transport>` segment names, `None` for anything else.
    pub fn from_path_segment(segment: &str) -> Option<Self> {
        [Self::Zenoh, Self::Iceoryx2]
            .into_iter()
            .find(|t| t.path_segment() == segment)
    }

    /// Tag shown in `sb topic list` listings: `[z]` / `[i]`.
    pub fn tag(self) -> &'static str {
        match self {
            Self::Zenoh => "[z]",
            Self::Iceoryx2 => "[i]",
        }
    }

    /// Wire identifier for a given topic on this transport. Both
    /// transports use the canonical form verbatim: Zenoh forbids leading
    /// slashes (the canonical form has none), iceoryx2 accepts internal `/`.
    pub fn wire_name(self, t: &TopicName) -> String {
        t.as_full()
    }
}

impl std::str::FromStr for Transport {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "zenoh" | "z" => Ok(Self::Zenoh),
            "iceoryx2" | "iceoryx" | "i" => Ok(Self::Iceoryx2),
            other => Err(format!("unknown transport: {other:?}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_name_is_canonical_form() {
        let t = TopicName::parse("dev01/ws/cam/zenoh/image_raw").unwrap();
        assert_eq!(Transport::Zenoh.wire_name(&t), t.as_full());
        assert_eq!(
            Transport::Zenoh.wire_name(&t),
            "dev01/ws/cam/zenoh/image_raw"
        );
        // A legacy slashed spelling yields the same wire key.
        let legacy = TopicName::parse("/dev01/ws/cam/zenoh/image_raw").unwrap();
        assert_eq!(Transport::Zenoh.wire_name(&legacy), t.as_full());
    }

    #[test]
    fn iceoryx2_wire_name_keeps_internal_slashes() {
        let t = TopicName::parse("dev01/ws/cam/iox2/image_raw").unwrap();
        // iceoryx2 0.9 `ServiceName::new` accepts `/`; no `__` encoding.
        assert_eq!(
            Transport::Iceoryx2.wire_name(&t),
            "dev01/ws/cam/iox2/image_raw"
        );
    }
}
