//! L3: `sb pub/sub/service` command logic.
//!
//! Owns the boring bits the CLI delegates to:
//!
//! - **Module resolution** ([`resolve_module`]) — three lookup rules.
//! - **Mutation** ([`pub_add`], [`pub_edit`], [`pub_rm`], [`sub_add`],
//!   [`sub_edit`], [`sub_rm`], [`service_add`], [`service_rm`]): reads
//!   `sb.dev.yml`, validates against the vault, mutates the in-memory
//!   `ModuleDevConfig`, writes it back, and invokes [`sb_codegen`] to emit /
//!   update / remove the matching language file under
//!   `<io_dir>/{publishers,subscribers,services}/`.
//! - **Regeneration** ([`regenerate_entry`], [`update_all`]): re-render
//!   generated files from the current `sb.dev.yml` without changing it.
//! - **Listing** ([`list`]) — pure read; produces a structured view the
//!   CLI renders as a table.
//!
//! Publishers, subscribers, and services share one name space per module
//! ([`find_entry`]); adding an entry whose name is taken by any of the
//! three is refused.
//!
//! Filesystem state owned by this crate (per module):
//!
//! ```text
//! <module_root>/
//!   sb.dev.yml                    ← read + write
//!   <io_dir>/publishers/<n>.<ext> ← write + delete
//!   <io_dir>/subscribers/<n>.<ext>
//!   <io_dir>/services/<n>.<ext>
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

use sb_codegen::{CodegenInput, Kind, Rendered};
use sb_core::{
    Encoding, EntryKind, Language, LegacyTopic, ModuleDevConfig, PubSpec, SbCliConfig, ServiceSpec,
    SubSpec, TopicRef, Transport, legacy_topics_in,
};
use sb_vault::{MessageName, Vault};
use sb_workspace::{SbHome, active_workspace, read_flow};

// ─────────────────────────────────────────────────────────────────────
// Module resolution — the three lookup rules from level3.html
// ─────────────────────────────────────────────────────────────────────

/// In-memory handle for a resolved module: where its `sb.dev.yml` lives
/// on disk + the parsed config. Created by [`resolve_module`].
#[derive(Debug, Clone)]
pub struct ModuleHandle {
    pub dev_yml_path: PathBuf,
    /// [`ModuleDevConfig::module_root`] of `config` for `dev_yml_path`.
    pub module_root: PathBuf,
    pub config: ModuleDevConfig,
    /// Legacy `topic: /...` entries found in the file as loaded. They are
    /// already read as `key:`; the next write of `sb.dev.yml` stores them
    /// in that form and empties this list.
    pub legacy_topics: Vec<LegacyTopic>,
}

impl ModuleHandle {
    /// Full path to the IO dir: `<module_root>/<io_dir>`.
    pub fn io_dir(&self) -> PathBuf {
        self.module_root.join(&self.config.io_dir)
    }
}

/// Resolve which `sb.dev.yml` `sb pub/sub *` should operate on.
///
/// Order (highest priority first), per level3.html §"Module resolution":
///   1. `explicit = Some(name)` AND `./<name>/sb.dev.yml` exists in `cwd`.
///   2. `explicit = Some(name)` AND active workspace's `flow.yaml` has it.
///   3. `explicit = None` AND `./sb.dev.yml` exists in `cwd`.
///
/// Errors with all paths searched if every rule misses.
pub fn resolve_module(explicit: Option<&str>, cwd: &Path, home: &SbHome) -> Result<ModuleHandle> {
    let mut tried: Vec<PathBuf> = Vec::new();

    match explicit {
        Some(name) => {
            // Rule 1 — child of cwd.
            let child = cwd.join(name).join("sb.dev.yml");
            tried.push(child.clone());
            if child.exists() {
                return load_handle(&child);
            }
            // Rule 2 — active workspace's flow.yaml.
            if let Some(ws) = active_workspace(home)? {
                let flow = read_flow(home, &ws)?;
                if let Some(m) = flow.modules.get(name) {
                    tried.push(m.path.clone());
                    if m.path.exists() {
                        return load_handle(&m.path);
                    }
                } else {
                    tried.push(home.flow_yaml(&ws));
                }
            }
            bail!(
                "module {name:?} not found. Searched:\n  - {}",
                tried
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join("\n  - ")
            );
        }
        None => {
            // Rule 3 — cwd.
            let here = cwd.join("sb.dev.yml");
            tried.push(here.clone());
            if here.exists() {
                return load_handle(&here);
            }
            bail!(
                "no sb.dev.yml in cwd ({}). Pass --module <name> or `cd` into a module dir.",
                cwd.display()
            );
        }
    }
}

fn load_handle(dev_yml: &Path) -> Result<ModuleHandle> {
    let s =
        fs::read_to_string(dev_yml).with_context(|| format!("reading {}", dev_yml.display()))?;
    let config: ModuleDevConfig =
        serde_yaml::from_str(&s).with_context(|| format!("parsing {}", dev_yml.display()))?;
    Ok(ModuleHandle {
        dev_yml_path: dev_yml.to_path_buf(),
        module_root: config.module_root(dev_yml),
        config,
        legacy_topics: legacy_topics_in(&s),
    })
}

// ─────────────────────────────────────────────────────────────────────
// GenCtx: what codegen needs beyond the module itself
// ─────────────────────────────────────────────────────────────────────

/// Everything rendering an entry needs besides the module: the merged CLI
/// config (message-target roots, default transport, iox2 variants), the
/// vault (message-type validation), and the two wire-key prefix segments.
/// Built once per CLI invocation and passed by reference.
#[derive(Debug, Clone, Copy)]
pub struct GenCtx<'a> {
    pub cfg: &'a SbCliConfig,
    pub vault: &'a Vault,
    pub workspace: &'a str,
    pub device: &'a str,
}

// ─────────────────────────────────────────────────────────────────────
// Validation helpers: vault lookup, encoding rules, duplicate detection
// ─────────────────────────────────────────────────────────────────────

/// Every message name in the vault. Listed once per operation and checked
/// with [`check_known_type`].
fn known_types(vault: &Vault) -> Result<Vec<MessageName>> {
    vault
        .list()
        .with_context(|| format!("listing vault at {}", vault.root().display()))
}

/// Validate that `msg_type` parses + exists in the vault. Renders a hint
/// pointing at `sb message list` on miss so users discover the right name.
///
/// A name may also be an **iox2 variant** — a type the build fans out from
/// one schema (`images/Image480pMono8` from `images/Image`, declared under
/// `iox2_variants`). Variants have no `.proto` of their own, so they are not
/// in `vault.list()`; they are resolved against the config instead. See
/// [`variant_base`].
fn validate_msg_type(cfg: &SbCliConfig, vault: &Vault, msg_type: &str) -> Result<MessageName> {
    check_known_type(cfg, &known_types(vault)?, msg_type)
}

