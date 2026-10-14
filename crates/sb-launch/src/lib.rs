//! L5: launch modules in a tmux/psmux session.
//!
//! `sb up` reads the active workspace's installed flow, loads every module
//! manifest (`sb.module.yaml`, or the deprecated `sb.dev.yml`), merges it
//! for the host ([`ModuleDevConfig::resolve`]), runs each module's build
//! inline, and then creates a tmux session named `sb-<workspace>` with one
//! window per instance: `<module>-<id>`, or `<module>` for a module without
//! instances.
//!
//! Each module launches in one of two modes:
//!
//! - **Native**: the manifest declares `scripts:` (top level or under a
//!   platform), or the module root has no `runscript.bash`. sb runs the
//!   resolved `start` argv itself, with tmux `-c <root>`, one `-e` per
//!   environment variable, the argv as separate arguments (no shell), and
//!   `--id <id>` appended per instance.
//! - **Legacy** (deprecated since 0.6.0): `runscript.bash` present and no
//!   `scripts:`. The pane runs `cd <root> && <root>/runscript.bash <args>`,
//!   exactly as before 0.6.
//!
//! Plan, then execute: the `plan_*` functions check every precondition
//! (flow, manifests, platform support, placeholder expansion) and
//! [`run_builds`] runs every build before any tmux call that changes state,
//! so a failed build never leaves a session behind (TDD #2 in
//! [level5.html](../../plan/level5.html)).
//!
//! tmux targets are exact: sessions as `=<name>`, windows by the `@<id>`
//! that `list-windows` reports, so `sb-demo` never matches `sb-demo2`.
//! On Windows `psmux` provides the `tmux` command; [`resolve_tmux`] also
//! accepts a binary named `psmux`.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};

use sb_core::expand::{ExpandCtx, ExpandError, expand_argv, expand_str};
use sb_core::topic::instance_module;
use sb_core::{
    Deprecation, FLOW_FILE, Host, LEGACY_MODULE_FILE, MODULE_FILE, ManifestError,
    ModuleDevConfig, ModuleInstance, Resolved, WorkspaceFlow, is_legacy_module_file,
    python_fallback, python_venv_relpath,
};
use sb_workspace::{SbHome, find_module_file, read_flow};

/// One argv, already split (the same type as `sb_core::Argv`).
type Argv = Vec<String>;

/// Workspace name, exported to every native pane and build.
pub const ENV_WORKSPACE: &str = "SB_WORKSPACE";
/// Manifest `module:` name, exported to every native pane and build.
pub const ENV_MODULE: &str = "SB_MODULE";
/// Instance id, exported to a native pane that runs one instance.
pub const ENV_INSTANCE_ID: &str = "SB_INSTANCE_ID";
/// Absolute module root (forward slashes), exported to every native pane and build.
pub const ENV_MODULE_ROOT: &str = "SB_MODULE_ROOT";

const RUNSCRIPT: &str = "runscript.bash";
const STOPSCRIPT: &str = "stopscript.bash";
const MIGRATING: &str = "see sbcli_module.md §Migrating";

// ─────────────────────────────────────────────────────────────────────
// Session name + plan types
// ─────────────────────────────────────────────────────────────────────

/// Tmux session name for a given workspace. Single source of truth: all
/// L5 verbs derive their target session through this function so a typo
/// in one place cannot desync the others.
pub fn session_name(workspace: &str) -> String {
    format!("sb-{workspace}")
}

/// What a pane runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneCmd {
    /// Deprecated: `cd <cwd> && <script> <args>` through the pane's shell.
    Legacy { script: PathBuf, args: Vec<String> },
    /// The module's process itself. `argv` is fully expanded (instance
    /// args, passthrough args and `--id <id>` included); `env` is fully
    /// merged, `SB_*` variables included.
    Native { argv: Argv, env: BTreeMap<String, String> },
}

/// One tmux window in a launch plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pane {
    /// The manifest's `module:` name.
    pub module: String,
    /// Window name: `<module>-<id>` for an instance, else `<module>`.
    pub window: String,
    /// Instance id, when the flow lists instances for the module.
    pub instance: Option<String>,
    /// Module root; the pane starts here (tmux `-c`).
    pub cwd: PathBuf,
    pub cmd: PaneCmd,
}

impl Pane {
    /// True for a [`PaneCmd::Native`] pane.
    pub fn is_native(&self) -> bool {
        matches!(self.cmd, PaneCmd::Native { .. })
    }

    /// The pane's command as one POSIX shell line, every word
    /// single-quoted.
    ///
    /// Legacy: `cd '<cwd>' && '<script>' '<arg>'...`, the string tmux hands
    /// to the pane's shell, so the script's relative paths resolve from the
    /// module root and an arg like `"foo bar"` survives both layers of shell
    /// parsing. Native: the argv, quoted word by word (for display, and for
    /// the one-word case where tmux would otherwise run a shell anyway).
    pub fn shell_command(&self) -> String {
        match &self.cmd {
            PaneCmd::Legacy { script, args } => {
                let mut cmd = format!(
                    "cd {} && {}",
                    shell_quote(&self.cwd.display().to_string()),
                    shell_quote(&script.display().to_string()),
                );
                for arg in args {
                    cmd.push(' ');
                    cmd.push_str(&shell_quote(arg));
                }
                cmd
            }
            PaneCmd::Native { argv, .. } => argv
                .iter()
                .map(|a| shell_quote(a))
                .collect::<Vec<_>>()
                .join(" "),
        }
    }
}

/// One module's inline commands (`scripts.build`, or `scripts.stop`), run
/// in order with inherited stdio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildStep {
    pub module: String,
    pub cwd: PathBuf,
    pub steps: Vec<Argv>,
    /// Applied on top of sb's own environment: the module's resolved `env`
    /// plus `SB_WORKSPACE`, `SB_MODULE` and `SB_MODULE_ROOT`.
    pub env: BTreeMap<String, String>,
}

/// What `sb up` / `sb run` will do. Built before any tmux call so the same
/// plan can be inspected by tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    pub workspace: String,
    pub session: String,
    /// One per native module that has build commands, in pane order.
    pub builds: Vec<BuildStep>,
    pub panes: Vec<Pane>,
    /// `(module, reason)` for modules `sb up` leaves out on this host.
    pub skipped: Vec<(String, String)>,
    pub deprecations: Vec<Deprecation>,
    /// Modules whose every instance is in this plan. [`run`] closes their
    /// `<module>` / `<module>-*` windows that the plan no longer names
    /// (instances removed from the flow). Empty for `<module>:<id>` targets
    /// and for legacy modules.
    pub prune: Vec<String>,
}

