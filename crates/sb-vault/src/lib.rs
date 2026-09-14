//! Message vault — embedded `std/*` bundle, CRUD, protoc invocation.
//!
//! The vault is rooted at `messages_root` and spans **every style**:
//! ```text
//! <messages_root>/
//!   ros2/message_definitions/
//!     std/         ← installed-on-first-run from the embedded bundle
//!     std_msgs/ geometry_msgs/ ...
//!   swarmbotix/message_definitions/
//!     header/ primitives/ images/ sensors/
//!   <yours>/message_definitions/
//! ```
//!
//! A [`MessageName`] is fully qualified — `<style>/<namespace>/<Leaf>` — so
//! the vault never needs a "current style". That is deliberate: a module
//! routinely publishes one style and subscribes to another, and a name whose
//! meaning depends on the reader's config is not an identity.
//!
//! The forge bundles `messages/ros2/message_definitions/std/*.proto` (two
//! directories up from this crate) via [`include_dir!`]. First touch copies
//! it into the **ros2** style if a file is absent. Existing files are never
//! overwritten (never clobber user edits).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use include_dir::{Dir, include_dir};
use sb_core::STYLE_ROS2;

/// The forge's source-of-truth `std/` bundle, which lives inside the ROS2
/// style. Path is relative to this crate's `Cargo.toml`.
const STD_BUNDLE: Dir<'_> =
    include_dir!("$CARGO_MANIFEST_DIR/../../messages/ros2/message_definitions/std");

/// Fully-qualified identifier for a message: `<style>/<namespace>/<Leaf>`,
/// e.g. `ros2/std/Header`, `swarmbotix/images/Image480pMono8`.
///
/// **The style is part of the name, not ambient state.** A module routinely
/// publishes one style and subscribes to another, and `Header.metadata
/// .msg_type` travels on the wire to a consumer that shares no config with
/// the publisher — so `images/Image480pMono8` alone is not an identity. It
/// does not say which schema produced the bytes.
///
/// There is deliberately no "active style" to resolve a 2-segment name
/// against: that would make the meaning of a name depend on the reader's
/// configuration, which is exactly the ambiguity this type exists to remove.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MessageName {
    pub style: String,
    pub namespace: String,
    pub leaf: String,
}

impl MessageName {
    pub fn parse(s: &str) -> Result<Self, MessageNameError> {
        let mut parts = s.split('/');
        let (Some(style), Some(ns), Some(leaf), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            // Distinguish the common near-miss so the error can teach.
            return Err(match s.matches('/').count() {
                0 | 1 => MessageNameError::MissingStyle { got: s.to_owned() },
                _ => MessageNameError::TooManySegments,
            });
        };
        if style.is_empty() {
            return Err(MessageNameError::EmptyStyle);
        }
        if ns.is_empty() {
            return Err(MessageNameError::EmptyNamespace);
        }
        if leaf.is_empty() {
            return Err(MessageNameError::EmptyLeaf);
        }
        if !sb_core::is_valid_message_style(style)
            || !style.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return Err(MessageNameError::IllegalStyleChar);
        }
        if !ns.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(MessageNameError::IllegalNamespaceChar);
        }
        // Leaf must be PascalCase: starts with uppercase, ascii-alphanumeric only.
        let mut chars = leaf.chars();
        match chars.next() {
            Some(c) if c.is_ascii_uppercase() => {}
            _ => return Err(MessageNameError::LeafNotPascalCase),
        }
        if !chars.all(|c| c.is_ascii_alphanumeric()) {
            return Err(MessageNameError::LeafNotPascalCase);
        }
        Ok(Self {
            style: style.to_owned(),
            namespace: ns.to_owned(),
            leaf: leaf.to_owned(),
        })
    }

    /// The forge-bundled `std/*` set, which lives in the ROS2 style.
    pub fn is_std(&self) -> bool {
        self.style == STYLE_ROS2 && self.namespace == "std"
    }

    /// `<style>/<ns>` — the two segments that identify the definitions tree.
    pub fn style_ns(&self) -> String {
        format!("{}/{}", self.style, self.namespace)
    }

    /// Disk path relative to that style's `message_definitions`:
    /// `<ns>/<Leaf>.proto`.
    pub fn rel_path(&self) -> PathBuf {
        PathBuf::from(&self.namespace).join(format!("{}.proto", self.leaf))
    }

    /// Disk path relative to `messages_root`:
    /// `<style>/message_definitions/<ns>/<Leaf>.proto`.
    pub fn rel_path_from_messages_root(&self) -> PathBuf {
        PathBuf::from(&self.style)
            .join("message_definitions")
            .join(self.rel_path())
    }
}

