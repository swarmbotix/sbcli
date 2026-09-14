//! L3 — `sb pub/sub` command logic.
//!
//! Owns the boring bits the CLI delegates to:
//!
//! - **Module resolution** ([`resolve_module`]) — three lookup rules.
//! - **Mutation** ([`pub_add`], [`pub_edit`], [`pub_rm`], [`sub_add`],
//!   [`sub_edit`], [`sub_rm`]) — reads `sb.dev.yml`, validates against
//!   the vault, mutates the in-memory `ModuleDevConfig`, writes it back,
//!   and invokes [`sb_codegen`] to emit / update / remove the matching
//!   language file under `<io_dir>/{publishers,subscribers}/`.
//! - **Listing** ([`list`]) — pure read; produces a structured view the
//!   CLI renders as a table.
//!
//! Filesystem state owned by this crate (per module):
//!
//! ```text
//! <module_root>/
//!   sb.dev.yml                    ← read + write
//!   <io_dir>/publishers/<n>.<ext> ← write + delete
//!   <io_dir>/subscribers/<n>.<ext>
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

use sb_codegen::{CodegenInput, Kind, Rendered};
use sb_core::{Language, ModuleDevConfig, PubSpec, SbCliConfig, SubSpec, Transport};
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
    pub module_root: PathBuf,
    pub config: ModuleDevConfig,
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
                if let Some(p) = flow.modules.get(name) {
                    tried.push(p.clone());
                    if p.exists() {
                        return load_handle(p);
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
    let module_root = dev_yml
        .parent()
        .ok_or_else(|| anyhow!("sb.dev.yml has no parent: {}", dev_yml.display()))?
        .to_path_buf();
    Ok(ModuleHandle {
        dev_yml_path: dev_yml.to_path_buf(),
        module_root,
        config,
    })
}

// ─────────────────────────────────────────────────────────────────────
// Validation helpers — vault lookup, duplicate detection
// ─────────────────────────────────────────────────────────────────────

/// Validate that `msg_type` parses + exists in the vault. Renders a hint
/// pointing at `sb message list` on miss so users discover the right name.
///
/// A name may also be an **iox2 variant** — a type the build fans out from
/// one schema (`images/Image480pMono8` from `images/Image`, declared under
/// `iox2_variants`). Variants have no `.proto` of their own, so they are not
/// in `vault.list()`; they are resolved against the config instead. See
/// [`variant_base`].
fn validate_msg_type(cfg: &SbCliConfig, vault: &Vault, msg_type: &str) -> Result<MessageName> {
    let parsed = MessageName::parse(msg_type)
        .map_err(|e| anyhow!("invalid message name {msg_type:?}: {e}"))?;
    let known = vault
        .list()
        .with_context(|| format!("listing vault at {}", vault.root().display()))?;
    if known.iter().any(|n| n == &parsed) {
        return Ok(parsed);
    }
    if variant_base(cfg, &parsed).is_some() {
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

/// Build a [`PubSpec`] or [`SubSpec`] check: is `topic` already used by a
/// publisher or subscriber on this module? Returns a friendly error if so.
fn check_topic_unique(
    cfg: &ModuleDevConfig,
    topic: &str,
    skip_pub_name: Option<&str>,
    skip_sub_topic: Option<&str>,
) -> Result<()> {
    for p in &cfg.publishers {
        if skip_pub_name == Some(&p.name) {
            continue;
        }
        if p.topic == topic {
            bail!("topic {topic:?} already used by publisher {:?}", p.name);
        }
    }
    for s in &cfg.subscribers {
        if skip_sub_topic == Some(&s.topic) {
            continue;
        }
        if s.topic == topic {
            bail!("topic {topic:?} already used by subscriber {:?}", s.name);
        }
    }
    Ok(())
}

/// Derive the default publisher / subscriber `name` from a topic if the
/// user didn't pass `--name`. Last `/`-separated segment, leading `/`
/// stripped. e.g. `/dev01/ws/cam/iox2/image_raw` → `image_raw`, `hello` → `hello`.
pub fn default_name_from_topic(topic: &str) -> String {
    topic
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(topic)
        .to_string()
}

/// Resolve default transport from sb.config.yml when `--zenoh` / `--iox2`
/// is absent. Defaults to the on-device transport (per requirements.md:
/// "For same-device communication, use Iceoryx2. No reason to use UDP/TCP
/// on-device.") unless the configured `transport_on_device` is overridden.
fn default_transport(cfg: &SbCliConfig) -> Transport {
    cfg.transport_on_device.unwrap_or(Transport::Iceoryx2)
}

// ─────────────────────────────────────────────────────────────────────
// Outcomes — what the CLI reports back to the user
// ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationOutcome {
    pub action: Action,
    /// `<module_root>/<io_dir>/{publishers,subscribers}/<name>.<ext>`
    /// (None for `list` / `rm` when no file existed yet).
    pub file_written: Option<PathBuf>,
    pub file_removed: Option<PathBuf>,
    pub dev_yml_written: bool,
    /// True when [`ConflictPolicy::Skip`] short-circuited a mutation.
    /// The CLI prints "skipped (already exists)" and exits 0.
    pub skipped: bool,
    /// Names of publishers / subscribers removed under
    /// [`ConflictPolicy::Force`] to make room for the new entry.
    pub overwrote: Vec<String>,
}

// ─────────────────────────────────────────────────────────────────────
// ConflictPolicy — how `pub_add` / `sub_add` handle name / topic clashes
// ─────────────────────────────────────────────────────────────────────

/// What to do when `pub_add` / `sub_add` find an existing publisher or
/// subscriber whose name or topic collides with the new entry.
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
    /// Remove every colliding publisher / subscriber (and their generated
    /// files), then add the new entry. Used when the user passes
    /// `--force` or chooses *overwrite* at the prompt.
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
}

// ─────────────────────────────────────────────────────────────────────
// pub_add / pub_edit / pub_rm
// ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PubAddArgs<'a> {
    pub topic: &'a str,
    pub msg_type: &'a str,
    pub name: Option<&'a str>,
    pub transport: Option<Transport>,
    /// What to do if a publisher / subscriber on this module already
    /// occupies the topic or name. Defaults to [`ConflictPolicy::Error`].
    pub policy: ConflictPolicy,
}