/// [`validate_msg_type`] against an already-listed vault.
fn check_known_type(
    cfg: &SbCliConfig,
    known: &[MessageName],
    msg_type: &str,
) -> Result<MessageName> {
    let parsed = MessageName::parse(msg_type)
        .map_err(|e| anyhow!("invalid message name {msg_type:?}: {e}"))?;
    if known.contains(&parsed) || variant_base(cfg, &parsed).is_some() {
        return Ok(parsed);
    }
    bail!("message {msg_type:?} not in vault. Run `sb message list` to see available types.");
}

/// If `name` is a declared iox2 variant, return the base message's
/// fully-qualified proto name.
///
/// `iox2_variants` is keyed by proto FQN (`swarmbotix.images.Image`) while a
/// [`MessageName`] is `<dir>/<Leaf>` (`images/Image480pMono8`). The two are
/// bridged on the **last** package segment, which the vault layout keeps
/// equal to the directory name.
fn variant_base(cfg: &SbCliConfig, name: &MessageName) -> Option<String> {
    let variants = cfg.iox2_variants.as_ref()?;
    variants.iter().find_map(|(base_fqn, vs)| {
        let base_pkg = base_fqn.rsplit_once('.')?.0;
        let last_seg = base_pkg.rsplit('.').next()?;
        (last_seg == name.namespace && vs.contains_key(&name.leaf)).then(|| base_fqn.clone())
    })
}

/// Check an entry's `(transport, encoding, type, mime)` combination
/// before it is stored: the same rules `sb.dev.yml` loading enforces,
/// worded for the CLI, plus a well-formed `mime`.
fn check_typing(
    transport: Transport,
    encoding: Encoding,
    msg_type: Option<&str>,
    mime: Option<&str>,
) -> Result<()> {
    if encoding != Encoding::Proto && transport != Transport::Zenoh {
        bail!(
            "encoding {encoding} needs the zenoh transport: an iceoryx2 payload is always \
             the fixed-layout struct generated from the .proto"
        );
    }
    match (encoding, msg_type) {
        (Encoding::Raw, Some(t)) => {
            bail!("encoding raw takes no message type (got {t:?}); drop the type or use proto/fb")
        }
        (Encoding::Proto | Encoding::Fb, None) => {
            bail!("encoding {encoding} needs a message type (-m/--msg)")
        }
        _ => {}
    }
    match mime {
        Some(m) if encoding != Encoding::Raw => {
            bail!("mime {m:?} is only valid with encoding raw (this entry is {encoding})")
        }
        Some(m) => validate_mime(m),
        None => Ok(()),
    }
}

/// A raw entry's `mime` must be `type/subtype` (parameters after `;`
/// allowed), with no whitespace or control characters. The protobuf and
/// FlatBuffers MIME types are refused: listeners read those tags as a
/// typed payload, which a raw entry is not.
fn validate_mime(mime: &str) -> Result<()> {
    let essence = mime.split(';').next().unwrap_or_default();
    let well_formed = essence
        .split_once('/')
        .is_some_and(|(ty, sub)| !ty.is_empty() && !sub.is_empty() && !sub.contains('/'))
        && !mime.chars().any(|c| c.is_whitespace() || c.is_control());
    if !well_formed {
        bail!("invalid mime {mime:?}: expected type/subtype, e.g. text/plain or application/json");
    }
    for typed in [sb_codegen::MIME_PROTOBUF, sb_codegen::MIME_FLATBUFFERS] {
        if essence.eq_ignore_ascii_case(typed) {
            bail!(
                "mime {mime:?} names a typed payload; use encoding proto or fb with a \
                 message type instead of raw"
            );
        }
    }
    Ok(())
}

/// Is `topic` already used on `transport` by a publisher or subscriber on
/// this module? Returns a friendly error if so. The same topic on the
/// other transport is a different channel and is allowed. `skip_pub_name`
/// exempts the publisher being edited; `skip_sub_topic` the subscriber.
fn check_topic_unique(
    cfg: &ModuleDevConfig,
    topic: &TopicRef,
    transport: Transport,
    skip_pub_name: Option<&str>,
    skip_sub_topic: Option<&TopicRef>,
) -> Result<()> {
    for p in &cfg.publishers {
        if skip_pub_name == Some(&p.name) {
            continue;
        }
        if p.topic == *topic && p.transport == transport {
            bail!(
                "topic {:?} on {} already used by publisher {:?}",
                topic.as_str(),
                transport.as_str(),
                p.name
            );
        }
    }
    for s in &cfg.subscribers {
        if skip_sub_topic == Some(&s.topic) {
            continue;
        }
        if s.topic == *topic && s.transport == transport {
            bail!(
                "topic {:?} on {} already used by subscriber {:?}",
                topic.as_str(),
                transport.as_str(),
                s.name
            );
        }
    }
    Ok(())
}

/// Refuse `name` when a publisher, subscriber, or service on the module
/// already uses it; the error names the kind. `skip` exempts one entry
/// (the one being edited).
fn check_entry_unique(cfg: &ModuleDevConfig, name: &str, skip: Option<EntryKind>) -> Result<()> {
    match cfg.find_name(name) {
        Some(found) if Some(found) != skip => bail!(
            "name {name:?} is already used by {} {name:?} on module {:?}; publishers, \
             subscribers, and services share one name space",
            kind_noun(found),
            cfg.module
        ),
        _ => Ok(()),
    }
}

/// Which entry, if any, is called `name`. Publishers, subscribers, and
/// services share one name space (see [`ModuleDevConfig::find_name`]).
pub fn find_entry(cfg: &ModuleDevConfig, name: &str) -> Option<EntryKind> {
    cfg.find_name(name)
}

fn kind_noun(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::Publisher(_) => "publisher",
        EntryKind::Subscriber(_) => "subscriber",
        EntryKind::Service(_) => "service",
    }
}

fn entry_name(cfg: &ModuleDevConfig, kind: EntryKind) -> &str {
    match kind {
        EntryKind::Publisher(i) => &cfg.publishers[i].name,
        EntryKind::Subscriber(i) => &cfg.subscribers[i].name,
        EntryKind::Service(i) => &cfg.services[i].name,
    }
}

/// Derive the default publisher / subscriber `name` from a topic if the
/// user didn't pass `--name`: the leaf segment. e.g.
/// `key: dev01/ws/cam/iox2/image_raw` → `image_raw`, `hello` → `hello`.
pub fn default_name(topic: &TopicRef) -> String {
    topic.leaf().to_string()
}

/// Resolve default transport from sb.config.yml when `--zenoh` / `--iox2`
/// is absent. Defaults to the on-device transport (per requirements.md:
/// "For same-device communication, use Iceoryx2. No reason to use UDP/TCP
/// on-device.") unless the configured `transport_on_device` is overridden.
pub fn default_transport(cfg: &SbCliConfig) -> Transport {
    cfg.transport_on_device.unwrap_or(Transport::Iceoryx2)
}

