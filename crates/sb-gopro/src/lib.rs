//! L5 — `sb gopro`: freeze a module for production.
//!
//! Reads the module's `sb.dev.yml`, walks `<module_root>/<io_dir>/` to
//! confirm every declared pub/sub has its generated file on disk, then
//! writes a sanitized `sb.prd.yml` next to `sb.dev.yml`.
//!
//! What gets stripped (host-specific, must not leak into a binary that
//! ships to other machines):
//! - `root:` (module root override; absent in current files, an absolute
//!   path to the dev box in pre-0.5.0 ones)
//! - `language:` (a dev-time hint for codegen, not a runtime contract)
//! - `io_dir:`   (dev-time codegen target, not relevant at runtime)
//!
//! What stays (language-agnostic, ships with the binary):
//! - `module:`
//! - `publishers:`, the full list with `name`, `topic` (bare) or `key`
//!   (verbatim wire key), `type`, `transport`
//! - `subscribers:`, the same, plus `encoding` / `mime` when set
//! - `services:`, the declared service names
//!
//! After serialization a sanitization lint runs over the YAML body —
//! any leak of `/home/`, `/Users/`, `C:\`, or a stray `root:` key trips
//! the lint and the gopro fails loudly (TDD #6).

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

use sb_core::{Language, ModuleDevConfig, ModulePrdConfig};
use sb_workspace::{SbHome, read_flow};

// ─────────────────────────────────────────────────────────────────────
// Pure transform — ModuleDevConfig → ModulePrdConfig
// ─────────────────────────────────────────────────────────────────────

/// Build a sanitized [`ModulePrdConfig`] from a dev config. Pure — no IO,
/// no filesystem walks. The IO-dir sanity check lives in [`gopro_module`].
pub fn dev_to_prd(dev: &ModuleDevConfig) -> ModulePrdConfig {
    ModulePrdConfig {
        module: dev.module.clone(),
        publishers: dev.publishers.clone(),
        subscribers: dev.subscribers.clone(),
        services: dev.services.clone(),
    }
}

// ─────────────────────────────────────────────────────────────────────
// IO-dir presence check — every declared pub/sub must have its file
// ─────────────────────────────────────────────────────────────────────

/// One pub/sub whose generated file we expected under `<io_dir>/` but
/// couldn't find. Surfaced in [`GoproOutcome::missing_files`] so the CLI
/// can warn the user that `sb gopro` is freezing a half-scaffolded
/// module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingFile {
    pub role: &'static str, // "pub" | "sub"
    pub name: String,
    pub expected_path: PathBuf,
}

/// Check that every declared pub/sub has its generated file on disk.
/// Doesn't error — returns the list so the caller decides whether
/// missing files should be a hard fail or a warning. The default
/// `gopro_module` treats them as a warning (the dev.yml is the source
/// of truth for shape; absent codegen just means `sb pub add` was
/// interrupted).
pub fn check_io_dir(dev: &ModuleDevConfig, module_root: &Path) -> Vec<MissingFile> {
    let ext = file_extension_for(dev.language);
    let io_root = module_root.join(&dev.io_dir);
    let mut out = Vec::new();
    for p in &dev.publishers {
        let path = io_root.join("publishers").join(format!("{}.{ext}", p.name));
        if !path.exists() {
            out.push(MissingFile {
                role: "pub",
                name: p.name.clone(),
                expected_path: path,
            });
        }
    }
    for s in &dev.subscribers {
        let path = io_root
            .join("subscribers")
            .join(format!("{}.{ext}", s.name));
        if !path.exists() {
            out.push(MissingFile {
                role: "sub",
                name: s.name.clone(),
                expected_path: path,
            });
        }
    }
    out
}

fn file_extension_for(lang: Language) -> &'static str {
    match lang {
        Language::Rust => "rs",
        Language::Python => "py",
        Language::Cpp => "cpp",
        Language::Flutter => "dart",
        Language::Unity => "cs",
    }
}

// ─────────────────────────────────────────────────────────────────────
// Sanitization lint
// ─────────────────────────────────────────────────────────────────────

/// One leak the sanitization lint caught in the serialized prd.yml body.
/// The CLI prints these before bailing so the user knows exactly what
/// would have shipped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leak {
    pub line_no: usize,
    pub kind: LeakKind,
    pub snippet: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeakKind {
    /// A `root:` YAML key — must never appear in prd.yml.
    RootKey,
    /// `/home/...` — Linux absolute home path.
    LinuxHome,
    /// `/Users/...` — macOS absolute home path.
    MacHome,
    /// `C:\...` (case-insensitive drive letter) — Windows path.
    WindowsPath,
    /// `language:` key — dev-time only.
    LanguageKey,
    /// `io_dir:` key — dev-time only.
    IoDirKey,
}

