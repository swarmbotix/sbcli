//! Bare-topic → fully-qualified resolution for `sb topic listen/pub`.
//!
//! Rules (level4.html §"sb topic listen"):
//!
//! - Leading `/` → already fully-qualified, pass through unchanged.
//! - No leading `/`, cwd has `sb.dev.yml` AND exactly one pub or sub uses
//!   that bare name as its topic last-segment → resolve to its full path
//!   via the device + workspace + module prefix the caller provides.
//! - Ambiguous (multiple matches) → error listing candidates.
//! - No match → error listing what we searched.
//!
//! Kept here (not in `sb-pubsub`) because L4 owns `sb topic` and we want
//! to depend down the stack, not sideways: `sb-cli` calls into us, not
//! into both us and `sb-pubsub` for resolution.

use sb_core::ModuleDevConfig;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveOutcome {
    pub full_topic: String,
    /// `Some(schema)` when the bare topic matched a publisher or
    /// subscriber on the cwd module — useful for `sb topic listen`'s
    /// decode step. `None` for fully-qualified passthroughs.
    pub schema: Option<String>,
}

#[derive(Debug, Error)]
pub enum ResolveError {
    #[error(
        "bare topic {topic:?} resolves ambiguously on module {module:?} — \
         matches: {candidates:?}. Use a fully-qualified path."
    )]
    Ambiguous {
        topic: String,
        module: String,
        candidates: Vec<String>,
    },
    #[error(
        "bare topic {topic:?} not found on module {module:?}. \
         Known publishers/subscribers: {known:?}. Use a fully-qualified path."
    )]
    NotFound {
        topic: String,
        module: String,
        known: Vec<String>,
    },
}

/// Resolve `topic` against the cwd module + `device` + `workspace`.
///
/// `device` and `workspace` are required to build the fully-qualified path
/// (matches the codegen's `topic_zenoh` derivation). Callers normally pull
/// them from the active workspace + `sb.config.yml::device`.
pub fn resolve_bare_topic(
    topic: &str,
    cwd_module: Option<&ModuleDevConfig>,
    device: &str,
    workspace: &str,
) -> Result<ResolveOutcome, ResolveError> {
    // Fully-qualified passthrough. We can't infer a schema for these
    // because the cwd module may not own the topic — caller will fall
    // back to listen-time hex.
    if topic.starts_with('/') {
        return Ok(ResolveOutcome {
            full_topic: topic.to_string(),
            schema: None,
        });
    }

    let cfg = match cwd_module {
        Some(c) => c,
        None => {
            return Err(ResolveError::NotFound {
                topic: topic.to_string(),
                module: "<no cwd module>".to_string(),
                known: Vec::new(),
            });
        }
    };

    // Match against the `name` field — `sb pub/sub add` derives `name`
    // from the topic's last segment, so bare lookups land naturally. The
    // transport segment comes from the matched entry, not from a global —
    // the same bare name on two transports resolves to two distinct keys.
    let mut matches: Vec<(String, &str)> = Vec::new();
    for p in &cfg.publishers {
        if p.name == topic || p.topic == topic {
            matches.push((
                build_full(
                    device,
                    workspace,
                    &cfg.module,
                    p.transport.path_segment(),
                    &p.topic,
                ),
                &p.msg_type,
            ));
        }
    }
    for s in &cfg.subscribers {
        if s.name == topic || s.topic == topic {
            matches.push((
                build_full(
                    device,
                    workspace,
                    &cfg.module,
                    s.transport.path_segment(),
                    &s.topic,
                ),
                &s.msg_type,
            ));
        }
    }

    // De-duplicate identical fully-qualified candidates (a pub + sub on
    // the same bare topic share one wire name — pick either).
    matches.sort_by(|a, b| a.0.cmp(&b.0));
    matches.dedup_by(|a, b| a.0 == b.0);

    match matches.as_slice() {
        [one] => Ok(ResolveOutcome {
            full_topic: one.0.clone(),
            schema: Some(one.1.to_string()),
        }),
        [] => Err(ResolveError::NotFound {
            topic: topic.to_string(),
            module: cfg.module.clone(),
            known: cfg
                .publishers
                .iter()
                .map(|p| p.name.clone())
                .chain(cfg.subscribers.iter().map(|s| s.name.clone()))
                .collect(),
        }),
        many => Err(ResolveError::Ambiguous {
            topic: topic.to_string(),
            module: cfg.module.clone(),
            candidates: many.iter().map(|(t, _)| t.clone()).collect(),
        }),
    }
}