impl std::fmt::Display for MessageName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}/{}", self.style, self.namespace, self.leaf)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MessageNameError {
    #[error(
        "message names are fully qualified as <style>/<namespace>/<Leaf> \
         (e.g. ros2/std/Header, swarmbotix/images/Image480pMono8) — got {got:?}. \
         Run `sb message list` to see every message with its style."
    )]
    MissingStyle { got: String },
    #[error("name has too many '/' segments (expected <style>/<namespace>/<Leaf>)")]
    TooManySegments,
    #[error("style is empty")]
    EmptyStyle,
    #[error("namespace is empty")]
    EmptyNamespace,
    #[error("leaf name is empty")]
    EmptyLeaf,
    #[error("style contains illegal characters (allowed: [A-Za-z0-9_])")]
    IllegalStyleChar,
    #[error("namespace contains illegal characters (allowed: [A-Za-z0-9_])")]
    IllegalNamespaceChar,
    #[error("leaf name must be PascalCase ASCII (e.g. ImageStamped)")]
    LeafNotPascalCase,
}

// ─────────────────────────────────────────────────────────────────────
// Vault
// ─────────────────────────────────────────────────────────────────────

/// Handle to a vault on disk. Cheap to construct.
#[derive(Debug, Clone)]
pub struct Vault {
    /// `messages_root` — the directory holding every style. The vault spans
    /// **all** styles because a message name carries its own style; there is
    /// no ambient "current style" to scope it to.
    root: PathBuf,
}

impl Vault {
    /// Open the vault rooted at `messages_root`. Does **not** create or
    /// install anything; call [`Vault::ensure_installed`] for that.
    pub fn at(messages_root: impl Into<PathBuf>) -> Self {
        Self {
            root: messages_root.into(),
        }
    }

    /// `messages_root`.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Every style on disk — each subdirectory with a `message_definitions/`
    /// child. Sorted.
    pub fn styles(&self) -> Vec<String> {
        let Ok(rd) = fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut out: Vec<String> = rd
            .flatten()
            .filter(|e| e.path().join("message_definitions").is_dir())
            .filter_map(|e| e.file_name().to_str().map(str::to_owned))
            .filter(|n| sb_core::is_valid_message_style(n))
            .collect();
        out.sort();
        out
    }

    /// `<root>/<style>/message_definitions` — the protoc include root for
    /// that style. Each style compiles independently; there are no
    /// cross-style imports.
    pub fn defs_dir(&self, style: &str) -> PathBuf {
        self.root.join(style).join("message_definitions")
    }

    /// `<root>/<style>/message_targets` — generated output for that style.
    pub fn targets_dir(&self, style: &str) -> PathBuf {
        self.root.join(style).join("message_targets")
    }

    /// `<root>/<style>/sb.style.yml` — per-style compile settings.
    ///
    /// A sibling of `message_definitions/` rather than a key in
    /// `sb.config.yml`, so it survives a `git clone` of a standalone style
    /// repo onto a machine that shares no config with this one.
    pub fn style_config_path(&self, style: &str) -> PathBuf {
        self.root.join(style).join("sb.style.yml")
    }

    /// Read `<style>/sb.style.yml`. `Ok(None)` when the file is absent —
    /// absence means "fall through to the next rule", never "emit nothing".
    /// A file that exists but does not parse is an error: silently ignoring a
    /// typo'd pin is exactly how someone ships the backends they pinned
    /// against.
    pub fn style_config(&self, style: &str) -> Result<Option<sb_core::StyleConfig>> {
        let path = self.style_config_path(style);
        if !path.is_file() {
            return Ok(None);
        }
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let cfg = sb_core::StyleConfig::parse(&text, &path.display().to_string())
            .with_context(|| format!("parsing {}", path.display()))?;
        Ok(Some(cfg))
    }

