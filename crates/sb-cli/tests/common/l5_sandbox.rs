//! L5 test sandbox: a workspace plus N modules.
//!
//! Builds on [`WsSandbox`] (L2 primitives). Two ways to add a module:
//!
//! - [`L5Sandbox::adopt`]: an empty Rust project root, `sb init --rust`.
//!   The module is registered and installed, but its root holds no
//!   `Cargo.toml`, so it suits tests that read or rewrite its manifest
//!   (gopro), not tests that launch it.
//! - [`L5Sandbox::native`]: a root holding only a caller-written
//!   `sb.module.yaml`. Register it with [`L5Sandbox::write_installed_flow`],
//!   which is also how a test lists instances. Launch tests use this.
//!
//! Tmux is required for any test that actually launches a session.
//! Use [`tmux_launch_supported`] to skip in environments without it (CI
//! sandboxes, headless boxes without a tmux package, Windows).

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

use super::WsSandbox;

/// L2 workspace sandbox + a list of modules.
pub struct L5Sandbox {
    pub ws: WsSandbox,
    pub workspace: String,
    pub modules: Vec<L5Module>,
}

#[derive(Debug, Clone)]
pub struct L5Module {
    pub name: String,
    pub root: PathBuf,
}

impl L5Module {
    /// `<root>/sb.module.yaml`.
    pub fn manifest(&self) -> PathBuf {
        self.root.join("sb.module.yaml")
    }
}

impl L5Sandbox {
    /// Build a workspace named `demo` with no modules. Tests then call
    /// [`Self::adopt`] or [`Self::native`] for each module they need.
    pub fn empty() -> Self {
        Self::with_workspace("demo")
    }

    /// Like [`Self::empty`] but lets the caller pick the workspace name.
    /// Tests that touch a real tmux session **and** run in parallel
    /// within the same test binary should use unique workspace names:
    /// the tmux session is named `sb-<workspace>` and is global to the
    /// dev box's tmux server, so two tests with the same workspace name
    /// will collide on `tmux new-session`.
    pub fn with_workspace(name: &str) -> Self {
        let ws = WsSandbox::new();
        let workspace = name.to_string();
        let out = ws
            .cmd()
            .args(["ws", "create", &workspace])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "ws create {workspace} failed: {out:?}"
        );
        let out = ws.cmd().args(["ws", "set", &workspace]).output().unwrap();
        assert!(out.status.success(), "ws set {workspace} failed: {out:?}");
        Self {
            ws,
            workspace,
            modules: Vec::new(),
        }
    }

    /// Create an empty Rust project root `<tempdir>/<name>/` and `sb init`
    /// it. Launching it would run the rust default build (`cargo build
    /// --release`) on an empty root, so launch tests use [`Self::native`].
    pub fn adopt(&mut self, name: &str) -> L5Module {
        let root = self.ws.tempdir().join(name);
        std::fs::create_dir_all(&root).unwrap();
        let out = self
            .ws
            .cmd()
            .args(["init", "--rust"])
            .arg(&root)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "sb init --rust {name} failed: {out:?}"
        );
        let m = L5Module {
            name: name.to_string(),
            root,
        };
        self.modules.push(m.clone());
        m
    }

    /// Create `<tempdir>/<name>/` holding only `sb.module.yaml` with
    /// `manifest` as its body. No `sb init`: it is not registered
    /// anywhere; list it in [`Self::write_installed_flow`].
    pub fn native(&mut self, name: &str, manifest: &str) -> L5Module {
        let root = self.ws.tempdir().join(name);
        std::fs::create_dir_all(&root).unwrap();
        let m = L5Module {
            name: name.to_string(),
            root,
        };
        std::fs::write(m.manifest(), manifest).unwrap();
        self.modules.push(m.clone());
        m
    }

    /// The active workspace's installed flow, `sb.flow.yaml`.
    pub fn installed_flow_path(&self) -> PathBuf {
        self.sb_home()
            .join("workspaces")
            .join(&self.workspace)
            .join("sb.flow.yaml")
    }

    /// Replace the active workspace's installed flow with `body`, the way
    /// a test registers native modules and their instances.
    pub fn write_installed_flow(&self, body: &str) {
        std::fs::write(self.installed_flow_path(), body).unwrap();
    }

    pub fn cmd(&self) -> Command {
        self.ws.cmd()
    }

    pub fn sb_home(&self) -> &Path {
        self.ws.sb_home()
    }

    pub fn tempdir(&self) -> &Path {
        self.ws.tempdir()
    }

    /// Tear down the workspace's tmux session if any tests left it running.
    /// Idempotent: safe to call multiple times.
    pub fn tmux_down(&self) {
        if let Some(tmux) = which_tmux() {
            let _ = Command::new(&tmux)
                .args(["kill-session", "-t"])
                .arg(format!("=sb-{}", self.workspace))
                .output();
        }
    }
}

impl Drop for L5Sandbox {
    fn drop(&mut self) {
        // Safety net: kill any orphan tmux session from a failed test.
        self.tmux_down();
    }
}

/// `path` as a YAML single-quoted scalar (no escape sequences inside),
/// for a module entry's `path:` in [`L5Sandbox::write_installed_flow`].
pub fn yaml_path(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "''"))
}

/// Poll `path` until it holds something or `max_ms` passes. tmux starts
/// panes asynchronously, so a sentinel written by a pane must be awaited.
pub fn wait_for_file(path: &Path, max_ms: u64) -> Option<String> {
    let step_ms = 50u64;
    let mut waited = 0u64;
    while waited < max_ms {
        if let Ok(body) = std::fs::read_to_string(path) {
            if !body.is_empty() {
                return Some(body);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(step_ms));
        waited += step_ms;
    }
    None
}

/// Is tmux on PATH? Tests that touch a real tmux session call this
/// at top and `return;` if false, mirroring the L4 "live transport"
/// guard pattern.
pub fn tmux_available() -> bool {
    which_tmux().is_some()
}

/// True if the multiplexer can run the panes these tests launch.
///
/// The fixtures start `sleep` or `sh -c ...` directly. On Windows the
/// `tmux` on PATH is psmux, whose pane semantics (direct exec, `-c`, `-e`,
/// exact targets) are not verified yet, so these tests skip there; see
/// installguide_windows.md §9.5.
///
/// Tests that only build a plan or assert preconditions can keep using
/// [`tmux_available`]; tests that expect a pane to run something must use
/// this.
pub fn tmux_launch_supported() -> bool {
    tmux_available() && !cfg!(windows)
}

fn which_tmux() -> Option<PathBuf> {
    which::which("tmux").ok()
}

/// `tmux has-session -t =<session>`: true if exactly that session exists.
pub fn has_session(session: &str) -> bool {
    let Some(tmux) = which_tmux() else {
        return false;
    };
    Command::new(&tmux)
        .args(["has-session", "-t"])
        .arg(format!("={session}"))
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// `tmux list-windows -t =<session> -F #{window_name}`: window names.
pub fn list_windows(session: &str) -> Vec<String> {
    let Some(tmux) = which_tmux() else {
        return Vec::new();
    };
    let Ok(out) = Command::new(&tmux)
        .args(["list-windows", "-t"])
        .arg(format!("={session}"))
        .arg("-F")
        .arg("#{window_name}")
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|s| s.to_string())
        .collect()
}
