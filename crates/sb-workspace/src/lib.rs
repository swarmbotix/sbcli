//! L2 — workspace lifecycle and module init.
//!
//! Three concerns, one crate:
//!
//! 1. **Workspace registry** under `~/.swarmbotix/`:
//!    - `workspaces/<name>/sb.workspace.yaml`: the module inventory
//!    - `workspaces/<name>/flows/<flow>.flow.yaml`: one file per flow
//!    - `workspaces/<name>/active_flow`: the flow `sb up` launched last
//!    - `active`: one-line marker file holding the active workspace name
//!
//! 2. **Install from a repository**: a repo-root `sb.workspace.yaml`
//!    (relative paths, committed with the code) and every `*.flow.yaml`
//!    beside it are the source of truth for a workspace.
//!    [`install_workspace`] derives the installed copy under
//!    `workspaces/<name>/` from them (absolute module and message paths
//!    plus a `source:` block in the workspace file; flows copied as they
//!    are, since they name nodes only) and never writes the repo copy. A
//!    repo-root `sb.flow.yaml` in the 0.7 shape (modules and wiring in one
//!    file) is still installed in 0.8.0: it is converted into a workspace
//!    and its default flow, and reported as deprecated.
//!
//! 3. **Module init** (`sb init`): adopt an existing user project as a
//!    swarmbotix module by writing `sb.module.yaml` and an empty
//!    `swarmbotix_io/`, then registering the module: in the enclosing
//!    repo's `sb.workspace.yaml` when there is one (followed by an
//!    install), otherwise in the active workspace's installed copy. A
//!    module is never added to a flow; flows are written by hand.
//!
//! File names: a module directory is probed for `sb.module.yaml` only
//! ([`find_module_file`]); a repository for `sb.workspace.yaml`
//! ([`find_workspace_file`]), its flows (`sb.flow.yaml` is the flow named
//! `default`, `<name>.flow.yaml` the flow `<name>`; [`find_flow_files`]),
//! and a 0.7 `sb.flow.yaml` ([`find_package_file`]). The 0.5 names
//! `sb.dev.yml` and `flow.yaml` are not read since 0.7.0; a caller that
//! finds only the old one refuses with a rename hint
//! ([`refuse_removed_module_file`], [`refuse_removed_flow_file`]).
//!
//! Filesystem layout owned by this crate:
//!
//! ```text
//! ~/.swarmbotix/
//!   active                         ← one-line file: active workspace name
//!   workspaces/
//!     <name>/
//!       sb.workspace.yaml          ← Workspace (absolute paths, source:)
//!       flows/
//!         sb.flow.yaml             ← the repo's default flow
//!         <flow>.flow.yaml         ← each other flow, under its file name
//!       active_flow                ← one-line file: the flow `sb up` launched
//! ```
//!
//! All paths are derived from a [`SbHome`] value so tests can pin `$HOME`
//! to a tempdir without touching the real `~/.swarmbotix/`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};

use sb_core::{
    Deprecation, Flow, Language, MessageSource, ModuleDevConfig, REMOVED_FLOW_FILE,
    REMOVED_MODULE_FILE, SbCliConfig, Workspace, WorkspaceSource, convert_legacy_flow,
    current_manifest_path, flow_file_name, flow_name_of, legacy_flow_deprecation,
    probe_legacy_flow_shape, removed_flow_file_message, removed_module_file_message,
    validate_identifier,
};

pub use sb_core::{DEFAULT_FLOW_FILE, DEFAULT_FLOW_NAME, FLOW_SUFFIX, MODULE_FILE, WORKSPACE_FILE};

/// Directory of an installed workspace that holds its flows.
const FLOWS_DIR: &str = "flows";

/// One-line file in an installed workspace: the flow `sb up` launched last.
const ACTIVE_FLOW_FILE: &str = "active_flow";

/// The `sb.workspace.yaml` of a new workspace: its name and no modules.
/// `sb ws create` writes exactly this text.
pub fn empty_workspace_text(name: &str) -> String {
    format!("name: {name}\nmodules:\n")
}

/// The `sb.flow.yaml` of a new workspace: the default flow, with no nodes.
/// `sb ws create` writes exactly this text.
pub const EMPTY_FLOW_TEXT: &str = "nodes: []\n";

// ─────────────────────────────────────────────────────────────────────
// File names
// ─────────────────────────────────────────────────────────────────────

/// The module manifest in `dir`: `sb.module.yaml` when it is a file, else
/// `None`. A caller that needs the manifest runs
/// [`refuse_removed_module_file`] on a miss.
pub fn find_module_file(dir: &Path) -> Option<PathBuf> {
    let path = new_module_file(dir);
    path.is_file().then_some(path)
}

/// The workspace inventory in `dir`: `sb.workspace.yaml` when it is a
/// file, else `None`.
pub fn find_workspace_file(dir: &Path) -> Option<PathBuf> {
    let path = new_workspace_file(dir);
    path.is_file().then_some(path)
}

/// Every flow file in `dir` as `(flow name, path)`: `sb.flow.yaml` is the
/// flow `default`, `<name>.flow.yaml` the flow `<name>`. The default flow
/// comes first, then the rest by name. A flow is addressed by its file
/// name; a `name:` key in it must agree ([`install_workspace`] refuses one
/// that does not).
pub fn find_flow_files(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_file() {
                return None;
            }
            let name = flow_name_of(entry.file_name().to_str()?)?;
            Some((name, path))
        })
        .collect();
    out.sort_by(|(a, pa), (b, pb)| flow_order(a, b).then_with(|| pa.cmp(pb)));
    out
}

/// The file a workspace package in `dir` installs from:
/// `sb.workspace.yaml`, else an `sb.flow.yaml` in the 0.7 shape (read in
/// 0.8.0 only, with a deprecation), else `None`. An `sb.flow.yaml` in the
/// 0.8 shape on its own is not a package: its modules live in
/// `sb.workspace.yaml`.
pub fn find_package_file(dir: &Path) -> Option<PathBuf> {
    if let Some(workspace) = find_workspace_file(dir) {
        return Some(workspace);
    }
    let flow = new_flow_file(dir);
    (flow.is_file() && is_legacy_flow_file(&flow)).then_some(flow)
}

/// Whether the file at `path` reads as a flow in the 0.7 shape.
fn is_legacy_flow_file(path: &Path) -> bool {
    fs::read_to_string(path).is_ok_and(|text| probe_legacy_flow_shape(&text))
}

/// Whether `path` names a `sb.workspace.yaml` (as opposed to a 0.7 flow).
fn is_workspace_file_name(path: &Path) -> bool {
    path.file_name().is_some_and(|n| n == WORKSPACE_FILE)
}

/// `Err` with the 0.7.0 rename hint when `dir` holds a 0.5 `sb.dev.yml`
/// and no `sb.module.yaml`; `Ok` otherwise, including when it holds
/// neither.
pub fn refuse_removed_module_file(dir: &Path) -> Result<()> {
    if !new_module_file(dir).is_file() && dir.join(REMOVED_MODULE_FILE).is_file() {
        bail!("{}", removed_module_file_message(dir));
    }
    Ok(())
}

/// `Err` with the 0.7.0 rename hint when `dir` holds a 0.5 `flow.yaml`
/// and neither `sb.workspace.yaml` nor `sb.flow.yaml`; `Ok` otherwise,
/// including when it holds none of them.
pub fn refuse_removed_flow_file(dir: &Path) -> Result<()> {
    let current = new_workspace_file(dir).is_file() || new_flow_file(dir).is_file();
    if !current && dir.join(REMOVED_FLOW_FILE).is_file() {
        bail!("{}", removed_flow_file_message(dir));
    }
    Ok(())
}

/// Where a new module manifest in `dir` goes: always `sb.module.yaml`.
pub fn new_module_file(dir: &Path) -> PathBuf {
    dir.join(MODULE_FILE)
}