    /// Write `<style>/sb.style.yml`, creating the style dir if needed.
    pub fn write_style_config(&self, style: &str, cfg: &sb_core::StyleConfig) -> Result<PathBuf> {
        let path = self.style_config_path(style);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::write(&path, cfg.to_yaml()).with_context(|| format!("writing {}", path.display()))?;
        Ok(path)
    }

    /// Filenames the forge-bundled `STD_BUNDLE` is expected to install into
    /// `<root>/std/` — `Header.proto`, `String.proto`, … Sorted.
    ///
    /// Exposed so `sb doctor`'s `std vault` check can verify the vault
    /// against the same list [`Vault::ensure_installed`] writes, rather than
    /// keeping a second hardcoded copy that can drift from the bundle.
    pub fn std_bundle_filenames() -> Vec<String> {
        let mut names: Vec<String> = STD_BUNDLE
            .files()
            .filter_map(|f| f.path().file_name()?.to_str().map(str::to_owned))
            .collect();
        names.sort();
        names
    }

    /// Directory the embedded bundle installs into:
    /// `<root>/ros2/message_definitions/std`.
    pub fn std_dir(&self) -> PathBuf {
        self.defs_dir(STYLE_ROS2).join("std")
    }

    /// Names from [`Self::std_bundle_filenames`] that are absent from
    /// [`Self::std_dir`]. Empty means the std vault is fully populated.
    pub fn missing_std_files(&self) -> Vec<String> {
        let std_dir = self.std_dir();
        Self::std_bundle_filenames()
            .into_iter()
            .filter(|n| !std_dir.join(n).exists())
            .collect()
    }

    /// Install the embedded `std/*` bundle into the **ros2** style if any
    /// file is missing. Idempotent. Existing files are never overwritten.
    ///
    /// `std/` belongs to ros2 and nowhere else — writing it into another
    /// style would drop files where that style's own definitions go.
    pub fn ensure_installed(&self) -> Result<InstallReport> {
        let std_dir = self.std_dir();
        fs::create_dir_all(&std_dir).with_context(|| format!("creating {}", std_dir.display()))?;
        let mut copied = Vec::new();
        let mut skipped = Vec::new();
        for f in STD_BUNDLE.files() {
            let target = std_dir.join(f.path().file_name().expect("bundle file has name"));
            if target.exists() {
                skipped.push(target.clone());
                continue;
            }
            fs::write(&target, f.contents())
                .with_context(|| format!("writing {}", target.display()))?;
            copied.push(target);
        }
        Ok(InstallReport { copied, skipped })
    }

    /// Walk the vault and return every message in `<ns>/<Leaf>` form,
    /// sorted by namespace then leaf.
    pub fn list(&self) -> Result<Vec<MessageName>> {
        let mut out = Vec::new();
        for style in self.styles() {
            out.extend(self.list_style(&style)?);
        }
        out.sort();
        Ok(out)
    }

