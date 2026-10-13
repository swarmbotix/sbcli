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

    /// Copy a directory of `.proto` files into `style`'s definitions.
    ///
    /// `<dir>/<ns>/<Leaf>.proto` lands at
    /// `<root>/<style>/message_definitions/<ns>/<Leaf>.proto`. A flat
    /// `<dir>/<Leaf>.proto` takes the directory's own name as its
    /// namespace. Hidden entries (`.git/`, ...) and non-`.proto` files are
    /// ignored; the style directory is created when missing.
    ///
    /// Per file: identical content is `unchanged`; different content
    /// already in the vault is a `conflict` and is left alone unless
    /// `force`; everything else is written and `imported`. The forge's
    /// `ros2/std/` is never overwritten, `force` or not.
    ///
    /// Nothing is written when any `.proto` cannot be named
    /// `<style>/<ns>/<Leaf>` (lowercase leaf, illegal characters, nested
    /// deeper than `<ns>/`, or two files mapping to one name); the error
    /// lists every offender.
    pub fn import_style_dir(&self, style: &str, dir: &Path, force: bool) -> Result<ImportReport> {
        if !sb_core::is_valid_message_style(style)
            || !style.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            anyhow::bail!("invalid style {style:?} (allowed: [A-Za-z0-9_])");
        }
        let dir = dir
            .canonicalize()
            .with_context(|| format!("resolving {}", dir.display()))?;
        if !dir.is_dir() {
            anyhow::bail!("{} is not a directory", dir.display());
        }
        let sources = import_sources(style, &dir)?;

        let defs = self.defs_dir(style);
        fs::create_dir_all(&defs).with_context(|| format!("creating {}", defs.display()))?;
        let mut report = ImportReport {
            style: style.to_owned(),
            ..ImportReport::default()
        };
        for (name, src) in sources {
            let label = format!("{}/{}", name.namespace, name.leaf);
            let bytes = fs::read(&src).with_context(|| format!("reading {}", src.display()))?;
            let dest = self.path_of(&name);
            match fs::read(&dest) {
                Ok(existing) if existing == bytes => report.unchanged.push(label),
                Ok(_) if !force || name.is_std() => report.conflicts.push(label),
                _ => {
                    if let Some(parent) = dest.parent() {
                        fs::create_dir_all(parent)
                            .with_context(|| format!("creating {}", parent.display()))?;
                    }
                    fs::write(&dest, &bytes)
                        .with_context(|| format!("writing {}", dest.display()))?;
                    report.imported.push(label);
                }
            }
        }
        Ok(report)
    }
}

/// Every `.proto` under `dir` paired with its vault name, sorted by name.
/// Errors, listing every offender, when any file cannot be named.
fn import_sources(style: &str, dir: &Path) -> Result<Vec<(MessageName, PathBuf)>> {
    let flat_ns = dir.file_name().and_then(|s| s.to_str()).unwrap_or("");
    let mut found: Vec<(MessageName, PathBuf)> = Vec::new();
    let mut offenders: Vec<String> = Vec::new();
    let walker = walkdir::WalkDir::new(dir)
        .min_depth(1)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.'));
    for entry in walker {
        let entry = entry.with_context(|| format!("walking {}", dir.display()))?;
        let path = entry.path();
        if !entry.file_type().is_file()
            || path.extension().and_then(|e| e.to_str()) != Some("proto")
        {
            continue;
        }
        let shown = path.strip_prefix(dir).unwrap_or(path).display().to_string();
        let ns = match entry.depth() {
            1 => flat_ns,
            2 => path
                .parent()
                .and_then(Path::file_name)
                .and_then(|s| s.to_str())
                .unwrap_or(""),
            _ => {
                offenders.push(format!("{shown}: nested deeper than <ns>/<Leaf>.proto"));
                continue;
            }
        };
        let leaf = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        match MessageName::parse(&format!("{style}/{ns}/{leaf}")) {
            Ok(name) => found.push((name, path.to_path_buf())),
            Err(e) => offenders.push(format!("{shown}: {e}")),
        }
    }
    found.sort();
    for pair in found.windows(2) {
        if pair[0].0 == pair[1].0 {
            offenders.push(format!(
                "{} and {} both map to {}",
                pair[0].1.display(),
                pair[1].1.display(),
                pair[0].0
            ));
        }
    }
    if !offenders.is_empty() {
        anyhow::bail!(
            "cannot import {} into style {style:?}; nothing was written:\n  {}",
            dir.display(),
            offenders.join("\n  ")
        );
    }
    Ok(found)
}

#[derive(Debug, Clone, Default)]
pub struct InstallReport {
    pub copied: Vec<PathBuf>,
    pub skipped: Vec<PathBuf>,
}

/// What [`Vault::import_style_dir`] did. Every list holds `<ns>/<Leaf>`
/// names (e.g. `custom/CubePoses`) in sorted order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub style: String,
    /// Written: new to the vault, or replaced under `force`.
    pub imported: Vec<String>,
    /// Already in the vault with identical content.
    pub unchanged: Vec<String>,
    /// In the vault with different content; not written.
    pub conflicts: Vec<String>,
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

    // ── import_style_dir ──────────────────────────────────────────────

    /// `<tmp>/shapes/` with a flat `Flat.proto` (namespace `shapes`) and a
    /// nested `custom/CubePoses.proto`.
    fn import_fixture(root: &Path) -> PathBuf {
        let src = root.join("shapes");
        fs::create_dir_all(src.join("custom")).unwrap();
        fs::create_dir_all(src.join(".git")).unwrap();
        fs::write(src.join("Flat.proto"), "syntax = \"proto3\";\n").unwrap();
        fs::write(src.join("README.md"), "not a proto\n").unwrap();
        fs::write(src.join(".git").join("Hidden.proto"), "x").unwrap();
        fs::write(
            src.join("custom").join("CubePoses.proto"),
            "syntax = \"proto3\";\nmessage CubePoses {}\n",
        )
        .unwrap();
        src
    }

    #[test]
    fn import_style_dir_reports_conflicts_unless_forced() {
        let d = tempdir().unwrap();
        let src = import_fixture(d.path());
        let v = Vault::at(d.path().join("vault"));
        v.import_style_dir("acme", &src, false).unwrap();

        let dest = v.defs_dir("acme").join("shapes").join("Flat.proto");
        fs::write(&dest, "// edited in the vault\n").unwrap();
        let r = v.import_style_dir("acme", &src, false).unwrap();
        assert_eq!(r.conflicts, vec!["shapes/Flat"]);
        assert_eq!(r.unchanged, vec!["custom/CubePoses"]);
        assert_eq!(
            fs::read_to_string(&dest).unwrap(),
            "// edited in the vault\n"
        );

        let r = v.import_style_dir("acme", &src, true).unwrap();
        assert_eq!(r.imported, vec!["shapes/Flat"]);
        assert!(r.conflicts.is_empty());
        assert_eq!(fs::read_to_string(&dest).unwrap(), "syntax = \"proto3\";\n");
    }
}
