//! L5 — launch modules in a tmux/psmux session.
//!
//! `sb up` reads the active workspace's `flow.yaml`, walks each module's
//! `sb.dev.yml`, confirms a `runscript.bash` exists at the module root,
//! and then creates a tmux session named `sb-<workspace>` with one pane
//! per module. Each pane just runs the module's `runscript.bash` from
//! the module root.
//!
//! Plan-then-execute: every precondition check (workspace exists,
//! runscripts present, tmux available) runs *before* tmux is touched.
//! That way a partial failure can never leave an orphan session
//! around with only some modules launched — see TDD #2 in [level5.html](../../plan/level5.html).
//!
//! Windows: `psmux` ships as the `tmux` command, so we only ever invoke
//! `tmux` by name (or the path from `sb.config.yml`). No platform-specific
//! code here.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};

use sb_core::ModuleDevConfig;
use sb_workspace::{SbHome, read_flow};

// ─────────────────────────────────────────────────────────────────────
// Session name + pane plan
// ─────────────────────────────────────────────────────────────────────

/// Tmux session name for a given workspace. Single source of truth — all
/// L5 verbs derive their target session through this function so a typo
/// in one place can't desync the others.
pub fn session_name(workspace: &str) -> String {
    format!("sb-{workspace}")
}

/// Which lifecycle script a single-module pane should run. The two
/// share a code path because `sb stop` is intentionally symmetric with
/// `sb run` — same resolution, same tmux wrap, same idempotency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Script {
    Run,
    Stop,
}

impl Script {
    /// Filename relative to the module root.
    pub fn filename(self) -> &'static str {
        match self {
            Self::Run => "runscript.bash",
            Self::Stop => "stopscript.bash",
        }
    }
}

/// One pane in the launch plan: a module, the working dir to `cd` into,
/// the script path to invoke, and any passthrough args appended to the
/// script's argv.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pane {
    pub module: String,
    pub cwd: PathBuf,
    pub script: PathBuf,
    /// Extra arguments appended verbatim to the script's argv. Each
    /// element is shell-quoted independently by [`Self::shell_command`].
    pub args: Vec<String>,
}

impl Pane {
    /// Back-compat alias for the field formerly named `runscript`.
    /// New code should prefer [`Self::script`].
    pub fn runscript(&self) -> &PathBuf {
        &self.script
    }

    /// Shell command tmux runs in the pane. Quote-safe; we `cd` first so
    /// the script's own `set -euo pipefail` + relative paths work
    /// regardless of where tmux spawned us from. Args are appended one
    /// at a time, each independently quoted, so a value like
    /// `--name "foo bar"` survives both layers of shell parsing.
    pub fn shell_command(&self) -> String {
        let mut cmd = format!(
            "cd {} && {}",
            shell_quote(&self.cwd.display().to_string()),
            shell_quote(&self.script.display().to_string()),
        );
        for arg in &self.args {
            cmd.push(' ');
            cmd.push_str(&shell_quote(arg));
        }
        cmd
    }
}

/// What `sb up` will spawn. Built by [`plan_workspace`] before any
/// tmux call so the same plan can be inspected by tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    pub workspace: String,
    pub session: String,
    pub panes: Vec<Pane>,
}

// ─────────────────────────────────────────────────────────────────────
// Plan — read flow.yaml + every module's sb.dev.yml
// ─────────────────────────────────────────────────────────────────────

/// Build a launch plan from the active workspace's `flow.yaml`. Walks
/// every module in the registry, verifies its `runscript.bash` exists.
///
/// Returns an error *before* tmux is touched if any precondition fails
/// (no active workspace, missing flow.yaml entry, missing dev.yml,
/// missing runscript) so partial state is impossible.
pub fn plan_workspace(home: &SbHome, workspace: &str) -> Result<LaunchPlan> {
    let flow = read_flow(home, workspace)
        .with_context(|| format!("reading flow.yaml for workspace {workspace:?}"))?;
    if flow.modules.is_empty() {
        bail!("workspace {workspace:?} has no modules in flow.yaml — run `sb init` to adopt one");
    }
    let mut panes = Vec::with_capacity(flow.modules.len());
    for (module, dev_yml_path) in &flow.modules {
        // `sb up` runs the runscript with no extra args (the L5 multi-
        // module workflow). Per-module passthrough args are an `sb run`
        // / `sb stop` thing only.
        let pane = plan_module(module, dev_yml_path, Script::Run, &[])?;
        panes.push(pane);
    }
    Ok(LaunchPlan {
        workspace: workspace.to_string(),
        session: session_name(workspace),
        panes,
    })
}

