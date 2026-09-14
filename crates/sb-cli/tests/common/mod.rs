//! Test helpers shared across integration tests.
//!
//! Each test file in `tests/` is a separate crate, so helpers are
//! per-file dead-code from Cargo's POV — silence those warnings here.
#![allow(dead_code)]

pub mod goldens;
pub mod l2_sandbox;
pub mod l3_sandbox;
pub mod l5_sandbox;
#[allow(unused_imports)]
pub use goldens::{assert_or_bless, assert_or_bless_with};
#[allow(unused_imports)]
pub use l2_sandbox::WsSandbox;
#[allow(unused_imports)]
pub use l3_sandbox::L3Sandbox;
#[allow(unused_imports)]
pub use l5_sandbox::{L5Sandbox, has_session, list_windows, tmux_available, tmux_launch_supported};

use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

/// Path to the compiled `sb` binary, provided by Cargo at build time.
pub fn sb_bin() -> &'static str {
    env!("CARGO_BIN_EXE_sb")
}

/// Build a hermetic sandbox: a tempdir acting as `~/.swarmbotix/`, plus a
/// `$SB_CONFIG` pointing at a fresh config with this dev box's real tool
/// paths and `sb_home_dir` pinned at the tempdir.
///
/// Isolation is via **`sb_home_dir`, not `$HOME`**. `dirs::home_dir()` reads
/// `$HOME` only on Unix; on Windows it calls `SHGetKnownFolderPath(Profile)`
/// and ignores the environment entirely, so a `HOME`-based sandbox silently
/// leaks into the developer's real `C:\Users\<you>\.swarmbotix`. `sb_home_dir`
/// is honored by `sb_config::resolve_sb_home_dir` ahead of `dirs::home_dir()`
/// on every platform. `HOME` is still set as Unix belt-and-braces.
pub struct Sandbox {
    pub home: TempDir,
    pub config_path: PathBuf,
}

impl Sandbox {
    /// Build a sandbox with the dev box's real `~/.swarmbotix/sb.config.yml`
    /// tool paths copied in. Falls back to fixture paths if the real file
    /// is absent — tests then become skip-checked via `protoc_available`.
    pub fn new() -> Self {
        let home = TempDir::new().expect("tempdir");
        // `<tempdir>/.swarmbotix` — mirrors the real install root, so tests
        // can assert on `sandbox.home().join(".swarmbotix")/...` exactly as
        // they would against a fresh `$HOME`.
        let sb_home = home.path().join(".swarmbotix");
        let cfg = home.path().join("sb.config.yml");
        let body = real_dev_config_or_fixture(&sb_home);
        std::fs::write(&cfg, body).unwrap();
        Self {
            home,
            config_path: cfg,
        }
    }

    /// Effective `~/.swarmbotix/` root for this sandbox.
    pub fn sb_home(&self) -> PathBuf {
        self.home.path().join(".swarmbotix")
    }

    /// `<sb_home>/messages` — the style tree root this sandbox pins.
    pub fn messages_root(&self) -> PathBuf {
        self.sb_home().join("messages")
    }

    /// Source vault for the sandbox's active style ([`SANDBOX_STYLE`]).
    /// Tests should use this rather than rebuilding the path, so the style
    /// layout lives in exactly one place.
    pub fn vault_dir(&self) -> PathBuf {
        self.messages_root()
            .join(SANDBOX_STYLE)
            .join("message_definitions")
    }

    /// Codegen output root for the sandbox's active style.
    pub fn targets_dir(&self) -> PathBuf {
        self.messages_root()
            .join(SANDBOX_STYLE)
            .join("message_targets")
    }

