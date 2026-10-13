//! Bare-topic → fully-qualified resolution for `sb topic listen/pub`.
//!
//! Rules (level4.html §"sb topic listen"):
//!
//! - Leading `/` → already fully-qualified, pass through unchanged. The
//!   transport is read from the `<transport>` segment of the canonical
//!   `/<device>/<workspace>/<module>/<transport>/<topic>` form; given a
//!   module registry, the schema comes from the owning module's
//!   `sb.dev.yml` ([`config_schema`]).
//! - No leading `/`, cwd has `sb.dev.yml` AND exactly one pub or sub uses
//!   that bare name as its topic last-segment → resolve to its full path
//!   via the device + workspace + module prefix the caller provides. The
//!   transport and schema come from the matched entry.
//! - Ambiguous (multiple matches) → error listing candidates.
//! - No match → error listing what we searched.
//!
//! Kept here (not in `sb-pubsub`) because L4 owns `sb topic` and we want
//! to depend down the stack, not sideways: `sb-cli` calls into us, not
//! into both us and `sb-pubsub` for resolution.

use std::path::PathBuf;

use sb_core::{ModuleDevConfig, TopicName, Transport};
use thiserror::Error;

/// Workspace module registry as a lookup: module name → path of its
/// `sb.dev.yml` (what `flow.yaml::modules` records), `None` when the
/// module is not registered.
pub type ModuleLookup<'a> = dyn Fn(&str) -> Option<PathBuf> + 'a;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveOutcome {
    pub full_topic: String,
    /// The [`sb_core::Transport`] the topic travels on: the matched entry's
    /// for a bare topic, the `<transport>` segment for a fully-qualified
    /// one. `None` when a fully-qualified path is not in the canonical
    /// five-segment form.
    pub transport: Option<Transport>,
    /// Vault name the module config declares for the topic, for
    /// `sb topic listen`'s decode step. From the cwd module's matched entry
    /// for a bare topic; for a fully-qualified one, from the owning
    /// module's `sb.dev.yml` when the caller passes a registry and the
    /// module is in it. `None` otherwise.
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
///
/// `modules` is the workspace registry used for fully-qualified topics
/// ([`ModuleLookup`]). `None` skips the lookup; bare topics never use it.
pub fn resolve_bare_topic(
    topic: &str,
    cwd_module: Option<&ModuleDevConfig>,
    device: &str,
    workspace: &str,
    modules: Option<&ModuleLookup<'_>>,
) -> Result<ResolveOutcome, ResolveError> {
    // Fully-qualified passthrough. The cwd module may not own the topic,
    // so the schema comes from the module the path names, through the
    // caller's registry.
    if topic.starts_with('/') {
        return Ok(ResolveOutcome {
            full_topic: topic.to_string(),
            transport: TopicName::parse(topic)
                .ok()
                .and_then(|t| Transport::from_path_segment(t.transport())),
            schema: modules.and_then(|lookup| config_schema(topic, lookup)),
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
    let mut matches: Vec<(String, &str, Transport)> = Vec::new();
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
                p.transport,
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
                s.transport,
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
            transport: Some(one.2),
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
            candidates: many.iter().map(|(t, ..)| t.clone()).collect(),
        }),
    }
}

/// Vault name the owning module's `sb.dev.yml` declares for a
/// fully-qualified topic, or `None`.
///
/// `full_topic` is the canonical
/// `/<device>/<workspace>/<module>/<transport>/<topic>`; the leading `/` is
/// optional, so a Zenoh key expression works as-is. `modules` maps a module
/// name to the path of its `sb.dev.yml` (the workspace registry). The
/// `<module>` segment is looked up as written, then without its
/// `-<instance>` suffix (`cam-sb00` → `cam`). Publishers are searched
/// before subscribers; an entry matches when its transport is the path's
/// and its `topic` is the path's last segment (or the whole path, for an
/// entry written fully-qualified).
///
/// The entry's `type` is returned verbatim: `sb pub add` / `sb sub add`
/// store the full `<style>/<ns>/<Leaf>` vault name, the same string the
/// generated publisher stamps into its Header, and older two-segment names
/// are understood by the decoder as they are.
///
/// Every miss (non-canonical path, unregistered module, unreadable or
/// unparseable `sb.dev.yml`, no matching entry) is `None`: the config is a
/// fallback, never a requirement.
pub fn config_schema(full_topic: &str, modules: &ModuleLookup<'_>) -> Option<String> {
    let parsed = TopicName::parse(&format!("/{}", full_topic.trim_start_matches('/'))).ok()?;
    let transport = Transport::from_path_segment(parsed.transport())?;
    let module = parsed.module();
    // Module identifiers never contain `-` (`validate_identifier`), so
    // everything from the first `-` is the instance id, which may itself
    // contain `-` (`front-left`).
    let dev_yml = modules(module).or_else(|| modules(module.split_once('-')?.0))?;
    let text = std::fs::read_to_string(dev_yml).ok()?;
    let cfg: ModuleDevConfig = serde_yaml::from_str(&text).ok()?;
    let canonical = parsed.as_full();
    let names_topic = |topic: &str, entry_transport: Transport| {
        entry_transport == transport && (topic == parsed.topic() || topic == canonical)
    };
    cfg.publishers
        .iter()
        .find(|p| names_topic(&p.topic, p.transport))
        .map(|p| &p.msg_type)
        .or_else(|| {
            cfg.subscribers
                .iter()
                .find(|s| names_topic(&s.topic, s.transport))
                .map(|s| &s.msg_type)
        })
        .filter(|t| !t.is_empty())
        .cloned()
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
        let r =
            resolve_bare_topic("/dev01/ws/mod/zenoh/foo", Some(&cfg), "dev01", "ws", None).unwrap();
        assert_eq!(r.full_topic, "/dev01/ws/mod/zenoh/foo");
        assert!(r.schema.is_none());
    }

    #[test]
    fn bare_name_matches_publisher_and_builds_full_path() {
        let cfg = mk_cfg();
        let r = resolve_bare_topic("chatter", Some(&cfg), "dev01", "ws", None).unwrap();
        assert_eq!(r.full_topic, "/dev01/ws/talker/zenoh/chatter");
        assert_eq!(r.schema.as_deref(), Some("std/StringStamped"));
    }

    #[test]
    fn bare_name_matches_subscriber() {
        let cfg = mk_cfg();
        let r = resolve_bare_topic("echo", Some(&cfg), "dev01", "ws", None).unwrap();
        assert_eq!(r.full_topic, "/dev01/ws/talker/zenoh/echo");
    }

    #[test]
    fn no_cwd_module_errors_for_bare_topic() {
        let err = resolve_bare_topic("chatter", None, "dev01", "ws", None).unwrap_err();
        matches!(err, ResolveError::NotFound { .. });
    }

    #[test]
    fn unknown_bare_topic_errors_with_candidate_list() {
        let cfg = mk_cfg();
        let err = resolve_bare_topic("missing", Some(&cfg), "dev01", "ws", None).unwrap_err();
        match err {
            ResolveError::NotFound { known, .. } => {
                assert!(known.contains(&"chatter".to_string()));
                assert!(known.contains(&"echo".to_string()));
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    /// A fully-qualified path takes its transport from the `<transport>`
    /// segment and its schema from the owning module's `sb.dev.yml`, found
    /// through the registry with or without an instance suffix on the
    /// module segment. A bare topic takes its transport from the entry.
    #[test]
    fn transport_from_path_or_entry_and_schema_from_registry() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dev_yml = tmp.path().join("sb.dev.yml");
        std::fs::write(
            &dev_yml,
            "module: ips_track\n\
             language: python\n\
             root: /tmp/ips_track\n\
             publishers:\n\
             - name: cube_poses\n  topic: cube_poses\n  type: ros2/custom/CubePoses\n  transport: zenoh\n\
             - name: image\n  topic: image\n  type: ros2/std/ImageStamped\n  transport: iceoryx2\n",
        )
        .unwrap();
        let registry = |module: &str| (module == "ips_track").then(|| dev_yml.clone());
        let full = |topic: &str| resolve_bare_topic(topic, None, "", "", Some(&registry)).unwrap();

        let zenoh = full("/elcom/ws_ips/ips_track/zenoh/cube_poses");
        assert_eq!(zenoh.transport, Some(Transport::Zenoh));
        assert_eq!(zenoh.schema.as_deref(), Some("ros2/custom/CubePoses"));

        let iox2 = full("/elcom/ws_ips/ips_track-sb00/iox2/image");
        assert_eq!(iox2.full_topic, "/elcom/ws_ips/ips_track-sb00/iox2/image");
        assert_eq!(iox2.transport, Some(Transport::Iceoryx2));
        assert_eq!(iox2.schema.as_deref(), Some("ros2/std/ImageStamped"));

        // The entry must be on the path's transport; an unregistered module
        // or a non-canonical path is a miss, not an error.
        assert_eq!(full("/elcom/ws_ips/ips_track/zenoh/image").schema, None);
        assert_eq!(full("/elcom/ws_ips/other/zenoh/cube_poses").schema, None);
        let loose = full("/elcom/cube_poses");
        assert_eq!((loose.transport, loose.schema), (None, None));
        // `sb topic list` passes Zenoh key expressions, which have no `/`.
        assert_eq!(
            config_schema("elcom/ws_ips/ips_track-sb00/zenoh/cube_poses", &registry).as_deref(),
            Some("ros2/custom/CubePoses")
        );

        let mut cfg = mk_cfg();
        cfg.publishers[0].transport = Transport::Iceoryx2;
        let bare = resolve_bare_topic("chatter", Some(&cfg), "dev01", "ws", None).unwrap();
        assert_eq!(bare.full_topic, "/dev01/ws/talker/iox2/chatter");
        assert_eq!(bare.transport, Some(Transport::Iceoryx2));
        let echo = resolve_bare_topic("echo", Some(&cfg), "dev01", "ws", None).unwrap();
        assert_eq!(echo.transport, Some(Transport::Zenoh));
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
        let err = resolve_bare_topic("echo", Some(&cfg), "dev01", "ws", None).unwrap_err();
        match err {
            ResolveError::Ambiguous { candidates, .. } => {
                assert_eq!(candidates.len(), 2);
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }
}