/// Like [`plan_workspace`] but for a single module — used by `sb run <module>`
/// and `sb stop <module>`. The `script` arg picks runscript.bash vs
/// stopscript.bash; `args` are appended verbatim to that script's argv.
pub fn plan_single(
    home: &SbHome,
    workspace: &str,
    module: &str,
    script: Script,
    args: &[String],
) -> Result<(LaunchPlan, Pane)> {
    let flow = read_flow(home, workspace)
        .with_context(|| format!("reading flow.yaml for workspace {workspace:?}"))?;
    let dev_yml = flow.modules.get(module).ok_or_else(|| {
        anyhow!(
            "module {module:?} not registered in workspace {workspace:?} — \
             run `sb init` from its rootpath first"
        )
    })?;
    let pane = plan_module(module, dev_yml, script, args)?;
    Ok((
        LaunchPlan {
            workspace: workspace.to_string(),
            session: session_name(workspace),
            panes: vec![pane.clone()],
        },
        pane,
    ))
}

/// Read `<dev_yml>`, validate the requested lifecycle script exists
/// alongside it, and emit a [`Pane`]. Errors include the offending
/// module name + the missing filename so the CLI can blame the right
/// pane (TDD #2 for `sb up`; analogous failure mode for `sb stop` when
/// the user never wrote a stopscript).
fn plan_module(module: &str, dev_yml_path: &Path, script: Script, args: &[String]) -> Result<Pane> {
    if !dev_yml_path.exists() {
        bail!(
            "module {module:?}: sb.dev.yml not found at {} (registered in flow.yaml \
             but the file is missing — did the module dir get moved?)",
            dev_yml_path.display()
        );
    }
    let body = fs::read_to_string(dev_yml_path)
        .with_context(|| format!("reading {}", dev_yml_path.display()))?;
    let dev: ModuleDevConfig = serde_yaml::from_str(&body)
        .with_context(|| format!("parsing {}", dev_yml_path.display()))?;
    let module_root = dev_yml_path
        .parent()
        .ok_or_else(|| anyhow!("sb.dev.yml has no parent: {}", dev_yml_path.display()))?
        .to_path_buf();
    let script_path = module_root.join(script.filename());
    if !script_path.exists() {
        let hint = match script {
            // `sb init` ships a runscript stub, so missing-runscript is a
            // user-deleted-the-file situation — point them back at init.
            Script::Run => format!(
                "missing runscript at {} — `sb init` writes a stub; \
                 fill it in (or re-run `sb init --force`) before `sb up`",
                script_path.display()
            ),
            // No stub for stopscripts today; tell the user to author one.
            Script::Stop => format!(
                "missing stopscript at {} — `sb stop` requires a \
                 stopscript.bash next to the module's sb.dev.yml \
                 (mirror your runscript's launch with a teardown)",
                script_path.display()
            ),
        };
        bail!("module {module:?}: {hint}");
    }
    Ok(Pane {
        module: dev.module,
        cwd: module_root,
        script: script_path,
        args: args.to_vec(),
    })
}

// ─────────────────────────────────────────────────────────────────────
// tmux invocation
// ─────────────────────────────────────────────────────────────────────

/// Resolve the tmux executable. Honors `sb.config.yml`'s `tmux:` field
/// when set; otherwise falls back to `which tmux` on PATH. We don't
/// hardcode `/usr/bin/tmux` because Windows ships psmux as the `tmux`
/// command in a different location.
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
    which::which("tmux").map_err(|_| {
        anyhow!(
            "tmux not found on PATH — install tmux (Linux/macOS) or psmux (Windows), \
             or set `tmux:` in sb.config.yml"
        )
    })
}

/// True if a tmux session named `<session>` is currently running.
pub fn has_session(tmux: &Path, session: &str) -> bool {
    Command::new(tmux)
        .args(["has-session", "-t"])
        .arg(session)
        .output()
        .is_ok_and(|o| o.status.success())
}

// ─────────────────────────────────────────────────────────────────────
// up / run / down — the verbs
// ─────────────────────────────────────────────────────────────────────