// ─────────────────────────────────────────────────────────────────────
// Outcomes — what the CLI reports back to the user
// ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationOutcome {
    pub action: Action,
    /// `<module_root>/<io_dir>/{publishers,subscribers,services}/<name>.<ext>`
    /// (None for `rm`, and when a mutation was skipped).
    pub file_written: Option<PathBuf>,
    pub file_removed: Option<PathBuf>,
    pub dev_yml_written: bool,
    /// True when [`ConflictPolicy::Skip`] short-circuited a mutation.
    /// The CLI prints "skipped (already exists)" and exits 0.
    pub skipped: bool,
    /// Entries removed under [`ConflictPolicy::Force`] to make room for
    /// the new one, as `"<role> <name>"` (`pub hello`, `service nav`).
    pub overwrote: Vec<String>,
    /// Legacy `topic: /...` entries stored as `key:` by this write of
    /// `sb.dev.yml`. Zero when `dev_yml_written` is false.
    pub legacy_rewritten: usize,
}

impl MutationOutcome {
    fn skipped(action: Action) -> Self {
        Self {
            action,
            file_written: None,
            file_removed: None,
            dev_yml_written: false,
            skipped: true,
            overwrote: Vec::new(),
            legacy_rewritten: 0,
        }
    }
}

/// What [`update_all`] did for one module.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdateReport {
    pub module: String,
    /// Files rendered and written (new, or different from what was on disk).
    pub written: Vec<PathBuf>,
    /// Files whose rendered bytes already equal the file on disk; not
    /// touched.
    pub unchanged: Vec<PathBuf>,
    /// Entries that could not be rendered, as `(name, reason)`.
    pub skipped: Vec<(String, String)>,
    /// Legacy `topic: /...` entries stored as `key:` (the only change
    /// [`update_all`] makes to `sb.dev.yml`). Zero when nothing was
    /// rewritten.
    pub legacy_rewritten: usize,
}

// ─────────────────────────────────────────────────────────────────────
// ConflictPolicy — how `pub_add` / `sub_add` handle name / topic clashes
// ─────────────────────────────────────────────────────────────────────

/// What to do when `pub_add` / `sub_add` find an existing entry whose
/// name or topic collides with the new one.
///
/// Default at the API layer is [`Error`], matching the safe behaviour
/// the CLI used pre-`--force`. The CLI maps `--force`, `--skip-existing`,
/// and the interactive TTY prompt onto these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConflictPolicy {
    /// Bail with an error that lists what would have to be removed.
    /// Safe default for non-interactive use (CI, scripts).
    #[default]
    Error,
    /// Remove every colliding entry (and its generated file), then add
    /// the new one. Used when the user passes `--force` or chooses
    /// *overwrite* at the prompt.
    Force,
    /// No-op: leave the existing state alone and exit 0. Used by
    /// `--skip-existing` and *skip* at the prompt.
    Skip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    PubAdd,
    PubEdit,
    PubRm,
    SubAdd,
    SubEdit,
    SubRm,
    ServiceAdd,
    ServiceRm,
    /// [`regenerate_entry`]: a file re-rendered from an unchanged entry.
    Regenerate,
}

// ─────────────────────────────────────────────────────────────────────
// Collisions
// ─────────────────────────────────────────────────────────────────────

/// Description of one entry that would have to be removed for the new
/// entry to fit. The CLI's interactive prompt renders these before asking
/// the user to confirm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collider {
    pub role: &'static str, // "pub" | "sub" | "service"
    pub name: String,
    /// The colliding entry's bare topic or wire key (no leading `/`);
    /// empty for a service.
    pub topic: String,
    pub reason: &'static str, // "same topic" | "same name" | "same name + topic"
}

/// The reason a pub/sub entry collides with `(name, topic)`, if it does.
fn collision(same_name: bool, same_topic: bool) -> Option<&'static str> {
    match (same_name, same_topic) {
        (true, true) => Some("same name + topic"),
        (true, false) => Some("same name"),
        (false, true) => Some("same topic"),
        (false, false) => None,
    }
}

/// Every entry on this module that blocks adding `(name, topic)` on
/// `transport`: any publisher or subscriber on the same topic over the same
/// transport, and any entry of any kind with the same name. The same topic
/// on the other transport is a different channel, so one module may hold
/// `image_raw` on iceoryx2 and on zenoh at once. Empty result = no conflict.
fn find_colliders(
    cfg: &ModuleDevConfig,
    name: &str,
    topic: &TopicRef,
    transport: Transport,
    pubs_first: bool,
) -> Vec<Collider> {
    let pubs = cfg.publishers.iter().filter_map(|p| {
        let same_topic = p.topic == *topic && p.transport == transport;
        collision(p.name == name, same_topic).map(|reason| Collider {
            role: "pub",
            name: p.name.clone(),
            topic: p.topic.as_str().to_owned(),
            reason,
        })
    });
    let subs = cfg.subscribers.iter().filter_map(|s| {
        let same_topic = s.topic == *topic && s.transport == transport;
        collision(s.name == name, same_topic).map(|reason| Collider {
            role: "sub",
            name: s.name.clone(),
            topic: s.topic.as_str().to_owned(),
            reason,
        })
    });
    let mut out: Vec<Collider> = if pubs_first {
        pubs.chain(subs).collect()
    } else {
        subs.chain(pubs).collect()
    };
    out.extend(
        cfg.services
            .iter()
            .filter(|s| s.name == name)
            .map(|s| Collider {
                role: "service",
                name: s.name.clone(),
                topic: String::new(),
                reason: "same name",
            }),
    );
    out
}

/// Every entry on this module that would block adding the publisher
/// `(name, topic)` on `transport`. Empty result = no conflict.
pub fn find_pub_colliders(
    cfg: &ModuleDevConfig,
    name: &str,
    topic: &TopicRef,
    transport: Transport,
) -> Vec<Collider> {
    find_colliders(cfg, name, topic, transport, true)
}

/// Mirror of [`find_pub_colliders`] for subscribers.
pub fn find_sub_colliders(
    cfg: &ModuleDevConfig,
    name: &str,
    topic: &TopicRef,
    transport: Transport,
) -> Vec<Collider> {
    find_colliders(cfg, name, topic, transport, false)
}

/// Format colliders as an error message tail. Used both by the inline
/// error and by the CLI's interactive prompt.
pub fn format_colliders(colliders: &[Collider]) -> String {
    let mut s = String::new();
    for c in colliders {
        if c.topic.is_empty() {
            s.push_str(&format!(
                "  - {role:<3} {name:?} ({reason})\n",
                role = c.role,
                name = c.name,
                reason = c.reason,
            ));
        } else {
            s.push_str(&format!(
                "  - {role:<3} {name:?} on topic {topic:?} ({reason})\n",
                role = c.role,
                name = c.name,
                topic = c.topic,
                reason = c.reason,
            ));
        }
    }
    s
}