fn build_full(device: &str, workspace: &str, module: &str, transport: &str, topic: &str) -> String {
    if topic.starts_with('/') {
        // User wrote a fully-qualified topic in sb.dev.yml — honor it.
        topic.to_string()
    } else {
        format!("/{device}/{workspace}/{module}/{transport}/{topic}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sb_core::{Language, PubSpec, SubSpec, Transport};
    use std::path::PathBuf;

    fn mk_cfg() -> ModuleDevConfig {
        ModuleDevConfig {
            module: "talker".into(),
            language: Language::Rust,
            root: PathBuf::from("/tmp/talker"),
            io_dir: PathBuf::from("swarmbotix_io"),
            publishers: vec![PubSpec {
                name: "chatter".into(),
                topic: "chatter".into(),
                msg_type: "std/StringStamped".into(),
                transport: Transport::Zenoh,
            }],
            subscribers: vec![SubSpec {
                name: "echo".into(),
                topic: "echo".into(),
                msg_type: "std/StringStamped".into(),
                transport: Transport::Zenoh,
            }],
        }
    }

    #[test]
    fn full_path_passthrough_returns_input_unchanged() {
        let cfg = mk_cfg();
        let r = resolve_bare_topic("/dev01/ws/mod/zenoh/foo", Some(&cfg), "dev01", "ws").unwrap();
        assert_eq!(r.full_topic, "/dev01/ws/mod/zenoh/foo");
        assert!(r.schema.is_none());
    }

    #[test]
    fn bare_name_matches_publisher_and_builds_full_path() {
        let cfg = mk_cfg();
        let r = resolve_bare_topic("chatter", Some(&cfg), "dev01", "ws").unwrap();
        assert_eq!(r.full_topic, "/dev01/ws/talker/zenoh/chatter");
        assert_eq!(r.schema.as_deref(), Some("std/StringStamped"));
    }

    #[test]
    fn bare_name_matches_subscriber() {
        let cfg = mk_cfg();
        let r = resolve_bare_topic("echo", Some(&cfg), "dev01", "ws").unwrap();
        assert_eq!(r.full_topic, "/dev01/ws/talker/zenoh/echo");
    }

    #[test]
    fn no_cwd_module_errors_for_bare_topic() {
        let err = resolve_bare_topic("chatter", None, "dev01", "ws").unwrap_err();
        matches!(err, ResolveError::NotFound { .. });
    }

    #[test]
    fn unknown_bare_topic_errors_with_candidate_list() {
        let cfg = mk_cfg();
        let err = resolve_bare_topic("missing", Some(&cfg), "dev01", "ws").unwrap_err();
        match err {
            ResolveError::NotFound { known, .. } => {
                assert!(known.contains(&"chatter".to_string()));
                assert!(known.contains(&"echo".to_string()));
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn ambiguous_matches_error_lists_candidates() {
        let mut cfg = mk_cfg();
        // Add another publisher with the same `name`/`topic` as the
        // subscriber `echo` to force an ambiguity.
        cfg.publishers.push(PubSpec {
            name: "echo".into(),
            topic: "echo_pub".into(),
            msg_type: "std/StringStamped".into(),
            transport: Transport::Zenoh,
        });
        let err = resolve_bare_topic("echo", Some(&cfg), "dev01", "ws").unwrap_err();
        match err {
            ResolveError::Ambiguous { candidates, .. } => {
                assert_eq!(candidates.len(), 2);
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }
}