/// Where a new workspace inventory in `dir` goes: always
/// `sb.workspace.yaml`.
pub fn new_workspace_file(dir: &Path) -> PathBuf {
    dir.join(WORKSPACE_FILE)
}

/// Where the default flow of a repository in `dir` goes: always
/// `sb.flow.yaml`.
pub fn new_flow_file(dir: &Path) -> PathBuf {
    dir.join(DEFAULT_FLOW_FILE)
}

/// Flow names in listing order: `default` first, then by name.
fn flow_order(a: &str, b: &str) -> std::cmp::Ordering {
    (a != DEFAULT_FLOW_NAME)
        .cmp(&(b != DEFAULT_FLOW_NAME))
        .then_with(|| a.cmp(b))
}

/// A flow name must be an identifier: it is a file name under `flows/`
/// and the argument of `sb up`.
fn validate_flow_name(flow: &str) -> Result<()> {
    validate_identifier(flow).map_err(|e| anyhow!("invalid flow name {flow:?}: {e}"))
}

/// Append `d` unless an entry with the same item is already there; the
/// CLI prints one line per item.
fn push_deprecation(list: &mut Vec<Deprecation>, d: Deprecation) {
    if !list.iter().any(|have| have.item == d.item) {
        list.push(d);
    }
}

fn read_text(path: &Path) -> Result<String> {
    fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
}

fn write_text(path: &Path, body: &str) -> Result<()> {
    fs::write(path, body).with_context(|| format!("writing {}", path.display()))
}

fn remove_if_file(path: &Path) -> Result<()> {
    if path.is_file() {
        fs::remove_file(path).with_context(|| format!("removing {}", path.display()))?;
    }
    Ok(())
}

fn parent_of(path: &Path) -> Result<&Path> {
    path.parent()
        .ok_or_else(|| anyhow!("{} has no parent directory", path.display()))
}

// ─────────────────────────────────────────────────────────────────────
// SbHome — `~/.swarmbotix/` paths
// ─────────────────────────────────────────────────────────────────────

/// All paths under the per-user `~/.swarmbotix/` tree.
///
/// The root is resolved from `sb.config.yml`'s `sb_home_dir:` field
/// via [`sb_config::resolve_sb_home_dir`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SbHome {
    root: PathBuf,
}

impl SbHome {
    /// Resolve from a merged config. The single entrypoint the CLI uses.
    pub fn from_config(cfg: &SbCliConfig) -> Result<Self> {
        let root = sb_config::resolve_sb_home_dir(cfg)?;
        Ok(Self { root })
    }

    /// Pin the root explicitly. Convenience for in-process unit tests
    /// that already have a path in hand and want to skip the config
    /// round-trip.
    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn workspaces_dir(&self) -> PathBuf {
        self.root.join("workspaces")
    }

    pub fn workspace_dir(&self, name: &str) -> PathBuf {
        self.workspaces_dir().join(name)
    }

    /// The installed inventory of workspace `ws`:
    /// `<sb_home>/workspaces/<ws>/sb.workspace.yaml`, whether or not it
    /// exists.
    pub fn workspace_file(&self, ws: &str) -> PathBuf {
        new_workspace_file(&self.workspace_dir(ws))
    }

    /// `<sb_home>/workspaces/<ws>/flows/`, the installed flows of `ws`.
    pub fn flows_dir(&self, ws: &str) -> PathBuf {
        self.workspace_dir(ws).join(FLOWS_DIR)
    }

    /// The installed flow `flow` of workspace `ws`, under the file name it
    /// has in the repository: `<sb_home>/workspaces/<ws>/flows/sb.flow.yaml`
    /// for `default`, `flows/<flow>.flow.yaml` for any other; whether or
    /// not it exists.
    pub fn flow_file(&self, ws: &str, flow: &str) -> PathBuf {
        self.flows_dir(ws).join(flow_file_name(flow))
    }

    /// `<sb_home>/workspaces/<ws>/active_flow`: the flow `sb up` launched
    /// last, which `sb run` and `sb stop` use.
    pub fn active_flow_marker(&self, ws: &str) -> PathBuf {
        self.workspace_dir(ws).join(ACTIVE_FLOW_FILE)
    }

    pub fn active_marker(&self) -> PathBuf {
        self.root.join("active")
    }
}

// ─────────────────────────────────────────────────────────────────────
// Workspace lifecycle
// ─────────────────────────────────────────────────────────────────────

/// Create an empty installed workspace `name`: its directory, a
/// `sb.workspace.yaml` naming it, and an empty default flow.
///
/// Errors if the workspace already exists or if `name` is not a valid
/// identifier (same rule as module names — see `sb_core::validate_identifier`).
pub fn create_workspace(home: &SbHome, name: &str) -> Result<()> {
    validate_identifier(name).map_err(|e| anyhow!("invalid workspace name {name:?}: {e}"))?;
    let dir = home.workspace_dir(name);
    if dir.exists() {
        bail!("workspace {name:?} already exists at {}", dir.display());
    }
    let flows = home.flows_dir(name);
    fs::create_dir_all(&flows).with_context(|| format!("creating {}", flows.display()))?;
    write_text(&home.workspace_file(name), &empty_workspace_text(name))?;
    write_text(&home.flow_file(name, DEFAULT_FLOW_NAME), EMPTY_FLOW_TEXT)?;
    Ok(())
}

/// `sb ws list` — every directory under `~/.swarmbotix/workspaces/`,
/// sorted. Each entry carries an `active` flag so the CLI can prefix
/// the active one with `*`.
pub fn list_workspaces(home: &SbHome) -> Result<Vec<WorkspaceEntry>> {
    let dir = home.workspaces_dir();
    let act = active_workspace(home)?;
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut names: Vec<String> = Vec::new();
    for entry in fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let n = entry.file_name().to_string_lossy().into_owned();
        names.push(n);
    }
    names.sort();
    Ok(names
        .into_iter()
        .map(|name| {
            let active = act.as_deref() == Some(name.as_str());
            WorkspaceEntry { name, active }
        })
        .collect())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceEntry {
    pub name: String,
    pub active: bool,
}

/// `sb ws set <name>` — record `<name>` in `~/.swarmbotix/active`.
/// Errors if the workspace dir does not exist.
pub fn set_active(home: &SbHome, name: &str) -> Result<()> {
    validate_identifier(name).map_err(|e| anyhow!("invalid workspace name {name:?}: {e}"))?;
    let dir = home.workspace_dir(name);
    if !dir.exists() {
        bail!("workspace {name:?} does not exist (run `sb ws create {name}` first)");
    }
    let marker = home.active_marker();
    if let Some(parent) = marker.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(&marker, format!("{name}\n"))
        .with_context(|| format!("writing {}", marker.display()))?;
    Ok(())
}

/// Read `~/.swarmbotix/active`. Returns `Ok(None)` if missing or empty
/// (which is a valid, non-erroneous state).
pub fn active_workspace(home: &SbHome) -> Result<Option<String>> {
    let marker = home.active_marker();
    if !marker.exists() {
        return Ok(None);
    }
    let s = fs::read_to_string(&marker).with_context(|| format!("reading {}", marker.display()))?;
    let trimmed = s.trim();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed.to_owned()))
    }
}