    /// Messages belonging to one style, sorted. Empty if the style has no
    /// `message_definitions/`.
    pub fn list_style(&self, style: &str) -> Result<Vec<MessageName>> {
        let defs = self.defs_dir(style);
        if !defs.exists() {
            return Ok(vec![]);
        }
        let mut out = Vec::new();
        for entry in walkdir::WalkDir::new(&defs)
            .min_depth(2)
            .max_depth(2)
            .into_iter()
            .filter_map(Result::ok)
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("proto") {
                continue;
            }
            let leaf = path.file_stem().and_then(|s| s.to_str());
            let ns = path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str());
            if let (Some(ns), Some(leaf)) = (ns, leaf) {
                // Skip files that don't match the strict naming rules.
                if let Ok(name) = MessageName::parse(&format!("{style}/{ns}/{leaf}")) {
                    out.push(name);
                }
            }
        }
        out.sort();
        Ok(out)
    }

    /// Create a new user message. Refuses to write under `std/`.
    /// Refuses if the file already exists (use `--force` semantics at the
    /// CLI layer if ever needed; here it's strict).
    pub fn new_message(&self, name: &MessageName) -> Result<PathBuf> {
        if name.is_std() {
            anyhow::bail!(
                "cannot create messages under 'ros2/std/' (reserved for the forge bundle)"
            );
        }
        let path = self.path_of(name);
        if path.exists() {
            anyhow::bail!("{} already exists", path.display());
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::write(&path, skeleton_proto(name))
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(path)
    }

    /// Delete a message. Refuses if the message is under `std/`.
    pub fn remove(&self, name: &MessageName) -> Result<()> {
        if name.is_std() {
            anyhow::bail!("refusing to delete {name} — std messages ship with the forge");
        }
        let path = self.path_of(name);
        if !path.exists() {
            anyhow::bail!("{} not found", path.display());
        }
        fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        Ok(())
    }

    /// Absolute path to a message's `.proto` (regardless of existence).
    pub fn path_of(&self, name: &MessageName) -> PathBuf {
        self.root.join(name.rel_path_from_messages_root())
    }
}

#[derive(Debug, Clone, Default)]
pub struct InstallReport {
    pub copied: Vec<PathBuf>,
    pub skipped: Vec<PathBuf>,
}

fn skeleton_proto(name: &MessageName) -> String {
    format!(
        "// {ns}/{leaf}.proto — generated skeleton.\n\
         syntax = \"proto3\";\n\n\
         package swarmbotix.{ns};\n\n\
         message {leaf} {{\n  // TODO: add fields\n}}\n",
        ns = name.namespace,
        leaf = name.leaf,
    )
}

// ─────────────────────────────────────────────────────────────────────
// protoc invocation
// ─────────────────────────────────────────────────────────────────────

/// Render a vault-relative `.proto` path as protoc expects it on the
/// command line: relative to a `--proto_path` entry, forward-slashed.
///
/// protoc's virtual filesystem treats `\` as an ordinary character rather
/// than a separator, so a native Windows `builtin_interfaces\Duration.proto`
/// fails with *"Could not make proto path relative"* even though the file
/// sits right there under `--proto_path`. No-op on Unix.
pub fn proto_rel_arg(rel: &Path) -> String {
    rel.to_string_lossy().replace('\\', "/")
}

/// Run `protoc --descriptor_set_out=<out> --proto_path=<vault_root> <files>`,
/// emitting a [`FileDescriptorSet`](https://protobuf.com/docs/descriptors)
/// to `out_path`. Returns the path on success.
pub fn compile_to_descriptor_set(
    protoc: &Path,
    vault_root: &Path,
    sources: &[PathBuf],
    out_path: &Path,
) -> Result<PathBuf> {
    if !protoc.exists() {
        anyhow::bail!("protoc not found at {}", protoc.display());
    }
    if sources.is_empty() {
        anyhow::bail!("no sources to compile");
    }
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let mut cmd = Command::new(protoc);
    cmd.arg(format!("--proto_path={}", vault_root.display()));
    cmd.arg(format!("--descriptor_set_out={}", out_path.display()));
    cmd.arg("--include_imports");
    cmd.arg("--include_source_info");
    for s in sources {
        let rel = s.strip_prefix(vault_root).unwrap_or(s);
        cmd.arg(proto_rel_arg(rel));
    }
    let out = cmd
        .output()
        .with_context(|| format!("spawning {}", protoc.display()))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!("protoc failed: {stderr}");
    }
    Ok(out_path.to_owned())
}

/// Compile every message in the vault to a single `FileDescriptorSet`.
/// Compile every message in ONE style into a descriptor set.
///
/// protoc gets exactly one include root — that style's `message_definitions/`
/// — because an `import "std/Header.proto"` must resolve within the style
/// that wrote it. Compiling styles together would let one style's import
/// silently bind to a same-named file in another.
pub fn compile_style(
    vault: &Vault,
    style: &str,
    protoc: &Path,
    out_path: &Path,
) -> Result<PathBuf> {
    let names = vault.list_style(style)?;
    let sources: Vec<PathBuf> = names.iter().map(|n| vault.path_of(n)).collect();
    compile_to_descriptor_set(protoc, &vault.defs_dir(style), &sources, out_path)
}