/// Resolve a collision according to `policy`. Returns:
///   - `Ok(Some(overwrote))` — colliders removed; caller proceeds with
///     adding the new entry.
///   - `Ok(None)` — Skip policy chose to no-op; caller returns a
///     skip outcome to the user.
///   - `Err(_)` — Error policy hit; caller bubbles the error up.
///
/// `kind` selects which "add" verb we're executing — only affects the
/// error message.
fn apply_conflict_policy(
    handle: &mut ModuleHandle,
    colliders: Vec<Collider>,
    policy: ConflictPolicy,
    kind: &'static str,
) -> Result<Option<Vec<String>>> {
    if colliders.is_empty() {
        return Ok(Some(Vec::new()));
    }
    match policy {
        ConflictPolicy::Error => Err(anyhow!(
            "{kind} blocked by {n} existing entr{y}:\n{tail}\
             Re-run with --force to overwrite, --skip-existing to no-op, \
             or `cd` to a TTY and re-run for an interactive prompt.",
            n = colliders.len(),
            y = if colliders.len() == 1 { "y" } else { "ies" },
            tail = format_colliders(&colliders),
        )),
        ConflictPolicy::Skip => Ok(None),
        ConflictPolicy::Force => {
            let mut overwrote = Vec::with_capacity(colliders.len());
            for c in &colliders {
                // Find by name: an earlier removal in this loop may have
                // shifted the indices.
                let cfg = &mut handle.config;
                let removed = match c.role {
                    "pub" => cfg
                        .publishers
                        .iter()
                        .position(|p| p.name == c.name)
                        .map(|i| (Kind::Publisher, cfg.publishers.remove(i).name)),
                    "sub" => cfg
                        .subscribers
                        .iter()
                        .position(|s| s.name == c.name)
                        .map(|i| (Kind::Subscriber, cfg.subscribers.remove(i).name)),
                    _ => cfg
                        .services
                        .iter()
                        .position(|s| s.name == c.name)
                        .map(|i| (Kind::Service, cfg.services.remove(i).name)),
                };
                if let Some((kind, name)) = removed {
                    remove_emitted_file(handle, kind, &name)?;
                    overwrote.push(format!("{} {name}", c.role));
                }
            }
            // The dev.yml is rewritten by the caller after the new entry
            // lands; no need to flush here.
            Ok(Some(overwrote))
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// pub_add / pub_edit / pub_rm
// ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PubAddArgs<'a> {
    pub topic: &'a TopicRef,
    /// Vault name (`-m`). Required for [`Encoding::Proto`] and
    /// [`Encoding::Fb`]; must be `None` for [`Encoding::Raw`].
    pub msg_type: Option<&'a str>,
    pub name: Option<&'a str>,
    pub transport: Option<Transport>,
    /// Payload framing. Anything but proto needs the zenoh transport.
    pub encoding: Encoding,
    /// Content type of a raw payload (`text/json`, …). Raw only; `None`
    /// tags samples `application/octet-stream`.
    pub mime: Option<&'a str>,
    /// What to do if an entry on this module already occupies the topic
    /// or name. Defaults to [`ConflictPolicy::Error`].
    pub policy: ConflictPolicy,
}

pub fn pub_add(
    handle: &mut ModuleHandle,
    args: &PubAddArgs<'_>,
    cfg: &SbCliConfig,
    vault: &Vault,
    workspace: &str,
    device: &str,
) -> Result<MutationOutcome> {
    let ctx = GenCtx {
        cfg,
        vault,
        workspace,
        device,
    };
    let transport = args.transport.unwrap_or_else(|| default_transport(cfg));
    check_typing(transport, args.encoding, args.msg_type, args.mime)?;
    let parsed_msg = args
        .msg_type
        .map(|m| validate_msg_type(cfg, vault, m))
        .transpose()?;
    let name = args
        .name
        .map_or_else(|| default_name(args.topic), str::to_string);
    sb_core::validate_identifier(&name)
        .map_err(|e| anyhow!("invalid publisher name {name:?}: {e}"))?;

    // Conflict resolution — handles dup-name AND dup-topic in one pass,
    // across publishers, subscribers, and services (a topic can only have
    // one role per module; a name only one entry).
    let colliders = find_pub_colliders(&handle.config, &name, args.topic, transport);
    let Some(overwrote) = apply_conflict_policy(handle, colliders, args.policy, "sb pub add")?
    else {
        return Ok(MutationOutcome::skipped(Action::PubAdd));
    };

    let spec = PubSpec {
        name,
        topic: args.topic.clone(),
        msg_type: parsed_msg.as_ref().map(ToString::to_string),
        transport,
        encoding: args.encoding,
        mime: args.mime.map(str::to_owned),
    };
    spec.validate(handle.config.publishers.len(), "publishers")
        .map_err(|e| anyhow!(e))?;
    let file_path = write_rendered(handle, &render_pub(handle, &spec, &ctx)?)?;
    handle.config.publishers.push(spec);
    let legacy_rewritten = write_dev_yml(handle)?;

    Ok(MutationOutcome {
        action: Action::PubAdd,
        file_written: Some(file_path),
        file_removed: None,
        dev_yml_written: true,
        skipped: false,
        overwrote,
        legacy_rewritten,
    })
}

/// Changes to one publisher. Every field but `name` is optional; at least
/// one must be set.
///
/// Switching to [`Encoding::Raw`] drops the stored message type, and
/// switching away from raw drops the stored `mime`, so neither has to be
/// cleared by hand. Switching away from raw needs `msg_type`.
#[derive(Debug, Clone)]
pub struct PubEditArgs<'a> {
    pub name: &'a str,
    /// New vault name (`-m`). Refused when the resulting encoding is raw.
    pub msg_type: Option<&'a str>,
    /// New bare topic (`-t`) or verbatim wire key (`--key`).
    pub topic: Option<&'a TopicRef>,
    /// New payload framing. `None` keeps the current one.
    pub encoding: Option<Encoding>,
    /// `None` keeps the current mime, `Some(None)` clears it,
    /// `Some(Some(m))` sets it. Setting it needs the resulting encoding to
    /// be raw.
    pub mime: Option<Option<&'a str>>,
}

pub fn pub_edit(
    handle: &mut ModuleHandle,
    args: &PubEditArgs<'_>,
    cfg: &SbCliConfig,
    vault: &Vault,
    workspace: &str,
    device: &str,
) -> Result<MutationOutcome> {
    if args.msg_type.is_none()
        && args.topic.is_none()
        && args.encoding.is_none()
        && args.mime.is_none()
    {
        bail!("at least one of -m/--msg, -t/--topic, or --key is required");
    }
    let ctx = GenCtx {
        cfg,
        vault,
        workspace,
        device,
    };
    let idx = handle
        .config
        .publishers
        .iter()
        .position(|p| p.name == args.name)
        .ok_or_else(|| {
            anyhow!(
                "publisher {:?} not found on module {:?}",
                args.name,
                handle.config.module
            )
        })?;

    // Take a working copy so validation runs against the proposed state.
    let mut new_spec = handle.config.publishers[idx].clone();
    if let Some(encoding) = args.encoding {
        new_spec.encoding = encoding;
    }
    if let Some(m) = args.msg_type {
        new_spec.msg_type = Some(m.to_owned());
    } else if new_spec.encoding == Encoding::Raw {
        new_spec.msg_type = None;
    }
    match args.mime {
        Some(mime) => new_spec.mime = mime.map(str::to_owned),
        None if new_spec.encoding != Encoding::Raw => new_spec.mime = None,
        None => {}
    }
    check_typing(
        new_spec.transport,
        new_spec.encoding,
        new_spec.msg_type.as_deref(),
        new_spec.mime.as_deref(),
    )?;
    if let Some(m) = args.msg_type {
        validate_msg_type(cfg, vault, m)?;
    }
    if let Some(t) = args.topic {
        check_topic_unique(&handle.config, t, new_spec.transport, Some(args.name), None)?;
        new_spec.topic = t.clone();
    }
    new_spec
        .validate(idx, "publishers")
        .map_err(|e| anyhow!(e))?;

    // Rewrite the generated file in place — same path (name unchanged).
    let file_path = write_rendered(handle, &render_pub(handle, &new_spec, &ctx)?)?;

    handle.config.publishers[idx] = new_spec;
    let legacy_rewritten = write_dev_yml(handle)?;
    Ok(MutationOutcome {
        action: Action::PubEdit,
        file_written: Some(file_path),
        file_removed: None,
        dev_yml_written: true,
        skipped: false,
        overwrote: Vec::new(),
        legacy_rewritten,
    })
}

pub fn pub_rm(handle: &mut ModuleHandle, name: &str) -> Result<MutationOutcome> {
    let idx = handle
        .config
        .publishers
        .iter()
        .position(|p| p.name == name)
        .ok_or_else(|| {
            anyhow!(
                "publisher {name:?} not found on module {:?}",
                handle.config.module
            )
        })?;
    let spec = handle.config.publishers.remove(idx);
    let removed = remove_emitted_file(handle, Kind::Publisher, &spec.name)?;
    let legacy_rewritten = write_dev_yml(handle)?;
    Ok(MutationOutcome {
        action: Action::PubRm,
        file_written: None,
        file_removed: removed,
        dev_yml_written: true,
        skipped: false,
        overwrote: Vec::new(),
        legacy_rewritten,
    })
}

// ─────────────────────────────────────────────────────────────────────
// sub_add / sub_edit / sub_rm
// ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SubAddArgs<'a> {
    pub topic: &'a TopicRef,
    /// Vault name (`-m`). Required for [`Encoding::Proto`] and
    /// [`Encoding::Fb`]; must be `None` for [`Encoding::Raw`].
    pub msg_type: Option<&'a str>,
    /// Entry name. `None` derives it from the topic's leaf
    /// ([`default_name`]). Needed when the leaf is already taken by
    /// another entry on the module (names are shared across kinds).
    pub name: Option<&'a str>,
    pub transport: Option<Transport>,
    /// Payload framing. Anything but proto needs the zenoh transport.
    pub encoding: Encoding,
    /// Content type of a raw payload. Raw only.
    pub mime: Option<&'a str>,
    pub policy: ConflictPolicy,
}