/// `sb ws delete <name>` — `rm -rf` the workspace dir. If the deleted
/// workspace was active, clears the active marker and returns
/// `cleared_active = true` so the CLI can emit a warning.
pub fn delete_workspace(home: &SbHome, name: &str) -> Result<DeleteOutcome> {
    let dir = home.workspace_dir(name);
    if !dir.exists() {
        bail!("workspace {name:?} does not exist");
    }
    fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;
    let was_active = active_workspace(home)?.as_deref() == Some(name);
    if was_active {
        let marker = home.active_marker();
        let _ = fs::write(&marker, "");
    }
    Ok(DeleteOutcome {
        cleared_active: was_active,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteOutcome {
    pub cleared_active: bool,
}

// ─────────────────────────────────────────────────────────────────────
// Installed workspace and flows: R/W
// ─────────────────────────────────────────────────────────────────────

/// Read the installed `sb.workspace.yaml` of workspace `ws`. A missing or
/// empty file is an empty workspace. A workspace directory that still
/// holds only the single `sb.flow.yaml` an sb 0.7 install wrote (or the
/// `flow.yaml` of a 0.5 install) is refused with a reinstall hint.
pub fn read_workspace(home: &SbHome, ws: &str) -> Result<Workspace> {
    let path = home.workspace_file(ws);
    if !path.exists() {
        refuse_pre_08_install(home, ws)?;
        return Ok(Workspace::default());
    }
    let text = read_text(&path)?;
    parse_workspace_text(&text, &path)
}

/// `Err` when the installed copy of `ws` predates the 0.8 layout.
fn refuse_pre_08_install(home: &SbHome, ws: &str) -> Result<()> {
    let dir = home.workspace_dir(ws);
    refuse_removed_flow_file(&dir)?;
    let old = new_flow_file(&dir);
    if old.is_file() {
        bail!(
            "workspace {ws:?} was installed by an older sb ({} holds its modules and wiring \
             in one file); install it again from its repository with `sb install <repo folder>`",
            old.display()
        );
    }
    Ok(())
}

/// Write the installed `sb.workspace.yaml` of workspace `ws`, creating the
/// workspace directory if missing.
pub fn write_workspace(home: &SbHome, ws: &str, workspace: &Workspace) -> Result<()> {
    let dir = home.workspace_dir(ws);
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let body = serde_yaml::to_string(workspace).context("serializing the workspace")?;
    write_text(&home.workspace_file(ws), &body)
}

/// Read and validate flow `flow` of installed workspace `ws`
/// (`flows/<flow>.flow.yaml`). A flow that is not installed is an error
/// naming the ones that are.
pub fn read_flow(home: &SbHome, ws: &str, flow: &str) -> Result<Flow> {
    validate_flow_name(flow)?;
    let path = home.flow_file(ws, flow);
    if !path.is_file() {
        let have = list_flows(home, ws)?;
        let have = if have.is_empty() {
            "none".to_owned()
        } else {
            have.join(", ")
        };
        bail!("no flow named {flow} in workspace {ws} (flows: {have})");
    }
    let text = read_text(&path)?;
    parse_flow_text(&text, &path)
}

/// The installed flows of workspace `ws`, by name: `default` first, then
/// the rest by name. Empty when it has none.
pub fn list_flows(home: &SbHome, ws: &str) -> Result<Vec<String>> {
    let dir = home.flows_dir(ws);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    Ok(find_flow_files(&dir)
        .into_iter()
        .map(|(name, _)| name)
        .collect())
}

/// The flow `sb up` launched last in workspace `ws`, from its
/// `active_flow` marker; `Ok(None)` when there is none.
pub fn active_flow(home: &SbHome, ws: &str) -> Result<Option<String>> {
    let marker = home.active_flow_marker(ws);
    if !marker.exists() {
        return Ok(None);
    }
    let s = fs::read_to_string(&marker).with_context(|| format!("reading {}", marker.display()))?;
    let trimmed = s.trim();
    Ok((!trimmed.is_empty()).then(|| trimmed.to_owned()))
}

/// Record `flow` as the active flow of workspace `ws` (`sb up` does this;
/// `sb run` and `sb stop` read it back).
pub fn set_active_flow(home: &SbHome, ws: &str, flow: &str) -> Result<()> {
    validate_flow_name(flow)?;
    let dir = home.workspace_dir(ws);
    if !dir.is_dir() {
        bail!("workspace {ws:?} does not exist");
    }
    write_text(&home.active_flow_marker(ws), &format!("{flow}\n"))
}

/// Parse and validate workspace text. An empty file is an empty
/// workspace.
fn parse_workspace_text(text: &str, path: &Path) -> Result<Workspace> {
    if text.trim().is_empty() {
        return Ok(Workspace::default());
    }
    let workspace: Workspace =
        serde_yaml::from_str(text).with_context(|| format!("parsing {}", path.display()))?;
    workspace
        .validate()
        .with_context(|| format!("validating {}", path.display()))?;
    Ok(workspace)
}

/// Parse and validate flow text. An empty file is an empty flow. A flow
/// in the 0.7 shape is refused with the split hint: it is read only as a
/// package's sole file ([`find_package_file`]).
fn parse_flow_text(text: &str, path: &Path) -> Result<Flow> {
    if text.trim().is_empty() {
        return Ok(Flow::default());
    }
    if probe_legacy_flow_shape(text) {
        bail!(
            "{} is a flow in the 0.7 shape (top-level modules:), which is read only when no \
             {WORKSPACE_FILE} sits beside it; move modules: and messages: into {WORKSPACE_FILE} \
             and keep nodes, instances and wires in the flow (see sbcli_workspace.md §Migrating)",
            path.display()
        );
    }
    let flow: Flow =
        serde_yaml::from_str(text).with_context(|| format!("parsing {}", path.display()))?;
    flow.validate()
        .with_context(|| format!("validating {}", path.display()))?;
    Ok(flow)
}

// ─────────────────────────────────────────────────────────────────────
// Repository package: discovery, reading, install
// ─────────────────────────────────────────────────────────────────────

/// The nearest workspace package at or above `start`: `start` first, then
/// each parent in turn. Each directory is probed with
/// [`find_package_file`], so the result is a `sb.workspace.yaml` or a
/// 0.7-shape `sb.flow.yaml`. One that holds only a 0.5 `flow.yaml` ends
/// the walk with the 0.7.0 rename hint ([`refuse_removed_flow_file`]).
///
/// The walk stops at the filesystem root, and never examines `$HOME`
/// itself (unless `start` is `$HOME`): a stray `~/sb.workspace.yaml`
/// would otherwise capture every project under the home directory.
pub fn find_workspace_upwards(start: &Path) -> Result<Option<PathBuf>> {
    let start = absolutize(start);
    let home = dirs::home_dir().map(|h| h.canonicalize().unwrap_or(h));
    find_workspace_upwards_until(&start, home.as_deref())
}

/// [`find_workspace_upwards`] with an explicit ceiling directory, which
/// is never examined unless the walk starts there.
fn find_workspace_upwards_until(start: &Path, ceiling: Option<&Path>) -> Result<Option<PathBuf>> {
    let mut dir = start;
    loop {
        if let Some(found) = find_package_file(dir) {
            return Ok(Some(found));
        }
        refuse_removed_flow_file(dir)?;
        let Some(parent) = dir.parent() else {
            return Ok(None);
        };
        if Some(parent) == ceiling {
            return Ok(None);
        }
        dir = parent;
    }
}

/// One flow of a package, ready to install.
struct PackageFlow {
    /// The name its file name gives (which its `name:`, if set, matches).
    name: String,
    /// What the installed copy holds: the repo file verbatim, or the
    /// default flow converted from a 0.7 `sb.flow.yaml`.
    text: String,
}

/// A workspace package as read from its repository.
struct Package {
    /// Paths still relative to the repository.
    workspace: Workspace,
    /// `default` first, then by name; names unique.
    flows: Vec<PackageFlow>,
    deprecations: Vec<Deprecation>,
}

/// Read the package whose root file is `repo_file`: a `sb.workspace.yaml`
/// plus every flow file beside it, or a 0.7 `sb.flow.yaml` converted into
/// a workspace and its default flow (with a [`Deprecation`]) plus every
/// other flow file beside it. Every file is parsed and validated.
fn read_package(repo_file: &Path) -> Result<Package> {
    let repo_dir = parent_of(repo_file)?;
    let text = read_text(repo_file)?;
    let mut deprecations = Vec::new();
    let mut flows: Vec<PackageFlow> = Vec::new();
    let legacy = !is_workspace_file_name(repo_file);
    let workspace = if legacy {
        if !probe_legacy_flow_shape(&text) {
            bail!(
                "{} is not a workspace package file: install from the {WORKSPACE_FILE} beside \
                 it (a {DEFAULT_FLOW_FILE} without one is read only in the 0.7 shape)",
                repo_file.display()
            );
        }
        let (workspace, flow) = convert_legacy_flow(&text)
            .with_context(|| format!("reading {} (0.7 shape)", repo_file.display()))?;
        workspace
            .validate()
            .with_context(|| format!("validating {}", repo_file.display()))?;
        flow.validate()
            .with_context(|| format!("validating {}", repo_file.display()))?;
        let name = flow_label(&flow, DEFAULT_FLOW_NAME, repo_file)?;
        let body = serde_yaml::to_string(&flow).context("serializing the converted flow")?;
        flows.push(PackageFlow { name, text: body });
        push_deprecation(&mut deprecations, legacy_flow_deprecation());
        workspace
    } else {
        parse_workspace_text(&text, repo_file)?
    };
    for (by_file, path) in find_flow_files(repo_dir) {
        if legacy && path.file_name() == repo_file.file_name() {
            continue;
        }
        let text = read_text(&path)?;
        let flow = parse_flow_text(&text, &path)?;
        let name = flow_label(&flow, &by_file, &path)?;
        flows.push(PackageFlow { name, text });
    }
    // File names map to flow names one to one, so this holds by
    // construction; checked because two flows of one name would overwrite
    // each other's installed copy.
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    if let Some(dup) = flows.iter().find(|f| !seen.insert(f.name.as_str())) {
        bail!("{} defines flow {} twice", repo_dir.display(), dup.name);
    }
    flows.sort_by(|a, b| flow_order(&a.name, &b.name));
    Ok(Package {
        workspace,
        flows,
        deprecations,
    })
}

/// The name `flow` (read from `path`) installs under: `by_file`, the name
/// its file name gives, validated. A `name:` key that says otherwise is
/// refused: `sb up <flow>` addresses a flow by its file name.
fn flow_label(flow: &Flow, by_file: &str, path: &Path) -> Result<String> {
    validate_identifier(by_file).map_err(|e| {
        anyhow!(
            "flow name {by_file:?} (from {}) is invalid: {e}; rename the file",
            path.display()
        )
    })?;
    if let Some(name) = flow.name.as_deref().filter(|n| *n != by_file) {
        bail!(
            "{} says `name: {name}`, but its file name makes it the flow {by_file}; leave \
             `name:` out or make it {by_file}",
            path.display()
        );
    }
    Ok(by_file.to_owned())
}

/// The workspace of the package whose root file is `repo_file` (a
/// `sb.workspace.yaml`, or a 0.7 `sb.flow.yaml`), parsed and validated,
/// with its paths still relative to the repository. Its flows are read
/// and validated too, so a package that would not install is an error
/// here.
pub fn read_repo_workspace(repo_file: &Path) -> Result<Workspace> {
    Ok(read_package(repo_file)?.workspace)
}

/// What [`install_workspace`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallReport {
    /// Workspace name: the repo workspace's `name:`, else its directory
    /// name.
    pub name: String,
    /// True when the workspace directory did not exist before.
    pub created: bool,
    /// `<sb_home>/workspaces/<name>/sb.workspace.yaml`.
    pub installed_workspace: PathBuf,
    /// The installed flows, by name: `default` first, then by name.
    pub flows: Vec<String>,
    pub modules: Vec<InstalledModule>,
    /// The repo workspace's `messages:` with every `path` made absolute.
    pub messages: Vec<MessageSource>,
    /// Deprecated forms the repo uses, one entry per item: in 0.8.0, a
    /// `sb.flow.yaml` in the 0.7 shape.
    pub deprecations: Vec<Deprecation>,
}

/// One module of an installed workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledModule {
    pub name: String,
    /// Absolute path of the module's manifest (`sb.module.yaml`).
    pub dev_yml: PathBuf,
    /// Whether `dev_yml` is present on disk (a module may be registered
    /// before `sb init` has run in it).
    pub exists: bool,
}

/// Install the workspace package whose root file is `repo_file`: a
/// `sb.workspace.yaml`, or a `sb.flow.yaml` in the 0.7 shape
/// ([`find_package_file`]).
///
/// - The name is the workspace's `name:`, else the name of its directory.
/// - Every module and message path is resolved against the repository
///   directory. A module path may name the module directory (its
///   `sb.module.yaml`) or the yaml itself.
/// - `<sb_home>/workspaces/<name>/sb.workspace.yaml` gets absolute paths
///   and `source: repo_workspace, url, installed_at`. Every flow file of the
///   repository is copied to `flows/<flow>.flow.yaml` as it is (flows
///   name nodes only); flows the repository no longer has are removed. A
///   0.7 `sb.flow.yaml` is converted: its modules and messages become the
///   workspace, the rest its default flow, and the report carries a
///   [`Deprecation`].
/// - The workspace directory is created when missing; re-installing is
///   idempotent. The workspace becomes the active one.
/// - The repo copy is never written.
///
/// Refuses, with the 0.7.0 rename hint, a repo directory that holds only
/// a 0.5 `flow.yaml` and a module directory that holds only a 0.5
/// `sb.dev.yml`. Refuses to replace a workspace of the same name that was
/// installed from a different repository which still exists; one of the
/// two needs a distinct `name:`.
pub fn install_workspace(
    home: &SbHome,
    repo_file: &Path,
    url: Option<&str>,
) -> Result<InstallReport> {
    if let Some(dir) = repo_file.parent() {
        refuse_removed_flow_file(dir)?;
    }
    let repo_file = repo_file
        .canonicalize()
        .with_context(|| format!("resolving {}", repo_file.display()))?;
    if !repo_file.is_file() {
        bail!("{} is not a file", repo_file.display());
    }
    refuse_installed_copy(home, &repo_file)?;
    let repo_dir = parent_of(&repo_file)?.to_path_buf();
    let package = read_package(&repo_file)?;
    let name = workspace_name_for(&package.workspace, &repo_file)?;
    refuse_removed_module_files(&package.workspace, &repo_dir)?;

    let created = !home.workspace_dir(&name).exists();
    if !created {
        refuse_foreign_install(home, &name, &repo_file)?;
    }
    // Re-installing from the same repository without a URL (`sb install .`
    // inside a clone, or `sb init`) keeps the URL recorded earlier, so
    // `sb ws update` still knows where to pull from.
    let url = url.map(str::to_owned).or_else(|| {
        if created {
            return None;
        }
        let prev = read_workspace(home, &name).ok()?.source?;
        if same_repo(&prev.repo_workspace, &repo_file) {
            prev.url
        } else {
            None
        }
    });

    let mut modules = Vec::with_capacity(package.workspace.modules.len());
    let mut installed_modules = BTreeMap::new();
    for (module, path) in &package.workspace.modules {
        let manifest = resolve_module_yaml(&repo_dir, path);
        modules.push(InstalledModule {
            name: module.clone(),
            exists: manifest.is_file(),
            dev_yml: manifest.clone(),
        });
        installed_modules.insert(module.clone(), manifest);
    }
    let messages: Vec<MessageSource> = package
        .workspace
        .messages
        .iter()
        .map(|src| MessageSource {
            path: resolve_path(&repo_dir, &src.path),
            ..src.clone()
        })
        .collect();

    let installed = Workspace {
        name: Some(name.clone()),
        sb: package.workspace.sb.clone(),
        modules: installed_modules,
        messages: messages.clone(),
        source: Some(WorkspaceSource {
            repo_workspace: repo_file.clone(),
            url,
            installed_at: rfc3339_utc(SystemTime::now()),
        }),
    };
    write_workspace(home, &name, &installed)?;
    write_installed_flows(home, &name, &package.flows)?;
    // The single file an sb 0.7 install wrote; the two above replace it.
    remove_if_file(&new_flow_file(&home.workspace_dir(&name)))?;
    set_active(home, &name)?;

    Ok(InstallReport {
        installed_workspace: home.workspace_file(&name),
        flows: package.flows.iter().map(|f| f.name.clone()).collect(),
        name,
        created,
        modules,
        messages,
        deprecations: package.deprecations,
    })
}

/// Replace the installed flows of workspace `ws` with `flows`. The
/// active-flow marker is cleared when it names a flow that is gone.
fn write_installed_flows(home: &SbHome, ws: &str, flows: &[PackageFlow]) -> Result<()> {
    let dir = home.flows_dir(ws);
    if dir.exists() {
        fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;
    }
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    for flow in flows {
        write_text(&home.flow_file(ws, &flow.name), &flow.text)?;
    }
    if let Some(active) = active_flow(home, ws)? {
        if !flows.iter().any(|f| f.name == active) {
            remove_if_file(&home.active_flow_marker(ws))?;
        }
    }
    Ok(())
}

/// Two package files of one repository: the same directory. A repository
/// that moved from a 0.7 `sb.flow.yaml` to `sb.workspace.yaml` stays the
/// same source.
fn same_repo(a: &Path, b: &Path) -> bool {
    a.parent() == b.parent()
}

/// [`refuse_removed_module_file`] for the directory of every module the
/// repo workspace `workspace` (in `repo_dir`) registers.
fn refuse_removed_module_files(workspace: &Workspace, repo_dir: &Path) -> Result<()> {
    for path in workspace.modules.values() {
        let manifest = resolve_module_yaml(repo_dir, path);
        if let Some(dir) = manifest.parent() {
            refuse_removed_module_file(dir)?;
        }
    }
    Ok(())
}

/// The workspace name a repo package installs under, validated.
fn workspace_name_for(workspace: &Workspace, repo_file: &Path) -> Result<String> {
    let name = match &workspace.name {
        Some(n) => n.clone(),
        None => repo_file
            .parent()
            .and_then(Path::file_name)
            .and_then(|s| s.to_str())
            .ok_or_else(|| {
                anyhow!(
                    "cannot derive a workspace name from {}; set `name:` in it",
                    repo_file.display()
                )
            })?
            .to_owned(),
    };
    validate_identifier(&name).map_err(|e| {
        anyhow!(
            "workspace name {name:?} (from {}) is invalid: {e}; set `name:` in that file",
            repo_file.display()
        )
    })?;
    Ok(name)
}

/// An installed copy is derived data; installing it would make the
/// workspace its own source.
fn refuse_installed_copy(home: &SbHome, file: &Path) -> Result<()> {
    if is_installed_copy(home, file) {
        bail!(
            "{} is an installed workspace copy; install from the repository's {WORKSPACE_FILE} \
             instead",
            file.display()
        );
    }
    Ok(())
}

/// Two live repositories must not take turns overwriting one workspace.
/// A previous source that no longer exists (the repo moved) is replaced.
fn refuse_foreign_install(home: &SbHome, name: &str, repo_file: &Path) -> Result<()> {
    // A corrupt installed copy is replaced rather than blocking the install.
    let Some(prev) = read_workspace(home, name).ok().and_then(|w| w.source) else {
        return Ok(());
    };
    if !same_repo(&prev.repo_workspace, repo_file) && prev.repo_workspace.is_file() {
        bail!(
            "workspace {name:?} is already installed from {}; uninstall it first, \
             or give {} a different `name:`",
            prev.repo_workspace.display(),
            repo_file.display()
        );
    }
    Ok(())
}

/// `base.join(p)`, canonical when it exists, lexically normalised when
/// it does not (a module may be listed before its directory exists).
fn resolve_path(base: &Path, p: &Path) -> PathBuf {
    let joined = base.join(p);
    joined
        .canonicalize()
        .unwrap_or_else(|_| normalize_lexically(&joined))
}

/// A module entry's manifest. The entry may name the module directory or
/// the yaml file; a path that does not exist is taken as a directory
/// unless it ends in `.yml` / `.yaml`.
///
/// A directory stands for its `sb.module.yaml`. A yaml path is used as
/// is, except that a 0.5 `.../sb.dev.yml` stands for the `sb.module.yaml`
/// beside it ([`current_manifest_path`]); the old file is never read.
fn resolve_module_yaml(base: &Path, p: &Path) -> PathBuf {
    let resolved = resolve_path(base, p);
    let names_yaml = if resolved.exists() {
        resolved.is_file()
    } else {
        resolved
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("yml") || e.eq_ignore_ascii_case("yaml"))
    };
    if names_yaml {
        current_manifest_path(&resolved)
    } else {
        new_module_file(&resolved)
    }
}

