//! Bare-topic → fully-qualified resolution for `sb topic listen/pub`.
//!
//! Rules (level4.html §"sb topic listen"):
//!
//! - Contains a `/` → fully-qualified. A leading `/` is accepted and
//!   dropped, then the path passes through. The canonical form
//!   `<device>/<workspace>/<module>/<transport>/<topic>` is identical to
//!   the wire key, so a Zenoh key expression works as-is. The transport is
//!   read from the `<transport>` segment; given a module registry, the
//!   schema comes from the owning module's `sb.dev.yml` ([`config_schema`]).
//! - No `/` at all, cwd has `sb.dev.yml` AND exactly one pub or sub uses
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

use sb_core::{ModuleDevConfig, TopicName, TopicRef, Transport};
use thiserror::Error;

/// Workspace module registry as a lookup: module name → path of its
/// `sb.dev.yml` (what `flow.yaml::modules` records), `None` when the
/// module is not registered.
pub type ModuleLookup<'a> = dyn Fn(&str) -> Option<PathBuf> + 'a;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveOutcome {
    /// The wire key: `<device>/<workspace>/<module>/<transport>/<topic>`
    /// for a resolved bare topic, or the fully-qualified input with any
    /// leading `/` dropped. Never starts with `/`.
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
    // Fully-qualified passthrough: any `/` makes the argument a path, and
    // a leading `/` is dropped (the canonical form has none). The cwd
    // module may not own the topic, so the schema comes from the module
    // the path names, through the caller's registry.
    if topic.contains('/') {
        let full_topic = topic.trim_start_matches('/').to_string();
        return Ok(ResolveOutcome {
            transport: TopicName::parse(&full_topic)
                .ok()
                .and_then(|t| Transport::from_path_segment(t.transport())),
            schema: modules.and_then(|lookup| config_schema(&full_topic, lookup)),
            full_topic,
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
    let mut matches: Vec<(String, Option<&str>, Transport)> = Vec::new();
    for p in &cfg.publishers {
        if p.name == topic || p.topic.matches_input(topic) {
            matches.push((
                p.topic
                    .full_key(device, workspace, &cfg.module, p.transport),
                p.msg_type.as_deref(),
                p.transport,
            ));
        }
    }
    for s in &cfg.subscribers {
        if s.name == topic || s.topic.matches_input(topic) {
            matches.push((
                s.topic
                    .full_key(device, workspace, &cfg.module, s.transport),
                s.msg_type.as_deref(),
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
            schema: one.1.map(str::to_owned),
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
/// `<device>/<workspace>/<module>/<transport>/<topic>` (the wire key); a
/// leading `/` is tolerated. `modules` maps a module name to the path of
/// its `sb.dev.yml` (the workspace registry). The `<module>` segment is
/// looked up as written, then without its `-<instance>` suffix
/// (`cam-sb00` → `cam`). Publishers are searched before subscribers; an
/// entry matches when its transport is the path's and either its bare
/// `topic:` is the path's last segment or its `key:` is the whole
/// canonical path.
///
/// The entry's `type` is returned verbatim: `sb pub add` / `sb sub add`
/// store the full `<style>/<ns>/<Leaf>` vault name, the same string the
/// generated publisher stamps into its Header, and older two-segment names
/// are understood by the decoder as they are. A raw entry has no `type`,
/// so it yields `None`.
///
/// Every miss (non-canonical path, unregistered module, unreadable or
/// unparseable `sb.dev.yml`, no matching entry) is `None`: the config is a
/// fallback, never a requirement.
pub fn config_schema(full_topic: &str, modules: &ModuleLookup<'_>) -> Option<String> {
    let parsed = TopicName::parse(full_topic).ok()?;
    let transport = Transport::from_path_segment(parsed.transport())?;
    let module = parsed.module();
    // Module identifiers never contain `-` (`validate_identifier`), so
    // everything from the first `-` is the instance id, which may itself
    // contain `-` (`front-left`).
    let dev_yml = modules(module).or_else(|| modules(module.split_once('-')?.0))?;
    let text = std::fs::read_to_string(dev_yml).ok()?;
    let cfg: ModuleDevConfig = serde_yaml::from_str(&text).ok()?;
    let canonical = parsed.as_full();
    let names_topic = |topic: &TopicRef, entry_transport: Transport| {
        entry_transport == transport
            && match topic {
                TopicRef::Owned(bare) => bare == parsed.topic(),
                TopicRef::Key(key) => *key == canonical,
            }
    };
    cfg.publishers
        .iter()
        .find(|p| names_topic(&p.topic, p.transport))
        .map(|p| p.msg_type.as_deref())
        .or_else(|| {
            cfg.subscribers
                .iter()
                .find(|s| names_topic(&s.topic, s.transport))
                .map(|s| s.msg_type.as_deref())
        })
        .flatten()
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
}