/// Description of one publisher / subscriber that would have to be
/// removed for the new entry to fit. The CLI's interactive prompt
/// renders these before asking the user to confirm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collider {
    pub role: &'static str, // "pub" | "sub"
    pub name: String,
    pub topic: String,
    pub reason: &'static str, // "same topic" | "same name" | "same name + topic"
}

/// Find every publisher / subscriber on this module that would block
/// adding `(name, topic)`. Empty result = no conflict.
pub fn find_pub_colliders(cfg: &ModuleDevConfig, name: &str, topic: &str) -> Vec<Collider> {
    let mut out = Vec::new();
    for p in &cfg.publishers {
        let same_name = p.name == name;
        let same_topic = p.topic == topic;
        if same_name && same_topic {
            out.push(Collider {
                role: "pub",
                name: p.name.clone(),
                topic: p.topic.clone(),
                reason: "same name + topic",
            });
        } else if same_name {
            out.push(Collider {
                role: "pub",
                name: p.name.clone(),
                topic: p.topic.clone(),
                reason: "same name",
            });
        } else if same_topic {
            out.push(Collider {
                role: "pub",
                name: p.name.clone(),
                topic: p.topic.clone(),
                reason: "same topic",
            });
        }
    }
    for s in &cfg.subscribers {
        if s.topic == topic {
            out.push(Collider {
                role: "sub",
                name: s.name.clone(),
                topic: s.topic.clone(),
                reason: "same topic",
            });
        }
    }
    out
}

/// Mirror of [`find_pub_colliders`] for subscribers.
pub fn find_sub_colliders(cfg: &ModuleDevConfig, name: &str, topic: &str) -> Vec<Collider> {
    let mut out = Vec::new();
    for s in &cfg.subscribers {
        let same_name = s.name == name;
        let same_topic = s.topic == topic;
        if same_name && same_topic {
            out.push(Collider {
                role: "sub",
                name: s.name.clone(),
                topic: s.topic.clone(),
                reason: "same name + topic",
            });
        } else if same_name {
            out.push(Collider {
                role: "sub",
                name: s.name.clone(),
                topic: s.topic.clone(),
                reason: "same name",
            });
        } else if same_topic {
            out.push(Collider {
                role: "sub",
                name: s.name.clone(),
                topic: s.topic.clone(),
                reason: "same topic",
            });
        }
    }
    for p in &cfg.publishers {
        if p.topic == topic {
            out.push(Collider {
                role: "pub",
                name: p.name.clone(),
                topic: p.topic.clone(),
                reason: "same topic",
            });
        }
    }
    out
}

