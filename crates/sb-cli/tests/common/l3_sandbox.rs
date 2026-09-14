//! L3 test sandbox — `WsSandbox` plus a fully-bootstrapped module.
//!
//! Every L3 mutation needs:
//!   1. An active workspace (the L2 primitive).
//!   2. A vault populated with std/* (so `MsgType` validation succeeds).
//!   3. A module adopted via `sb init` so `sb.dev.yml` exists.
//!
//! The helper does all three and exposes the module root + a `cmd_in()`
//! that runs `sb` with cwd = module root (so bare `sb pub list` resolves
//! to the test module).

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

use super::WsSandbox;

/// L2 sandbox + a single rust module adopted into a workspace.
pub struct L3Sandbox {
    pub ws: WsSandbox,
    pub workspace: String,
    pub module: String,
    pub module_root: PathBuf,
}

impl L3Sandbox {
    /// Workspace `demo`, module `<name>` at `<tempdir>/<name>`, language Rust.
    pub fn rust(module_name: &str) -> Self {
        Self::with_language(module_name, "--rust")
    }

    pub fn python(module_name: &str) -> Self {
        Self::with_language(module_name, "--python")
    }

    pub fn cpp(module_name: &str) -> Self {
        Self::with_language(module_name, "--cpp")
    }

    pub fn flutter(module_name: &str) -> Self {
        Self::with_language(module_name, "--flutter")
    }

    pub fn unity(module_name: &str) -> Self {
        Self::with_language(module_name, "--unity")
    }

    fn with_language(module_name: &str, lang_flag: &str) -> Self {
        let ws = WsSandbox::new();
        let workspace = "demo".to_string();

        // Seed the committed iox2 fixture into the active style's targets
        // root. The C++ iceoryx2 template reads
        // `<message_targets>/iox2/std/ImageStamped/ImageStamped.h` to resolve
        // the payload's namespace; without this the test silently depends on
        // whatever the dev box happens to have compiled into its real
        // `~/.swarmbotix/`, and fails anywhere else.
        //
        // No explicit `message_targets:` pin — `WsSandbox` already pins
        // `messages_root` + `message_style`, so this derived path is exactly
        // what the CLI resolves. Seeding the derived location keeps the test
        // exercising the real style resolution instead of the override.
        copy_tree(&fixture_iox2_targets(), &ws.targets_dir());

        // L2: create + activate workspace.
        let out = ws
            .cmd()
            .args(["ws", "create", &workspace])
            .output()
            .unwrap();
        assert!(out.status.success(), "ws create demo failed: {out:?}");
        let out = ws.cmd().args(["ws", "set", &workspace]).output().unwrap();
        assert!(out.status.success(), "ws set demo failed: {out:?}");

        // Prepopulate the vault so `sb pub add` MsgType validation works
        // without depending on `--all` codegen.
        let out = ws.cmd().args(["message", "list"]).output().unwrap();
        assert!(out.status.success(), "message list failed: {out:?}");

        // Project root under the tempdir.
        let module_root = ws.tempdir().join(module_name);
        std::fs::create_dir_all(&module_root).unwrap();
        let out = ws
            .cmd()
            .args(["init", lang_flag])
            .arg(&module_root)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "sb init {lang_flag} {module_name} failed: {out:?}"
        );

        Self {
            ws,
            workspace,
            module: module_name.to_string(),
            module_root,
        }
    }

    /// Run `sb` with `cwd = module_root` (so bare `sb pub list` resolves).
    pub fn cmd_in_module(&self) -> Command {
        let mut c = self.ws.cmd();
        c.current_dir(&self.module_root);
        c
    }

    /// Run `sb` with `cwd = tempdir` (parent of module dirs) — used by
    /// the module-resolution tests.
    pub fn cmd_in_tempdir(&self) -> Command {
        let mut c = self.ws.cmd();
        c.current_dir(self.ws.tempdir());
        c
    }

    pub fn sb_home(&self) -> &Path {
        self.ws.sb_home()
    }

    pub fn dev_yml(&self) -> PathBuf {
        self.module_root.join("sb.dev.yml")
    }

    /// `<module_root>/swarmbotix_io` (default io_dir for non-Unity).
    pub fn io_dir(&self) -> PathBuf {
        self.module_root.join("swarmbotix_io")
    }
}

/// Committed iox2 target tree — `tests/fixtures/iox2_targets/`, containing
/// `iox2/std/ImageStamped/ImageStamped.h` declaring `namespace
/// swarmbotix_std`, mirroring what sb-iox2-typegen emits.
fn fixture_iox2_targets() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/iox2_targets")
}

fn copy_tree(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("mkdir target tree");
    for entry in std::fs::read_dir(src).expect("read fixture dir") {
        let entry = entry.expect("fixture dir entry");
        let to = dst.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&entry.path(), &to);
        } else {
            std::fs::copy(entry.path(), &to).expect("copy fixture file");
        }
    }
}