/// Drop `.` components and fold `..` into its parent, without touching
/// the filesystem.
fn normalize_lexically(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() && !out.has_root() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Absolute form of `p`: canonical when it exists, else made absolute
/// against the current directory, else unchanged.
fn absolutize(p: &Path) -> PathBuf {
    p.canonicalize()
        .or_else(|_| std::path::absolute(p))
        .unwrap_or_else(|_| p.to_path_buf())
}

/// `t` as an RFC 3339 UTC timestamp with second precision,
/// e.g. `2026-10-13T08:35:23Z`.
fn rfc3339_utc(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Days since 1970-01-01 to a proleptic Gregorian `(year, month, day)`
/// (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

// ─────────────────────────────────────────────────────────────────────
// Installed workspaces: provenance, staleness, update, uninstall
// ─────────────────────────────────────────────────────────────────────

/// The `source:` of workspace `name`'s installed copy; `None` for a
/// workspace that was not installed from a repository.
pub fn installed_source(home: &SbHome, name: &str) -> Result<Option<WorkspaceSource>> {
    if !home.workspace_dir(name).exists() {
        bail!("workspace {name:?} does not exist");
    }
    Ok(read_workspace(home, name)?.source)
}

/// `Some((repo_file, installed))` when a repository file of the workspace
/// (its package file, or any flow file beside it) was modified after the
/// installed `sb.workspace.yaml` was written, i.e. a re-install is due;
/// `repo_file` is the newest such file. `None` when the workspace has no
/// `source:` or the files are missing.
pub fn is_stale(home: &SbHome, name: &str) -> Result<Option<(PathBuf, PathBuf)>> {
    let Some(src) = installed_source(home, name)? else {
        return Ok(None);
    };
    let installed = home.workspace_file(name);
    let mtime = |p: &Path| fs::metadata(p).and_then(|m| m.modified()).ok();
    let Some(installed_at) = mtime(&installed) else {
        return Ok(None);
    };
    let mut repo_files = vec![src.repo_workspace.clone()];
    if let Some(dir) = src.repo_workspace.parent() {
        repo_files.extend(find_flow_files(dir).into_iter().map(|(_, path)| path));
    }
    let newest = repo_files
        .into_iter()
        .filter_map(|path| mtime(&path).map(|m| (m, path)))
        .filter(|(m, _)| *m > installed_at)
        .max_by_key(|(m, _)| *m);
    Ok(newest.map(|(_, path)| (path, installed)))
}

/// Pull the repository a workspace was installed from, then re-install
/// it. Requires `source.url`: a workspace installed from a local path has
/// no remote to pull and is refreshed by re-installing it.
///
/// Runs `git -C <repo dir> pull --ff-only`, then installs the package
/// file the pulled repository holds ([`find_package_file`]), which may
/// have moved from a 0.7 `sb.flow.yaml` to `sb.workspace.yaml`. The
/// report's `name` can differ from `name` if the pull changed `name:`.
pub fn update_from_source(home: &SbHome, name: &str) -> Result<InstallReport> {
    let src = installed_source(home, name)?
        .ok_or_else(|| anyhow!("workspace {name:?} was not installed from a repository"))?;
    let url = src.url.as_deref().ok_or_else(|| {
        anyhow!(
            "workspace {name:?} was installed from a local path ({}) with no URL; \
             update that checkout yourself and install it again",
            src.repo_workspace.display()
        )
    })?;
    let repo_root = parent_of(&src.repo_workspace)?;
    let out = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["pull", "--ff-only"])
        .output()
        .context("running git (is it on PATH?)")?;
    if !out.status.success() {
        bail!(
            "git -C {} pull --ff-only failed: {}",
            repo_root.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let package = find_package_file(repo_root).ok_or_else(|| {
        anyhow!(
            "{} holds no {WORKSPACE_FILE} after the pull",
            repo_root.display()
        )
    })?;
    install_workspace(home, &package, Some(url))
}

/// What [`uninstall_workspace`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UninstallReport {
    pub name: String,
    /// The workspace directory that was removed.
    pub workspace_dir: PathBuf,
    /// True when it was the active workspace (the marker is now empty).
    pub cleared_active: bool,
    /// IO directories deleted because `clean` was set.
    pub removed: Vec<PathBuf>,
    /// Under `clean`, modules whose IO directory was left alone: `(module,
    /// reason)`. A module with no IO directory on disk is not listed.
    pub skipped: Vec<(String, String)>,
}

/// Remove workspace `name` (its directory under `<sb_home>/workspaces/`,
/// and the active marker if it named it). The repo and its modules are
/// untouched unless `clean` is set, in which case each module's generated
/// IO directory (`io_dir:` of its manifest, default `swarmbotix_io`) is
/// deleted from the module root first.
///
/// An `io_dir` that is absolute or climbs out of the module root is never
/// deleted; it is reported in `skipped`.
pub fn uninstall_workspace(home: &SbHome, name: &str, clean: bool) -> Result<UninstallReport> {
    validate_identifier(name).map_err(|e| anyhow!("invalid workspace name {name:?}: {e}"))?;
    let workspace_dir = home.workspace_dir(name);
    if !workspace_dir.exists() {
        bail!("workspace {name:?} does not exist");
    }
    let mut removed = Vec::new();
    let mut skipped = Vec::new();
    if clean {
        let workspace = read_workspace(home, name)?;
        for (module, path) in &workspace.modules {
            match module_io_dir(path) {
                Ok(Some(io)) => {
                    fs::remove_dir_all(&io)
                        .with_context(|| format!("removing {}", io.display()))?;
                    removed.push(io);
                }
                Ok(None) => {}
                Err(reason) => skipped.push((module.clone(), reason)),
            }
        }
    }
    let outcome = delete_workspace(home, name)?;
    Ok(UninstallReport {
        name: name.to_owned(),
        workspace_dir,
        cleared_active: outcome.cleared_active,
        removed,
        skipped,
    })
}

/// The IO directory of the module whose manifest is `dev_yml`, if it
/// exists. `Err` carries why it cannot be cleaned safely.
fn module_io_dir(dev_yml: &Path) -> std::result::Result<Option<PathBuf>, String> {
    if !dev_yml.is_file() {
        return Err(format!("{} not found", dev_yml.display()));
    }
    let text =
        fs::read_to_string(dev_yml).map_err(|e| format!("reading {}: {e}", dev_yml.display()))?;
    let dev: ModuleDevConfig =
        serde_yaml::from_str(&text).map_err(|e| format!("parsing {}: {e}", dev_yml.display()))?;
    let plain_relative = dev
        .io_dir
        .components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
        && dev
            .io_dir
            .components()
            .any(|c| matches!(c, Component::Normal(_)));
    if !plain_relative {
        return Err(format!(
            "io_dir {} is not a directory inside the module root",
            dev.io_dir.display()
        ));
    }
    let io = dev.module_root(dev_yml).join(&dev.io_dir);
    if io.is_dir() {
        Ok(Some(io))
    } else if io.exists() {
        Err(format!("{} is not a directory", io.display()))
    } else {
        Ok(None)
    }
}

// ─────────────────────────────────────────────────────────────────────
// sb init — adopt an existing project as a swarmbotix module
// ─────────────────────────────────────────────────────────────────────

/// Options for [`init_module`]. Mirrors the `sb init` flag surface.
#[derive(Debug, Clone)]
pub struct InitOptions<'a> {
    /// Project root to adopt. Must already exist. Module name = `basename(root)`.
    pub root: &'a Path,
    /// Language flag, if the user passed one. Required if `root` has no
    /// module manifest yet.
    pub language: Option<Language>,
    /// `--force` — write a fresh `sb.module.yaml` over the existing
    /// manifest (its publishers, subscribers and services are dropped).
    pub force: bool,
}

/// What [`init_module`] actually changed on disk. Lets the CLI render
/// "wrote / left alone" feedback per artifact.
// One bool per artifact is the point: each one answers "did init write
// this?" independently, and a state enum would have to enumerate every
// combination. Grouping them would make the CLI's rendering harder, not
// easier.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InitOutcome {
    pub module: String,
    pub root_abs: PathBuf,
    pub language: Option<Language>,
    /// The module's manifest: the `sb.module.yaml` this call wrote, or the
    /// existing file it adopted.
    pub manifest_path: PathBuf,
    /// `manifest_path` was written by this call.
    pub manifest_written: bool,
    pub io_dir_created: bool,
    /// The module was added to (or repointed in) the workspace that
    /// registers it: `repo_workspace` when set, else the active
    /// workspace's installed copy.
    pub workspace_appended: bool,
    /// Workspace the module is registered in. In repo mode this is the
    /// workspace the repository installs as, which is now the active one.
    pub workspace: String,
    /// The repository `sb.workspace.yaml` the module was registered in,
    /// when the module sits under one. `None` means the module went
    /// straight into the active workspace's installed copy.
    pub repo_workspace: Option<PathBuf>,
    /// Deprecated forms met on the way (in repo mode, whatever
    /// [`install_workspace`] reported). One entry per item.
    pub deprecations: Vec<Deprecation>,
}