impl LaunchPlan {
    fn new(workspace: &str) -> Self {
        Self {
            workspace: workspace.to_string(),
            session: session_name(workspace),
            builds: Vec::new(),
            panes: Vec::new(),
            skipped: Vec::new(),
            deprecations: Vec::new(),
            prune: Vec::new(),
        }
    }
}

/// What `sb stop <module>` will do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopPlan {
    pub session: String,
    /// The manifest's `module:` name; every `<module>` and `<module>-*`
    /// window belongs to it.
    pub module: String,
    /// Some: a legacy module with a `stopscript.bash`, run in the module's
    /// window as before 0.6.
    pub legacy: Option<Pane>,
    /// Some: `scripts.stop`, run inline after the windows are closed.
    pub stop: Option<BuildStep>,
    pub deprecations: Vec<Deprecation>,
}

// ─────────────────────────────────────────────────────────────────────
// Plan: read the installed flow + every module manifest
// ─────────────────────────────────────────────────────────────────────

/// Plan `sb up`: every module with `up: true`, one pane per instance.
///
/// Modules the host does not support, modules with no start command
/// (unity), and modules whose instances ask for docker are listed in
/// `skipped` instead. Every other problem (missing manifest, parse error,
/// bad placeholder) is an error here, before tmux is touched.
pub fn plan_workspace(home: &SbHome, workspace: &str, host: &Host) -> Result<LaunchPlan> {
    let flow = load_flow(home, workspace)?;
    if flow.modules.is_empty() {
        bail!("workspace {workspace:?} has no modules in its flow; run `sb init` to adopt one");
    }
    let mut plan = LaunchPlan::new(workspace);
    flow_deprecations(&flow, &mut plan.deprecations);
    for (key, entry) in flow.modules.iter().filter(|(_, m)| m.up) {
        let lm = load_module(key, &entry.path)?;
        push_unique(&mut plan.deprecations, &lm.deprecations);
        if !lm.dev.supports_host(host) {
            plan.skipped.push((key.clone(), unsupported_reason(&lm.dev, host)));
            continue;
        }
        if lm.mode == Mode::Legacy {
            plan.panes.push(legacy_pane(&lm, RUNSCRIPT, &[]));
            continue;
        }
        let resolved = match lm.dev.resolve(host) {
            Ok(resolved) => resolved,
            Err(ManifestError::NoStart(lang)) => {
                plan.skipped.push((
                    key.clone(),
                    format!("{lang} modules have no start command (launch them from their editor)"),
                ));
                continue;
            }
            Err(e) => return Err(resolve_error(&lm, e)),
        };
        let instances = flow.instances_for(key);
        if let Some(id) = docker_instance(instances) {
            plan.skipped.push((key.clone(), docker_reason(id)));
            continue;
        }
        let native = Native::new(&lm, resolved, workspace, host)?;
        if let Some(step) = native.build_step()? {
            push_build(&mut plan.builds, step);
        }
        if instances.is_empty() {
            plan.panes.push(native.pane(None, &[])?);
        } else {
            for inst in instances {
                plan.panes.push(native.pane(Some(inst), &[])?);
            }
        }
    }
    if plan.panes.is_empty() {
        if plan.skipped.is_empty() {
            bail!(
                "workspace {workspace:?} has no modules to launch: every module in its flow \
                 has `up: false` (use `sb run <module>` to start one)"
            );
        }
        let reasons: Vec<String> = plan
            .skipped
            .iter()
            .map(|(module, reason)| format!("{module}: {reason}"))
            .collect();
        bail!(
            "workspace {workspace:?} has nothing to launch on {}: {}",
            host_label(host),
            reasons.join("; ")
        );
    }
    Ok(plan)
}

/// Plan `sb run <target> [args...]`.
///
/// `target` is `<module>` (every instance) or `<module>:<id>` (one
/// instance). `args` are appended after each instance's own `args` (or
/// forwarded verbatim to a legacy `runscript.bash`). `up: false` is
/// ignored here: it only keeps a module out of `sb up`. An unsupported
/// platform is an error that names the supported ones.
pub fn plan_run(
    home: &SbHome,
    workspace: &str,
    target: &str,
    args: &[String],
    host: &Host,
) -> Result<LaunchPlan> {
    let (key, id) = parse_target(target)?;
    let flow = load_flow(home, workspace)?;
    let entry = registered(&flow, workspace, key)?;
    let lm = load_module(key, &entry.path)?;
    let mut plan = LaunchPlan::new(workspace);
    flow_deprecations(&flow, &mut plan.deprecations);
    push_unique(&mut plan.deprecations, &lm.deprecations);
    if !lm.dev.supports_host(host) {
        bail!("module {key:?} is {}", unsupported_reason(&lm.dev, host));
    }
    if lm.mode == Mode::Legacy {
        if let Some(id) = id {
            bail!(
                "module {key:?} still launches through {RUNSCRIPT}, which has no instance \
                 targets; declare scripts: in its {MODULE_FILE} to run {key}:{id}, or pass \
                 --id {id} to the script (`sb run {key} --id {id}`)"
            );
        }
        plan.panes.push(legacy_pane(&lm, RUNSCRIPT, args));
        return Ok(plan);
    }
    let resolved = lm.dev.resolve(host).map_err(|e| resolve_error(&lm, e))?;
    let instances = flow.instances_for(key);
    let chosen: Vec<&ModuleInstance> = match id {
        None => instances.iter().collect(),
        Some(id) => {
            if instances.is_empty() {
                bail!("module {key:?} has no instances; run it as `sb run {key}`");
            }
            let inst = instances.iter().find(|i| i.id == id).ok_or_else(|| {
                let ids: Vec<&str> = instances.iter().map(|i| i.id.as_str()).collect();
                anyhow!(
                    "module {key:?} has no instance {id:?} (instances: {})",
                    ids.join(", ")
                )
            })?;
            vec![inst]
        }
    };
    if let Some(docker) = docker_instance(chosen.iter().copied()) {
        bail!("module {key:?}: {}", docker_reason(docker));
    }
    let native = Native::new(&lm, resolved, workspace, host)?;
    if let Some(step) = native.build_step()? {
        plan.builds.push(step);
    }
    if chosen.is_empty() {
        plan.panes.push(native.pane(None, args)?);
    } else {
        for inst in chosen {
            plan.panes.push(native.pane(Some(inst), args)?);
        }
    }
    if id.is_none() {
        plan.prune.push(lm.dev.module.clone());
    }
    Ok(plan)
}

