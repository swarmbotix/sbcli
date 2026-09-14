//! L4 — snapshot topic discovery.
//!
//! Two backends, one shape ([`TopicEntry`]):
//!
//! - **Zenoh** — short-lived session, `**` wildcard subscriber, sleep
//!   `window` (default 500 ms), collect unique key expressions. First
//!   sample per topic wins. The payload is probed as a protobuf
//!   `Header`-leading `*Stamped` message; on success, the topic's
//!   `schema` (= `header.metadata.msg_type`) and `rate_hz`
//!   (= `header.metadata.msg_freq_desired`) populate from that one
//!   frame. Foreign / non-Stamped payloads stay `None`.
//! - **iceoryx2** — `Service::list(Config::global_config(), ...)`. The
//!   schema column comes free from `static_details.message_type_details()
//!   .payload.type_name()` (e.g. `"swarmbotix_std::StringStamped"`). No
//!   subscribe is attempted at L4 — rate stays `None` for iox2 services.
//!
//! Both functions return rows sorted alphabetically by topic, with
//! [`matches_keyword`] applied so the CLI and any future server share one
//! filter definition.

mod probe;
mod render;
mod resolve;

pub use probe::{HeaderProbe, MetadataProbe, StampedProbe, probe_header};
pub use render::{TableLayout, render_json_lines, render_table};
pub use resolve::{ResolveError, ResolveOutcome, resolve_bare_topic};

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Serialize;
use zenoh::Wait;

/// Default Zenoh discovery window. Short enough to keep `sb topic list`
/// snappy; long enough to receive ≥ 1 frame from any publisher at ≥ 2 Hz.
/// Overridden by `-t / --timeout`.
pub const DEFAULT_DISCOVERY_WINDOW: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    Zenoh,
    Iceoryx2,
}