/// Where [`init_module`] registers a module.
enum Registry {
    /// A repo `sb.workspace.yaml` enclosing the module root (canonical
    /// path).
    Repo(PathBuf),
    /// The active workspace's installed copy (workspace name).
    Active(String),
}

/// Adopt `opts.root` as a swarmbotix module. Drops only the missing
/// artifacts (`sb.module.yaml`, `swarmbotix_io/`) unless `--force`, then
/// registers the module. Idempotent.
///
/// An existing `sb.module.yaml` is adopted as is. A root that holds only a
/// 0.5 `sb.dev.yml` is refused with the 0.7.0 rename hint, before anything
/// is written.
///
/// Registration: when [`find_workspace_upwards`] finds a repo
/// `sb.workspace.yaml` above the module root, the module is added to its
/// `modules:` as `<name>: <path relative to that file>` (layout and
/// comments are kept when the entry can be appended as one line) and the
/// repository is installed with [`install_workspace`]. Otherwise the
/// module goes into the active workspace's installed `sb.workspace.yaml`,
/// which then must exist. A 0.7-shape `sb.flow.yaml` above the module is
/// refused with the split hint. The module is never added to a flow.
///
/// `sb.module.yaml` is written without `root:`; its own directory is the
/// module root.
// Long by construction: this is the numbered adoption procedure, and each
// step's preconditions are checked against the ones before it. Splitting it
// would mean threading a dozen locals through private helpers that no other
// caller has any use for.
#[allow(clippy::too_many_lines)]
pub fn init_module(home: &SbHome, opts: &InitOptions<'_>) -> Result<InitOutcome> {
    // 1. Resolve rootpath → absolute canonical form.
    if !opts.root.exists() {
        bail!("rootpath {} does not exist", opts.root.display());
    }
    let root_abs = opts
        .root
        .canonicalize()
        .with_context(|| format!("canonicalizing {}", opts.root.display()))?;
    if !root_abs.is_dir() {
        bail!("rootpath {} is not a directory", root_abs.display());
    }

    // 2. Registry: the enclosing repo workspace, else the active one. A
    //    repository that cannot be installed (unparseable, bad name, name
    //    held by another repo, a module still on a 0.5 `sb.dev.yml`)
    //    fails here, before anything is written.
    let registry = match find_workspace_upwards(&root_abs)?.filter(|f| !is_installed_copy(home, f))
    {
        Some(file) => {
            let file = file.canonicalize().unwrap_or(file);
            if !is_workspace_file_name(&file) {
                bail!(
                    "{} is a flow in the 0.7 shape; sb init registers modules in \
                     {WORKSPACE_FILE}, so split it into {WORKSPACE_FILE} and {DEFAULT_FLOW_FILE} \
                     first (see sbcli_workspace.md §Migrating)",
                    file.display()
                );
            }
            let package = read_package(&file)?;
            let name = workspace_name_for(&package.workspace, &file)?;
            if home.workspace_dir(&name).exists() {
                refuse_foreign_install(home, &name, &file)?;
            }
            refuse_removed_module_files(&package.workspace, parent_of(&file)?)?;
            Registry::Repo(file)
        }
        None => Registry::Active(active_workspace(home)?.ok_or_else(|| {
            anyhow!(
                "no active workspace and no {WORKSPACE_FILE} above {}; run \
                 `sb ws create <name>` at the repository root first, or `sb ws set <name>` \
                 to pick an installed workspace",
                root_abs.display()
            )
        })?),
    };

    // 3. Module name = basename(root_abs). Validate.
    let module = root_abs
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow!("rootpath {} has no usable basename", root_abs.display()))?
        .to_owned();
    validate_identifier(&module)
        .map_err(|e| anyhow!("invalid module name {module:?} (from rootpath basename): {e}"))?;

    // 4. The existing `sb.module.yaml` (an empty file counts as none; a
    //    lone 0.5 `sb.dev.yml` is refused), then the language: flag vs
    //    file on disk.
    let existing: Option<(PathBuf, ModuleDevConfig)> =
        if let Some(path) = find_module_file(&root_abs) {
            read_manifest(&path)?.map(|dev| (path, dev))
        } else {
            refuse_removed_module_file(&root_abs)?;
            None
        };

    let language = match (&existing, opts.language) {
        (Some((path, d)), Some(flag)) if d.language != flag => {
            bail!(
                "language tag mismatch: {} has language: {}, but --{} was passed",
                path.display(),
                d.language,
                flag
            );
        }
        (Some((_, d)), _) => d.language,
        (None, Some(flag)) => flag,
        (None, None) => bail!(
            "no module manifest ({MODULE_FILE}) at {} and no \
             --rust|--python|--cpp|--flutter|--unity flag",
            root_abs.display()
        ),
    };

    let io_dir = existing
        .as_ref()
        .map_or_else(|| language.default_io_dir(), |(_, d)| d.io_dir.clone());

    let mut outcome = InitOutcome {
        module: module.clone(),
        root_abs: root_abs.clone(),
        language: Some(language),
        ..Default::default()
    };

    // 5. The manifest: adopt the existing one, or write `sb.module.yaml`
    //    when there is none or under --force. No `root:`: the file's own
    //    directory is the module root, which keeps it portable.
    match existing {
        Some((path, _)) if !opts.force => {
            outcome.manifest_path = path;
        }
        _ => {
            let path = new_module_file(&root_abs);
            let cfg = fresh_manifest(&module, language, io_dir.clone());
            let body = serde_yaml::to_string(&cfg).context("serializing the module manifest")?;
            write_text(&path, &body)?;
            outcome.manifest_written = true;
            outcome.manifest_path = path;
        }
    }

    // 6. Create the IO directory (empty). Safe to call if already present.
    let io_full = root_abs.join(&io_dir);
    if !io_full.exists() {
        fs::create_dir_all(&io_full).with_context(|| format!("creating {}", io_full.display()))?;
        outcome.io_dir_created = true;
    }

    // 7. Register (idempotent).
    match registry {
        Registry::Repo(repo_workspace) => {
            outcome.workspace_appended =
                register_in_workspace(&repo_workspace, &module, &root_abs, &outcome.manifest_path)?;
            // install_workspace keeps any URL an earlier install recorded.
            let report = install_workspace(home, &repo_workspace, None)?;
            for d in report.deprecations {
                push_deprecation(&mut outcome.deprecations, d);
            }
            outcome.workspace = report.name;
            outcome.repo_workspace = Some(repo_workspace);
        }
        Registry::Active(active) => {
            let mut workspace = read_workspace(home, &active)?;
            let registered = workspace.modules.get(&module).map(PathBuf::as_path);
            if registered != Some(outcome.manifest_path.as_path()) {
                workspace
                    .modules
                    .insert(module.clone(), outcome.manifest_path.clone());
                write_workspace(home, &active, &workspace)?;
                outcome.workspace_appended = true;
            }
            outcome.workspace = active;
        }
    }

    Ok(outcome)
}