/// Outcome of `sb up`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpOutcome {
    pub session: String,
    pub modules: Vec<String>,
    /// True if the session already existed and we attached panes to it
    /// instead of recreating from scratch.
    pub reused_session: bool,
}

/// `sb up` — execute a [`LaunchPlan`].
///
/// If the session already exists, this errors. Re-launching a workspace
/// is "sb down && sb up" — explicit, no surprises.
pub fn up(plan: &LaunchPlan, tmux: &Path) -> Result<UpOutcome> {
    if has_session(tmux, &plan.session) {
        bail!(
            "tmux session {:?} already exists — `sb down` first, or use `sb run <module>` to restart a single pane",
            plan.session
        );
    }
    let mut iter = plan.panes.iter();
    let first = iter.next().ok_or_else(|| anyhow!("empty plan"))?;
    new_session(tmux, &plan.session, first)
        .with_context(|| format!("creating tmux session {:?}", plan.session))?;
    for pane in iter {
        new_window(tmux, &plan.session, pane)
            .with_context(|| format!("adding window for {:?}", pane.module))?;
    }
    Ok(UpOutcome {
        session: plan.session.clone(),
        modules: plan.panes.iter().map(|p| p.module.clone()).collect(),
        reused_session: false,
    })
}

/// `sb run <module> [args...]` and `sb stop <module> [args...]` —
/// launch a single module's pane around the requested script.
///
/// The two verbs share this function on purpose: stop is the symmetric
/// counterpart of run (same module resolution, same tmux session, same
/// idempotency). The caller picks runscript vs stopscript via
/// [`Script`] when building the pane through [`plan_single`].
///
/// Idempotent (TDD #4): if a window already exists for `<module>` in
/// `sb-<workspace>`, the running command is killed and re-spawned in
/// the same window — whether that means swapping in a fresh runscript,
/// swapping in the stopscript, or restarting with new passthrough args.
/// The session is created on demand if it doesn't exist yet.
///
/// Uses `tmux respawn-window -k` for the replace case rather than
/// `kill-window` + `new-window` — killing the last window in a session
/// also kills the session (and tmux server on default config), which
/// would then break the subsequent `new-window`.
pub fn run_single(plan: &LaunchPlan, pane: &Pane, tmux: &Path) -> Result<RunOutcome> {
    let mut replaced = false;
    if has_session(tmux, &plan.session) {
        if window_exists(tmux, &plan.session, &pane.module) {
            respawn_window(tmux, &plan.session, pane)?;
            replaced = true;
        } else {
            new_window(tmux, &plan.session, pane)?;
        }
    } else {
        new_session(tmux, &plan.session, pane)?;
    }
    Ok(RunOutcome {
        session: plan.session.clone(),
        module: pane.module.clone(),
        replaced,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutcome {
    pub session: String,
    pub module: String,
    /// True if an existing window for this module was killed and replaced.
    pub replaced: bool,
}

/// `sb down` — `tmux kill-session -t sb-<workspace>`.
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
    let out = Command::new(tmux)
        .args(["kill-session", "-t"])
        .arg(&session)
        .output()
        .with_context(|| format!("spawning tmux kill-session -t {session}"))?;
    if !out.status.success() {
        bail!(
            "tmux kill-session failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
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

/// `sb attach` — best-effort info about the running session.
///
/// In an interactive TTY the CLI would `exec tmux attach -t <session>`;
/// in non-interactive mode (CI, scripts) we just report the session
/// name + that it exists so callers can scrape stdout.
pub fn attach_info(workspace: &str, tmux: &Path) -> Result<AttachInfo> {
    let session = session_name(workspace);
    if !has_session(tmux, &session) {
        bail!("no tmux session {session:?} — run `sb up` first");
    }
    Ok(AttachInfo { session })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachInfo {
    pub session: String,
}

// ─────────────────────────────────────────────────────────────────────
// Internals — direct tmux subcommand wrappers
// ─────────────────────────────────────────────────────────────────────

/// `tmux new-session -d -s <session> -n <module> <cmd>`. The `-d` flag
/// keeps the session detached so `sb up` doesn't block.
fn new_session(tmux: &Path, session: &str, first: &Pane) -> Result<()> {
    let cmd = first.shell_command();
    let out = Command::new(tmux)
        .args(["new-session", "-d", "-s"])
        .arg(session)
        .arg("-n")
        .arg(&first.module)
        .arg(&cmd)
        .output()
        .with_context(|| format!("spawning tmux new-session for {session}"))?;
    if !out.status.success() {
        bail!(
            "tmux new-session failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// `tmux new-window -t <session> -n <module> <cmd>` — one window per
/// module, so each module's output is independently scrollable.
fn new_window(tmux: &Path, session: &str, pane: &Pane) -> Result<()> {
    let cmd = pane.shell_command();
    let out = Command::new(tmux)
        .args(["new-window", "-t"])
        .arg(session)
        .arg("-n")
        .arg(&pane.module)
        .arg(&cmd)
        .output()
        .with_context(|| format!("spawning tmux new-window for {}", pane.module))?;
    if !out.status.success() {
        bail!(
            "tmux new-window failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

fn window_exists(tmux: &Path, session: &str, window: &str) -> bool {
    let Ok(out) = Command::new(tmux)
        .args(["list-windows", "-t"])
        .arg(session)
        .arg("-F")
        .arg("#{window_name}")
        .output()
    else {
        return false;
    };
    if !out.status.success() {
        return false;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .any(|name| name == window)
}

/// `tmux respawn-window -k -t <session>:<window> <cmd>` — kill any
/// process running in the window and start `<cmd>` in its place. Used
/// by `run_single` to make `sb run <module>` idempotent without ever
/// emptying the session (which would tear down the server too).
fn respawn_window(tmux: &Path, session: &str, pane: &Pane) -> Result<()> {
    let target = format!("{session}:{}", pane.module);
    let cmd = pane.shell_command();
    let out = Command::new(tmux)
        .args(["respawn-window", "-k", "-t"])
        .arg(&target)
        .arg(&cmd)
        .output()
        .with_context(|| format!("spawning tmux respawn-window -t {target}"))?;
    if !out.status.success() {
        bail!(
            "tmux respawn-window failed for {target}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────
// shell quoting — single source of truth, used by Pane::shell_command.
// ─────────────────────────────────────────────────────────────────────

/// Single-quote `s` for safe inclusion in a bash command. POSIX safe:
/// no shell-special chars survive (including `$`, backticks, spaces).
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

    use sb_core::{Language, ModuleDevConfig, WorkspaceFlow};
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    fn write_module(root: &Path, name: &str, with_runscript: bool) -> PathBuf {
        write_module_full(root, name, with_runscript, false)
    }

    fn write_module_full(
        root: &Path,
        name: &str,
        with_runscript: bool,
        with_stopscript: bool,
    ) -> PathBuf {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        let dev = ModuleDevConfig {
            module: name.to_string(),
            language: Language::Rust,
            root: dir.clone(),
            io_dir: PathBuf::from("swarmbotix_io"),
            publishers: vec![],
            subscribers: vec![],
        };
        let dev_yml = dir.join("sb.dev.yml");
        fs::write(&dev_yml, serde_yaml::to_string(&dev).unwrap()).unwrap();
        if with_runscript {
            fs::write(dir.join("runscript.bash"), "#!/usr/bin/env bash\nexit 0\n").unwrap();
        }
        if with_stopscript {
            fs::write(dir.join("stopscript.bash"), "#!/usr/bin/env bash\nexit 0\n").unwrap();
        }
        dev_yml
    }

    fn sandbox_home() -> (TempDir, SbHome) {
        let td = TempDir::new().unwrap();
        let home = SbHome::at(td.path().to_path_buf());
        fs::create_dir_all(home.workspaces_dir()).unwrap();
        (td, home)
    }

    #[test]
    fn session_name_format_stable() {
        assert_eq!(session_name("demo"), "sb-demo");
        assert_eq!(session_name("my-ws"), "sb-my-ws");
    }

    #[test]
    fn pane_shell_command_quotes_safely() {
        let p = Pane {
            module: "m".into(),
            cwd: PathBuf::from("/tmp/with space"),
            script: PathBuf::from("/tmp/with space/runscript.bash"),
            args: vec![],
        };
        let cmd = p.shell_command();
        // Each quoted segment survives the embedded space.
        assert!(cmd.contains("'/tmp/with space'"));
        assert!(cmd.contains("'/tmp/with space/runscript.bash'"));
        assert!(cmd.starts_with("cd '"));
    }

    #[test]
    fn pane_shell_command_appends_passthrough_args() {
        let p = Pane {
            module: "m".into(),
            cwd: PathBuf::from("/m"),
            script: PathBuf::from("/m/runscript.bash"),
            args: vec!["--id".into(), "sb01".into(), "with space".into()],
        };
        let cmd = p.shell_command();
        assert!(cmd.starts_with("cd '/m' && '/m/runscript.bash'"));
        // Each arg is single-quoted independently; the embedded space
        // survives a second pass through the shell.
        assert!(cmd.contains(" '--id' 'sb01' 'with space'"), "got: {cmd}");
    }

    #[test]
    fn shell_quote_handles_single_quote() {
        // `it's` becomes `'it'\''s'`
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(shell_quote("safe"), "'safe'");
    }

    #[test]
    fn plan_workspace_succeeds_with_two_modules() {
        let (_td, home) = sandbox_home();
        let ws = "demo";
        fs::create_dir_all(home.workspace_dir(ws)).unwrap();

        let projects = home.root().join("projects");
        fs::create_dir_all(&projects).unwrap();
        let dev_a = write_module(&projects, "alpha", true);
        let dev_b = write_module(&projects, "beta", true);

        let mut flow = WorkspaceFlow::default();
        flow.modules.insert("alpha".into(), dev_a);
        flow.modules.insert("beta".into(), dev_b);
        let body = serde_yaml::to_string(&flow).unwrap();
        fs::write(home.flow_yaml(ws), body).unwrap();

        let plan = plan_workspace(&home, ws).unwrap();
        assert_eq!(plan.session, "sb-demo");
        assert_eq!(plan.panes.len(), 2);
        let names: Vec<&str> = plan.panes.iter().map(|p| p.module.as_str()).collect();
        // BTreeMap iteration sorts keys alphabetically.
        assert_eq!(names, ["alpha", "beta"]);
    }

    #[test]
    fn plan_workspace_blames_missing_runscript() {
        let (_td, home) = sandbox_home();
        let ws = "demo";
        fs::create_dir_all(home.workspace_dir(ws)).unwrap();
        let projects = home.root().join("projects");
        fs::create_dir_all(&projects).unwrap();
        let dev = write_module(&projects, "alpha", false); // no runscript

        let mut flow = WorkspaceFlow::default();
        flow.modules.insert("alpha".into(), dev);
        fs::write(home.flow_yaml(ws), serde_yaml::to_string(&flow).unwrap()).unwrap();

        let err = plan_workspace(&home, ws).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("alpha"), "want module name in error: {msg}");
        assert!(
            msg.contains("runscript"),
            "want 'runscript' in error: {msg}"
        );
    }

    #[test]
    fn plan_workspace_errors_on_empty_flow() {
        let (_td, home) = sandbox_home();
        let ws = "demo";
        fs::create_dir_all(home.workspace_dir(ws)).unwrap();
        fs::write(
            home.flow_yaml(ws),
            serde_yaml::to_string(&WorkspaceFlow::default()).unwrap(),
        )
        .unwrap();
        let err = plan_workspace(&home, ws).unwrap_err();
        assert!(format!("{err:#}").contains("no modules"), "got: {err:#}");
    }

    #[test]
    fn plan_single_finds_the_named_module() {
        let (_td, home) = sandbox_home();
        let ws = "demo";
        fs::create_dir_all(home.workspace_dir(ws)).unwrap();
        let projects = home.root().join("projects");
        fs::create_dir_all(&projects).unwrap();
        let dev_a = write_module(&projects, "alpha", true);
        let dev_b = write_module(&projects, "beta", true);
        let mut flow = WorkspaceFlow::default();
        flow.modules.insert("alpha".into(), dev_a);
        flow.modules.insert("beta".into(), dev_b);
        fs::write(home.flow_yaml(ws), serde_yaml::to_string(&flow).unwrap()).unwrap();

        let (plan, pane) = plan_single(&home, ws, "beta", Script::Run, &[]).unwrap();
        assert_eq!(plan.panes.len(), 1);
        assert_eq!(pane.module, "beta");
        assert!(pane.args.is_empty());
    }

    #[test]
    fn plan_single_threads_passthrough_args_into_pane() {
        let (_td, home) = sandbox_home();
        let ws = "demo";
        fs::create_dir_all(home.workspace_dir(ws)).unwrap();
        let projects = home.root().join("projects");
        fs::create_dir_all(&projects).unwrap();
        let dev_a = write_module(&projects, "alpha", true);
        let mut flow = WorkspaceFlow::default();
        flow.modules.insert("alpha".into(), dev_a);
        fs::write(home.flow_yaml(ws), serde_yaml::to_string(&flow).unwrap()).unwrap();

        let args = vec!["--id".to_string(), "sb01".to_string()];
        let (_plan, pane) = plan_single(&home, ws, "alpha", Script::Run, &args).unwrap();
        assert_eq!(pane.args, args);
        let cmd = pane.shell_command();
        assert!(cmd.ends_with(" '--id' 'sb01'"), "got: {cmd}");
    }

    #[test]
    fn plan_single_stop_finds_stopscript() {
        let (_td, home) = sandbox_home();
        let ws = "demo";
        fs::create_dir_all(home.workspace_dir(ws)).unwrap();
        let projects = home.root().join("projects");
        fs::create_dir_all(&projects).unwrap();
        let dev_a = write_module_full(&projects, "alpha", true, true);
        let mut flow = WorkspaceFlow::default();
        flow.modules.insert("alpha".into(), dev_a);
        fs::write(home.flow_yaml(ws), serde_yaml::to_string(&flow).unwrap()).unwrap();

        let (_plan, pane) = plan_single(&home, ws, "alpha", Script::Stop, &[]).unwrap();
        assert_eq!(pane.script.file_name().unwrap(), "stopscript.bash");
    }

    #[test]
    fn plan_single_stop_blames_missing_stopscript() {
        let (_td, home) = sandbox_home();
        let ws = "demo";
        fs::create_dir_all(home.workspace_dir(ws)).unwrap();
        let projects = home.root().join("projects");
        fs::create_dir_all(&projects).unwrap();
        // runscript present, but no stopscript.
        let dev_a = write_module_full(&projects, "alpha", true, false);
        let mut flow = WorkspaceFlow::default();
        flow.modules.insert("alpha".into(), dev_a);
        fs::write(home.flow_yaml(ws), serde_yaml::to_string(&flow).unwrap()).unwrap();

        let err = plan_single(&home, ws, "alpha", Script::Stop, &[]).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("alpha"), "want module name in error: {msg}");
        assert!(
            msg.contains("stopscript"),
            "want 'stopscript' in error: {msg}"
        );
    }

    #[test]
    fn plan_single_errors_when_module_missing() {
        let (_td, home) = sandbox_home();
        let ws = "demo";
        fs::create_dir_all(home.workspace_dir(ws)).unwrap();
        fs::write(
            home.flow_yaml(ws),
            serde_yaml::to_string(&WorkspaceFlow::default()).unwrap(),
        )
        .unwrap();
        let err = plan_single(&home, ws, "nope", Script::Run, &[]).unwrap_err();
        assert!(format!("{err:#}").contains("nope"), "got: {err:#}");
    }

    #[test]
    fn resolve_tmux_honors_config_path() {
        let td = TempDir::new().unwrap();
        let fake = td.path().join("tmux");
        fs::write(&fake, "#!/bin/sh\nexit 0\n").unwrap();
        let resolved = resolve_tmux(Some(&fake)).unwrap();
        assert_eq!(resolved, fake);
    }

    #[test]
    fn resolve_tmux_errors_when_configured_path_missing() {
        let bad = PathBuf::from("/definitely/not/here/tmux-binary-do-not-create");
        // The path doesn't exist on disk → should error out, not fall back to PATH.
        let err = resolve_tmux(Some(&bad)).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("tmux"), "want 'tmux' in error: {msg}");
        assert!(msg.contains("does not exist"), "got: {msg}");
    }

    #[test]
    fn plan_module_uses_dev_yml_module_name_not_flow_key() {
        // Lets us assert the dev.yml's `module:` field — not the flow.yaml key —
        // wins for the pane name. (Today they're identical, but the function
        // should still read from sb.dev.yml for consistency.)
        let (_td, home) = sandbox_home();
        let projects = home.root().join("projects");
        fs::create_dir_all(&projects).unwrap();
        let dev = write_module(&projects, "alpha", true);
        let pane = plan_module("alpha", &dev, Script::Run, &[]).unwrap();
        assert_eq!(pane.module, "alpha");
        assert_eq!(pane.script.file_name().unwrap(), "runscript.bash");
    }

    // Drop the unused BTreeMap import warning in fixtures.
    #[allow(dead_code)]
    fn _silence(_x: BTreeMap<String, PathBuf>) {}
}
