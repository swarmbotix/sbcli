//! L2 — workspace lifecycle and module init.
//!
//! Three concerns, one crate:
//!
//! 1. **Workspace registry** under `~/.swarmbotix/`:
//!    - `workspaces/<name>/flow.yaml` — module registry per workspace
//!    - `active` — one-line marker file holding the active workspace name
//!
//! 2. **Install from a repository**: a repo-root `flow.yaml` (relative
//!    paths, committed with the code) is the source of truth for a
//!    workspace. [`install_flow`] derives the installed copy under
//!    `workspaces/<name>/` from it (absolute paths plus a `source:` block)
//!    and never writes the repo copy.
//!
//! 3. **Module init** (`sb init`): adopt an existing user project as a
//!    swarmbotix module by writing `sb.dev.yml`, a `runscript.bash` stub,
//!    and an empty `swarmbotix_io/`, then registering the module: in the
//!    enclosing repo's `flow.yaml` when there is one (followed by an
//!    install), otherwise in the active workspace's installed copy.
//!
//! Filesystem layout owned by this crate:
//!
//! ```text
//! ~/.swarmbotix/
//!   active                         ← one-line file: active workspace name
//!   workspaces/
//!     <name>/
//!       flow.yaml                  ← WorkspaceFlow
//! ```
//!
//! All paths are derived from a [`SbHome`] value so tests can pin `$HOME`
//! to a tempdir without touching the real `~/.swarmbotix/`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};

use sb_core::{
    FlowSource, Language, MessageSource, ModuleDevConfig, ModuleRef, SbCliConfig, WorkspaceFlow,
    validate_identifier,
};

pub mod runscript;

// ─────────────────────────────────────────────────────────────────────
// SbHome — `~/.swarmbotix/` paths
// ─────────────────────────────────────────────────────────────────────

/// All paths under the per-user `~/.swarmbotix/` tree.
///
/// The root is resolved from `sb.config.yml`'s `sb_home_dir:` field
/// via [`sb_config::resolve_sb_home_dir`].
#[derive(Debug, Clone)]
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

    pub fn flow_yaml(&self, ws: &str) -> PathBuf {
        self.workspace_dir(ws).join("flow.yaml")
    }

    pub fn active_marker(&self) -> PathBuf {
        self.root.join("active")
    }
}

// ─────────────────────────────────────────────────────────────────────
// Workspace lifecycle
// ─────────────────────────────────────────────────────────────────────