/// Plan `sb stop <module> [args...]`.
///
/// Native: close every `<module>` / `<module>-*` window, then run
/// `scripts.stop` (with `args` available as `{args}`) if the manifest
/// declares one. Legacy with a `stopscript.bash`: run it in the module's
/// window with `args`, as before 0.6. Legacy without one: close the
/// windows.
pub fn plan_stop(
    home: &SbHome,
    workspace: &str,
    module: &str,
    args: &[String],
    host: &Host,
) -> Result<StopPlan> {
    if module.contains(':') {
        bail!(
            "`sb stop` takes a module name and closes every instance of it; \
             drop the :<id> from {module:?}"
        );
    }
    let flow = load_flow(home, workspace)?;
    let entry = registered(&flow, workspace, module)?;
    let lm = load_module(module, &entry.path)?;
    let mut plan = StopPlan {
        session: session_name(workspace),
        module: lm.dev.module.clone(),
        legacy: None,
        stop: None,
        deprecations: lm.deprecations.clone(),
    };
    let has_stopscript = lm.root.join(STOPSCRIPT).is_file();
    if lm.mode == Mode::Legacy {
        if has_stopscript {
            plan.deprecations.push(Deprecation {
                item: STOPSCRIPT.to_string(),
                hint: format!(
                    "declare scripts.stop in {MODULE_FILE}, or nothing (sb closes the \
                     windows itself); {MIGRATING}"
                ),
            });
            plan.legacy = Some(legacy_pane(&lm, STOPSCRIPT, args));
        }
        return Ok(plan);
    }
    if has_stopscript {
        plan.deprecations.push(Deprecation {
            item: STOPSCRIPT.to_string(),
            hint: format!(
                "{module} launches natively, so its {STOPSCRIPT} is ignored; use \
                 scripts.stop in {MODULE_FILE}, or nothing ({MIGRATING})"
            ),
        });
    }
    match lm.dev.resolve(host) {
        Ok(resolved) => {
            plan.stop = Native::new(&lm, resolved, workspace, host)?.stop_step(args)?;
        }
        // Nothing runs here on this host, but its windows still close.
        Err(ManifestError::Unsupported(..) | ManifestError::NoStart(_)) => {}
        Err(e) => return Err(resolve_error(&lm, e)),
    }
    Ok(plan)
}

fn load_flow(home: &SbHome, workspace: &str) -> Result<WorkspaceFlow> {
    read_flow(home, workspace)
        .with_context(|| format!("reading the installed flow of workspace {workspace:?}"))
}

fn registered<'f>(
    flow: &'f WorkspaceFlow,
    workspace: &str,
    key: &str,
) -> Result<&'f sb_core::ModuleRef> {
    flow.modules.get(key).ok_or_else(|| {
        anyhow!(
            "module {key:?} not registered in workspace {workspace:?}; \
             run `sb init` from its root first"
        )
    })
}

/// `<module>` or `<module>:<id>`.
fn parse_target(target: &str) -> Result<(&str, Option<&str>)> {
    match target.split_once(':') {
        Some((module, id)) if !module.is_empty() && !id.is_empty() => Ok((module, Some(id))),
        Some(_) => bail!("bad target {target:?}: expected <module> or <module>:<id>"),
        None => Ok((target, None)),
    }
}

/// How a module launches; see the crate docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Native,
    Legacy,
}

/// A module manifest read from disk, with its launch mode decided.
struct LoadedModule {
    /// Key in the flow's `modules:` map.
    key: String,
    manifest: PathBuf,
    dev: ModuleDevConfig,
    /// Absolute module root.
    root: PathBuf,
    mode: Mode,
    /// Legacy manifest name, legacy or ignored `runscript.bash`.
    deprecations: Vec<Deprecation>,
}

fn load_module(key: &str, entry: &Path) -> Result<LoadedModule> {
    let manifest = locate_manifest(key, entry)?;
    let body = fs::read_to_string(&manifest)
        .with_context(|| format!("reading {}", manifest.display()))?;
    let dev: ModuleDevConfig = serde_yaml::from_str(&body)
        .with_context(|| format!("parsing {}", manifest.display()))?;
    dev.platforms
        .validate()
        .with_context(|| format!("validating {}", manifest.display()))?;
    let root = plain_path(absolute(&dev.module_root(&manifest)));

    let mut deprecations = Vec::new();
    if is_legacy_module_file(&manifest) {
        deprecations.push(Deprecation {
            item: LEGACY_MODULE_FILE.to_string(),
            hint: format!("rename it to {MODULE_FILE}; {MIGRATING}"),
        });
    }
    let declares_scripts = !dev.scripts.is_empty()
        || dev.platforms.0.values().any(|o| !o.scripts.is_empty());
    let has_runscript = root.join(RUNSCRIPT).is_file();
    let mode = if declares_scripts || !has_runscript {
        Mode::Native
    } else {
        Mode::Legacy
    };
    if has_runscript {
        let hint = if mode == Mode::Legacy {
            format!("declare scripts: in {MODULE_FILE}; {MIGRATING}")
        } else {
            format!(
                "{key} declares scripts:, so its {RUNSCRIPT} is ignored; delete it ({MIGRATING})"
            )
        };
        deprecations.push(Deprecation {
            item: RUNSCRIPT.to_string(),
            hint,
        });
    }
    Ok(LoadedModule {
        key: key.to_string(),
        manifest,
        dev,
        root,
        mode,
        deprecations,
    })
}

/// The manifest a flow entry points at. Installed flows record the file
/// itself; a directory is probed for `sb.module.yaml`, then `sb.dev.yml`.
fn locate_manifest(key: &str, entry: &Path) -> Result<PathBuf> {
    if entry.is_dir() {
        return find_module_file(entry).ok_or_else(|| {
            anyhow!(
                "module {key:?}: no module manifest ({MODULE_FILE}) in {}",
                entry.display()
            )
        });
    }
    if entry.is_file() {
        return Ok(entry.to_path_buf());
    }
    // The recorded file is gone, but the module may only have been migrated
    // (`sb.dev.yml` renamed to `sb.module.yaml`) without a re-install.
    if let Some(found) = entry.parent().and_then(find_module_file) {
        return Ok(found);
    }
    bail!(
        "module {key:?}: module manifest not found at {} (registered in the workspace \
         flow, but the file is missing; did the module directory move? run `sb install` again)",
        entry.display()
    )
}