/// Parse the module manifest at `path`; `None` for an empty file.
fn read_manifest(path: &Path) -> Result<Option<ModuleDevConfig>> {
    let text = read_text(path)?;
    if text.trim().is_empty() {
        return Ok(None);
    }
    serde_yaml::from_str(&text)
        .map(Some)
        .with_context(|| format!("parsing {}", path.display()))
}

/// The manifest `sb init` writes: identity and IO dir only. Every other
/// key is left out so it takes its language default.
fn fresh_manifest(module: &str, language: Language, io_dir: PathBuf) -> ModuleDevConfig {
    ModuleDevConfig {
        io_dir,
        ..ModuleDevConfig::new(module, language)
    }
}

/// Whether `file` lies under `<sb_home>/workspaces/`, i.e. is an
/// installed copy rather than a repository's file.
fn is_installed_copy(home: &SbHome, file: &Path) -> bool {
    let ws_root = home.workspaces_dir();
    let ws_root = ws_root.canonicalize().unwrap_or(ws_root);
    file.starts_with(&ws_root)
}

/// Add `module` (rooted at `module_root`, manifest at `manifest`) to the
/// repo `sb.workspace.yaml` at `ws_path` as `modules.<module>: <path>`,
/// the path `/`-separated and relative to that file's directory. Returns
/// whether the file changed.
///
/// A new entry is appended as one line under `modules:` so the user's
/// comments and layout survive; the edit is re-parsed and must equal the
/// intended workspace, else the file is rewritten from the parsed form.
/// An entry of the same name pointing at another live module is an error
/// (module names are unique in a workspace); one pointing at a missing
/// module, or at another manifest file of this same module, is repointed
/// at the module directory.
fn register_in_workspace(
    ws_path: &Path,
    module: &str,
    module_root: &Path,
    manifest: &Path,
) -> Result<bool> {
    let repo_dir = parent_of(ws_path)?;
    let text = read_text(ws_path)?;
    let mut workspace = parse_workspace_text(&text, ws_path)?;
    let rel = relative_slash_path(module_root, repo_dir).ok_or_else(|| {
        anyhow!(
            "{} is not inside {}",
            module_root.display(),
            repo_dir.display()
        )
    })?;

    if let Some(existing) = workspace.modules.get(module) {
        let want = manifest
            .canonicalize()
            .unwrap_or_else(|_| manifest.to_path_buf());
        let have = resolve_module_yaml(repo_dir, existing);
        if have == want {
            return Ok(false);
        }
        if have.is_file() && have.parent() != Some(module_root) {
            bail!(
                "module {module:?} is already registered in {} for {}; module names \
                 must be unique within a workspace",
                ws_path.display(),
                have.display()
            );
        }
        workspace
            .modules
            .insert(module.to_owned(), PathBuf::from(&rel));
        let body = serde_yaml::to_string(&workspace).context("serializing the repo workspace")?;
        write_text(ws_path, &body)?;
        return Ok(true);
    }

    let mut expected = workspace;
    expected
        .modules
        .insert(module.to_owned(), PathBuf::from(&rel));
    let body = match append_module_line(&text, module, &rel)
        .filter(|t| parse_workspace_text(t, ws_path).is_ok_and(|w| w == expected))
    {
        Some(t) => t,
        None => serde_yaml::to_string(&expected).context("serializing the repo workspace")?,
    };
    write_text(ws_path, &body)?;
    Ok(true)
}