/// `sb ws create <name>` — mkdir the workspace dir, initialize an empty
/// `flow.yaml`.
///
/// Errors if the workspace already exists or if `name` is not a valid
/// identifier (same rule as module names — see `sb_core::validate_identifier`).
pub fn create_workspace(home: &SbHome, name: &str) -> Result<()> {
    validate_identifier(name).map_err(|e| anyhow!("invalid workspace name {name:?}: {e}"))?;
    let dir = home.workspace_dir(name);
    if dir.exists() {
        bail!("workspace {name:?} already exists at {}", dir.display());
    }
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let flow_path = home.flow_yaml(name);
    let empty = WorkspaceFlow::default();
    let body = serde_yaml::to_string(&empty).context("serializing empty flow.yaml")?;
    fs::write(&flow_path, body).with_context(|| format!("writing {}", flow_path.display()))?;
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
// flow.yaml R/W
// ─────────────────────────────────────────────────────────────────────

/// Read `~/.swarmbotix/workspaces/<ws>/flow.yaml`. Returns an empty
/// `WorkspaceFlow` if the file is missing or empty.
pub fn read_flow(home: &SbHome, ws: &str) -> Result<WorkspaceFlow> {
    let path = home.flow_yaml(ws);
    if !path.exists() {
        return Ok(WorkspaceFlow::default());
    }
    read_flow_file(&path)
}

/// Read and validate any `flow.yaml` (repo or installed copy). An empty
/// file is an empty flow.
fn read_flow_file(path: &Path) -> Result<WorkspaceFlow> {
    let s = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    parse_flow_text(&s, path)
}

fn parse_flow_text(text: &str, path: &Path) -> Result<WorkspaceFlow> {
    if text.trim().is_empty() {
        return Ok(WorkspaceFlow::default());
    }
    let flow: WorkspaceFlow =
        serde_yaml::from_str(text).with_context(|| format!("parsing {}", path.display()))?;
    flow.validate()
        .with_context(|| format!("validating {}", path.display()))?;
    Ok(flow)
}

/// Write `flow.yaml`, creating the workspace dir if missing.
pub fn write_flow(home: &SbHome, ws: &str, flow: &WorkspaceFlow) -> Result<()> {
    let path = home.flow_yaml(ws);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let body = serde_yaml::to_string(flow).context("serializing flow.yaml")?;
    fs::write(&path, body).with_context(|| format!("writing {}", path.display()))
}

// ─────────────────────────────────────────────────────────────────────
// Repo flow.yaml: discovery and install
// ─────────────────────────────────────────────────────────────────────

/// File name of a workspace flow, in a repository and when installed.
pub const FLOW_FILE: &str = "flow.yaml";

/// File name of a module's development config.
const DEV_YML: &str = "sb.dev.yml";

/// The nearest `flow.yaml` at or above `start`: `start/flow.yaml` first,
/// then each parent in turn.
///
/// The walk stops at the filesystem root, and never examines `$HOME`
/// itself (unless `start` is `$HOME`): a stray `~/flow.yaml` would
/// otherwise capture every project under the home directory.
pub fn find_flow_upwards(start: &Path) -> Option<PathBuf> {
    let start = absolutize(start);
    let home = dirs::home_dir().map(|h| h.canonicalize().unwrap_or(h));
    find_flow_upwards_until(&start, home.as_deref())
}

/// [`find_flow_upwards`] with an explicit ceiling directory, which is
/// never examined unless the walk starts there.
fn find_flow_upwards_until(start: &Path, ceiling: Option<&Path>) -> Option<PathBuf> {
    let mut dir = start;
    loop {
        let candidate = dir.join(FLOW_FILE);
        if candidate.is_file() {
            return Some(candidate);
        }
        let parent = dir.parent()?;
        if Some(parent) == ceiling {
            return None;
        }
        dir = parent;
    }
}

/// What [`install_flow`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallReport {
    /// Workspace name: the repo flow's `name:`, else its directory name.
    pub name: String,
    /// True when the workspace directory did not exist before.
    pub created: bool,
    /// `<sb_home>/workspaces/<name>/flow.yaml`.
    pub installed_flow: PathBuf,
    pub modules: Vec<InstalledModule>,
    /// The repo flow's `messages:` with every `path` made absolute.
    pub messages: Vec<MessageSource>,
}

/// One module of an installed workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledModule {
    pub name: String,
    /// Absolute path of the module's `sb.dev.yml`.
    pub dev_yml: PathBuf,
    /// False when `sb up` skips the module.
    pub up: bool,
    /// Whether `dev_yml` is present on disk (a module may be registered
    /// before `sb init` has run in it).
    pub exists: bool,
}