/// Format colliders as an error message tail. Used both by the inline
/// error and by the CLI's interactive prompt.
pub fn format_colliders(colliders: &[Collider]) -> String {
    let mut s = String::new();
    for c in colliders {
        s.push_str(&format!(
            "  - {role:<3} {name:?} on topic {topic:?} ({reason})\n",
            role = c.role,
            name = c.name,
            topic = c.topic,
            reason = c.reason,
        ));
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
                if c.role == "pub" {
                    // Find by name (after any prior removal in this loop the
                    // index may have shifted).
                    if let Some(idx) = handle
                        .config
                        .publishers
                        .iter()
                        .position(|p| p.name == c.name)
                    {
                        let spec = handle.config.publishers.remove(idx);
                        let _ = remove_emitted_file(handle, Kind::Publisher, &spec.name)?;
                        overwrote.push(format!("pub {}", spec.name));
                    }
                } else if c.role == "sub" {
                    if let Some(idx) = handle
                        .config
                        .subscribers
                        .iter()
                        .position(|s| s.name == c.name)
                    {
                        let spec = handle.config.subscribers.remove(idx);
                        let _ = remove_emitted_file(handle, Kind::Subscriber, &spec.name)?;
                        overwrote.push(format!("sub {}", spec.name));
                    }
                }
            }
            // The dev.yml is rewritten by the caller after the new entry
            // lands; no need to flush here.
            Ok(Some(overwrote))
        }
    }
}

pub fn pub_add(
    handle: &mut ModuleHandle,
    args: &PubAddArgs<'_>,
    cfg: &SbCliConfig,
    vault: &Vault,
    workspace: &str,
    device: &str,
) -> Result<MutationOutcome> {
    let parsed_msg = validate_msg_type(cfg, vault, args.msg_type)?;
    let name = args
        .name
        .map(str::to_string)
        .unwrap_or_else(|| default_name_from_topic(args.topic));
    sb_core::validate_identifier(&name)
        .map_err(|e| anyhow!("invalid publisher name {name:?}: {e}"))?;

    // Conflict resolution — handles dup-name AND dup-topic in one pass,
    // across both publishers and subscribers (a topic can only have one
    // role per module).
    let colliders = find_pub_colliders(&handle.config, &name, args.topic);
    let overwrote = match apply_conflict_policy(handle, colliders, args.policy, "sb pub add")? {
        Some(v) => v,
        None => {
            return Ok(MutationOutcome {
                action: Action::PubAdd,
                file_written: None,
                file_removed: None,
                dev_yml_written: false,
                skipped: true,
                overwrote: Vec::new(),
            });
        }
    };

    let transport = args.transport.unwrap_or_else(|| default_transport(cfg));
    let spec = PubSpec {
        name: name.clone(),
        topic: args.topic.to_string(),
        msg_type: format!("{}", parsed_msg),
        transport,
    };
    let file_path = emit_pub(handle, &spec, &parsed_msg, workspace, device, cfg)?;
    handle.config.publishers.push(spec);
    write_dev_yml(handle)?;

    Ok(MutationOutcome {
        action: Action::PubAdd,
        file_written: Some(file_path),
        file_removed: None,
        dev_yml_written: true,
        skipped: false,
        overwrote,
    })
}

#[derive(Debug, Clone)]
pub struct PubEditArgs<'a> {
    pub name: &'a str,
    pub msg_type: Option<&'a str>,
    pub topic: Option<&'a str>,
}