impl LeakKind {
    pub fn as_str(self) -> &'static str {
        match self {
            LeakKind::RootKey => "root: key",
            LeakKind::LinuxHome => "/home/ path",
            LeakKind::MacHome => "/Users/ path",
            LeakKind::WindowsPath => "C:\\ path",
            LeakKind::LanguageKey => "language: key",
            LeakKind::IoDirKey => "io_dir: key",
        }
    }
}

/// Scan a serialized prd.yml body for host-specific leaks. Returns
/// every offender found — empty result = sanitized body is safe to
/// write. Used by [`gopro_module`] after serialization to refuse
/// writing a prd.yml that would leak host state into the binary.
pub fn lint_sanitized(body: &str) -> Vec<Leak> {
    let mut out = Vec::new();
    for (i, line) in body.lines().enumerate() {
        let trimmed = line.trim_start();
        // YAML-key checks: anchor at the start of the key, ignoring
        // indentation. We never want any nested `root:` either (none
        // of the prd shape produces one, but be conservative).
        if trimmed.starts_with("root:") {
            out.push(Leak {
                line_no: i + 1,
                kind: LeakKind::RootKey,
                snippet: line.to_string(),
            });
        }
        if trimmed.starts_with("language:") {
            out.push(Leak {
                line_no: i + 1,
                kind: LeakKind::LanguageKey,
                snippet: line.to_string(),
            });
        }
        if trimmed.starts_with("io_dir:") {
            out.push(Leak {
                line_no: i + 1,
                kind: LeakKind::IoDirKey,
                snippet: line.to_string(),
            });
        }
        // Path substring checks: anywhere on the line.
        if line.contains("/home/") {
            out.push(Leak {
                line_no: i + 1,
                kind: LeakKind::LinuxHome,
                snippet: line.to_string(),
            });
        }
        if line.contains("/Users/") {
            out.push(Leak {
                line_no: i + 1,
                kind: LeakKind::MacHome,
                snippet: line.to_string(),
            });
        }
        if has_windows_drive_path(line) {
            out.push(Leak {
                line_no: i + 1,
                kind: LeakKind::WindowsPath,
                snippet: line.to_string(),
            });
        }
    }
    out
}

/// `<letter>:\` — e.g. `C:\`, `d:\`. We require the colon-backslash
/// pair so we don't trip on `key: value` or `time: 12:34`.
fn has_windows_drive_path(line: &str) -> bool {
    let bytes = line.as_bytes();
    for i in 0..bytes.len().saturating_sub(2) {
        let c = bytes[i];
        if !c.is_ascii_alphabetic() {
            continue;
        }
        if bytes[i + 1] == b':' && bytes[i + 2] == b'\\' {
            return true;
        }
    }
    false
}

/// Pretty-print leaks for an error message.
pub fn format_leaks(leaks: &[Leak]) -> String {
    let mut s = String::new();
    for l in leaks {
        // Writing to a String is infallible; the Result exists only to
        // satisfy the fmt::Write signature.
        let _ = writeln!(
            s,
            "  line {}: {} — {}",
            l.line_no,
            l.kind.as_str(),
            l.snippet.trim(),
        );
    }
    s
}

// ─────────────────────────────────────────────────────────────────────
// gopro_module — the verb
// ─────────────────────────────────────────────────────────────────────

/// Outcome of `sb gopro` on one module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoproOutcome {
    pub module: String,
    pub prd_yml_path: PathBuf,
    /// Files declared in dev.yml but not present under `io_dir`/. Non-fatal
    /// — CLI surfaces as a warning. Lets the user know `sb gopro`
    /// shipped a definition for which no codegen exists yet.
    pub missing_files: Vec<MissingFile>,
}

/// Run gopro on a single module given its `sb.dev.yml` on disk.
///
/// Steps:
///   1. Parse `sb.dev.yml`.
///   2. Walk `io_dir/` and record any missing files (non-fatal).
///   3. Transform → `ModulePrdConfig`.
///   4. Serialize to YAML.
///   5. Sanitization lint — abort if any leak.
///   6. Write `<module_root>/sb.prd.yml`.
pub fn gopro_module(dev_yml_path: &Path) -> Result<GoproOutcome> {
    if !dev_yml_path.exists() {
        bail!("sb.dev.yml not found at {}", dev_yml_path.display());
    }
    let body = fs::read_to_string(dev_yml_path)
        .with_context(|| format!("reading {}", dev_yml_path.display()))?;
    let dev: ModuleDevConfig = serde_yaml::from_str(&body)
        .with_context(|| format!("parsing {}", dev_yml_path.display()))?;
    // sb.prd.yml lands next to sb.dev.yml; the io files live under the
    // module root, which `root:` may move away from that directory.
    let yml_dir = dev_yml_path
        .parent()
        .ok_or_else(|| anyhow!("sb.dev.yml has no parent: {}", dev_yml_path.display()))?
        .to_path_buf();
    let module_root = dev.module_root(dev_yml_path);

    let missing = check_io_dir(&dev, &module_root);

    let prd = dev_to_prd(&dev);
    let serialized = serde_yaml::to_string(&prd).context("serializing sb.prd.yml")?;

    let leaks = lint_sanitized(&serialized);
    if !leaks.is_empty() {
        bail!(
            "sb.prd.yml would leak host-specific data — refusing to write:\n{}\
             (this is a bug in sb-gopro — please file an issue with the offending sb.dev.yml)",
            format_leaks(&leaks),
        );
    }

    let prd_path = yml_dir.join("sb.prd.yml");
    fs::write(&prd_path, serialized).with_context(|| format!("writing {}", prd_path.display()))?;

    Ok(GoproOutcome {
        module: dev.module,
        prd_yml_path: prd_path,
        missing_files: missing,
    })
}