pub fn sub_add(
    handle: &mut ModuleHandle,
    args: &SubAddArgs<'_>,
    cfg: &SbCliConfig,
    vault: &Vault,
    workspace: &str,
    device: &str,
) -> Result<MutationOutcome> {
    let ctx = GenCtx {
        cfg,
        vault,
        workspace,
        device,
    };
    let transport = args.transport.unwrap_or_else(|| default_transport(cfg));
    check_typing(transport, args.encoding, args.msg_type, args.mime)?;
    let parsed_msg = args
        .msg_type
        .map(|m| validate_msg_type(cfg, vault, m))
        .transpose()?;
    let name = args
        .name
        .map_or_else(|| default_name(args.topic), str::to_string);
    sb_core::validate_identifier(&name).map_err(|e| {
        let origin = if args.name.is_some() {
            ""
        } else {
            " (derived from topic)"
        };
        anyhow!("invalid subscriber name {name:?}{origin}: {e}")
    })?;

    let colliders = find_sub_colliders(&handle.config, &name, args.topic, transport);
    let Some(overwrote) = apply_conflict_policy(handle, colliders, args.policy, "sb sub add")?
    else {
        return Ok(MutationOutcome::skipped(Action::SubAdd));
    };

    let spec = SubSpec {
        name,
        topic: args.topic.clone(),
        msg_type: parsed_msg.as_ref().map(ToString::to_string),
        transport,
        encoding: args.encoding,
        mime: args.mime.map(str::to_owned),
    };
    spec.validate(handle.config.subscribers.len(), "subscribers")
        .map_err(|e| anyhow!(e))?;
    let file_path = write_rendered(handle, &render_sub(handle, &spec, &ctx)?)?;
    handle.config.subscribers.push(spec);
    let legacy_rewritten = write_dev_yml(handle)?;
    Ok(MutationOutcome {
        action: Action::SubAdd,
        file_written: Some(file_path),
        file_removed: None,
        dev_yml_written: true,
        skipped: false,
        overwrote,
        legacy_rewritten,
    })
}

/// Index of the subscriber a user-typed bare topic or wire key names
/// (a leading `/` on a wire key is ignored). A topic held on both
/// transports names two subscribers; that is an error naming both, since
/// only the entry name tells them apart.
fn find_subscriber(handle: &ModuleHandle, input: &str) -> Result<usize> {
    let subs = &handle.config.subscribers;
    let hits: Vec<usize> = (0..subs.len())
        .filter(|&i| subs[i].topic.matches_input(input))
        .collect();
    match hits.as_slice() {
        [] => bail!(
            "subscriber for topic/key {input:?} not found on module {:?}",
            handle.config.module
        ),
        [one] => Ok(*one),
        many => {
            let names: Vec<String> = many
                .iter()
                .map(|&i| format!("{} ({})", subs[i].name, subs[i].transport.as_str()))
                .collect();
            bail!(
                "topic/key {input:?} names {} subscribers on module {:?}: {}; pick one by \
                 name with `sb io edit <name>` or `sb io rm <name>`",
                many.len(),
                handle.config.module,
                names.join(", ")
            )
        }
    }
}

/// Regenerate a subscriber's file. `input` is the bare topic or wire key
/// as the user typed it. `sb.dev.yml` is left alone unless it still holds
/// legacy `topic: /...` entries, which this rewrites as `key:`.
pub fn sub_edit(
    handle: &mut ModuleHandle,
    input: &str,
    cfg: &SbCliConfig,
    vault: &Vault,
    workspace: &str,
    device: &str,
) -> Result<MutationOutcome> {
    let idx = find_subscriber(handle, input)?;
    let ctx = GenCtx {
        cfg,
        vault,
        workspace,
        device,
    };
    let mut outcome = regenerate(handle, EntryKind::Subscriber(idx), &ctx)?;
    outcome.action = Action::SubEdit;
    Ok(outcome)
}

/// Remove a subscriber. `input` is the bare topic or wire key as the user
/// typed it (a leading `/` on a wire key is ignored).
pub fn sub_rm(handle: &mut ModuleHandle, input: &str) -> Result<MutationOutcome> {
    let idx = find_subscriber(handle, input)?;
    remove_subscriber(handle, idx)
}