pub fn pub_edit(
    handle: &mut ModuleHandle,
    args: &PubEditArgs<'_>,
    cfg: &SbCliConfig,
    vault: &Vault,
    workspace: &str,
    device: &str,
) -> Result<MutationOutcome> {
    if args.msg_type.is_none() && args.topic.is_none() {
        bail!("at least one of -m/--msg or -t/--topic is required");
    }
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
    if let Some(m) = args.msg_type {
        let _ = validate_msg_type(cfg, vault, m)?;
        new_spec.msg_type = m.to_string();
    }
    if let Some(t) = args.topic {
        check_topic_unique(&handle.config, t, Some(args.name), None)?;
        new_spec.topic = t.to_string();
    }

    let parsed_msg = MessageName::parse(&new_spec.msg_type)
        .map_err(|e| anyhow!("invalid stored msg_type {:?}: {e}", new_spec.msg_type))?;
    // Rewrite the generated file in place — same path (name unchanged).
    let file_path = emit_pub(handle, &new_spec, &parsed_msg, workspace, device, cfg)?;

    handle.config.publishers[idx] = new_spec;
    write_dev_yml(handle)?;
    Ok(MutationOutcome {
        action: Action::PubEdit,
        file_written: Some(file_path),
        file_removed: None,
        dev_yml_written: true,
        skipped: false,
        overwrote: Vec::new(),
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
    write_dev_yml(handle)?;
    Ok(MutationOutcome {
        action: Action::PubRm,
        file_written: None,
        file_removed: removed,
        dev_yml_written: true,
        skipped: false,
        overwrote: Vec::new(),
    })
}

// ─────────────────────────────────────────────────────────────────────
// sub_add / sub_edit / sub_rm
// ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SubAddArgs<'a> {
    pub topic: &'a str,
    pub msg_type: &'a str,
    pub transport: Option<Transport>,
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
    let parsed_msg = validate_msg_type(cfg, vault, args.msg_type)?;
    let name = default_name_from_topic(args.topic);
    sb_core::validate_identifier(&name)
        .map_err(|e| anyhow!("invalid subscriber name {name:?} (derived from topic): {e}"))?;

    let colliders = find_sub_colliders(&handle.config, &name, args.topic);
    let overwrote = match apply_conflict_policy(handle, colliders, args.policy, "sb sub add")? {
        Some(v) => v,
        None => {
            return Ok(MutationOutcome {
                action: Action::SubAdd,
                file_written: None,
                file_removed: None,
                dev_yml_written: false,
                skipped: true,
                overwrote: Vec::new(),
            });
        }
    };

    let transport = args.transport.unwrap_or_else(|| default_transport(cfg));
    let spec = SubSpec {
        name: name.clone(),
        topic: args.topic.to_string(),
        msg_type: format!("{}", parsed_msg),
        transport,
    };
    let file_path = emit_sub(handle, &spec, &parsed_msg, workspace, device, cfg)?;
    handle.config.subscribers.push(spec);
    write_dev_yml(handle)?;
    Ok(MutationOutcome {
        action: Action::SubAdd,
        file_written: Some(file_path),
        file_removed: None,
        dev_yml_written: true,
        skipped: false,
        overwrote,
    })
}

pub fn sub_edit(
    handle: &mut ModuleHandle,
    topic: &str,
    cfg: &SbCliConfig,
    vault: &Vault,
    workspace: &str,
    device: &str,
) -> Result<MutationOutcome> {
    let idx = handle
        .config
        .subscribers
        .iter()
        .position(|s| s.topic == topic)
        .ok_or_else(|| {
            anyhow!(
                "subscriber for topic {topic:?} not found on module {:?}",
                handle.config.module
            )
        })?;
    let spec = handle.config.subscribers[idx].clone();
    let parsed_msg = validate_msg_type(cfg, vault, &spec.msg_type)?;
    let file_path = emit_sub(handle, &spec, &parsed_msg, workspace, device, cfg)?;
    Ok(MutationOutcome {
        action: Action::SubEdit,
        file_written: Some(file_path),
        file_removed: None,
        dev_yml_written: false,
        skipped: false,
        overwrote: Vec::new(),
    })
}

pub fn sub_rm(handle: &mut ModuleHandle, topic: &str) -> Result<MutationOutcome> {
    let idx = handle
        .config
        .subscribers
        .iter()
        .position(|s| s.topic == topic)
        .ok_or_else(|| {
            anyhow!(
                "subscriber for topic {topic:?} not found on module {:?}",
                handle.config.module
            )
        })?;
    let spec = handle.config.subscribers.remove(idx);
    let removed = remove_emitted_file(handle, Kind::Subscriber, &spec.name)?;
    write_dev_yml(handle)?;
    Ok(MutationOutcome {
        action: Action::SubRm,
        file_written: None,
        file_removed: removed,
        dev_yml_written: true,
        skipped: false,
        overwrote: Vec::new(),
    })
}

// ─────────────────────────────────────────────────────────────────────
// list
// ─────────────────────────────────────────────────────────────────────

/// Filter for [`list`] / `sb {pub|sub} list`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    All,
    PubsOnly,
    SubsOnly,
}

/// Structured rows the CLI renders into a table. Field order matches the
/// `sb list` column spec from level3.html §"TDD plan" item 8.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListRow {
    pub role: &'static str, // "pub" | "sub"
    pub name: String,
    pub msg_type: String,
    pub topic: String,
    pub transport: &'static str,
}