    /// `Command::new(sb_bin()).envs(sandbox.env())` — sets SB_CONFIG (which
    /// carries `sb_home_dir`) plus HOME for Unix.
    pub fn cmd(&self) -> Command {
        let mut c = Command::new(sb_bin());
        c.env("SB_CONFIG", &self.config_path);
        // Redirects the *global* config layer, which $SB_CONFIG does not
        // suppress. Without this a real ~/.swarmbotix/sb.config.yml merges
        // in underneath and supplies whatever the sandbox left unset.
        c.env("SB_HOME", self.sb_home());
        c.env("HOME", self.home.path());
        // `dirs::home_dir` does not consult XDG_CONFIG_HOME, but sibling
        // lookups do; drop it so a developer's export can't leak in.
        c.env_remove("XDG_CONFIG_HOME");
        c
    }

    pub fn home(&self) -> &Path {
        self.home.path()
    }

    /// Replace the sandbox config with `body`, re-pinning `sb_home_dir`.
    ///
    /// Tests that need a specific config (say, protoc set but flatc absent)
    /// must go through this rather than writing `config_path` directly — a
    /// bare overwrite drops the pin and the command under test then resolves
    /// against the developer's real `~/.swarmbotix/`.
    pub fn write_config(&self, body: &str) {
        std::fs::write(&self.config_path, pin_sb_home(body, &self.sb_home())).unwrap();
    }
}

fn real_dev_config_or_fixture(sb_home: &Path) -> String {
    let real = dirs::home_dir()
        .map(|h| h.join(".swarmbotix").join("sb.config.yml"))
        .filter(|p| p.exists())
        .and_then(|p| std::fs::read_to_string(p).ok());
    let out = real.unwrap_or_else(|| {
        // Minimal fallback: tests that need protoc will check
        // `protoc_available()` and skip if absent.
        "device: dev01\ntransport_on_device: iceoryx2\ntransport_cross_device: zenoh\n".into()
    });
    pin_sb_home(&out, sb_home)
}

/// Every config key a sandbox must pin rather than inherit from the dev box.
/// Anything not listed here (tool paths, transport defaults) is inherited on
/// purpose, so tests exercise realistic values.
///
/// `message_definitions` / `message_targets` are stripped but **not** re-set:
/// the sandbox pins `messages_root` + `message_style` and lets the paths
/// derive, which is what exercises the style resolution the CLI actually
/// uses. They still have to be stripped, because an inherited absolute value
/// from the dev box's config would beat the derived path.
const PINNED_KEYS: [&str; 6] = [
    "sb_home_dir:",
    "messages_root:",
    "message_style:",
    "message_definitions:",
    "message_targets:",
    "device:",
];

/// Deterministic device for every sandbox. Tests assert on full topic
/// paths (`dev01/demo/pubber/zenoh/hello`), so this cannot be the real
/// hostname — an inherited `device:` makes those assertions machine-specific.
pub const SANDBOX_DEVICE: &str = "dev01";

/// Active style for every sandbox. `ros2` rather than the built-in default
/// `swarmbotix` because `std/` — which nearly every test compiles against —
/// ships in the ROS2 style, and `swarmbotix` is scaffolding-only for now.
pub const SANDBOX_STYLE: &str = "ros2";

/// Strip the host-specific keys from `body` and pin them at sandbox values.
///
/// Two reasons the vault paths are set **explicitly** rather than left to
/// fall out of `sb_home_dir`:
///
/// 1. `$SB_CONFIG` is the highest *layer*, not an exclusive one — the global
///    `<sb_home>/sb.config.yml` still merges underneath it. A seeded global
///    config sets `message_definitions` to an absolute path, so any key the
///    sandbox leaves unset falls through to the developer's real vault. On a
///    box with no global config (fresh CI) the fallback happens to be
///    correct, which is why this stayed hidden.
/// 2. `resolve_message_definitions` checks `cfg.message_definitions` *before*
///    the `sb_home_dir` fallback, so an inherited absolute value wins even
///    when `sb_home_dir` points at a tempdir.
///
/// Callers should also set `SB_HOME` on the spawned command (see
/// [`Sandbox::cmd`]) — that is what redirects the global *layer* itself, and
/// is the only mechanism that works on Windows.
///
/// Stripping first is what keeps the result parseable — serde's derived
/// `Deserialize` rejects duplicate fields.
pub fn pin_sb_home(body: &str, sb_home: &Path) -> String {
    let mut out: String = body
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            !PINNED_KEYS.iter().any(|k| t.starts_with(k))
        })
        .map(|l| format!("{l}\n"))
        .collect();
    let home = sb_home.display();
    out.push_str(&format!("sb_home_dir: {home}\n"));
    out.push_str(&format!("messages_root: {home}/messages\n"));
    out.push_str(&format!("message_style: {SANDBOX_STYLE}\n"));
    out.push_str(&format!("device: {SANDBOX_DEVICE}\n"));
    out
}