/// Remove the subscriber called `name` (entry names are unique per module,
/// unlike topics, which may repeat across transports).
pub fn sub_rm_by_name(handle: &mut ModuleHandle, name: &str) -> Result<MutationOutcome> {
    let idx = handle
        .config
        .subscribers
        .iter()
        .position(|s| s.name == name)
        .ok_or_else(|| {
            anyhow!(
                "subscriber {name:?} not found on module {:?}",
                handle.config.module
            )
        })?;
    remove_subscriber(handle, idx)
}

fn remove_subscriber(handle: &mut ModuleHandle, idx: usize) -> Result<MutationOutcome> {
    let spec = handle.config.subscribers.remove(idx);
    let removed = remove_emitted_file(handle, Kind::Subscriber, &spec.name)?;
    let legacy_rewritten = write_dev_yml(handle)?;
    Ok(MutationOutcome {
        action: Action::SubRm,
        file_written: None,
        file_removed: removed,
        dev_yml_written: true,
        skipped: false,
        overwrote: Vec::new(),
        legacy_rewritten,
    })
}

// ─────────────────────────────────────────────────────────────────────
// service_add / service_rm
// ─────────────────────────────────────────────────────────────────────

/// Add a service: append `{name}` to `services:` and render
/// `<io_dir>/services/<name>.<ext>`. Rust and Python modules only. The
/// name must be a valid identifier and unused by any entry on the module.
pub fn service_add(
    handle: &mut ModuleHandle,
    name: &str,
    ctx: &GenCtx<'_>,
) -> Result<MutationOutcome> {
    sb_core::validate_identifier(name)
        .map_err(|e| anyhow!("invalid service name {name:?}: {e}"))?;
    check_entry_unique(&handle.config, name, None)?;
    let spec = ServiceSpec {
        name: name.to_owned(),
    };
    let file_path = write_rendered(handle, &render_service(handle, &spec, ctx)?)?;
    handle.config.services.push(spec);
    let legacy_rewritten = write_dev_yml(handle)?;
    Ok(MutationOutcome {
        action: Action::ServiceAdd,
        file_written: Some(file_path),
        file_removed: None,
        dev_yml_written: true,
        skipped: false,
        overwrote: Vec::new(),
        legacy_rewritten,
    })
}

/// Remove a service from `services:` and delete its generated file.
pub fn service_rm(handle: &mut ModuleHandle, name: &str) -> Result<MutationOutcome> {
    let idx = handle
        .config
        .services
        .iter()
        .position(|s| s.name == name)
        .ok_or_else(|| {
            anyhow!(
                "service {name:?} not found on module {:?}",
                handle.config.module
            )
        })?;
    let spec = handle.config.services.remove(idx);
    let removed = remove_emitted_file(handle, Kind::Service, &spec.name)?;
    let legacy_rewritten = write_dev_yml(handle)?;
    Ok(MutationOutcome {
        action: Action::ServiceRm,
        file_written: None,
        file_removed: removed,
        dev_yml_written: true,
        skipped: false,
        overwrote: Vec::new(),
        legacy_rewritten,
    })
}

// ─────────────────────────────────────────────────────────────────────
// regenerate_entry / update_all
// ─────────────────────────────────────────────────────────────────────

/// Re-render the generated file of the publisher, subscriber, or service
/// called `name` from its current entry. `sb.dev.yml` is left alone
/// unless it still holds legacy `topic: /...` entries, which this
/// rewrites as `key:`. The file is written even when its bytes would not
/// change.
pub fn regenerate_entry(
    handle: &mut ModuleHandle,
    name: &str,
    ctx: &GenCtx<'_>,
) -> Result<MutationOutcome> {
    let kind = find_entry(&handle.config, name).ok_or_else(|| {
        anyhow!(
            "no publisher, subscriber, or service named {name:?} on module {:?}",
            handle.config.module
        )
    })?;
    regenerate(handle, kind, ctx)
}

fn regenerate(
    handle: &mut ModuleHandle,
    kind: EntryKind,
    ctx: &GenCtx<'_>,
) -> Result<MutationOutcome> {
    let known = known_types(ctx.vault)?;
    let file_path = write_rendered(handle, &render_entry(handle, kind, ctx, &known)?)?;
    let dev_yml_written = !handle.legacy_topics.is_empty();
    let legacy_rewritten = if dev_yml_written {
        write_dev_yml(handle)?
    } else {
        0
    };
    Ok(MutationOutcome {
        action: Action::Regenerate,
        file_written: Some(file_path),
        file_removed: None,
        dev_yml_written,
        skipped: false,
        overwrote: Vec::new(),
        legacy_rewritten,
    })
}

/// Re-render every publisher and subscriber of the module (and every
/// service when `include_services`; services are user-edited routers, so
/// they are opt-in). A file whose rendered bytes equal what is on disk is
/// reported `unchanged` and not touched; an entry that does not render
/// (type no longer in the vault, no template for the language, …) is
/// reported in `skipped` and the rest still run. `sb.dev.yml` is left
/// alone unless it holds legacy `topic: /...` entries.
///
/// Errors only when the vault cannot be listed or a file cannot be
/// read or written.
pub fn update_all(
    handle: &mut ModuleHandle,
    ctx: &GenCtx<'_>,
    include_services: bool,
) -> Result<UpdateReport> {
    let known = known_types(ctx.vault)?;
    let cfg = &handle.config;
    let services = if include_services {
        cfg.services.len()
    } else {
        0
    };
    let kinds: Vec<EntryKind> = (0..cfg.publishers.len())
        .map(EntryKind::Publisher)
        .chain((0..cfg.subscribers.len()).map(EntryKind::Subscriber))
        .chain((0..services).map(EntryKind::Service))
        .collect();

    let mut report = UpdateReport {
        module: cfg.module.clone(),
        ..UpdateReport::default()
    };
    for kind in kinds {
        let rendered = match render_entry(handle, kind, ctx, &known) {
            Ok(r) => r,
            Err(e) => {
                let name = entry_name(&handle.config, kind).to_owned();
                report.skipped.push((name, format!("{e:#}")));
                continue;
            }
        };
        let abs = handle.io_dir().join(&rendered.rel_path);
        let on_disk = match fs::read(&abs) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e).with_context(|| format!("reading {}", abs.display())),
        };
        if on_disk.as_deref() == Some(rendered.contents.as_bytes()) {
            report.unchanged.push(abs);
        } else {
            report.written.push(write_rendered(handle, &rendered)?);
        }
    }
    if !handle.legacy_topics.is_empty() {
        report.legacy_rewritten = write_dev_yml(handle)?;
    }
    Ok(report)
}

// ─────────────────────────────────────────────────────────────────────
// list
// ─────────────────────────────────────────────────────────────────────

/// Filter for [`list`] / `sb {pub|sub} list`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Publishers, subscribers, and services.
    All,
    PubsOnly,
    SubsOnly,
    ServicesOnly,
}