pub fn list(handle: &ModuleHandle, scope: Scope) -> Vec<ListRow> {
    let mut out = Vec::new();
    if matches!(scope, Scope::All | Scope::PubsOnly) {
        for p in &handle.config.publishers {
            out.push(ListRow {
                role: "pub",
                name: p.name.clone(),
                msg_type: p.msg_type.clone(),
                topic: p.topic.clone(),
                transport: p.transport.as_str(),
            });
        }
    }
    if matches!(scope, Scope::All | Scope::SubsOnly) {
        for s in &handle.config.subscribers {
            out.push(ListRow {
                role: "sub",
                name: s.name.clone(),
                msg_type: s.msg_type.clone(),
                topic: s.topic.clone(),
                transport: s.transport.as_str(),
            });
        }
    }
    out
}

/// Render rows as the human-readable table the CLI prints.
pub fn render_table(rows: &[ListRow]) -> String {
    if rows.is_empty() {
        return String::from("(no publishers or subscribers)\n");
    }
    // Column widths.
    let w_role = "role"
        .len()
        .max(rows.iter().map(|r| r.role.len()).max().unwrap_or(0));
    let w_name = "name"
        .len()
        .max(rows.iter().map(|r| r.name.len()).max().unwrap_or(0));
    let w_type = "type"
        .len()
        .max(rows.iter().map(|r| r.msg_type.len()).max().unwrap_or(0));
    let w_topic = "topic"
        .len()
        .max(rows.iter().map(|r| r.topic.len()).max().unwrap_or(0));
    let w_tr = "transport"
        .len()
        .max(rows.iter().map(|r| r.transport.len()).max().unwrap_or(0));

    let mut out = String::new();
    let header = format!(
        "{role:<w_role$}  {name:<w_name$}  {ty:<w_type$}  {topic:<w_topic$}  {tr:<w_tr$}\n",
        role = "role",
        name = "name",
        ty = "type",
        topic = "topic",
        tr = "transport",
    );
    out.push_str(&header);
    out.push_str(&"-".repeat(header.len() - 1));
    out.push('\n');
    for r in rows {
        out.push_str(&format!(
            "{role:<w_role$}  {name:<w_name$}  {ty:<w_type$}  {topic:<w_topic$}  {tr:<w_tr$}\n",
            role = r.role,
            name = r.name,
            ty = r.msg_type,
            topic = r.topic,
            tr = r.transport,
        ));
    }
    out
}

// ─────────────────────────────────────────────────────────────────────
// Codegen invocation + filesystem IO
// ─────────────────────────────────────────────────────────────────────

fn emit_pub(
    handle: &ModuleHandle,
    spec: &PubSpec,
    parsed_msg: &MessageName,
    workspace: &str,
    device: &str,
    cfg: &SbCliConfig,
) -> Result<PathBuf> {
    let targets = targets_for(cfg, parsed_msg)?;
    let input = CodegenInput::from_pub(
        &handle.config.module,
        workspace,
        device,
        handle.config.language,
        spec,
        parsed_msg,
        &targets,
    );
    emit_into_io_dir(handle, handle.config.language, spec.transport, &input)
}

