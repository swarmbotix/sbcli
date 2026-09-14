//! L2 test sandbox — driven entirely by `sb.config.yml`.
//!
//! Builds a tempdir, writes a `sb.config.yml` that inherits the real
//! dev-box config (so `protoc`, `flatc`, transport defaults stay
//! realistic) and layers `sb_home_dir: <tempdir>` on top. Spawned `sb`
//! commands resolve every workspace path into the tempdir.
//!
//! Pattern in tests:
//! ```ignore
//! let s = WsSandbox::new();
//! s.cmd().args(["ws", "create", "demo"]).output()?;
//! assert!(s.sb_home().join("workspaces").join("demo").is_dir());
//! ```

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

/// Hermetic L2 test environment.
///
/// `_tempdir` keeps the tempdir alive for the test's lifetime via Drop;
/// don't reference it directly — use [`Self::sb_home`].
pub struct WsSandbox {
    _tempdir: TempDir,
    sb_home: PathBuf,
    config_path: PathBuf,
}

impl WsSandbox {
    /// Build a fresh sandbox: tempdir + a `sb.config.yml` that points
    /// `sb_home_dir` at it. `SB_CONFIG` is set on every spawned `sb`
    /// command via [`Self::cmd`].
    pub fn new() -> Self {
        let tempdir = TempDir::new().expect("tempdir");
        let sb_home = tempdir.path().join("sb_home");
        std::fs::create_dir_all(&sb_home).expect("mkdir sb_home");

        let config_path = tempdir.path().join("sb.config.yml");
        let body = build_test_config(&sb_home);
        std::fs::write(&config_path, body).expect("write sb.config.yml");

        Self {
            _tempdir: tempdir,
            sb_home,
            config_path,
        }
    }

    /// Spawn an `sb` command with `SB_CONFIG` pointed at the test config.
    pub fn cmd(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_sb"));
        c.env("SB_CONFIG", &self.config_path);
        // Redirects the global config layer; $SB_CONFIG does not suppress it.
        c.env("SB_HOME", &self.sb_home);
        c
    }

    /// Effective `~/.swarmbotix/` root for this sandbox (the tempdir).
    pub fn sb_home(&self) -> &Path {
        &self.sb_home
    }

    /// Source vault for the sandbox's active style — the path
    /// `messages_root` + `message_style` derive to. See
    /// [`super::SANDBOX_STYLE`].
    pub fn vault_dir(&self) -> PathBuf {
        self.sb_home
            .join("messages")
            .join(super::SANDBOX_STYLE)
            .join("message_definitions")
    }

    /// Codegen output root for the sandbox's active style.
    pub fn targets_dir(&self) -> PathBuf {
        self.sb_home
            .join("messages")
            .join(super::SANDBOX_STYLE)
            .join("message_targets")
    }

    /// Tempdir that holds the test `sb.config.yml` + sb_home. Used by
    /// tests that need an isolated project root.
    pub fn tempdir(&self) -> &Path {
        self._tempdir.path()
    }

    pub fn config_path(&self) -> &Path {
        &self.config_path
    }
}

/// Inherit the real dev-box `sb.config.yml` (so realistic tool paths,
/// transport defaults, etc. carry through) and append a `sb_home_dir`
/// override pointing at the per-test tempdir.
///
/// If no dev-box config is present (fresh box / CI), fall back to a
/// minimal body — `sb_home_dir` is the only field these tests really
/// need.
fn build_test_config(sb_home: &Path) -> String {
    let real_body = dirs::home_dir()
        .map(|h| h.join(".swarmbotix").join("sb.config.yml"))
        .filter(|p| p.exists())
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default();
    super::pin_sb_home(&real_body, sb_home)
}