/// Structured rows the CLI renders into a table. Field order matches the
/// `sb list` column spec from level3.html §"TDD plan" item 8.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListRow {
    /// `"pub"` | `"sub"` | `"service"`.
    pub kind: &'static str,
    pub name: String,
    /// Vault name; empty for a raw entry and for a service.
    pub msg_type: String,
    /// Bare topic or wire key; empty for a service.
    pub topic: String,
    /// `"zenoh"` | `"iceoryx2"`. Services are always zenoh.
    pub transport: &'static str,
    /// `"proto"` | `"fb"` | `"raw"`; empty for a service.
    pub encoding: &'static str,
}

pub fn list(handle: &ModuleHandle, scope: Scope) -> Vec<ListRow> {
    let cfg = &handle.config;
    let mut out = Vec::new();
    if matches!(scope, Scope::All | Scope::PubsOnly) {
        for p in &cfg.publishers {
            out.push(ListRow {
                kind: "pub",
                name: p.name.clone(),
                msg_type: p.msg_type.clone().unwrap_or_default(),
                topic: p.topic.as_str().into(),
                transport: p.transport.as_str(),
                encoding: p.encoding.as_str(),
            });
        }
    }
    if matches!(scope, Scope::All | Scope::SubsOnly) {
        for s in &cfg.subscribers {
            out.push(ListRow {
                kind: "sub",
                name: s.name.clone(),
                msg_type: s.msg_type.clone().unwrap_or_default(),
                topic: s.topic.as_str().into(),
                transport: s.transport.as_str(),
                encoding: s.encoding.as_str(),
            });
        }
    }
    if matches!(scope, Scope::All | Scope::ServicesOnly) {
        for s in &cfg.services {
            out.push(ListRow {
                kind: "service",
                name: s.name.clone(),
                msg_type: String::new(),
                topic: String::new(),
                transport: Transport::Zenoh.as_str(),
                encoding: "",
            });
        }
    }
    out
}

/// Render rows as the human-readable table the CLI prints. Columns:
/// `role`, `name`, `type`, `topic`, `transport`, `encoding`.
pub fn render_table(rows: &[ListRow]) -> String {
    if rows.is_empty() {
        return String::from("(no publishers or subscribers)\n");
    }
    const HEADERS: [&str; 6] = ["role", "name", "type", "topic", "transport", "encoding"];
    let cells = |r: &ListRow| -> [String; 6] {
        [
            r.kind.to_owned(),
            r.name.clone(),
            r.msg_type.clone(),
            r.topic.clone(),
            r.transport.to_owned(),
            r.encoding.to_owned(),
        ]
    };
    let mut widths = HEADERS.map(str::len);
    for r in rows {
        for (w, cell) in widths.iter_mut().zip(cells(r)) {
            *w = (*w).max(cell.len());
        }
    }
    let line = |cols: [&str; 6]| -> String {
        let mut s = String::new();
        for (i, (col, w)) in cols.iter().zip(widths).enumerate() {
            if i > 0 {
                s.push_str("  ");
            }
            s.push_str(&format!("{col:<w$}"));
        }
        s.push('\n');
        s
    };

    let mut out = line(HEADERS);
    out.push_str(&"-".repeat(out.len() - 1));
    out.push('\n');
    for r in rows {
        let c = cells(r);
        out.push_str(&line([&c[0], &c[1], &c[2], &c[3], &c[4], &c[5]]));
    }
    out
}

// ─────────────────────────────────────────────────────────────────────
// Codegen invocation + filesystem IO
// ─────────────────────────────────────────────────────────────────────

/// Render the entry at `kind`, checking a typed entry's message against
/// `known` (the listed vault) first.
fn render_entry(
    handle: &ModuleHandle,
    kind: EntryKind,
    ctx: &GenCtx<'_>,
    known: &[MessageName],
) -> Result<Rendered> {
    let cfg = &handle.config;
    let check = |msg_type: Option<&str>| {
        msg_type
            .map(|m| check_known_type(ctx.cfg, known, m))
            .transpose()
    };
    match kind {
        EntryKind::Publisher(i) => {
            check(cfg.publishers[i].msg_type.as_deref())?;
            render_pub(handle, &cfg.publishers[i], ctx)
        }
        EntryKind::Subscriber(i) => {
            check(cfg.subscribers[i].msg_type.as_deref())?;
            render_sub(handle, &cfg.subscribers[i], ctx)
        }
        EntryKind::Service(i) => render_service(handle, &cfg.services[i], ctx),
    }
}

/// A stored `type:` parsed, and the `message_targets` root of its style.
/// `(None, empty)` for a raw entry.
fn parse_stored_type(
    cfg: &SbCliConfig,
    msg_type: Option<&str>,
) -> Result<(Option<MessageName>, PathBuf)> {
    let Some(m) = msg_type else {
        return Ok((None, PathBuf::new()));
    };
    let parsed =
        MessageName::parse(m).map_err(|e| anyhow!("invalid stored msg_type {m:?}: {e}"))?;
    let targets = targets_for(cfg, &parsed)?;
    Ok((Some(parsed), targets))
}

fn render_pub(handle: &ModuleHandle, spec: &PubSpec, ctx: &GenCtx<'_>) -> Result<Rendered> {
    let (msg, targets) = parse_stored_type(ctx.cfg, spec.msg_type.as_deref())?;
    let input = CodegenInput::from_pub(
        &handle.config.module,
        ctx.workspace,
        ctx.device,
        handle.config.language,
        spec,
        msg.as_ref(),
        &targets,
    );
    render_checked(&input)
}

fn render_sub(handle: &ModuleHandle, spec: &SubSpec, ctx: &GenCtx<'_>) -> Result<Rendered> {
    let (msg, targets) = parse_stored_type(ctx.cfg, spec.msg_type.as_deref())?;
    let input = CodegenInput::from_sub(
        &handle.config.module,
        ctx.workspace,
        ctx.device,
        handle.config.language,
        spec,
        msg.as_ref(),
        &targets,
    );
    render_checked(&input)
}

fn render_service(handle: &ModuleHandle, spec: &ServiceSpec, ctx: &GenCtx<'_>) -> Result<Rendered> {
    let input = CodegenInput::from_service(
        spec,
        &handle.config.module,
        ctx.workspace,
        ctx.device,
        handle.config.language,
    );
    render_checked(&input)
}

/// Codegen pulls iox2-flat payload types from `<message_targets>/iox2/...`
/// and bakes their absolute path into the generated pub/sub.
///
/// The targets root comes from **the message's own style**, not from any
/// ambient setting. A module that publishes `swarmbotix/images/Image720pRgb8`
/// and subscribes to `ros2/sensor_msgs/Image` needs two different roots in
/// the same `sb.dev.yml`; one "current style" could not express that.
fn targets_for(cfg: &SbCliConfig, msg: &MessageName) -> Result<PathBuf> {
    sb_config::message_targets_for(cfg, &msg.style)
}