// ─────────────────────────────────────────────────────────────────────
// gopro_all — every module in the active workspace
// ─────────────────────────────────────────────────────────────────────

/// One per-module result from [`gopro_all`]. `Err` is a non-fatal
/// per-module failure — the CLI keeps going with the rest and surfaces
/// the failures at the end so one broken module doesn't block freezing
/// the others (TDD #8).
#[derive(Debug)]
pub struct ModuleResult {
    pub module: String,
    pub dev_yml_path: PathBuf,
    pub result: Result<GoproOutcome>,
}

/// `sb gopro --all` — run [`gopro_module`] on every module in the
/// active workspace's `flow.yaml`. Errors per-module are collected,
/// not propagated, so a single broken `sb.dev.yml` doesn't stop the
/// rest from freezing.
pub fn gopro_all(home: &SbHome, workspace: &str) -> Result<Vec<ModuleResult>> {
    let flow = read_flow(home, workspace)
        .with_context(|| format!("reading flow.yaml for workspace {workspace:?}"))?;
    let modules = flow.module_paths();
    if modules.is_empty() {
        bail!("workspace {workspace:?} has no modules in flow.yaml — run `sb init` first");
    }
    let mut out = Vec::with_capacity(modules.len());
    for (module, dev_yml) in modules {
        let result = gopro_module(&dev_yml);
        out.push(ModuleResult {
            module,
            dev_yml_path: dev_yml,
            result,
        });
    }
    Ok(out)
}

// ─────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    use sb_core::{Encoding, Language, PubSpec, ServiceSpec, SubSpec, TopicRef, Transport};

    fn spec_pub(name: &str, msg: &str, transport: Transport) -> PubSpec {
        PubSpec {
            name: name.into(),
            topic: TopicRef::Owned(name.into()),
            msg_type: Some(msg.into()),
            transport,
            encoding: Encoding::Proto,
            mime: None,
        }
    }

    fn sample_dev() -> ModuleDevConfig {
        ModuleDevConfig {
            module: "camera".into(),
            language: Language::Rust,
            // A pre-0.5.0 absolute root: must never reach sb.prd.yml.
            root: Some(PathBuf::from("/home/user/projects/camera_app")),
            io_dir: PathBuf::from("swarmbotix_io"),
            publishers: vec![
                spec_pub("image_raw", "std/ImageStamped", Transport::Iceoryx2),
                spec_pub("heartbeat", "std/StringStamped", Transport::Zenoh),
            ],
            subscribers: vec![SubSpec {
                name: "cmd_vel".into(),
                topic: TopicRef::Owned("cmd_vel".into()),
                msg_type: Some("std/TwistStamped".into()),
                transport: Transport::Zenoh,
                encoding: Encoding::Proto,
                mime: None,
            }],
            services: vec![ServiceSpec {
                name: "calib".into(),
            }],
        }
    }

    #[test]
    fn dev_to_prd_preserves_key_entries() {
        let mut dev = sample_dev();
        dev.subscribers.push(SubSpec {
            name: "hello".into(),
            topic: TopicRef::Key("dev01/demo/pubber/zenoh/hello".into()),
            msg_type: Some("std/StringStamped".into()),
            transport: Transport::Zenoh,
            encoding: Encoding::Proto,
            mime: None,
        });
        let prd = dev_to_prd(&dev);
        assert_eq!(prd.subscribers, dev.subscribers);
        let y = serde_yaml::to_string(&prd).unwrap();
        assert!(y.contains("key: dev01/demo/pubber/zenoh/hello"), "{y}");
        assert!(y.contains("topic: cmd_vel"), "{y}");
        assert_eq!(lint_sanitized(&y), Vec::new(), "{y}");
        let back: ModulePrdConfig = serde_yaml::from_str(&y).unwrap();
        assert_eq!(back, prd);
    }

    #[test]
    fn lint_catches_root_key() {
        let leaks = lint_sanitized("module: m\nroot: /home/u/proj\n");
        assert_eq!(leaks.len(), 2); // root: AND /home/ path
        assert!(leaks.iter().any(|l| l.kind == LeakKind::RootKey));
        assert!(leaks.iter().any(|l| l.kind == LeakKind::LinuxHome));
    }
}