/// Install the repo-root `flow.yaml` at `repo_flow` as a workspace.
///
/// - The name is the flow's `name:`, else the name of its directory.
/// - Every module and message path is resolved against the flow's
///   directory. A module path may name the module directory (`sb.dev.yml`
///   is appended) or the yaml itself.
/// - The installed copy at `<sb_home>/workspaces/<name>/flow.yaml` gets
///   absolute paths and `source: { repo_flow, url, installed_at }`. The
///   workspace directory is created when missing; re-installing is
///   idempotent. The workspace becomes the active one.
/// - The repo copy is never written.
///
/// Refuses to replace a workspace of the same name that was installed
/// from a different repo flow which still exists; one of the two repos
/// needs a distinct `name:`.
pub fn install_flow(home: &SbHome, repo_flow: &Path, url: Option<&str>) -> Result<InstallReport> {
    let repo_flow = repo_flow
        .canonicalize()
        .with_context(|| format!("resolving {}", repo_flow.display()))?;
    if !repo_flow.is_file() {
        bail!("{} is not a file", repo_flow.display());
    }
    refuse_installed_copy(home, &repo_flow)?;
    let repo_dir = repo_flow
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent directory", repo_flow.display()))?;
    let repo = read_flow_file(&repo_flow)?;
    let name = workspace_name_for(&repo, &repo_flow)?;

    let created = !home.workspace_dir(&name).exists();
    if !created {
        refuse_foreign_install(home, &name, &repo_flow)?;
    }
    // Re-installing from the same repo flow without a URL (`sb install .`
    // inside a clone, or `sb init`) keeps the URL recorded earlier, so
    // `sb ws update` still knows where to pull from.
    let url = url.map(str::to_owned).or_else(|| {
        if created {
            return None;
        }
        let prev = read_flow(home, &name).ok()?.source?;
        if prev.repo_flow == repo_flow {
            prev.url
        } else {
            None
        }
    });

    let mut modules = Vec::with_capacity(repo.modules.len());
    let mut installed_modules = BTreeMap::new();
    for (module, entry) in &repo.modules {
        let dev_yml = resolve_module_yaml(repo_dir, &entry.path);
        modules.push(InstalledModule {
            name: module.clone(),
            exists: dev_yml.is_file(),
            up: entry.up,
            dev_yml: dev_yml.clone(),
        });
        installed_modules.insert(
            module.clone(),
            ModuleRef {
                path: dev_yml,
                up: entry.up,
            },
        );
    }
    let messages: Vec<MessageSource> = repo
        .messages
        .iter()
        .map(|src| MessageSource {
            path: resolve_path(repo_dir, &src.path),
            ..src.clone()
        })
        .collect();

    let installed = WorkspaceFlow {
        name: Some(name.clone()),
        modules: installed_modules,
        messages: messages.clone(),
        instances: repo.instances,
        source: Some(FlowSource {
            repo_flow: repo_flow.clone(),
            url,
            installed_at: rfc3339_utc(SystemTime::now()),
        }),
    };
    write_flow(home, &name, &installed)?;
    set_active(home, &name)?;

    Ok(InstallReport {
        installed_flow: home.flow_yaml(&name),
        name,
        created,
        modules,
        messages,
    })
}

/// The workspace name a repo flow installs under, validated.
fn workspace_name_for(flow: &WorkspaceFlow, repo_flow: &Path) -> Result<String> {
    let name = match &flow.name {
        Some(n) => n.clone(),
        None => repo_flow
            .parent()
            .and_then(Path::file_name)
            .and_then(|s| s.to_str())
            .ok_or_else(|| {
                anyhow!(
                    "cannot derive a workspace name from {}; set `name:` in it",
                    repo_flow.display()
                )
            })?
            .to_owned(),
    };
    validate_identifier(&name).map_err(|e| {
        anyhow!(
            "workspace name {name:?} (from {}) is invalid: {e}; set `name:` in that flow.yaml",
            repo_flow.display()
        )
    })?;
    Ok(name)
}

/// An installed copy is derived data; installing it would make the
/// workspace its own source.
fn refuse_installed_copy(home: &SbHome, flow: &Path) -> Result<()> {
    if is_installed_copy(home, flow) {
        bail!(
            "{} is an installed workspace copy; install from the repository's flow.yaml instead",
            flow.display()
        );
    }
    Ok(())
}