fn legacy_pane(lm: &LoadedModule, script: &str, args: &[String]) -> Pane {
    Pane {
        module: lm.dev.module.clone(),
        window: instance_module(&lm.dev.module, None),
        instance: None,
        cwd: lm.root.clone(),
        cmd: PaneCmd::Legacy {
            script: lm.root.join(script),
            args: args.to_vec(),
        },
    }
}

fn flow_deprecations(flow: &WorkspaceFlow, out: &mut Vec<Deprecation>) {
    if flow.has_legacy_instances() {
        out.push(Deprecation {
            item: "top-level `instances:`".to_string(),
            hint: format!(
                "move each list under modules.<name>.instances in {FLOW_FILE}; {MIGRATING}"
            ),
        });
    }
}

fn push_unique(out: &mut Vec<Deprecation>, items: &[Deprecation]) {
    for item in items {
        if !out.contains(item) {
            out.push(item.clone());
        }
    }
}

fn push_build(builds: &mut Vec<BuildStep>, step: BuildStep) {
    if builds.iter().all(|b| b.module != step.module) {
        builds.push(step);
    }
}

fn host_label(host: &Host) -> String {
    format!("{}-{}", host.os, host.arch)
}

fn unsupported_reason(dev: &ModuleDevConfig, host: &Host) -> String {
    let declared: Vec<&str> = dev.platforms.0.keys().map(String::as_str).collect();
    format!(
        "not supported on {} (declares: {})",
        host_label(host),
        declared.join(", ")
    )
}

fn resolve_error(lm: &LoadedModule, e: ManifestError) -> anyhow::Error {
    anyhow::Error::new(e).context(format!(
        "module {:?}: resolving {}",
        lm.key,
        lm.manifest.display()
    ))
}

/// The first instance that declares `docker:`. Native launch does not run
/// containers yet, and running the host binary in their place would be
/// wrong, so such a module is skipped (`sb up`) or refused (`sb run`).
fn docker_instance<'f>(instances: impl IntoIterator<Item = &'f ModuleInstance>) -> Option<&'f str> {
    instances
        .into_iter()
        .find(|i| i.docker.is_some())
        .map(|i| i.id.as_str())
}

fn docker_reason(id: &str) -> String {
    format!(
        "instance {id} declares docker:, which native launch does not run yet \
         (keep its {RUNSCRIPT} and no scripts: until it does)"
    )
}

// ─────────────────────────────────────────────────────────────────────
// Native assembly: placeholders, env, argv
// ─────────────────────────────────────────────────────────────────────

/// One native module resolved for this host, with the per-module values
/// every placeholder expansion shares.
struct Native<'m> {
    lm: &'m LoadedModule,
    resolved: Resolved,
    ws: &'m str,
    host: &'m Host,
    /// `{root}`: absolute module root, forward slashes.
    root: String,
    /// `{python}`: the module's venv interpreter, else the host default.
    python: String,
    /// `{bin}`: absolute path of the built binary, or empty.
    bin: String,
}

impl<'m> Native<'m> {
    fn new(lm: &'m LoadedModule, resolved: Resolved, ws: &'m str, host: &'m Host) -> Result<Self> {
        let mut native = Self {
            lm,
            resolved,
            ws,
            host,
            root: slash(&lm.root),
            python: python_for(&lm.root, host),
            bin: String::new(),
        };
        // `bin` is relative to the module root and may itself use
        // `{module}` / `{exe}`. Absolute, so it runs from any directory.
        let rel = expand_str(&native.resolved.bin, &native.ctx(None, &[]), &env_var)
            .map_err(|e| native.expand_error(e, "bin"))?;
        if !rel.is_empty() {
            native.bin = slash(&lm.root.join(rel));
        }
        Ok(native)
    }

