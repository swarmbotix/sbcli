//! L5 test sandbox — workspace + N adopted modules with runscripts.
//!
//! Builds on [`WsSandbox`] (L2 primitives). Each module gets an empty
//! Rust project root, an `sb.dev.yml` (via `sb init --rust`), and a
//! caller-supplied runscript body (or no runscript at all, to drive
//! TDD #2 "missing runscript" cases).
//!
//! Tmux is required for any test that actually launches a session.
//! Use [`tmux_available`] to skip in environments without it (CI
//! sandboxes, headless boxes without a tmux package).

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

use super::WsSandbox;

/// L2 workspace sandbox + a list of adopted modules.
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

impl L5Sandbox {
    /// Build a workspace named `demo` with no modules. Tests then call
    /// [`Self::adopt`] for each module they need.
    pub fn empty() -> Self {
        Self::with_workspace("demo")
    }

    /// Like [`Self::empty`] but lets the caller pick the workspace name.
    /// Tests that touch a real tmux session **and** run in parallel
    /// within the same test binary should use unique workspace names —
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

    /// Create a Rust project root `<tempdir>/<name>/`, `sb init` it,
    /// then write the runscript body (or skip if `runscript = None`).
    pub fn adopt(&mut self, name: &str, runscript: Option<&str>) -> L5Module {
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
        let runscript_path = root.join("runscript.bash");
        match runscript {
            Some(body) => {
                std::fs::write(&runscript_path, body).unwrap();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mut perms = std::fs::metadata(&runscript_path).unwrap().permissions();
                    perms.set_mode(0o755);
                    std::fs::set_permissions(&runscript_path, perms).unwrap();
                }
            }
            None => {
                // `sb init` writes a runscript stub; tests that want
                // "no runscript on disk" must delete it explicitly.
                if runscript_path.exists() {
                    std::fs::remove_file(&runscript_path).unwrap();
                }
            }
        }
        let m = L5Module {
            name: name.to_string(),
            root,
        };
        self.modules.push(m.clone());
        m
    }

    /// Write an executable `stopscript.bash` into an already-adopted
    /// module's root. Mirror of [`Self::adopt`] for the runscript half;
    /// `sb stop` requires this to be present (no `sb init` stub exists).
    pub fn write_stopscript(&self, module: &str, body: &str) {
        let m = self
            .modules
            .iter()
            .find(|m| m.name == module)
            .unwrap_or_else(|| panic!("module {module:?} not adopted"));
        let path = m.root.join("stopscript.bash");
        std::fs::write(&path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
        }
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
    /// Idempotent — safe to call multiple times.
    pub fn tmux_down(&self) {
        if let Some(tmux) = which_tmux() {
            let _ = Command::new(&tmux)
                .args(["kill-session", "-t"])
                .arg(format!("sb-{}", self.workspace))
                .output();
        }
    }
}

impl Drop for L5Sandbox {
    fn drop(&mut self) {
        // Safety net — kill any orphan tmux session from a failed test.
        self.tmux_down();
    }
}

/// Is tmux on PATH? Tests that touch a real tmux session call this
/// at top and `return;` if false, mirroring the L4 "live transport"
/// guard pattern.
pub fn tmux_available() -> bool {
    which_tmux().is_some()
}

/// True if the multiplexer can actually *execute* a launch pane.
///
/// `sb up` / `sb run` hand tmux a POSIX command line — `cd '<dir>' &&
/// '<dir>/runscript.bash'` (see `sb_launch::Pane::shell_command`) — and the
/// script itself is bash (`#!/usr/bin/env bash`, `${BASH_SOURCE[0]}`,
/// `printf %q`). On Windows the `tmux` on PATH is psmux, which dispatches
/// pane commands through PowerShell, so that command line is not
/// interpretable no matter which bash is installed.
///
/// Tests that only build a plan or assert preconditions can keep using
/// [`tmux_available`]; tests that expect a pane to run something must use
/// this. Porting the launch path to Windows (a `runscript.ps1` variant, or
/// routing panes through bash explicitly) is tracked as §9.5 in
/// installguide_windows.md.
pub fn tmux_launch_supported() -> bool {
    tmux_available() && !cfg!(windows)
}

fn which_tmux() -> Option<PathBuf> {
    which::which("tmux").ok()
}

/// `tmux has-session -t <session>` — true if the session exists.
pub fn has_session(session: &str) -> bool {
    let Some(tmux) = which_tmux() else {
        return false;
    };
    Command::new(&tmux)
        .args(["has-session", "-t"])
        .arg(session)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// `tmux list-windows -t <session> -F #{window_name}` → vector of window names.
pub fn list_windows(session: &str) -> Vec<String> {
    let Some(tmux) = which_tmux() else {
        return Vec::new();
    };
    let Ok(out) = Command::new(&tmux)
        .args(["list-windows", "-t"])
        .arg(session)
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