/// Two live repositories must not take turns overwriting one workspace.
/// A previous source that no longer exists (the repo moved) is replaced.
fn refuse_foreign_install(home: &SbHome, name: &str, repo_flow: &Path) -> Result<()> {
    // A corrupt installed copy is replaced rather than blocking the install.
    let Some(prev) = read_flow(home, name).ok().and_then(|f| f.source) else {
        return Ok(());
    };
    if prev.repo_flow != repo_flow && prev.repo_flow.is_file() {
        bail!(
            "workspace {name:?} is already installed from {}; uninstall it first, \
             or give {} a different `name:`",
            prev.repo_flow.display(),
            repo_flow.display()
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

/// A module entry's `sb.dev.yml`. The entry may name the module
/// directory or the yaml file; a path that does not exist is taken as a
/// directory unless it ends in `.yml` / `.yaml`.
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
        resolved
    } else {
        resolved.join(DEV_YML)
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
/// workspace that was not installed from a repo flow.
pub fn installed_source(home: &SbHome, name: &str) -> Result<Option<FlowSource>> {
    if !home.workspace_dir(name).exists() {
        bail!("workspace {name:?} does not exist");
    }
    Ok(read_flow(home, name)?.source)
}

/// `Some((repo_flow, installed_flow))` when the repo flow was modified
/// after the installed copy was written, i.e. a re-install is due.
/// `None` when the workspace has no `source:` or either file is missing.
pub fn is_stale(home: &SbHome, name: &str) -> Result<Option<(PathBuf, PathBuf)>> {
    let Some(src) = installed_source(home, name)? else {
        return Ok(None);
    };
    let installed = home.flow_yaml(name);
    let mtime = |p: &Path| fs::metadata(p).and_then(|m| m.modified()).ok();
    let (Some(repo_m), Some(inst_m)) = (mtime(&src.repo_flow), mtime(&installed)) else {
        return Ok(None);
    };
    Ok((repo_m > inst_m).then_some((src.repo_flow, installed)))
}

/// Pull the repository a workspace was installed from, then re-install
/// its flow. Requires `source.url`: a workspace installed from a local
/// path has no remote to pull and is refreshed by re-installing it.
///
/// Runs `git -C <repo flow dir> pull --ff-only`. The report's `name` can
/// differ from `name` if the pulled flow changed its `name:`.
pub fn update_from_source(home: &SbHome, name: &str) -> Result<InstallReport> {
    let src = installed_source(home, name)?.ok_or_else(|| {
        anyhow!("workspace {name:?} was not installed from a repository flow.yaml")
    })?;
    let url = src.url.as_deref().ok_or_else(|| {
        anyhow!(
            "workspace {name:?} was installed from a local path ({}) with no URL; \
             update that checkout yourself and install it again",
            src.repo_flow.display()
        )
    })?;
    let repo_root = src
        .repo_flow
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent directory", src.repo_flow.display()))?;
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
    install_flow(home, &src.repo_flow, Some(url))
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
/// IO directory (`io_dir:` of its `sb.dev.yml`, default `swarmbotix_io`)
/// is deleted from the module root first.
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
        let flow = read_flow(home, name)?;
        for (module, entry) in &flow.modules {
            match module_io_dir(&entry.path) {
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

/// The IO directory of the module whose `sb.dev.yml` is `dev_yml`, if it
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
    /// Language flag, if the user passed one. Required if no `sb.dev.yml`
    /// exists yet at `root`.
    pub language: Option<Language>,
    /// `--force` — overwrite existing `sb.dev.yml` / `runscript.bash`.
    pub force: bool,
    /// `--docker` — emit a docker-flavored runscript that dispatches on
    /// `--id <name>` to one pre-resolved `docker run` invocation per
    /// instance in `flow.yaml::instances.<module>`. Requires that block
    /// to exist and every listed instance to have a `docker: { image }`.
    /// The block is read from the repo `flow.yaml` when the module sits
    /// under one, else from the active workspace's.
    pub docker: bool,
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
    pub dev_yml_written: bool,
    pub runscript_written: bool,
    pub io_dir_created: bool,
    /// The module was added to (or repointed in) the flow that registers
    /// it: `repo_flow` when set, else the active workspace's installed copy.
    pub flow_appended: bool,
    /// Workspace the module is registered in. In repo mode this is the
    /// workspace the repo flow installs as, which is now the active one.
    pub workspace: String,
    /// The repository `flow.yaml` the module was registered in, when the
    /// module sits under one. `None` means the module went straight into
    /// the active workspace's installed copy.
    pub repo_flow: Option<PathBuf>,
}

/// Where [`init_module`] registers a module.
enum Registry {
    /// A repo `flow.yaml` enclosing the module root (canonical path).
    Repo(PathBuf),
    /// The active workspace's installed copy (workspace name).
    Active(String),
}

/// Adopt `opts.root` as a swarmbotix module. Drops only the missing
/// artifacts (`sb.dev.yml`, `runscript.bash`, `swarmbotix_io/`) unless
/// `--force`, then registers the module. Idempotent.
///
/// Registration: when [`find_flow_upwards`] finds a repo `flow.yaml`
/// above the module root, the module is added there with a path relative
/// to that file (layout and comments are kept when the entry can be
/// appended as one line) and the repo flow is installed with
/// [`install_flow`]. Otherwise the module goes into the active
/// workspace's installed `flow.yaml`, which then must exist.
///
/// `sb.dev.yml` is written without `root:`; its own directory is the
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

    // 2. Registry: the enclosing repo flow.yaml, else the active workspace.
    //    A repo flow that cannot be installed (unparseable, bad name, name
    //    held by another repo) fails here, before anything is written.
    let registry = match find_flow_upwards(&root_abs).filter(|f| !is_installed_copy(home, f)) {
        Some(flow) => {
            let flow = flow.canonicalize().unwrap_or(flow);
            let name = workspace_name_for(&read_flow_file(&flow)?, &flow)?;
            if home.workspace_dir(&name).exists() {
                refuse_foreign_install(home, &name, &flow)?;
            }
            Registry::Repo(flow)
        }
        None => Registry::Active(active_workspace(home)?.ok_or_else(|| {
            anyhow!(
                "no active workspace and no flow.yaml above {}; run \
                 `sb ws create <name> && sb ws set <name>` first, or add a flow.yaml \
                 at the repository root",
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

    let dev_yml_path = root_abs.join(DEV_YML);
    let runscript_path = root_abs.join("runscript.bash");

    // 4. Reconcile language: flag vs file-on-disk.
    let existing_dev: Option<ModuleDevConfig> = if dev_yml_path.exists() {
        let s = fs::read_to_string(&dev_yml_path)
            .with_context(|| format!("reading {}", dev_yml_path.display()))?;
        if s.trim().is_empty() {
            None
        } else {
            Some(
                serde_yaml::from_str(&s)
                    .with_context(|| format!("parsing {}", dev_yml_path.display()))?,
            )
        }
    } else {
        None
    };

    let language = match (&existing_dev, opts.language) {
        (Some(d), Some(flag)) if d.language != flag => {
            bail!(
                "language tag mismatch: {} has language: {}, but --{} was passed",
                dev_yml_path.display(),
                d.language,
                flag
            );
        }
        (Some(d), _) => d.language,
        (None, Some(flag)) => flag,
        (None, None) => bail!(
            "no sb.dev.yml at {} and no --rust|--python|--cpp|--flutter|--unity flag",
            root_abs.display()
        ),
    };

    let io_dir = existing_dev
        .as_ref()
        .map_or_else(|| language.default_io_dir(), |d| d.io_dir.clone());

    let mut outcome = InitOutcome {
        module: module.clone(),
        root_abs: root_abs.clone(),
        language: Some(language),
        ..Default::default()
    };

    // 5. Write sb.dev.yml, only if missing or --force. No `root:`: the
    //    file's own directory is the module root, which keeps it portable.
    if existing_dev.is_none() || opts.force {
        let cfg = ModuleDevConfig {
            module: module.clone(),
            language,
            root: None,
            io_dir: io_dir.clone(),
            publishers: Vec::new(),
            subscribers: Vec::new(),
            services: Vec::new(),
        };
        let body = serde_yaml::to_string(&cfg).context("serializing sb.dev.yml")?;
        fs::write(&dev_yml_path, body)
            .with_context(|| format!("writing {}", dev_yml_path.display()))?;
        outcome.dev_yml_written = true;
    }

    // 6. Write runscript.bash — only if missing, or --force.
    //    `--docker` picks a different template, but the missing-or-force
    //    gate is the same. Instances come from the registry's flow.yaml.
    if !runscript_path.exists() || opts.force {
        let body = if opts.docker {
            let (flow, flow_path, origin) = match &registry {
                Registry::Repo(path) => (
                    read_flow_file(path)?,
                    path.clone(),
                    format!("the repository flow.yaml at {}", path.display()),
                ),
                Registry::Active(ws) => (
                    read_flow(home, ws)?,
                    home.flow_yaml(ws),
                    format!(
                        "the active workspace's flow.yaml ({})",
                        home.flow_yaml(ws).display()
                    ),
                ),
            };
            let instances = preflight_docker_instances(&flow, &module, &origin)?;
            runscript::render_docker(&runscript_path, &module, instances, &flow_path)
        } else {
            runscript::render(&runscript_path, &module)
        };
        fs::write(&runscript_path, body)
            .with_context(|| format!("writing {}", runscript_path.display()))?;
        set_executable(&runscript_path)?;
        outcome.runscript_written = true;
    }

    // 7. Create the IO directory (empty). Safe to call if already present.
    let io_full = root_abs.join(&io_dir);
    if !io_full.exists() {
        fs::create_dir_all(&io_full).with_context(|| format!("creating {}", io_full.display()))?;
        outcome.io_dir_created = true;
    }

    // 8. Register (idempotent).
    match registry {
        Registry::Repo(repo_flow) => {
            outcome.flow_appended = register_in_repo_flow(&repo_flow, &module, &root_abs)?;
            // install_flow keeps any URL an earlier install recorded.
            let report = install_flow(home, &repo_flow, None)?;
            outcome.workspace = report.name;
            outcome.repo_flow = Some(repo_flow);
        }
        Registry::Active(active) => {
            let mut flow = read_flow(home, &active)?;
            let entry = flow.modules.get(&module);
            if entry.map(|m| m.path.as_path()) != Some(dev_yml_path.as_path()) {
                // Repointing keeps the entry's `up:` choice.
                let up = entry.is_none_or(|m| m.up);
                flow.modules.insert(
                    module.clone(),
                    ModuleRef {
                        path: dev_yml_path.clone(),
                        up,
                    },
                );
                write_flow(home, &active, &flow)?;
                outcome.flow_appended = true;
            }
            outcome.workspace = active;
        }
    }

    Ok(outcome)
}

/// Whether `flow` lies under `<sb_home>/workspaces/`, i.e. is an
/// installed copy rather than a repository's flow.
fn is_installed_copy(home: &SbHome, flow: &Path) -> bool {
    let ws_root = home.workspaces_dir();
    let ws_root = ws_root.canonicalize().unwrap_or(ws_root);
    flow.starts_with(&ws_root)
}

/// Add `module` (rooted at `module_root`) to the repo flow at `flow_path`
/// with a `/`-separated path relative to that file's directory. Returns
/// whether the file changed.
///
/// A new entry is appended as one line under `modules:` so the user's
/// comments and layout survive; the edit is re-parsed and must equal the
/// intended flow, else the file is rewritten from the parsed form. An
/// entry of the same name pointing at another live module is an error
/// (module names are unique in a workspace); one pointing at a missing
/// module is repointed, keeping its `up:`.
fn register_in_repo_flow(flow_path: &Path, module: &str, module_root: &Path) -> Result<bool> {
    let repo_dir = flow_path
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent directory", flow_path.display()))?;
    let text = fs::read_to_string(flow_path)
        .with_context(|| format!("reading {}", flow_path.display()))?;
    let mut flow = parse_flow_text(&text, flow_path)?;
    let rel = relative_slash_path(module_root, repo_dir).ok_or_else(|| {
        anyhow!(
            "{} is not inside {}",
            module_root.display(),
            repo_dir.display()
        )
    })?;

    if let Some(existing) = flow.modules.get(module) {
        let want = module_root.join(DEV_YML);
        let want = want.canonicalize().unwrap_or(want);
        let have = resolve_module_yaml(repo_dir, &existing.path);
        if have == want {
            return Ok(false);
        }
        if have.is_file() {
            bail!(
                "module {module:?} is already registered in {} for {}; module names \
                 must be unique within a workspace",
                flow_path.display(),
                have.display()
            );
        }
        let up = existing.up;
        flow.modules.insert(
            module.to_owned(),
            ModuleRef {
                path: PathBuf::from(&rel),
                up,
            },
        );
        let body = serde_yaml::to_string(&flow).context("serializing flow.yaml")?;
        fs::write(flow_path, body).with_context(|| format!("writing {}", flow_path.display()))?;
        return Ok(true);
    }

    let mut expected = flow;
    expected
        .modules
        .insert(module.to_owned(), ModuleRef::new(&rel));
    let body = match append_module_line(&text, module, &rel)
        .filter(|t| parse_flow_text(t, flow_path).is_ok_and(|f| f == expected))
    {
        Some(t) => t,
        None => serde_yaml::to_string(&expected).context("serializing flow.yaml")?,
    };
    fs::write(flow_path, body).with_context(|| format!("writing {}", flow_path.display()))?;
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

/// `target` relative to `base`, `/`-separated so a committed flow.yaml
/// reads the same on every OS. `.` when they are equal; `None` when
/// `target` is not under `base` or is not valid UTF-8.
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

/// Resolve the list of instances `sb init --docker` should bake into
/// the runscript. Surfaces the error messages users see when they
/// forget to populate `flow.yaml::instances` or leave an instance
/// without a `docker.image`. `origin` names the flow the instances were
/// read from (the repository's, or the active workspace's installed copy).
fn preflight_docker_instances<'a>(
    flow: &'a sb_core::WorkspaceFlow,
    module: &str,
    origin: &str,
) -> Result<&'a [sb_core::ModuleInstance]> {
    let list = flow.instances.get(module).map_or(&[][..], Vec::as_slice);
    if list.is_empty() {
        bail!(
            "--docker requires at least one instance for module {module:?} \
             declared in {origin} under `instances.{module}:`; \
             see documents/sbcli_docker_runscript.md"
        );
    }
    for inst in list {
        let docker = inst.docker.as_ref().ok_or_else(|| {
            anyhow!(
                "--docker: instance {:?} of module {module:?} has no `docker:` block \
                 in {origin}; see documents/sbcli_docker_runscript.md",
                inst.id
            )
        })?;
        if docker.image.as_deref().unwrap_or("").is_empty() {
            bail!(
                "--docker: instance {:?} of module {module:?} has no `docker.image` \
                 in {origin}; see documents/sbcli_docker_runscript.md",
                inst.id
            );
        }
    }
    Ok(list)
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)
        .with_context(|| format!("stat {}", path.display()))?
        .permissions();
    // 0o755 — owner rwx, group/other r-x.
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).with_context(|| format!("chmod 0755 {}", path.display()))?;
    Ok(())
}

// The Result is load-bearing on the other side of the cfg: the unix twin
// really can fail, and callers reach both through one `?`. Narrowing this
// half to `()` would split the call sites by platform.
#[allow(clippy::unnecessary_wraps)]
#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<()> {
    // Windows: no chmod bit; .bat / .cmd extensions or a launcher do the work.
    Ok(())
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
    /// (registered by yaml path, `up: false`, not adopted yet), and `msgs/`.
    fn demo_repo(root: &Path) -> PathBuf {
        let repo = root.join("repo");
        write(
            &repo.join("pubber").join("sb.dev.yml"),
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
        let flow = repo.join(FLOW_FILE);
        write(
            &flow,
            "# demo repo\nname: demo\nmodules:\n  pubber: pubber\n  subber:\n    \
             path: apps/subber/sb.dev.yml\n    up: false\nmessages:\n  - style: acme\n    \
             path: ./msgs\n    backends: [proto]\n",
        );
        flow
    }

    #[test]
    fn install_flow_refuses_a_second_live_repo_with_the_same_name() {
        let (td, home) = sandbox();
        let a = td.path().join("a").join(FLOW_FILE);
        let b = td.path().join("b").join(FLOW_FILE);
        write(&a, "name: shared\nmodules: {}\n");
        write(&b, "name: shared\nmodules: {}\n");
        install_flow(&home, &a, None).unwrap();
        let e = format!("{:#}", install_flow(&home, &b, None).unwrap_err());
        assert!(e.contains("already installed from"), "{e}");

        // Once the first repo is gone, the name is free again.
        fs::remove_file(&a).unwrap();
        install_flow(&home, &b, None).unwrap();
        // An installed copy is never itself a source.
        let e = format!(
            "{:#}",
            install_flow(&home, &home.flow_yaml("shared"), None).unwrap_err()
        );
        assert!(e.contains("installed workspace copy"), "{e}");
    }

    #[test]
    fn uninstall_with_clean_removes_io_dirs_and_skips_unsafe_ones() {
        let (td, home) = sandbox();
        let repo_flow = demo_repo(td.path());
        // A third module whose io_dir climbs out of its root.
        write(
            &td.path().join("repo").join("rogue").join("sb.dev.yml"),
            "module: rogue\nlanguage: rust\nio_dir: ../pubber\n",
        );
        let text = fs::read_to_string(&repo_flow).unwrap();
        fs::write(
            &repo_flow,
            text.replace("modules:\n", "modules:\n  rogue: rogue\n"),
        )
        .unwrap();
        install_flow(&home, &repo_flow, None).unwrap();
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
        assert!(repo_flow.is_file(), "the repo is never removed");
        assert!(uninstall_workspace(&home, "demo", false).is_err());
    }
}