    fn ctx<'c>(&'c self, id: Option<&'c str>, args: &'c [String]) -> ExpandCtx<'c> {
        ExpandCtx {
            module: &self.lm.dev.module,
            root: &self.root,
            bin: &self.bin,
            exe: self.host.exe_suffix(),
            id,
            args,
            ws: self.ws,
            os: self.host.os,
            python: &self.python,
        }
    }

    fn expand_error(&self, e: ExpandError, what: &str) -> anyhow::Error {
        anyhow::Error::new(e).context(format!(
            "module {:?}: expanding {what} from {}",
            self.lm.key,
            self.lm.manifest.display()
        ))
    }

    /// The resolved `env`, expanded. `lenient` (builds and stop, which have
    /// no instance) drops a value that names `{id}` instead of failing.
    fn env(
        &self,
        id: Option<&str>,
        args: &[String],
        lenient: bool,
    ) -> Result<BTreeMap<String, String>> {
        let ctx = self.ctx(id, args);
        let mut out = BTreeMap::new();
        for (key, raw) in &self.resolved.env {
            match expand_str(raw, &ctx, &env_var) {
                Ok(value) => {
                    out.insert(key.clone(), value);
                }
                Err(ExpandError::NoInstance) if lenient => {}
                Err(e) => return Err(self.expand_error(e, &format!("env {key}"))),
            }
        }
        Ok(out)
    }

    fn insert_sb_env(&self, env: &mut BTreeMap<String, String>, id: Option<&str>) {
        env.insert(ENV_WORKSPACE.to_string(), self.ws.to_string());
        env.insert(ENV_MODULE.to_string(), self.lm.dev.module.clone());
        env.insert(ENV_MODULE_ROOT.to_string(), self.root.clone());
        if let Some(id) = id {
            env.insert(ENV_INSTANCE_ID.to_string(), id.to_string());
        }
    }

    /// Expand `raw` (build or stop commands) into a [`BuildStep`]; `None`
    /// when the slot has no commands.
    fn inline_step(&self, raw: &[Argv], args: &[String], what: &str) -> Result<Option<BuildStep>> {
        if raw.is_empty() {
            return Ok(None);
        }
        let ctx = self.ctx(None, args);
        let mut steps = Vec::with_capacity(raw.len());
        for argv in raw {
            let mut expanded =
                expand_argv(argv, &ctx, &env_var).map_err(|e| self.expand_error(e, what))?;
            if expanded.is_empty() {
                bail!("module {:?}: {what} expands to an empty command", self.lm.key);
            }
            absolutize_program(&mut expanded, &self.lm.root);
            steps.push(expanded);
        }
        let mut env = self.env(None, args, true)?;
        self.insert_sb_env(&mut env, None);
        Ok(Some(BuildStep {
            module: self.lm.dev.module.clone(),
            cwd: self.lm.root.clone(),
            steps,
            env,
        }))
    }

    fn build_step(&self) -> Result<Option<BuildStep>> {
        self.inline_step(&self.resolved.build, &[], "scripts.build")
    }

    fn stop_step(&self, args: &[String]) -> Result<Option<BuildStep>> {
        self.inline_step(&self.resolved.stop, args, "scripts.stop")
    }

    /// One window: the expanded `start` argv (instance args, then
    /// passthrough args, as `{args}`), `--id <id>` appended after expansion,
    /// and env = resolved env, then instance env, then the `SB_*` variables.
    fn pane(&self, instance: Option<&ModuleInstance>, passthrough: &[String]) -> Result<Pane> {
        let id = instance.map(|i| i.id.as_str());
        let mut inst_args = instance.map_or_else(Vec::new, |i| i.args.clone());
        inst_args.extend_from_slice(passthrough);

        let mut argv = expand_argv(&self.resolved.start, &self.ctx(id, &inst_args), &env_var)
            .map_err(|e| self.expand_error(e, "scripts.start"))?;
        if argv.is_empty() {
            bail!(
                "module {:?}: scripts.start expands to an empty command",
                self.lm.key
            );
        }
        absolutize_program(&mut argv, &self.lm.root);
        if let Some(id) = id {
            argv.push("--id".to_string());
            argv.push(id.to_string());
        }

        let mut env = self.env(id, &inst_args, false)?;
        if let Some(inst) = instance {
            env.extend(inst.env.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        self.insert_sb_env(&mut env, id);

        Ok(Pane {
            module: self.lm.dev.module.clone(),
            window: instance_module(&self.lm.dev.module, id),
            instance: id.map(str::to_string),
            cwd: self.lm.root.clone(),
            cmd: PaneCmd::Native { argv, env },
        })
    }
}

/// `${VAR}` lookups read sb's own process environment.
fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// `{python}`: `<root>/.venv/bin/python` (`.venv/Scripts/python.exe` on
/// Windows) when it exists, else `python3` (`python` on Windows).
fn python_for(root: &Path, host: &Host) -> String {
    let venv = root.join(python_venv_relpath(host));
    if venv.is_file() {
        slash(&venv)
    } else {
        python_fallback(host).to_string()
    }
}

/// A relative program path with a separator (`./run.sh`, `bin/app`) is
/// made absolute against the module root, so it means the same thing to
/// tmux, to an inline build, and to Windows process creation (which would
/// otherwise resolve it against sb's own working directory).
fn absolutize_program(argv: &mut [String], root: &Path) {
    let Some(program) = argv.first_mut() else {
        return;
    };
    let has_separator = program.contains('/') || (cfg!(windows) && program.contains('\\'));
    if has_separator && Path::new(program.as_str()).is_relative() {
        *program = slash(&root.join(program.as_str()));
    }
}

fn absolute(p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
    }
}

