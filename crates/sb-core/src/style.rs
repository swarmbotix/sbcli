//! `sb.style.yml` — per-style compile settings that travel **with** the style.
//!
//! Backend selection cannot live in `sb.config.yml` alone. A style is often a
//! standalone git repo cloned onto machines that share no configuration (see
//! the cwd-discovery rules in requirements.md §Message Vault), and the answer
//! to "which backends is it safe to emit here" is a property of the *style and
//! its host project*, not of the box. Putting it beside `message_definitions/`
//! is what makes the safe choice survive a `git clone`.

use serde::{Deserialize, Serialize};

/// One codegen backend of `sb message compile`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    /// Fixed-layout POD structs for iceoryx2 zero-copy IPC.
    Iox2,
    /// Standard protobuf bindings via protoc's native plugins.
    Proto,
    /// `FlatBuffers` bindings via `.proto` → `.fbs` → flatc.
    Fb,
}

impl Backend {
    pub const ALL: [Backend; 3] = [Backend::Iox2, Backend::Proto, Backend::Fb];

    pub fn as_str(self) -> &'static str {
        match self {
            Backend::Iox2 => "iox2",
            Backend::Proto => "proto",
            Backend::Fb => "fb",
        }
    }
}

impl std::fmt::Display for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Parsed `<messages_root>/<style>/sb.style.yml`.
///
/// Every field is optional so an empty or partial file still loads; absence
/// means "fall through to the next rule", never "emit nothing".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StyleConfig {
    /// Backends `sb message compile` emits for this style when no explicit
    /// `--iox2` / `--proto` / `--fb` flag is given.
    ///
    /// An empty list is meaningful and distinct from absence: it says "emit
    /// nothing automatically", which is the only way to express "this style is
    /// compiled by hand" without the tool second-guessing it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backends: Option<Vec<Backend>>,
}

#[derive(Debug, thiserror::Error)]
pub enum StyleConfigError {
    #[error("{path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: serde_yaml::Error,
    },
}

impl StyleConfig {
    /// Parse YAML text. `path` is only used to make the error name the file.
    pub fn parse(text: &str, path: &str) -> Result<Self, StyleConfigError> {
        // An empty file parses to YAML null, which `deny_unknown_fields`
        // would otherwise reject — treat it as "no settings".
        if text.trim().is_empty() {
            return Ok(Self::default());
        }
        serde_yaml::from_str(text).map_err(|source| StyleConfigError::Parse {
            path: path.to_owned(),
            source,
        })
    }

    /// Render the file `sb` writes when pinning backends for a style.
    pub fn to_yaml(&self) -> String {
        let mut s = String::from(
            "# sb.style.yml — per-style settings for `sb message compile`.\n\
             #\n\
             # Lives beside `message_definitions/` so it travels with the style:\n\
             # a clone of this repo on another machine compiles the same way.\n\
             #\n\
             # `backends` pins which codegen backends are emitted when no\n\
             # explicit --iox2 / --proto / --fb flag is passed. Remove the key\n\
             # to fall back to \"whatever the host project and installed tools\n\
             # support\".\n",
        );
        match &self.backends {
            Some(b) if b.is_empty() => s.push_str("backends: []\n"),
            Some(b) => {
                let list: Vec<&str> = b.iter().map(|x| x.as_str()).collect();
                s.push_str("backends: [");
                s.push_str(&list.join(", "));
                s.push_str("]\n");
            }
            None => {}
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_backend_list() {
        let c = StyleConfig::parse("backends: [fb]\n", "sb.style.yml").unwrap();
        assert_eq!(c.backends, Some(vec![Backend::Fb]));
    }

    #[test]
    fn parses_all_three_backends() {
        let c = StyleConfig::parse("backends: [iox2, proto, fb]\n", "x").unwrap();
        assert_eq!(
            c.backends,
            Some(vec![Backend::Iox2, Backend::Proto, Backend::Fb])
        );
    }

    /// An empty list is not the same as no key. It means "emit nothing
    /// automatically" — the only way to say "I compile this by hand".
    #[test]
    fn empty_list_is_distinct_from_absent() {
        assert_eq!(
            StyleConfig::parse("backends: []\n", "x").unwrap().backends,
            Some(vec![])
        );
        assert_eq!(StyleConfig::parse("", "x").unwrap().backends, None);
        assert_eq!(StyleConfig::parse("{}\n", "x").unwrap().backends, None);
    }

    /// A typo must not be silently ignored — that is how someone ends up
    /// believing a pin took effect when it did not.
    #[test]
    fn unknown_key_is_rejected() {
        assert!(StyleConfig::parse("backend: [fb]\n", "x").is_err());
    }

    #[test]
    fn unknown_backend_is_rejected() {
        assert!(StyleConfig::parse("backends: [capnp]\n", "x").is_err());
    }

    #[test]
    fn round_trips_through_yaml() {
        let c = StyleConfig {
            backends: Some(vec![Backend::Fb]),
        };
        let back = StyleConfig::parse(&c.to_yaml(), "x").unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn round_trips_an_empty_list() {
        let c = StyleConfig {
            backends: Some(vec![]),
        };
        assert_eq!(StyleConfig::parse(&c.to_yaml(), "x").unwrap(), c);
    }
}