/// Locate a bash that can run a generated `runscript.bash` against a
/// native path.
///
/// On Windows a bare `bash` usually resolves to `C:\Windows\System32\bash.exe`
/// — the **WSL** launcher. WSL bash cannot open `C:/Users/…`; it only sees
/// `/mnt/c/Users/…`, so it reports a misleading "No such file or directory"
/// for a file that plainly exists. Git Bash handles drive-letter paths fine.
/// So: prefer a bash that is not under System32, and fall back to the usual
/// Git for Windows install locations.
///
/// Returns `None` when only WSL bash is available — callers should skip,
/// which matches the documented requirement that `sb up` on Windows needs
/// Git Bash (installguide_windows.md §9.5).
#[cfg(windows)]
pub fn find_bash() -> Option<PathBuf> {
    let is_wsl_shim = |p: &Path| {
        p.to_string_lossy()
            .to_ascii_lowercase()
            .contains(r"\windows\system32\")
    };
    if let Ok(found) = which::which_all("bash") {
        for p in found {
            if !is_wsl_shim(&p) {
                return Some(p);
            }
        }
    }
    for base in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
        let Some(root) = std::env::var_os(base) else {
            continue;
        };
        for tail in ["Git\\bin\\bash.exe", "Git\\usr\\bin\\bash.exe"] {
            let cand = PathBuf::from(&root).join(tail);
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

#[cfg(not(windows))]
pub fn find_bash() -> Option<PathBuf> {
    which::which("bash").ok()
}

/// True if a usable bash exists. Generated `runscript.bash` files need one.
pub fn bash_available() -> bool {
    find_bash().is_some()
}

/// Spawn `bash <script>`, with the interpreter resolved by [`find_bash`] and
/// the script path normalized to forward slashes.
///
/// The normalization matters independently of which bash is chosen: a native
/// `C:\Users\…` argument reaches bash with each backslash read as an escape,
/// arriving mangled as `C:Usershylee…`. No-op on Unix.
///
/// Panics if no usable bash is present — guard with [`bash_available`].
pub fn bash_script(script: &Path) -> Command {
    let bash = find_bash().expect("no usable bash — guard the test with bash_available()");
    let mut c = Command::new(bash);
    c.arg(script.display().to_string().replace('\\', "/"));
    c
}

/// True if the merged config exposes a working `protoc` path.
pub fn protoc_available(sandbox: &Sandbox) -> bool {
    let s = std::fs::read_to_string(&sandbox.config_path).unwrap_or_default();
    s.lines().any(|l| {
        l.trim_start().starts_with("protoc:")
            && l.split(':').nth(1).map(|p| {
                let path = p.split_whitespace().next().unwrap_or("");
                !path.is_empty() && std::path::Path::new(path).exists()
            }) == Some(true)
    })
}

/// Read the configured protoc path out of the sandbox config (or None).
pub fn protoc_path(sandbox: &Sandbox) -> Option<PathBuf> {
    let s = std::fs::read_to_string(&sandbox.config_path).ok()?;
    for l in s.lines() {
        let t = l.trim_start();
        if let Some(rest) = t.strip_prefix("protoc:") {
            let path = rest.split_whitespace().next().unwrap_or("");
            if !path.is_empty() {
                return Some(PathBuf::from(path));
            }
        }
    }
    None
}