fn emit_sub(
    handle: &ModuleHandle,
    spec: &SubSpec,
    parsed_msg: &MessageName,
    workspace: &str,
    device: &str,
    cfg: &SbCliConfig,
) -> Result<PathBuf> {
    let targets = targets_for(cfg, parsed_msg)?;
    let input = CodegenInput::from_sub(
        &handle.config.module,
        workspace,
        device,
        handle.config.language,
        spec,
        parsed_msg,
        &targets,
    );
    emit_into_io_dir(handle, handle.config.language, spec.transport, &input)
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

fn emit_into_io_dir(
    handle: &ModuleHandle,
    language: Language,
    transport: Transport,
    input: &CodegenInput<'_>,
) -> Result<PathBuf> {
    if !sb_codegen::template_exists(language, transport, input.kind) {
        // Two structurally-different reasons a template might be missing.
        // Surface each one clearly so users don't waste time chasing the
        // wrong fix.
        let impossible = matches!(
            (language, transport),
            (Language::Flutter, Transport::Iceoryx2) | (Language::Unity, Transport::Iceoryx2)
        );
        if impossible {
            bail!(
                "{language} has no iceoryx2 path — Flutter and Unity run on mobile / sandboxed game runtimes where shared memory is unavailable. Use --zenoh."
            );
        } else {
            bail!(
                "{language} {transport} codegen is not yet implemented at this point in L3 \
                 (this build only ships the templates for languages that have landed). \
                 See plan/lv3_report.md for the slice schedule.",
                language = language,
                transport = transport.as_str()
            );
        }
    }
    let Rendered { rel_path, contents } = sb_codegen::render(input)
        .with_context(|| format!("rendering {} {} template", language, transport.as_str()))?;
    let abs = handle.io_dir().join(&rel_path);
    if let Some(parent) = abs.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(&abs, contents).with_context(|| format!("writing {}", abs.display()))?;
    Ok(abs)
}

fn remove_emitted_file(handle: &ModuleHandle, kind: Kind, name: &str) -> Result<Option<PathBuf>> {
    let ext = match handle.config.language {
        Language::Rust => "rs",
        Language::Python => "py",
        Language::Cpp => "cpp",
        Language::Flutter => "dart",
        Language::Unity => "cs",
    };
    let dir = match kind {
        Kind::Publisher => "publishers",
        Kind::Subscriber => "subscribers",
    };
    let path = handle.io_dir().join(dir).join(format!("{name}.{ext}"));
    if path.exists() {
        fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        Ok(Some(path))
    } else {
        Ok(None)
    }
}

fn write_dev_yml(handle: &ModuleHandle) -> Result<()> {
    let body = serde_yaml::to_string(&handle.config).context("serializing sb.dev.yml")?;
    fs::write(&handle.dev_yml_path, body)
        .with_context(|| format!("writing {}", handle.dev_yml_path.display()))?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_name_from_topic_simple() {
        assert_eq!(default_name_from_topic("hello"), "hello");
        assert_eq!(
            default_name_from_topic("/dev01/ws/mod/iox2/image_raw"),
            "image_raw"
        );
        assert_eq!(default_name_from_topic("/a/b/"), "/a/b/");
    }

    #[test]
    fn check_topic_unique_detects_pub_collision() {
        let mut cfg = ModuleDevConfig {
            module: "m".into(),
            language: Language::Rust,
            root: PathBuf::from("/tmp"),
            io_dir: PathBuf::from("swarmbotix_io"),
            publishers: vec![PubSpec {
                name: "hello".into(),
                topic: "hello".into(),
                msg_type: "std/StringStamped".into(),
                transport: Transport::Zenoh,
            }],
            subscribers: vec![],
        };
        let err = check_topic_unique(&cfg, "hello", None, None).unwrap_err();
        assert!(format!("{err:#}").contains("hello"));
        // Skipping the same pub's name allows reusing its topic (used by `pub edit`).
        cfg.publishers[0].name = "hello".into();
        assert!(check_topic_unique(&cfg, "hello", Some("hello"), None).is_ok());
    }

    #[test]
    fn render_table_empty_message() {
        assert_eq!(render_table(&[]), "(no publishers or subscribers)\n");
    }

    #[test]
    fn render_table_columns_align() {
        let rows = vec![
            ListRow {
                role: "pub",
                name: "hello".into(),
                msg_type: "std/StringStamped".into(),
                topic: "hello".into(),
                transport: "zenoh",
            },
            ListRow {
                role: "sub",
                name: "image_raw".into(),
                msg_type: "std/ImageStamped".into(),
                topic: "/dev01/ws/cam/iox2/image_raw".into(),
                transport: "iceoryx2",
            },
        ];
        let out = render_table(&rows);
        // Header present.
        assert!(out.starts_with("role"));
        // Each row appears on its own line. `role` column is padded to
        // max("role".len()=4, "pub"=3, "sub"=3) = 4 chars.
        let sub_line = out
            .lines()
            .find(|l| l.contains("image_raw"))
            .expect("missing sub row");
        assert!(sub_line.starts_with("sub "), "sub row: {sub_line:?}");
        assert!(sub_line.contains("image_raw"));
        assert!(sub_line.contains("iceoryx2"));
        // Padding makes columns line up — every line same length.
        let lens: Vec<usize> = out.lines().map(|l| l.len()).collect();
        let first = lens[0];
        for (i, l) in lens.iter().enumerate() {
            // Separator line is one shorter (no trailing space) — accept.
            assert!(
                *l == first || *l == first - 1,
                "row {i} length {l} != header length {first}: {:?}",
                out.lines().nth(i)
            );
        }
    }
}