impl Transport {
    /// Single-character tag rendered in the table — `z` / `i`. Mirrors
    /// the prototype at `swarmbotix_old/crates/sb-introspect`.
    pub fn tag(self) -> char {
        match self {
            Transport::Zenoh => 'z',
            Transport::Iceoryx2 => 'i',
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct TopicEntry {
    pub topic: String,
    pub transport: Transport,
    /// Schema name — `"std/StringStamped"` for Zenoh-probed `*Stamped`
    /// payloads, `"swarmbotix_std::StringStamped"` (the iox2 typed-tag
    /// form) for iceoryx2 services. `None` for foreign / unknown.
    pub schema: Option<String>,
    /// Reserved for future envelope hashing. Always `None` at L4 —
    /// kept on the wire so later levels can populate without breaking
    /// the JSON / table contracts checked by the snapshot tests.
    pub hash: Option<String>,
    /// `header.metadata.msg_freq_desired` for any Zenoh sample that
    /// parses as Header-leading. `None` for foreign / iox2.
    pub rate_hz: Option<f64>,
    /// Owning publisher process id. Populated for iceoryx2 from the
    /// service's dynamic_details (iox2 records the owning Node's PID
    /// at registration time). `None` for Zenoh until we extend
    /// `std/Header.metadata` to carry it on the wire.
    pub publisher_pid: Option<u32>,
    /// iceoryx2 service has node entries but every one of them maps
    /// to a process that the OS reports as dead. The on-disk service
    /// registration outlived the process — `sb topic prune` cleans
    /// these via [`prune_iceoryx`]. Always `false` for Zenoh.
    pub is_dead: bool,
}

/// Case-insensitive (by default) substring match — public so the CLI and
/// any future server share one filter rule. Ported from the prototype.
pub fn matches_keyword(text: &str, keyword: Option<&str>, case_sensitive: bool) -> bool {
    let Some(k) = keyword else { return true };
    if case_sensitive {
        text.contains(k)
    } else {
        text.to_lowercase().contains(&k.to_lowercase())
    }
}

/// Open a short-lived Zenoh session, listen on `**` for `window`, return
/// one row per unique key expression. First sample wins.
pub fn discover_zenoh(
    window: Duration,
    keyword: Option<&str>,
    case_sensitive: bool,
) -> Result<Vec<TopicEntry>> {
    #[derive(Default)]
    struct Seen {
        schema: Option<String>,
        rate_hz: Option<f64>,
    }

    let session = zenoh::open(zenoh::Config::default())
        .wait()
        .map_err(|e| anyhow::anyhow!("zenoh open failed: {e}"))?;

    let acc: Arc<Mutex<HashMap<String, Seen>>> = Arc::new(Mutex::new(HashMap::new()));
    let acc_cb = Arc::clone(&acc);

    let sub = session
        .declare_subscriber("**")
        .callback(move |sample| {
            let key = sample.key_expr().to_string();
            let mut g = match acc_cb.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            let entry = g.entry(key).or_default();
            // First sample per topic wins — drop subsequent frames so
            // a chatty 200 Hz publisher doesn't waste CPU on decodes.
            if entry.schema.is_some() || entry.rate_hz.is_some() {
                return;
            }
            let bytes = sample.payload().to_bytes();
            if let Some(meta) = probe_header(&bytes) {
                if !meta.msg_type.is_empty() {
                    entry.schema = Some(meta.msg_type);
                }
                if meta.msg_freq_desired.is_finite() && meta.msg_freq_desired > 0.0 {
                    entry.rate_hz = Some(meta.msg_freq_desired);
                }
            }
        })
        .wait()
        .map_err(|e| anyhow::anyhow!("zenoh `**` subscriber failed: {e}"))?;

    std::thread::sleep(window);

    // Best-effort cleanup — undeclare + close errors can't undo what we
    // already collected, so swallow them.
    let _ = sub.undeclare().wait();
    let _ = session.close().wait();

    let g = acc
        .lock()
        .map_err(|_| anyhow::anyhow!("zenoh accumulator poisoned"))?;
    let mut entries: Vec<TopicEntry> = g
        .iter()
        .filter(|(topic, _)| matches_keyword(topic, keyword, case_sensitive))
        .map(|(topic, s)| TopicEntry {
            topic: topic.clone(),
            transport: Transport::Zenoh,
            schema: s.schema.clone(),
            hash: None,
            rate_hz: s.rate_hz,
            // Zenoh doesn't expose the publisher PID on the wire and
            // we don't (yet) stamp it into `Header.metadata`. Stays
            // `None` until we extend the schema.
            publisher_pid: None,
            is_dead: false,
        })
        .collect();
    entries.sort_by(|a, b| a.topic.cmp(&b.topic));
    Ok(entries)
}

/// Walk iceoryx2's service registry. Names come from `static_details.name()`;
/// the schema column is populated from the type name iceoryx2 records at
/// service creation time (e.g. `"swarmbotix_std::StringStamped"`).
///
/// PID + liveness come from `dynamic_details.nodes` — every iox2 service
/// records the [`UniqueNodeId`] of each Node that holds a publisher or
/// subscriber port, and [`UniqueNodeId::pid()`] gives the owning process.
/// We pick the first Alive node's PID; if none are alive but the service
/// still has registrations, we report the first node's PID and flag
/// `is_dead = true` so the renderer can mark the entry as stale. No
/// subscribe — rate stays `None` here.
pub fn discover_iceoryx(keyword: Option<&str>, case_sensitive: bool) -> Result<Vec<TopicEntry>> {
    use iceoryx2::node::NodeState;
    use iceoryx2::prelude::*;
    use iceoryx2::service::static_config::messaging_pattern::MessagingPattern;

    let mut entries: Vec<TopicEntry> = Vec::new();
    ipc::Service::list(Config::global_config(), |svc| {
        let name = svc.static_details.name().as_str().to_string();
        if !matches_keyword(&name, keyword, case_sensitive) {
            return CallbackProgression::Continue;
        }
        let schema = match svc.static_details.messaging_pattern() {
            MessagingPattern::PublishSubscribe(c) => {
                let t = c.message_type_details().payload.type_name().to_string();
                if t.is_empty() { None } else { Some(t) }
            }
            _ => None,
        };

        // Pick a representative PID. Preference order:
        //   1. first Alive node — the live owner you actually care about
        //   2. otherwise the first node regardless of state (Dead /
        //      Inaccessible / Undefined) so we can still surface the PID
        //      that leaked the registration, flagged as dead
        // When no nodes at all are registered, both fields stay None /
        // false and the row renders with `PID -` like Zenoh entries.
        // `pid().value()` is not one type across platforms: it is `i32` on
        // unix (pid_t) and `u32` on Windows (DWORD). The cast is required on
        // unix and is a no-op on Windows, where clippy then calls it
        // unnecessary — so the lint is off here rather than the cast, which
        // cannot be dropped without breaking the other platform.
        #[allow(clippy::unnecessary_cast)]
        let (publisher_pid, is_dead) = match &svc.dynamic_details {
            Some(d) => {
                let alive = d.nodes.iter().find(|n| matches!(n, NodeState::Alive(_)));
                if let Some(n) = alive {
                    (Some(n.node_id().pid().value() as u32), false)
                } else if let Some(n) = d.nodes.first() {
                    (Some(n.node_id().pid().value() as u32), true)
                } else {
                    (None, false)
                }
            }
            None => (None, false),
        };

        entries.push(TopicEntry {
            topic: name,
            transport: Transport::Iceoryx2,
            schema,
            hash: None,
            rate_hz: None,
            publisher_pid,
            is_dead,
        });
        CallbackProgression::Continue
    })
    .map_err(|e| anyhow::anyhow!("iceoryx2 Service::list failed: {e}"))?;
    entries.sort_by(|a, b| a.topic.cmp(&b.topic));
    Ok(entries)
}

/// One [`prune_iceoryx`] outcome — used by both the CLI (`sb topic prune`)
/// and any future swarmctl proxy. Mirrors [`DiscoverReport`]'s shape:
/// successes don't kill on partial failures, errors surface non-fatally.
#[derive(Default, Debug, Clone, Serialize)]
pub struct PruneReport {
    /// PIDs whose stale registrations were actually removed. iceoryx2's
    /// `try_remove_stale_resources()` returning `ResourcesAlreadyCleanedUp`
    /// counts as cleaned — another instance got there first, but the net
    /// state is what the user asked for.
    pub cleaned_pids: Vec<u32>,
    /// `(pid, reason)` for nodes the call enumerated but couldn't clean
    /// — e.g. `InsufficientPermissions`, `VersionMismatch`. PID is the
    /// owning process recorded in the dead node's view.
    pub failed: Vec<(u32, String)>,
}

/// Remove stale iceoryx2 on-disk node registrations whose owning process
/// is gone. Walks [`Node::list`] for the global config and, for every
/// [`NodeState::Dead`] view, calls [`DeadNodeView::try_remove_stale_resources`].
///
/// `Service::list` does **not** expose a per-service cleanup path — the
/// stale-resource lifecycle is anchored on the *node*, not the service.
/// Cleaning the dead nodes is what makes their services drop out of
/// subsequent `sb topic list` runs.
///
/// Returns a [`PruneReport`] tallying cleaned PIDs and per-PID failures.
/// Backend-level failures (e.g. `Node::list` itself errored) surface as
/// the outer `Result::Err`.
pub fn prune_iceoryx() -> Result<PruneReport> {
    use iceoryx2::node::{Node, NodeState, NodeView};
    use iceoryx2::prelude::*;

    let mut report = PruneReport::default();
    Node::<ipc::Service>::list(Config::global_config(), |node_state| {
        if let NodeState::Dead(view) = node_state {
            // See the note in the listing path: `pid().value()` is `i32` on
            // unix and `u32` on Windows.
            #[allow(clippy::unnecessary_cast)]
            let pid = view.id().pid().value() as u32;
            match view.try_remove_stale_resources() {
                Ok(()) => report.cleaned_pids.push(pid),
                // Another instance got there first — the net state is the
                // resource being gone, which is exactly what the caller
                // asked for. Count it as cleaned.
                Err(iceoryx2::node::NodeCleanupFailure::ResourcesAlreadyCleanedUp) => {
                    report.cleaned_pids.push(pid)
                }
                Err(e) => report.failed.push((pid, format!("{e:?}"))),
            }
        }
        CallbackProgression::Continue
    })
    .map_err(|e| anyhow::anyhow!("iceoryx2 Node::list failed: {e:?}"))?;

    report.cleaned_pids.sort_unstable();
    report.cleaned_pids.dedup();
    report.failed.sort_by_key(|(p, _)| *p);
    Ok(report)
}

/// Merge sort two transport snapshots. Used by `sb topic list` when
/// `--transport all` (the default) — guarantees a stable, alphabetical
/// view regardless of which backend returned first.
pub fn merge_sorted(mut zen: Vec<TopicEntry>, mut iox: Vec<TopicEntry>) -> Vec<TopicEntry> {
    zen.append(&mut iox);
    zen.sort_by(|a, b| {
        a.topic
            .cmp(&b.topic)
            .then(a.transport.tag().cmp(&b.transport.tag()))
    });
    zen
}

/// Convenience used by the CLI: run both backends honoring `transport`
/// (`None` = both) and return a merged, sorted view. Errors from one
/// backend don't kill the other — they surface as a warning the caller
/// can log to stderr.
pub fn discover_all(
    transport: Option<Transport>,
    window: Duration,
    keyword: Option<&str>,
    case_sensitive: bool,
) -> Result<DiscoverReport> {
    let mut report = DiscoverReport::default();
    let want_zen = transport.is_none() || transport == Some(Transport::Zenoh);
    let want_iox = transport.is_none() || transport == Some(Transport::Iceoryx2);

    if want_zen {
        match discover_zenoh(window, keyword, case_sensitive).context("zenoh discovery") {
            Ok(rows) => report.zenoh = rows,
            Err(e) => report.errors.push(format!("{e:#}")),
        }
    }
    if want_iox {
        match discover_iceoryx(keyword, case_sensitive).context("iceoryx2 discovery") {
            Ok(rows) => report.iceoryx = rows,
            Err(e) => report.errors.push(format!("{e:#}")),
        }
    }
    Ok(report)
}

#[derive(Default, Debug, Clone)]
pub struct DiscoverReport {
    pub zenoh: Vec<TopicEntry>,
    pub iceoryx: Vec<TopicEntry>,
    /// Non-fatal per-backend failures (e.g. zenoh open failed but iox2 was
    /// fine). The CLI surfaces these on stderr; the merged rows from the
    /// healthy backend are still returned.
    pub errors: Vec<String>,
}

impl DiscoverReport {
    pub fn into_merged(self) -> Vec<TopicEntry> {
        merge_sorted(self.zenoh, self.iceoryx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyword_case_insensitive_by_default() {
        assert!(matches_keyword(
            "/forge/default/Chatter",
            Some("chatter"),
            false
        ));
        assert!(!matches_keyword(
            "/forge/default/Chatter",
            Some("chatter"),
            true
        ));
        assert!(matches_keyword(
            "/forge/default/Chatter",
            Some("Chatter"),
            true
        ));
    }

    #[test]
    fn empty_keyword_matches_everything() {
        assert!(matches_keyword("anything", None, false));
        assert!(matches_keyword("anything", None, true));
    }

    #[test]
    fn transport_tag_matches_prototype() {
        assert_eq!(Transport::Zenoh.tag(), 'z');
        assert_eq!(Transport::Iceoryx2.tag(), 'i');
    }

    #[test]
    fn merge_sorted_is_alphabetical_with_stable_transport_order() {
        let zen = vec![TopicEntry {
            topic: "/b".into(),
            transport: Transport::Zenoh,
            schema: None,
            hash: None,
            rate_hz: None,
            publisher_pid: None,
            is_dead: false,
        }];
        let iox = vec![
            TopicEntry {
                topic: "/a".into(),
                transport: Transport::Iceoryx2,
                schema: None,
                hash: None,
                rate_hz: None,
                publisher_pid: None,
                is_dead: false,
            },
            TopicEntry {
                topic: "/b".into(),
                transport: Transport::Iceoryx2,
                schema: None,
                hash: None,
                rate_hz: None,
                publisher_pid: None,
                is_dead: false,
            },
        ];
        let merged = merge_sorted(zen, iox);
        let order: Vec<_> = merged
            .iter()
            .map(|t| (t.topic.as_str(), t.transport.tag()))
            .collect();
        assert_eq!(order, vec![("/a", 'i'), ("/b", 'i'), ("/b", 'z')]);
    }
}