/// `text` with `<indent><module>: <rel>` appended to its block-style
/// `modules:` mapping (or a new `modules:` block when the key is absent).
/// `None` when `modules:` is written in a form this line edit does not
/// handle, such as a flow mapping; the caller then rewrites the file.
fn append_module_line(text: &str, module: &str, rel: &str) -> Option<String> {
    let value = serde_yaml::to_string(rel).ok()?;
    let entry_body = format!("{module}: {}", value.trim_end());
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let lines: Vec<&str> = text.split_inclusive('\n').collect();

    let Some(header) = lines.iter().position(|l| is_block_key_line(l, "modules")) else {
        if lines.iter().any(|l| l.starts_with("modules:")) {
            return None;
        }
        let mut out = text.to_owned();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push_str(eol);
        }
        for part in ["modules:", eol, "  ", &entry_body, eol] {
            out.push_str(part);
        }
        return Some(out);
    };

    // The block runs until the next line that starts at column 0 with
    // content; blank and comment lines inside it do not end it. Insert
    // after its last content line, at its first entry's indentation.
    let mut indent: Option<&str> = None;
    let mut insert_at = header + 1;
    for (i, line) in lines.iter().enumerate().skip(header + 1) {
        let content = line.trim_end_matches(['\r', '\n']);
        let trimmed = content.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if trimmed.len() == content.len() {
            break;
        }
        indent.get_or_insert(&content[..content.len() - trimmed.len()]);
        insert_at = i + 1;
    }
    let indent = indent.unwrap_or("  ");

    let mut out = String::with_capacity(text.len() + indent.len() + entry_body.len() + 2);
    for line in &lines[..insert_at] {
        out.push_str(line);
    }
    if !out.ends_with('\n') {
        out.push_str(eol);
    }
    out.push_str(indent);
    out.push_str(&entry_body);
    out.push_str(eol);
    for line in &lines[insert_at..] {
        out.push_str(line);
    }
    Some(out)
}