/// The Unity project root containing `path`, if any.
///
/// Walks up from `path` looking for a directory that has **both** `Assets/`
/// and `ProjectSettings/`. Unity creates both for every project and neither
/// name is generic enough to collide by accident; matching on `Assets/` alone
/// would fire on any tree that happens to have one.
///
/// Why the CLI cares: Unity auto-compiles every `.cs` under `Assets/`, and
/// Unity 6 caps at C# 9 on .NET Standard 2.1. The iox2 C# emitter needs
/// file-scoped namespaces (C# 10) and `[InlineArray]` (.NET 8 / C# 12), so
/// dropping those files into `Assets/` breaks the host build — and iceoryx2
/// is already refused for Unity at the pub/sub layer, which makes emitting
/// them pointless as well as harmful.
///
/// `path` need not exist; a not-yet-created output directory still resolves
/// through its ancestors.
pub fn unity_project_root(path: &Path) -> Option<PathBuf> {
    let mut cur = path.to_path_buf();
    loop {
        if cur.join("Assets").is_dir() && cur.join("ProjectSettings").is_dir() {
            return Some(cur);
        }
        if !cur.pop() {
            return None;
        }
    }
}

/// Compile a single message (and its imports via `--include_imports`).
pub fn compile_one(
    vault: &Vault,
    name: &MessageName,
    protoc: &Path,
    out_path: &Path,
) -> Result<PathBuf> {
    let path = vault.path_of(name);
    if !path.exists() {
        anyhow::bail!("{} not in vault", name);
    }
    // Include root is the message's OWN style — see compile_style.
    compile_to_descriptor_set(protoc, &vault.defs_dir(&name.style), &[path], out_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // ── Name validator (L1 TDD test #4 partial) ───────────────────────

    #[test]
    fn message_name_accepts_pascal_case() {
        let n = MessageName::parse("ros2/std/Header").unwrap();
        assert_eq!(n.namespace, "std");
        assert_eq!(n.leaf, "Header");
        assert_eq!(n.rel_path(), PathBuf::from("std/Header.proto"));
    }

    #[test]
    fn message_name_rejects_lower_leaf() {
        assert!(matches!(
            MessageName::parse("swarmbotix/custom/foo"),
            Err(MessageNameError::LeafNotPascalCase)
        ));
    }

    #[test]
    fn message_name_rejects_missing_style_and_empty_parts() {
        assert!(matches!(
            MessageName::parse("Header"),
            Err(MessageNameError::MissingStyle { .. })
        ));
        // Two segments is the near-miss that matters: it looks like a name
        // but names no style.
        assert!(matches!(
            MessageName::parse("std/Header"),
            Err(MessageNameError::MissingStyle { .. })
        ));
        assert!(matches!(
            MessageName::parse("ros2//Header"),
            Err(MessageNameError::EmptyNamespace)
        ));
        assert!(matches!(
            MessageName::parse("ros2/std/"),
            Err(MessageNameError::EmptyLeaf)
        ));
    }

    #[test]
    fn message_name_rejects_illegal_namespace_chars() {
        assert!(matches!(
            MessageName::parse("ros2/my namespace/Foo"),
            Err(MessageNameError::IllegalNamespaceChar)
        ));
    }

    // ── Install-on-first-run (L1 TDD test #5) ─────────────────────────

    #[test]
    fn first_run_installs_std_bundle() {
        let d = tempdir().unwrap();
        let v = Vault::at(d.path());
        let report = v.ensure_installed().unwrap();
        assert!(!report.copied.is_empty(), "should have copied std files");
        // Expected std/* files exist — inside the ros2 style.
        for fname in ["Header.proto", "String.proto", "StringStamped.proto"] {
            assert!(v.std_dir().join(fname).exists(), "missing {fname}");
        }
    }

    #[test]
    fn second_install_is_noop() {
        let d = tempdir().unwrap();
        let v = Vault::at(d.path());
        v.ensure_installed().unwrap();
        let r2 = v.ensure_installed().unwrap();
        assert!(r2.copied.is_empty(), "second run should not copy");
        assert!(!r2.skipped.is_empty(), "second run should skip");
    }

    #[test]
    fn install_preserves_user_edits() {
        let d = tempdir().unwrap();
        let v = Vault::at(d.path());
        v.ensure_installed().unwrap();
        let header = v.std_dir().join("Header.proto");
        fs::write(&header, "// user-edited\n").unwrap();
        v.ensure_installed().unwrap();
        let after = fs::read_to_string(&header).unwrap();
        assert_eq!(after, "// user-edited\n", "user edit clobbered");
    }

    // ── CRUD (L1 TDD test #4) ─────────────────────────────────────────

    #[test]
    fn crud_round_trip_for_user_message() {
        let d = tempdir().unwrap();
        let v = Vault::at(d.path());
        v.ensure_installed().unwrap();
        let name = MessageName::parse("swarmbotix/custom/Foo").unwrap();
        v.new_message(&name).unwrap();
        let list = v.list().unwrap();
        assert!(list.contains(&name));
        v.remove(&name).unwrap();
        let list = v.list().unwrap();
        assert!(!list.contains(&name));
    }

    #[test]
    fn cannot_create_under_std() {
        let d = tempdir().unwrap();
        let v = Vault::at(d.path());
        v.ensure_installed().unwrap();
        let name = MessageName::parse("ros2/std/Sneaky").unwrap();
        let err = v.new_message(&name).unwrap_err();
        assert!(format!("{err:#}").contains("std/"));
    }

    #[test]
    fn cannot_remove_std_messages() {
        let d = tempdir().unwrap();
        let v = Vault::at(d.path());
        v.ensure_installed().unwrap();
        let name = MessageName::parse("ros2/std/Header").unwrap();
        let err = v.remove(&name).unwrap_err();
        assert!(format!("{err:#}").contains("refusing"));
        // Still on disk.
        assert!(v.path_of(&name).exists());
    }

    #[test]
    fn list_includes_all_bundled_std_messages() {
        let d = tempdir().unwrap();
        let v = Vault::at(d.path());
        v.ensure_installed().unwrap();
        let names: Vec<String> = v.list().unwrap().iter().map(ToString::to_string).collect();
        for expected in [
            "ros2/std/Header",
            "ros2/std/String",
            "ros2/std/StringStamped",
            "ros2/std/Vector3",
            "ros2/std/Twist",
            "ros2/std/TwistStamped",
            "ros2/std/Image",
            "ros2/std/ImageStamped",
        ] {
            assert!(names.contains(&expected.to_owned()), "missing {expected}");
        }
    }

    // ── std/* fixture (L1 TDD test #7) ────────────────────────────────

    #[test]
    fn header_contains_msg_freq_desired() {
        // Header is load-bearing for L4. If someone "fixes" Header to
        // include a payload, this test breaks loudly.
        let header = STD_BUNDLE
            .get_file("Header.proto")
            .expect("Header.proto in bundle");
        let src = std::str::from_utf8(header.contents()).unwrap();
        assert!(
            src.contains("msg_freq_desired"),
            "Header must declare metadata.msg_freq_desired (load-bearing for L4)"
        );
        assert!(
            src.contains("topic_full_name"),
            "Header must declare metadata.topic_full_name"
        );
        assert!(
            src.contains("msg_type"),
            "Header must declare metadata.msg_type"
        );
        assert!(
            src.contains("timestamp_ns"),
            "Header must declare timestamp_ns"
        );
    }

    #[test]
    fn string_has_no_header_field() {
        // std/String mirrors std_msgs/String — bare payload, no header.
        let s = STD_BUNDLE.get_file("String.proto").unwrap();
        let src = std::str::from_utf8(s.contents()).unwrap();
        assert!(!src.contains("Header"), "std/String must not embed Header");
        assert!(src.contains("string data"));
    }

    #[test]
    fn string_stamped_composes_header_plus_data() {
        let s = STD_BUNDLE.get_file("StringStamped.proto").unwrap();
        let src = std::str::from_utf8(s.contents()).unwrap();
        assert!(src.contains("Header header"));
        assert!(src.contains("string data"));
    }

    // ── fully-qualified names ─────────────────────────────────────────

    #[test]
    fn a_name_without_a_style_is_rejected_with_a_teaching_error() {
        // The whole point: "images/Image" is not an identity. Two styles
        // may each define one, and msg_type travels to a consumer that
        // shares no config with the publisher.
        let e = MessageName::parse("images/Image").unwrap_err();
        assert!(matches!(e, MessageNameError::MissingStyle { .. }));
        let msg = e.to_string();
        assert!(msg.contains("<style>/<namespace>/<Leaf>"), "{msg}");
        assert!(msg.contains("ros2/std/Header"), "{msg}");
    }

    #[test]
    fn fully_qualified_names_round_trip() {
        for raw in [
            "ros2/std/Header",
            "swarmbotix/images/Image480pMono8",
            "acme/robot_msgs/ArmGoal",
        ] {
            let n = MessageName::parse(raw).unwrap();
            assert_eq!(n.to_string(), raw);
        }
    }

    #[test]
    fn same_namespace_in_two_styles_are_different_messages() {
        let a = MessageName::parse("ros2/images/Image").unwrap();
        let b = MessageName::parse("swarmbotix/images/Image").unwrap();
        assert_ne!(a, b);
        assert_ne!(
            a.rel_path_from_messages_root(),
            b.rel_path_from_messages_root()
        );
    }

    #[test]
    fn only_ros2_std_counts_as_the_forge_bundle() {
        assert!(MessageName::parse("ros2/std/Header").unwrap().is_std());
        // A user style may legitimately have its own `std/`; it is theirs.
        assert!(!MessageName::parse("acme/std/Header").unwrap().is_std());
    }

    // ── vault spans every style ───────────────────────────────────────

    #[test]
    fn list_spans_all_styles_and_names_carry_their_own() {
        let d = tempdir().unwrap();
        let v = Vault::at(d.path());
        for (style, ns, leaf) in [("ros2", "std", "Header"), ("swarmbotix", "images", "Image")] {
            let dir = v.defs_dir(style).join(ns);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(format!("{leaf}.proto")), "syntax=\"proto3\";").unwrap();
        }
        let got: Vec<String> = v.list().unwrap().iter().map(|n| n.to_string()).collect();
        assert_eq!(got, vec!["ros2/std/Header", "swarmbotix/images/Image"]);
        assert_eq!(v.styles(), vec!["ros2", "swarmbotix"]);
    }

    #[test]
    fn list_style_scopes_to_one_style() {
        let d = tempdir().unwrap();
        let v = Vault::at(d.path());
        for style in ["ros2", "swarmbotix"] {
            let dir = v.defs_dir(style).join("ns");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("A.proto"), "syntax=\"proto3\";").unwrap();
        }
        let got: Vec<String> = v
            .list_style("ros2")
            .unwrap()
            .iter()
            .map(|n| n.to_string())
            .collect();
        assert_eq!(got, vec!["ros2/ns/A"]);
    }

    #[test]
    fn bundle_installs_into_ros2_and_only_ros2() {
        let d = tempdir().unwrap();
        let v = Vault::at(d.path());
        let rep = v.ensure_installed().unwrap();
        assert!(!rep.copied.is_empty());
        assert!(v.defs_dir("ros2").join("std").join("Header.proto").exists());
        assert!(
            !v.defs_dir("swarmbotix").join("std").exists(),
            "std/ must not leak into another style"
        );
        assert!(v.missing_std_files().is_empty());
    }

    #[test]
    fn path_of_places_a_message_inside_its_own_style() {
        let d = tempdir().unwrap();
        let v = Vault::at(d.path());
        let n = MessageName::parse("swarmbotix/images/Image").unwrap();
        assert_eq!(
            v.path_of(&n),
            d.path()
                .join("swarmbotix")
                .join("message_definitions")
                .join("images")
                .join("Image.proto")
        );
    }
}
