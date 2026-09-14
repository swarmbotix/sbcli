//! L2 — workspace lifecycle and module init.
//!
//! Two concerns, one crate:
//!
//! 1. **Workspace registry** under `~/.swarmbotix/`:
//!    - `workspaces/<name>/flow.yaml` — module registry per workspace
//!    - `active` — one-line marker file holding the active workspace name
//!
//! 2. **Module init** (`sb init`) — adopt an existing user project as a
//!    swarmbotix module by writing `sb.dev.yml`, a `runscript.bash` stub,
//!    and an empty `swarmbotix_io/`, then registering the module in the
//!    active workspace's `flow.yaml`.
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

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

use sb_core::{Language, ModuleDevConfig, SbCliConfig, WorkspaceFlow, validate_identifier};

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
    let s = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    if s.trim().is_empty() {
        return Ok(WorkspaceFlow::default());
    }
    let flow: WorkspaceFlow =
        serde_yaml::from_str(&s).with_context(|| format!("parsing {}", path.display()))?;
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
    pub flow_appended: bool,
}

/// Adopt `opts.root` as a swarmbotix module. Drops only the missing
/// artifacts (`sb.dev.yml`, `runscript.bash`, `swarmbotix_io/`) unless
/// `--force`, then registers the module in the active workspace's
/// `flow.yaml`. Idempotent.
// Long by construction: this is the numbered adoption procedure, and each
// step's preconditions are checked against the ones before it. Splitting it
// would mean threading a dozen locals through private helpers that no other
// caller has any use for.
#[allow(clippy::too_many_lines)]
pub fn init_module(home: &SbHome, opts: &InitOptions<'_>) -> Result<InitOutcome> {
    // 1. Active workspace required.
    let active = active_workspace(home)?.ok_or_else(|| {
        anyhow!("no active workspace — run `sb ws create <name> && sb ws set <name>` first")
    })?;

    // 2. Resolve rootpath → absolute canonical form.
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

    // 3. Module name = basename(root_abs). Validate.
    let module = root_abs
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow!("rootpath {} has no usable basename", root_abs.display()))?
        .to_owned();
    validate_identifier(&module)
        .map_err(|e| anyhow!("invalid module name {module:?} (from rootpath basename): {e}"))?;

    let dev_yml_path = root_abs.join("sb.dev.yml");
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

    // 5. Write sb.dev.yml — only if missing, or --force.
    if existing_dev.is_none() || opts.force {
        let cfg = ModuleDevConfig {
            module: module.clone(),
            language,
            root: root_abs.clone(),
            io_dir: io_dir.clone(),
            publishers: Vec::new(),
            subscribers: Vec::new(),
        };
        let body = serde_yaml::to_string(&cfg).context("serializing sb.dev.yml")?;
        fs::write(&dev_yml_path, body)
            .with_context(|| format!("writing {}", dev_yml_path.display()))?;
        outcome.dev_yml_written = true;
    }

    // 6. Write runscript.bash — only if missing, or --force.
    //    `--docker` picks a different template, but the missing-or-force
    //    gate is the same.
    if !runscript_path.exists() || opts.force {
        let body = if opts.docker {
            let flow = read_flow(home, &active)?;
            let instances = preflight_docker_instances(&flow, &module)?;
            runscript::render_docker(
                &runscript_path,
                &module,
                instances,
                &home.flow_yaml(&active),
            )
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

    // 8. Register in active workspace's flow.yaml — idempotent.
    let mut flow = read_flow(home, &active)?;
    let entry = flow.modules.get(&module).cloned();
    if entry.as_deref() != Some(dev_yml_path.as_path()) {
        flow.modules.insert(module.clone(), dev_yml_path.clone());
        write_flow(home, &active, &flow)?;
        outcome.flow_appended = true;
    }

    Ok(outcome)
}

/// Resolve the list of instances `sb init --docker` should bake into
/// the runscript. Surfaces the error messages users see when they
/// forget to populate `flow.yaml::instances` or leave an instance
/// without a `docker.image`.
fn preflight_docker_instances<'a>(
    flow: &'a sb_core::WorkspaceFlow,
    module: &str,
) -> Result<&'a [sb_core::ModuleInstance]> {
    let list = flow.instances.get(module).map_or(&[][..], Vec::as_slice);
    if list.is_empty() {
        bail!(
            "--docker requires at least one instance for module {module:?} \
             declared in the active workspace's flow.yaml under `instances.{module}:`; \
             see documents/sbcli_docker_runscript.md"
        );
    }
    for inst in list {
        let docker = inst.docker.as_ref().ok_or_else(|| {
            anyhow!(
                "--docker: instance {:?} of module {module:?} has no `docker:` block \
                 in flow.yaml; see documents/sbcli_docker_runscript.md",
                inst.id
            )
        })?;
        if docker.image.as_deref().unwrap_or("").is_empty() {
            bail!(
                "--docker: instance {:?} of module {module:?} has no `docker.image` \
                 in flow.yaml; see documents/sbcli_docker_runscript.md",
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