/// `key:` at column 0 opening a block (nothing after the colon but
/// whitespace or a comment).
fn is_block_key_line(line: &str, key: &str) -> bool {
    line.strip_prefix(key)
        .and_then(|rest| rest.strip_prefix(':'))
        .is_some_and(|rest| {
            let rest = rest.trim();
            rest.is_empty() || rest.starts_with('#')
        })
}

/// `target` relative to `base`, `/`-separated so a committed flow reads
/// the same on every OS. `.` when they are equal; `None` when `target` is
/// not under `base` or is not valid UTF-8.
fn relative_slash_path(target: &Path, base: &Path) -> Option<String> {
    let rel = target.strip_prefix(base).ok()?;
    let parts: Vec<&str> = rel
        .components()
        .map(|c| c.as_os_str().to_str())
        .collect::<Option<_>>()?;
    Some(if parts.is_empty() {
        ".".to_owned()
    } else {
        parts.join("/")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sandbox() -> (TempDir, SbHome) {
        let td = TempDir::new().unwrap();
        let home = SbHome::at(td.path().join("sb_home"));
        (td, home)
    }

    fn write(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    /// `<tmp>/repo/` with `pubber/` (adopted, with an IO dir), `apps/subber`
    /// (registered by yaml path, not adopted yet), `msgs/`, and a default
    /// flow running `pubber`. Returns the repo `sb.workspace.yaml`.
    fn demo_repo(root: &Path) -> PathBuf {
        let repo = root.join("repo");
        write(
            &repo.join("pubber").join(MODULE_FILE),
            "module: pubber\nlanguage: rust\n",
        );
        write(
            &repo.join("pubber").join("swarmbotix_io").join("gen.rs"),
            "// generated\n",
        );
        write(
            &repo.join("msgs").join("Flat.proto"),
            "syntax = \"proto3\";\n",
        );
        write(&repo.join(DEFAULT_FLOW_FILE), "nodes: [pubber]\n");
        let workspace = repo.join(WORKSPACE_FILE);
        write(
            &workspace,
            &format!(
                "# demo repo\nname: demo\nmodules:\n  pubber: pubber\n  \
                 subber: apps/subber/{MODULE_FILE}\nmessages:\n  msgs:\n    style: acme\n    \
                 path: ./msgs\n    backends: [proto]\n"
            ),
        );
        workspace
    }

    #[test]
    fn install_workspace_refuses_a_second_live_repo_with_the_same_name() {
        let (td, home) = sandbox();
        let a = td.path().join("a").join(WORKSPACE_FILE);
        let b = td.path().join("b").join(WORKSPACE_FILE);
        write(&a, "name: shared\nmodules:\n");
        write(&b, "name: shared\nmodules:\n");
        install_workspace(&home, &a, None).unwrap();
        let e = format!("{:#}", install_workspace(&home, &b, None).unwrap_err());
        assert!(e.contains("already installed from"), "{e}");

        // Once the first repo is gone, the name is free again.
        fs::remove_file(&a).unwrap();
        install_workspace(&home, &b, None).unwrap();
        // An installed copy is never itself a source.
        let e = format!(
            "{:#}",
            install_workspace(&home, &home.workspace_file("shared"), None).unwrap_err()
        );
        assert!(e.contains("installed workspace copy"), "{e}");
    }

    #[test]
    fn uninstall_with_clean_removes_io_dirs_and_skips_unsafe_ones() {
        let (td, home) = sandbox();
        let repo_workspace = demo_repo(td.path());
        // A third module whose io_dir climbs out of its root.
        write(
            &td.path().join("repo").join("rogue").join(MODULE_FILE),
            "module: rogue\nlanguage: rust\nio_dir: ../pubber\n",
        );
        let text = fs::read_to_string(&repo_workspace).unwrap();
        fs::write(
            &repo_workspace,
            text.replace("modules:\n", "modules:\n  rogue: rogue\n"),
        )
        .unwrap();
        let report = install_workspace(&home, &repo_workspace, None).unwrap();
        assert_eq!(report.flows, [DEFAULT_FLOW_NAME]);
        assert!(home.flow_file("demo", DEFAULT_FLOW_NAME).is_file());
        let io = td
            .path()
            .join("repo")
            .join("pubber")
            .join("swarmbotix_io")
            .canonicalize()
            .unwrap();

        let r = uninstall_workspace(&home, "demo", true).unwrap();
        assert_eq!(r.removed, vec![io.clone()]);
        assert_eq!(r.skipped.len(), 2, "{:?}", r.skipped);
        assert!(
            r.skipped
                .iter()
                .any(|(m, why)| m == "rogue" && why.contains("io_dir"))
        );
        assert!(
            r.skipped
                .iter()
                .any(|(m, why)| m == "subber" && why.contains("not found"))
        );
        assert!(r.cleared_active);
        assert!(!io.exists());
        assert!(!home.workspace_dir("demo").exists());
        assert!(repo_workspace.is_file(), "the repo is never removed");
        assert!(uninstall_workspace(&home, "demo", false).is_err());
    }
}