/// [`sb_codegen::render`] behind an actionable error for the
/// (language, transport, kind) combinations that have no template.
fn render_checked(input: &CodegenInput<'_>) -> Result<Rendered> {
    let (language, transport) = (input.language, input.transport);
    if !sb_codegen::template_exists(language, transport, input.kind) {
        // Structurally different reasons a template might be missing.
        // Surface each one clearly so users don't waste time chasing the
        // wrong fix.
        if input.kind == Kind::Service {
            bail!(
                "services are generated for Rust and Python modules only; module {:?} is {language}",
                input.module
            );
        }
        let impossible = matches!(
            (language, transport),
            (Language::Flutter, Transport::Iceoryx2) | (Language::Unity, Transport::Iceoryx2)
        );
        if impossible {
            bail!(
                "{language} has no iceoryx2 path — Flutter and Unity run on mobile / sandboxed game runtimes where shared memory is unavailable. Use --zenoh."
            );
        }
        bail!(
            "{language} {transport} codegen is not yet implemented at this point in L3 \
             (this build only ships the templates for languages that have landed). \
             See plan/lv3_report.md for the slice schedule.",
            transport = transport.as_str()
        );
    }
    sb_codegen::render(input)
        .with_context(|| format!("rendering {} {} template", language, transport.as_str()))
}

/// Write a rendered file under the module's io dir; returns its path.
fn write_rendered(handle: &ModuleHandle, rendered: &Rendered) -> Result<PathBuf> {
    let abs = handle.io_dir().join(&rendered.rel_path);
    if let Some(parent) = abs.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(&abs, &rendered.contents).with_context(|| format!("writing {}", abs.display()))?;
    Ok(abs)
}

fn ext_for(language: Language) -> &'static str {
    match language {
        Language::Rust => "rs",
        Language::Python => "py",
        Language::Cpp => "cpp",
        Language::Flutter => "dart",
        Language::Unity => "cs",
    }
}

fn remove_emitted_file(handle: &ModuleHandle, kind: Kind, name: &str) -> Result<Option<PathBuf>> {
    let ext = ext_for(handle.config.language);
    let path = handle
        .io_dir()
        .join(kind.dir())
        .join(format!("{name}.{ext}"));
    if path.exists() {
        fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        Ok(Some(path))
    } else {
        Ok(None)
    }
}

/// Write `sb.dev.yml` back. Every entry is serialised in its canonical
/// form, so any legacy `topic: /...` entries become `key:`. Returns how
/// many legacy entries the file held (and clears that list).
fn write_dev_yml(handle: &mut ModuleHandle) -> Result<usize> {
    let body = serde_yaml::to_string(&handle.config).context("serializing sb.dev.yml")?;
    fs::write(&handle.dev_yml_path, body)
        .with_context(|| format!("writing {}", handle.dev_yml_path.display()))?;
    let rewritten = handle.legacy_topics.len();
    handle.legacy_topics.clear();
    Ok(rewritten)
}

// ─────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// A vault holding `ros2/std/String` and the config pointing at it.
    struct Env {
        _tmp: tempfile::TempDir,
        cfg: SbCliConfig,
        vault: Vault,
    }

    /// An [`Env`] plus a Rust module `m` with no entries.
    fn fixture() -> (Env, ModuleHandle) {
        let tmp = tempfile::TempDir::new().unwrap();
        let messages = tmp.path().join("messages");
        let std_dir = messages.join("ros2/message_definitions/std");
        fs::create_dir_all(&std_dir).unwrap();
        fs::write(std_dir.join("String.proto"), "syntax = \"proto3\";\n").unwrap();
        let module_dir = tmp.path().join("m");
        fs::create_dir_all(&module_dir).unwrap();
        let dev_yml = module_dir.join("sb.dev.yml");
        fs::write(&dev_yml, "module: m\nlanguage: rust\n").unwrap();
        let env = Env {
            cfg: SbCliConfig {
                messages_root: Some(messages.clone()),
                transport_on_device: Some(Transport::Zenoh),
                ..SbCliConfig::default()
            },
            vault: Vault::at(messages),
            _tmp: tmp,
        };
        (env, load_handle(&dev_yml).unwrap())
    }

    /// The handle as `sb.dev.yml` now stores it.
    fn reload(handle: &ModuleHandle) -> ModuleHandle {
        load_handle(&handle.dev_yml_path).unwrap()
    }

    fn pub_args<'a>(topic: &'a TopicRef, msg_type: Option<&'a str>) -> PubAddArgs<'a> {
        PubAddArgs {
            topic,
            msg_type,
            name: None,
            transport: None,
            encoding: Encoding::Proto,
            mime: None,
            policy: ConflictPolicy::Error,
        }
    }

    #[test]
    fn typing_rules_are_enforced_before_any_write() {
        let (env, mut handle) = fixture();
        let (cfg, vault) = (env.cfg.clone(), env.vault.clone());
        let topic = TopicRef::Owned("blob".into());
        let add = |handle: &mut ModuleHandle, args: PubAddArgs<'_>| {
            pub_add(handle, &args, &cfg, &vault, "ws", "dev01").map_err(|e| format!("{e:#}"))
        };
        let base = pub_args(&topic, None);
        let cases = [
            (
                PubAddArgs {
                    encoding: Encoding::Raw,
                    msg_type: Some("ros2/std/String"),
                    ..base.clone()
                },
                "takes no message type",
            ),
            (base.clone(), "needs a message type"),
            (
                PubAddArgs {
                    msg_type: Some("ros2/std/String"),
                    mime: Some("text/plain"),
                    ..base.clone()
                },
                "only valid with encoding raw",
            ),
            (
                PubAddArgs {
                    encoding: Encoding::Raw,
                    transport: Some(Transport::Iceoryx2),
                    ..base.clone()
                },
                "needs the zenoh transport",
            ),
            (
                PubAddArgs {
                    encoding: Encoding::Raw,
                    mime: Some("not a mime"),
                    ..base.clone()
                },
                "invalid mime",
            ),
            (
                PubAddArgs {
                    encoding: Encoding::Raw,
                    mime: Some("application/protobuf;ros2/std/String"),
                    ..base.clone()
                },
                "names a typed payload",
            ),
        ];
        for (args, expected) in cases {
            let err = add(&mut handle, args).unwrap_err();
            assert!(err.contains(expected), "expected {expected:?} in {err:?}");
        }
        assert!(handle.config.publishers.is_empty());
        assert!(!handle.io_dir().exists());

        // A raw publisher with a mime is accepted and stored without a type.
        add(
            &mut handle,
            PubAddArgs {
                encoding: Encoding::Raw,
                mime: Some("text/json"),
                ..base
            },
        )
        .unwrap();
        handle = reload(&handle);
        let stored = &handle.config.publishers[0];
        assert_eq!(
            (
                stored.msg_type.as_deref(),
                stored.encoding,
                stored.mime.as_deref()
            ),
            (None, Encoding::Raw, Some("text/json"))
        );
    }
}