/// Drop the `\\?\` prefix `canonicalize` puts on Windows drive paths; tmux,
/// psmux and most programs do not expect it. UNC paths are left alone.
fn plain_path(p: PathBuf) -> PathBuf {
    let stripped = p
        .to_str()
        .and_then(|s| s.strip_prefix(r"\\?\"))
        .filter(|rest| rest.as_bytes().get(1) == Some(&b':'))
        .map(PathBuf::from);
    stripped.unwrap_or(p)
}

/// `p` as a string with forward slashes on Windows; unchanged elsewhere
/// (a backslash is an ordinary file name character on Unix).
fn slash(p: &Path) -> String {
    let s = p.to_string_lossy();
    if cfg!(windows) {
        s.replace('\\', "/")
    } else {
        s.into_owned()
    }
}

// ─────────────────────────────────────────────────────────────────────
// Inline commands: builds, scripts.stop, `sb run --fg`
// ─────────────────────────────────────────────────────────────────────

/// Run every [`BuildStep`] of the plan inline (inherited stdio), in order,
/// stopping at the first failure. Prints `building <module>: <argv>` to
/// stdout before each command. Never touches tmux.
pub fn run_builds(plan: &LaunchPlan) -> Result<()> {
    for step in &plan.builds {
        for argv in &step.steps {
            println!("building {}: {}", step.module, argv.join(" "));
            run_inline(step, argv).with_context(|| format!("building {}", step.module))?;
        }
    }
    Ok(())
}

fn run_inline(step: &BuildStep, argv: &[String]) -> Result<()> {
    let (program, rest) = argv
        .split_first()
        .ok_or_else(|| anyhow!("empty command"))?;
    let status = Command::new(program)
        .args(rest)
        .current_dir(&step.cwd)
        .envs(&step.env)
        .status()
        .with_context(|| format!("running `{}` in {}", argv.join(" "), step.cwd.display()))?;
    if !status.success() {
        bail!("`{}` failed ({status})", argv.join(" "));
    }
    Ok(())
}

/// `sb run --fg`: build, then run the plan's single native pane in this
/// terminal with no tmux.
///
/// Unix: this process is replaced by the module (exec), so on success the
/// function does not return. Windows: the module is spawned and waited for,
/// and its exit code is returned for sb to exit with.
pub fn run_foreground(plan: &LaunchPlan) -> Result<i32> {
    let pane = match plan.panes.as_slice() {
        [pane] => pane,
        [first, ..] => bail!(
            "module {:?} has {} instances and `sb run --fg` runs one; pick it with {}:<id>",
            first.module,
            plan.panes.len(),
            first.module
        ),
        [] => bail!("nothing to run: the plan has no panes"),
    };
    let PaneCmd::Native { argv, env } = &pane.cmd else {
        bail!(
            "`sb run --fg` needs a native module, but {:?} still launches through \
             {RUNSCRIPT}; declare scripts: in its {MODULE_FILE}",
            pane.module
        );
    };
    run_builds(plan)?;
    let (program, rest) = argv
        .split_first()
        .ok_or_else(|| anyhow!("module {:?}: empty start command", pane.module))?;
    let mut cmd = Command::new(program);
    cmd.args(rest).current_dir(&pane.cwd).envs(env);
    exec_foreground(&mut cmd)
        .with_context(|| format!("running `{}` in {}", argv.join(" "), pane.cwd.display()))
}

#[cfg(unix)]
fn exec_foreground(cmd: &mut Command) -> Result<i32> {
    use std::os::unix::process::CommandExt;
    // `exec` only returns when it failed.
    Err(anyhow::Error::new(cmd.exec()))
}

#[cfg(not(unix))]
fn exec_foreground(cmd: &mut Command) -> Result<i32> {
    let status = cmd.status()?;
    Ok(status.code().unwrap_or(1))
}

// ─────────────────────────────────────────────────────────────────────
// tmux resolution + read-only queries
// ─────────────────────────────────────────────────────────────────────

/// Resolve the tmux executable. Honors `sb.config.yml`'s `tmux:` field
/// when set; otherwise `which tmux`, and on Windows `which psmux` after
/// that. We do not hardcode `/usr/bin/tmux` because Windows ships psmux
/// in a different location.
pub fn resolve_tmux(cfg_tmux: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = cfg_tmux {
        if !p.exists() {
            bail!(
                "tmux path from sb.config.yml does not exist: {} \
                 (fix sb.config.yml's `tmux:` line or run `sb doctor`)",
                p.display()
            );
        }
        return Ok(p.to_path_buf());
    }
    if let Ok(p) = which::which("tmux") {
        return Ok(p);
    }
    if cfg!(windows) {
        if let Ok(p) = which::which("psmux") {
            return Ok(p);
        }
    }
    bail!(
        "tmux not found on PATH; install tmux (Linux/macOS) or psmux (Windows), \
         or set `tmux:` in sb.config.yml"
    )
}

/// True if a tmux session named exactly `<session>` is running.
pub fn has_session(tmux: &Path, session: &str) -> bool {
    Command::new(tmux)
        .args(["has-session", "-t"])
        .arg(format!("={session}"))
        .output()
        .is_ok_and(|o| o.status.success())
}

/// `sb up` refuses to touch a running session. Checked by [`up`], and
/// exposed so the CLI can refuse before it spends time on builds.
pub fn ensure_no_session(tmux: &Path, session: &str) -> Result<()> {
    if has_session(tmux, session) {
        bail!(
            "tmux session {session:?} already exists; run `sb down` first, or \
             `sb run <module>` to restart one module"
        );
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────
// up / run / stop / down / attach: the verbs
// ─────────────────────────────────────────────────────────────────────

/// Outcome of `sb up`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpOutcome {
    pub session: String,
    /// Window names, in launch order.
    pub windows: Vec<String>,
}

/// `sb up`: create the session and one window per pane. Run
/// [`run_builds`] first.
///
/// If the session already exists, this errors: re-launching a workspace
/// is `sb down && sb up`, explicit, no surprises. If adding a window fails,
/// the half-launched session is killed before the error is returned.
pub fn up(plan: &LaunchPlan, tmux: &Path) -> Result<UpOutcome> {
    ensure_no_session(tmux, &plan.session)?;
    let (first, rest) = plan
        .panes
        .split_first()
        .ok_or_else(|| anyhow!("nothing to launch: the plan has no panes"))?;
    new_session(tmux, &plan.session, first)?;
    for pane in rest {
        if let Err(err) = new_window(tmux, &plan.session, pane) {
            let _ = kill_session(tmux, &plan.session);
            return Err(err.context(format!(
                "closed the partly launched session {:?}",
                plan.session
            )));
        }
    }
    Ok(UpOutcome {
        session: plan.session.clone(),
        windows: plan.panes.iter().map(|p| p.window.clone()).collect(),
    })
}

/// Outcome of `sb run`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutcome {
    pub session: String,
    /// Every window the plan launched, in order.
    pub windows: Vec<String>,
    /// The subset that already existed and was restarted in place.
    pub replaced: Vec<String>,
}

/// `sb run <target> [args...]`: launch the plan's panes. Run
/// [`run_builds`] first.
///
/// Idempotent (TDD #4): the session is created on demand; a window that
/// already exists is restarted in place (`respawn-window -k`, never
/// `kill-window` + `new-window`, which could empty the session and end it);
/// the rest are created. Then, for the modules in [`LaunchPlan::prune`],
/// windows named `<module>` / `<module>-*` that the plan does not name are
/// closed (instances no longer in the flow).
pub fn run(plan: &LaunchPlan, tmux: &Path) -> Result<RunOutcome> {
    let before = if has_session(tmux, &plan.session) {
        Some(list_windows(tmux, &plan.session)?)
    } else {
        None
    };
    let mut session_up = before.is_some();
    let mut reused: Vec<&str> = Vec::new();
    let mut windows = Vec::with_capacity(plan.panes.len());
    let mut replaced = Vec::new();
    for pane in &plan.panes {
        let existing = before.as_deref().and_then(|list| {
            list.iter()
                .find(|(id, name)| *name == pane.window && !reused.contains(&id.as_str()))
        });
        if let Some((id, _)) = existing {
            respawn_window(tmux, id, pane)?;
            reused.push(id.as_str());
            replaced.push(pane.window.clone());
        } else if session_up {
            new_window(tmux, &plan.session, pane)?;
        } else {
            new_session(tmux, &plan.session, pane)?;
            session_up = true;
        }
        windows.push(pane.window.clone());
    }
    if let Some(list) = &before {
        for (id, name) in list {
            let stale = !reused.contains(&id.as_str())
                && plan.prune.iter().any(|module| owned_by(name, module));
            if stale {
                close_window(tmux, &plan.session, id)?;
            }
        }
    }
    Ok(RunOutcome {
        session: plan.session.clone(),
        windows,
        replaced,
    })
}

/// Outcome of `sb stop`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopOutcome {
    pub session: String,
    /// Native: the windows closed. Legacy: the window whose running
    /// command was killed to run the stopscript in its place, if any.
    pub killed: Vec<String>,
    /// `scripts.stop` ran inline (native only).
    pub ran_stop: bool,
    /// The legacy `stopscript.bash` path was taken.
    pub legacy: bool,
}

/// `sb stop <module> [args...]`.
///
/// Native: close every window named `<module>` or `<module>-<anything>`
/// by window id (if that empties the session, tmux ends it), then run
/// `scripts.stop` inline. Legacy: run `stopscript.bash` in the module's
/// window (respawned if it exists, created otherwise), as before 0.6.
pub fn stop(plan: &StopPlan, tmux: &Path) -> Result<StopOutcome> {
    if let Some(pane) = &plan.legacy {
        let mut killed = Vec::new();
        if has_session(tmux, &plan.session) {
            let list = list_windows(tmux, &plan.session)?;
            if let Some((id, _)) = list.iter().find(|(_, name)| *name == pane.window) {
                respawn_window(tmux, id, pane)?;
                killed.push(pane.window.clone());
            } else {
                new_window(tmux, &plan.session, pane)?;
            }
        } else {
            new_session(tmux, &plan.session, pane)?;
        }
        return Ok(StopOutcome {
            session: plan.session.clone(),
            killed,
            ran_stop: false,
            legacy: true,
        });
    }

    let mut killed = Vec::new();
    if has_session(tmux, &plan.session) {
        for (id, name) in list_windows(tmux, &plan.session)? {
            if owned_by(&name, &plan.module) {
                close_window(tmux, &plan.session, &id)?;
                killed.push(name);
            }
        }
    }
    let mut ran_stop = false;
    if let Some(step) = &plan.stop {
        for argv in &step.steps {
            run_inline(step, argv).with_context(|| {
                format!(
                    "scripts.stop for {} (after closing {} window(s))",
                    plan.module,
                    killed.len()
                )
            })?;
        }
        ran_stop = true;
    }
    Ok(StopOutcome {
        session: plan.session.clone(),
        killed,
        ran_stop,
        legacy: false,
    })
}

/// `sb down`: `tmux kill-session -t =sb-<workspace>`.
///
/// Returns `cleared = false` if no session was running (so the CLI can
/// print a friendly "nothing to tear down" message rather than erroring).
pub fn down(workspace: &str, tmux: &Path) -> Result<DownOutcome> {
    let session = session_name(workspace);
    if !has_session(tmux, &session) {
        return Ok(DownOutcome {
            session,
            cleared: false,
        });
    }
    kill_session(tmux, &session)?;
    Ok(DownOutcome {
        session,
        cleared: true,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownOutcome {
    pub session: String,
    /// True if a session was actually killed; false if none existed.
    pub cleared: bool,
}

/// `sb attach`: best-effort info about the running session.
///
/// In an interactive TTY the CLI runs `tmux attach -t =<session>`; in
/// non-interactive mode (CI, scripts) it just reports the session name
/// so callers can scrape stdout.
pub fn attach_info(workspace: &str, tmux: &Path) -> Result<AttachInfo> {
    let session = session_name(workspace);
    if !has_session(tmux, &session) {
        bail!("no tmux session {session:?}; run `sb up` first");
    }
    Ok(AttachInfo { session })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachInfo {
    pub session: String,
}

// ─────────────────────────────────────────────────────────────────────
// Internals: tmux command sequences
// ─────────────────────────────────────────────────────────────────────
//
// Verified against `man tmux` for tmux 3.4: new-session, new-window and
// respawn-window take `-c start-directory`, repeatable `-e VARIABLE=value`,
// and a shell-command given as two or more arguments is executed directly
// (no `sh -c`); exactly one argument is run by the shell. `-c` and `-n` are
// format-expanded (`#` is special). An argument ending in `;` ends a
// command, so a literal trailing `;` is sent as `\;`. psmux (Windows) is
// not verified against any of this.

/// One tmux client invocation; `args` may hold a `;`-separated sequence,
/// which the server runs in one go.
fn tmux_exec(tmux: &Path, args: &[OsString], what: &str) -> Result<String> {
    let out = Command::new(tmux)
        .args(args)
        .output()
        .with_context(|| format!("spawning {} ({what})", tmux.display()))?;
    if !out.status.success() {
        bail!(
            "tmux failed ({what}): {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `-c <cwd> [-e KEY=VALUE]... <command>`: the tail new-session,
/// new-window and respawn-window share.
fn push_spawn_args(args: &mut Vec<OsString>, pane: &Pane) {
    args.push("-c".into());
    args.push(tmux_cwd(&pane.cwd));
    match &pane.cmd {
        PaneCmd::Native { argv, env } => {
            for (key, value) in env {
                args.push("-e".into());
                args.push(tmux_escape(&format!("{key}={value}")).into());
            }
            if let [only] = argv.as_slice() {
                // tmux runs a single trailing argument through the shell.
                args.push(tmux_escape(&shell_quote(only)).into());
            } else {
                args.extend(argv.iter().map(|a| OsString::from(tmux_escape(a))));
            }
        }
        PaneCmd::Legacy { .. } => args.push(tmux_escape(&pane.shell_command()).into()),
    }
}

/// `; set-option -w -t <target> remain-on-exit on`, so a native process
/// that exits leaves its window, and its output, readable. Chained in the
/// same invocation as the spawn, before tmux can notice the exit.
fn push_remain_on_exit(args: &mut Vec<OsString>, target: &str) {
    args.extend(
        [";", "set-option", "-w", "-t", target, "remain-on-exit", "on"].map(OsString::from),
    );
}

/// `tmux new-session -d -s <session> -n <window> ...`. `-d` keeps the
/// session detached so `sb up` does not block.
fn new_session(tmux: &Path, session: &str, pane: &Pane) -> Result<()> {
    let mut args: Vec<OsString> = vec![
        "new-session".into(),
        "-d".into(),
        "-s".into(),
        session.into(),
        "-n".into(),
        (&pane.window).into(),
    ];
    push_spawn_args(&mut args, pane);
    if let PaneCmd::Native { env, .. } = &pane.cmd {
        // The session's only window is its current one.
        push_remain_on_exit(&mut args, &format!("={session}:"));
        // new-session's `-e` lands in the session environment, which every
        // later window would inherit (a singleton would see the first
        // pane's SB_INSTANCE_ID). The first process already has it.
        let target = format!("={session}");
        for key in env.keys() {
            args.extend(
                [";", "set-environment", "-t", target.as_str(), "-u", key.as_str()]
                    .map(OsString::from),
            );
        }
    }
    tmux_exec(tmux, &args, "new-session").with_context(|| {
        format!("creating tmux session {session:?} with window {:?}", pane.window)
    })?;
    Ok(())
}

/// `tmux new-window -t =<session>: -n <window> ...`: one window per pane,
/// so each output is independently scrollable. Without `-d` the new window
/// becomes the session's current one, which the chained `set-option`
/// targets.
fn new_window(tmux: &Path, session: &str, pane: &Pane) -> Result<()> {
    let target = format!("={session}:");
    let mut args: Vec<OsString> = vec![
        "new-window".into(),
        "-t".into(),
        (&target).into(),
        "-n".into(),
        (&pane.window).into(),
    ];
    push_spawn_args(&mut args, pane);
    if pane.is_native() {
        push_remain_on_exit(&mut args, &target);
    }
    tmux_exec(tmux, &args, "new-window")
        .with_context(|| format!("adding window {:?} to tmux session {session:?}", pane.window))?;
    Ok(())
}

/// `tmux respawn-window -k -t @<id> ...`: kill whatever runs in the window
/// and start the pane's command in its place.
fn respawn_window(tmux: &Path, window_id: &str, pane: &Pane) -> Result<()> {
    let mut args: Vec<OsString> = Vec::new();
    if pane.is_native() {
        args.extend(
            ["set-option", "-w", "-t", window_id, "remain-on-exit", "on", ";"].map(OsString::from),
        );
    }
    args.extend(["respawn-window", "-k", "-t", window_id].map(OsString::from));
    push_spawn_args(&mut args, pane);
    tmux_exec(tmux, &args, "respawn-window")
        .with_context(|| format!("restarting window {:?} ({window_id})", pane.window))?;
    Ok(())
}

/// `(window_id, window_name)` for every window of `session`.
fn list_windows(tmux: &Path, session: &str) -> Result<Vec<(String, String)>> {
    let target = format!("={session}");
    let out = tmux_exec(
        tmux,
        &["list-windows", "-t", target.as_str(), "-F", "#{window_id}\t#{window_name}"]
            .map(OsString::from),
        "list-windows",
    )?;
    Ok(out
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(id, name)| (id.to_string(), name.to_string()))
        .collect())
}

/// `tmux kill-window -t @<id>`. A window that is already gone counts as
/// closed.
fn close_window(tmux: &Path, session: &str, window_id: &str) -> Result<()> {
    match tmux_exec(tmux, &["kill-window", "-t", window_id].map(OsString::from), "kill-window") {
        Ok(_) => Ok(()),
        Err(err) => {
            let alive = has_session(tmux, session)
                && list_windows(tmux, session)
                    .is_ok_and(|list| list.iter().any(|(id, _)| id == window_id));
            if alive {
                Err(err.context(format!("closing window {window_id}")))
            } else {
                Ok(())
            }
        }
    }
}

fn kill_session(tmux: &Path, session: &str) -> Result<()> {
    let target = format!("={session}");
    tmux_exec(
        tmux,
        &["kill-session", "-t", target.as_str()].map(OsString::from),
        "kill-session",
    )?;
    Ok(())
}

/// Window `name` belongs to `module`: `<module>` or `<module>-<id>`.
/// Module names are identifiers (no `-`), so the prefix is unambiguous.
fn owned_by(name: &str, module: &str) -> bool {
    name.strip_prefix(module)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('-'))
}

/// `-c` is format-expanded by tmux, so a literal `#` is written `##`.
fn tmux_cwd(cwd: &Path) -> OsString {
    match cwd.to_str() {
        Some(s) => tmux_escape(&s.replace('#', "##")).into(),
        None => cwd.as_os_str().to_owned(),
    }
}

/// tmux reads an argument ending in `;` as a command separator; `\;` at
/// the end keeps the `;` literal.
fn tmux_escape(arg: &str) -> String {
    match arg.strip_suffix(';') {
        Some(head) => format!("{head}\\;"),
        None => arg.to_string(),
    }
}

// ─────────────────────────────────────────────────────────────────────
// shell quoting: single source of truth, used by Pane::shell_command.
// ─────────────────────────────────────────────────────────────────────

/// Single-quote `s` for safe inclusion in a POSIX shell command: no
/// shell-special chars survive (including `$`, backticks, spaces).
/// Internal single quotes are encoded as `'\''`.
fn shell_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

// ─────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    /// `<root>/<name>/sb.module.yaml` with no `scripts:`, plus a
    /// `runscript.bash` when asked (which makes the module legacy).
    fn write_module(root: &Path, name: &str, with_runscript: bool) -> PathBuf {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        let manifest = dir.join(MODULE_FILE);
        fs::write(&manifest, format!("module: {name}\nlanguage: rust\n")).unwrap();
        if with_runscript {
            fs::write(dir.join(RUNSCRIPT), "#!/usr/bin/env bash\nexit 0\n").unwrap();
        }
        manifest
    }

    fn sandbox_home() -> (TempDir, SbHome) {
        let td = TempDir::new().unwrap();
        let home = SbHome::at(td.path().to_path_buf());
        fs::create_dir_all(home.workspaces_dir()).unwrap();
        (td, home)
    }

    /// A path as a YAML single-quoted scalar (no escapes inside).
    fn yaml_path(p: &Path) -> String {
        format!("'{}'", p.display().to_string().replace('\'', "''"))
    }

    #[test]
    fn pane_shell_command_quotes_safely() {
        let p = Pane {
            module: "m".into(),
            window: "m".into(),
            instance: None,
            cwd: PathBuf::from("/tmp/with space"),
            cmd: PaneCmd::Legacy {
                script: PathBuf::from("/tmp/with space/runscript.bash"),
                args: vec![],
            },
        };
        let cmd = p.shell_command();
        // Each quoted segment survives the embedded space.
        assert!(cmd.contains("'/tmp/with space'"));
        assert!(cmd.contains("'/tmp/with space/runscript.bash'"));
        assert!(cmd.starts_with("cd '"));
    }

    #[test]
    fn up_false_is_skipped_by_plan_workspace_but_not_plan_run() {
        let (_td, home) = sandbox_home();
        let ws = "demo";
        let ws_dir = home.workspace_dir(ws);
        fs::create_dir_all(&ws_dir).unwrap();
        let projects = home.root().join("projects");
        fs::create_dir_all(&projects).unwrap();
        let dev_a = write_module(&projects, "alpha", true);
        let dev_b = write_module(&projects, "beta", true);
        let flow = format!(
            "modules:\n  alpha: {}\n  beta:\n    path: {}\n    up: false\n",
            yaml_path(&dev_a),
            yaml_path(&dev_b)
        );
        fs::write(ws_dir.join(FLOW_FILE), flow).unwrap();
        let host = Host::current();

        let plan = plan_workspace(&home, ws, &host).unwrap();
        let names: Vec<&str> = plan.panes.iter().map(|p| p.window.as_str()).collect();
        assert_eq!(names, ["alpha"]);
        assert!(plan.builds.is_empty(), "legacy modules build nothing");

        let plan = plan_run(&home, ws, "beta", &[], &host).unwrap();
        let pane = &plan.panes[0];
        assert_eq!(pane.module, "beta");
        assert_eq!(pane.cwd, projects.join("beta"));
        assert!(matches!(pane.cmd, PaneCmd::Legacy { .. }));
    }
}
