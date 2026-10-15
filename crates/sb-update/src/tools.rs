//! `sb tools install`: the pinned `protoc` and `flatc`.
//!
//! `sb message compile` runs `protoc` and `flatc`. Their versions are pinned
//! here (and declared in `versions.json`), each with one prebuilt release zip
//! per platform and that zip's sha256. [`plan`] looks at what sits under a
//! prefix; [`install`] downloads, verifies and unpacks whatever is missing or
//! at another version into a fixed layout:
//!
//! ```text
//! <prefix>/                    /opt on Linux, C:/cmake_ext_libs on Windows
//!   protobuf/protoc-35.0-linux-x86_64/bin/protoc    Windows: protoc/35.0/bin/protoc.exe
//!   flatbuffers/flatc-25.12.19/flatc                Windows: flatbuffers/25.12.19/flatc.exe
//! ```
//!
//! Nothing here escalates privileges. When `<prefix>/protobuf` or
//! `<prefix>/flatbuffers` cannot be written, [`unwritable_parents`] names
//! them and [`privilege_hint`] renders the one command that fixes it, for the
//! user to run.

use std::env::{self, VarError};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow, bail};

use crate::fetch::{Fetcher as _, GithubFetcher, dir_from_file_url};
use crate::update::{parse_sidecar, sha256_hex, unzip};
use crate::version;

/// The protoc version `sb tools install` installs; `versions.json` declares the same.
pub const PROTOC_VERSION: &str = "35.0";

/// The flatc version `sb tools install` installs; `versions.json` declares the same.
pub const FLATC_VERSION: &str = "25.12.19";

/// Environment variable that points `sb tools install` at a local folder.
///
/// `SB_TOOLS_BASE=file:///<dir>`: assets are copied from `<dir>/<asset>` instead
/// of downloaded, and when `<dir>/<asset>.sha256` exists its first token is the
/// expected digest instead of the pinned one (offline tests only). Any other
/// non-empty value is an error. Unset/empty: download the pinned URLs.
pub const BASE_ENV: &str = "SB_TOOLS_BASE";

const PROTOC_SOURCE: &str = "https://github.com/protocolbuffers/protobuf";
const FLATC_SOURCE: &str = "https://github.com/google/flatbuffers";

/// A code generator `sb tools install` manages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    /// The Protocol Buffers compiler, `protoc`.
    Protoc,
    /// The flatbuffers compiler, `flatc`.
    Flatc,
}

impl Tool {
    /// Every tool, in install order.
    pub const ALL: [Tool; 2] = [Tool::Protoc, Tool::Flatc];

    /// The tool's name, which is also its executable's stem: `protoc` or `flatc`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Protoc => "protoc",
            Self::Flatc => "flatc",
        }
    }

    /// The pinned version: [`PROTOC_VERSION`] or [`FLATC_VERSION`].
    pub fn version(self) -> &'static str {
        match self {
            Self::Protoc => PROTOC_VERSION,
            Self::Flatc => FLATC_VERSION,
        }
    }

    /// The tool called `s`, ignoring case and surrounding blanks.
    pub fn parse(s: &str) -> Option<Tool> {
        let s = s.trim();
        Self::ALL
            .into_iter()
            .find(|t| t.name().eq_ignore_ascii_case(s))
    }

    /// The flag that prints the version on the first stdout line: `--version`.
    pub fn version_flag(self) -> &'static str {
        match self {
            Self::Protoc | Self::Flatc => "--version",
        }
    }

    /// Where to build the tool from when no prebuilt asset exists.
    fn source_url(self) -> &'static str {
        match self {
            Self::Protoc => PROTOC_SOURCE,
            Self::Flatc => FLATC_SOURCE,
        }
    }
}

/// One prebuilt release zip of a tool, and where it goes under the prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSpec {
    /// The tool this zip holds.
    pub tool: Tool,
    /// Release asset file name, e.g. `protoc-35.0-linux-x86_64.zip`.
    pub asset: &'static str,
    /// Where the asset is downloaded from.
    pub url: &'static str,
    /// The asset's sha256, lowercase hex.
    pub sha256: &'static str,
    /// Install folder, relative to the prefix, with `/` separators.
    pub dir: &'static str,
    /// The executable, relative to `dir`, with `/` separators.
    pub exe: &'static str,
}

/// Every prebuilt asset, keyed by the platform name [`version::arch`] gives.
/// Keep in step with `versions.json` and the version consts (`/sb-deps`).
const ASSETS: &[(&str, ToolSpec)] = &[
    (
        "linux-x86_64",
        ToolSpec {
            tool: Tool::Protoc,
            asset: "protoc-35.0-linux-x86_64.zip",
            url: "https://github.com/protocolbuffers/protobuf/releases/download/v35.0/protoc-35.0-linux-x86_64.zip",
            sha256: "a45cda0989c17dd950db55f6fbe1e5814c50fda08e87aa422980ac1f89dddbbc",
            dir: "protobuf/protoc-35.0-linux-x86_64",
            exe: "bin/protoc",
        },
    ),
    (
        "linux-aarch64",
        ToolSpec {
            tool: Tool::Protoc,
            asset: "protoc-35.0-linux-aarch_64.zip",
            url: "https://github.com/protocolbuffers/protobuf/releases/download/v35.0/protoc-35.0-linux-aarch_64.zip",
            sha256: "36b518ac14d90351cc6598228ed2bbe5afe4e357b1af470b07e0ec1609875de2",
            dir: "protobuf/protoc-35.0-linux-aarch_64",
            exe: "bin/protoc",
        },
    ),
    (
        "windows-x86_64",
        ToolSpec {
            tool: Tool::Protoc,
            asset: "protoc-35.0-win64.zip",
            url: "https://github.com/protocolbuffers/protobuf/releases/download/v35.0/protoc-35.0-win64.zip",
            sha256: "d1cede9e308cc3eb072392af1c02ccae4bdd3d2f374ec2970dbd8cdfdaa91363",
            dir: "protoc/35.0",
            exe: "bin/protoc.exe",
        },
    ),
    (
        "linux-x86_64",
        ToolSpec {
            tool: Tool::Flatc,
            asset: "Linux.flatc.binary.g++-13.zip",
            url: "https://github.com/google/flatbuffers/releases/download/v25.12.19/Linux.flatc.binary.g++-13.zip",
            sha256: "9f87066dc5dfa7fe02090b55bab5f3e55df03e32c9b0cdf229004ade7d091039",
            dir: "flatbuffers/flatc-25.12.19",
            exe: "flatc",
        },
    ),
    (
        "windows-x86_64",
        ToolSpec {
            tool: Tool::Flatc,
            asset: "Windows.flatc.binary.zip",
            url: "https://github.com/google/flatbuffers/releases/download/v25.12.19/Windows.flatc.binary.zip",
            sha256: "fff9445c9db907227bc64b54cc98743084c4949282aa4e576cff6a955724ddc8",
            dir: "flatbuffers/25.12.19",
            exe: "flatc.exe",
        },
    ),
];

/// The asset of `tool` for this build's platform, or why there is none.
///
/// The platform is this binary's own target, fixed at compile time like
/// [`version::arch`]. For flatc on `linux-aarch64` the reason reads
/// `no prebuilt flatc 25.12.19 for linux-aarch64; build it from source: https://github.com/google/flatbuffers`.
pub fn spec(tool: Tool) -> Result<ToolSpec, String> {
    let platform = version::arch().map_or_else(
        |_| format!("{}-{}", env::consts::OS, env::consts::ARCH),
        str::to_owned,
    );
    ASSETS
        .iter()
        .find(|(p, s)| *p == platform && s.tool == tool)
        .map(|(_, s)| s.clone())
        .ok_or_else(|| {
            format!(
                "no prebuilt {} {} for {platform}; build it from source: {}",
                tool.name(),
                tool.version(),
                tool.source_url()
            )
        })
}

/// Where tools go without `--prefix`: `/opt` on Unix, `C:/cmake_ext_libs` on Windows.
pub fn default_prefix() -> PathBuf {
    PathBuf::from(if cfg!(windows) {
        "C:/cmake_ext_libs"
    } else {
        "/opt"
    })
}

/// What sits at a tool's install path now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolState {
    /// The exe runs, and the first line of its `--version` contains the pinned version.
    Installed,
    /// There is no exe at the install path.
    Missing,
    /// The exe reports another version, or `--version` failed; carries that line or the error.
    Stale(String),
}

/// One tool [`install`] would put in place.
#[derive(Debug, Clone)]
pub struct ToolPlan {
    /// The asset to install.
    pub spec: ToolSpec,
    /// Install folder: the prefix joined with [`ToolSpec::dir`].
    pub dest: PathBuf,
    /// The executable: `dest` joined with [`ToolSpec::exe`].
    pub exe: PathBuf,
    /// What is at `exe` now.
    pub state: ToolState,
}

/// What `sb tools install` would do under one prefix.
#[derive(Debug, Clone)]
pub struct InstallPlan {
    /// The install prefix, absolute.
    pub prefix: PathBuf,
    /// Tools with an asset on this platform, in [`Tool::ALL`] order, filtered by `only`.
    pub tools: Vec<ToolPlan>,
    /// Tools with no asset on this platform, and why.
    pub unavailable: Vec<(Tool, String)>,
}

/// A tool [`install`] put in place.
#[derive(Debug, Clone)]
pub struct Installed {
    /// The tool.
    pub tool: Tool,
    /// Its executable, as written to sb.config.yml.
    pub exe: PathBuf,
    /// The first line of its `--version`.
    pub version_line: String,
}

/// Plan an install under `prefix`: every tool of this platform (only `only`,
/// when given) with its install path and current [`ToolState`]. A relative
/// `prefix` is made absolute, since the paths end up in sb.config.yml.
pub fn plan(prefix: &Path, only: Option<Tool>) -> Result<InstallPlan> {
    let prefix = if prefix.is_absolute() {
        prefix.to_path_buf()
    } else {
        std::path::absolute(prefix)
            .with_context(|| format!("resolving the prefix {}", prefix.display()))?
    };
    let mut tools = Vec::new();
    let mut unavailable = Vec::new();
    let wanted = Tool::ALL
        .into_iter()
        .filter(|t| only.is_none_or(|o| o == *t));
    for tool in wanted {
        match spec(tool) {
            Ok(spec) => {
                let dest = join_rel(&prefix, spec.dir);
                let exe = join_rel(&dest, spec.exe);
                let state = probe(tool, &exe);
                tools.push(ToolPlan {
                    spec,
                    dest,
                    exe,
                    state,
                });
            }
            Err(reason) => unavailable.push((tool, reason)),
        }
    }
    Ok(InstallPlan {
        prefix,
        tools,
        unavailable,
    })
}

/// Parents (`prefix/<first component of dir>`) this process cannot write to,
/// for tools whose state is not [`ToolState::Installed`]. Dedup, tool order.
///
/// A parent that does not exist yet is judged by its nearest existing
/// ancestor: it counts as writable when that ancestor is (because [`install`]
/// creates it with `create_dir_all`), and otherwise the parent itself, not the
/// ancestor, is reported. So `--prefix ~/tools` on a fresh folder needs no sudo.
pub fn unwritable_parents(plan: &InstallPlan) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let pending = plan
        .tools
        .iter()
        .filter(|t| t.state != ToolState::Installed);
    for t in pending {
        let first = t.spec.dir.split('/').next().unwrap_or(t.spec.dir);
        let parent = join_rel(&plan.prefix, first);
        if !out.contains(&parent) && !can_create_in(&parent) {
            out.push(parent);
        }
    }
    out
}

/// The command that makes `dirs` writable, for the user to run once: on Unix
/// `sudo mkdir -p <dirs> && sudo chown "$USER" <dirs>`, on Windows one
/// `mkdir "<dir>"` line per folder. Empty for no folders.
pub fn privilege_hint(dirs: &[PathBuf]) -> String {
    if cfg!(windows) {
        return dirs
            .iter()
            .map(|d| format!("mkdir \"{}\"", d.display()))
            .collect::<Vec<_>>()
            .join("\n");
    }
    if dirs.is_empty() {
        return String::new();
    }
    let list = dirs
        .iter()
        .map(|d| sh_quote(&d.display().to_string()))
        .collect::<Vec<_>>()
        .join(" ");
    format!("sudo mkdir -p {list} && sudo chown \"$USER\" {list}")
}

/// Install every tool whose state is Missing or Stale.
///
/// `progress` receives one line per step, e.g. `downloading <url> ...`, `verifying sha256`,
/// `unpacking into <dest>`. Steps per tool: tempdir → fetch (per [`BASE_ENV`])
/// → sha256 compare → unzip into `<tmp>/unpacked` → chmod 0755 the exe (unix)
/// → `create_dir_all(dest.parent)` → if dest exists, `remove_dir_all(dest)` →
/// move `<tmp>/unpacked` to dest (`fs::rename`, falling back to a recursive copy
/// when rename fails, because /tmp and /opt are usually different
/// filesystems) → run `exe --version`, fail unless the first line contains
/// the pinned version. Sha mismatch: delete the download and fail with
/// `sha256 mismatch for <asset>: pinned <hex>, download is <hex>`.
pub fn install(plan: &InstallPlan, progress: &mut dyn FnMut(&str)) -> Result<Vec<Installed>> {
    let local = local_dir()?;
    let mut done = Vec::new();
    let pending = plan
        .tools
        .iter()
        .filter(|t| t.state != ToolState::Installed);
    for t in pending {
        done.push(install_one(t, local.as_deref(), progress)?);
    }
    Ok(done)
}

/// The steps of [`install`] for one tool.
fn install_one(
    t: &ToolPlan,
    local: Option<&Path>,
    progress: &mut dyn FnMut(&str),
) -> Result<Installed> {
    let spec = &t.spec;
    let tool = spec.tool;
    let tmp = tempfile::tempdir().context("creating a temporary folder for the download")?;
    let zip_path = tmp.path().join(spec.asset);
    let pinned = fetch(spec, local, &zip_path, progress)?;

    progress("verifying sha256");
    let actual = sha256_hex(&zip_path)?;
    if actual != pinned {
        let _ = fs::remove_file(&zip_path);
        bail!(
            "sha256 mismatch for {}: pinned {pinned}, download is {actual}",
            spec.asset
        );
    }

    progress(&format!("unpacking into {}", t.dest.display()));
    let unpacked = tmp.path().join("unpacked");
    unzip(&zip_path, &unpacked)?;
    let staged_exe = join_rel(&unpacked, spec.exe);
    if !staged_exe.is_file() {
        bail!("unexpected zip layout: {} has no {}", spec.asset, spec.exe);
    }
    #[cfg(unix)]
    make_executable(&staged_exe)?;
    if let Some(parent) = t.dest.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    if t.dest.exists() {
        fs::remove_dir_all(&t.dest)
            .with_context(|| format!("removing the old {}", t.dest.display()))?;
    }
    move_dir(&unpacked, &t.dest)?;
    #[cfg(unix)]
    make_executable(&t.exe)?;

    let line = version_line(tool, &t.exe).map_err(|e| anyhow!("{}: {e}", t.exe.display()))?;
    if !line.contains(tool.version()) {
        bail!(
            "{} {} printed `{line}`, expected {} {}",
            t.exe.display(),
            tool.version_flag(),
            tool.name(),
            tool.version()
        );
    }
    Ok(Installed {
        tool,
        exe: t.exe.clone(),
        version_line: line,
    })
}

/// The folder [`BASE_ENV`] names, or `None` (unset or empty) to download.
fn local_dir() -> Result<Option<PathBuf>> {
    let base = match env::var(BASE_ENV) {
        Ok(v) => v,
        Err(VarError::NotPresent) => return Ok(None),
        Err(VarError::NotUnicode(v)) => {
            bail!("{BASE_ENV}={} is not valid UTF-8", v.to_string_lossy())
        }
    };
    let base = base.trim();
    if base.is_empty() {
        return Ok(None);
    }
    dir_from_file_url(base)
        .filter(|d| !d.as_os_str().is_empty())
        .map(Some)
        .ok_or_else(|| {
            anyhow!(
                "{BASE_ENV}={base}: expected file:///<dir> (offline tests only); \
                 unset it to download the pinned release zips"
            )
        })
}

/// Put the asset of `spec` at `dest`, copied from `local` when set, else
/// downloaded. Returns the digest it must have: the pinned one, or in
/// `local` mode the one in `<local>/<asset>.sha256` when that file exists.
fn fetch(
    spec: &ToolSpec,
    local: Option<&Path>,
    dest: &Path,
    progress: &mut dyn FnMut(&str),
) -> Result<String> {
    let Some(dir) = local else {
        progress(&format!("downloading {} ...", spec.url));
        GithubFetcher::default()
            .download(spec.url, dest)
            .with_context(|| format!("downloading {}", spec.asset))?;
        return Ok(spec.sha256.to_owned());
    };
    let src = dir.join(spec.asset);
    progress(&format!("copying {} ...", src.display()));
    fs::copy(&src, dest)
        .with_context(|| format!("copying {} to {}", src.display(), dest.display()))?;
    let sidecar = dir.join(format!("{}.sha256", spec.asset));
    if !sidecar.is_file() {
        return Ok(spec.sha256.to_owned());
    }
    let text =
        fs::read_to_string(&sidecar).with_context(|| format!("reading {}", sidecar.display()))?;
    parse_sidecar(&text).with_context(|| format!("reading {}", sidecar.display()))
}

/// The [`ToolState`] of the executable at `exe`.
fn probe(tool: Tool, exe: &Path) -> ToolState {
    if !exe.exists() {
        return ToolState::Missing;
    }
    match version_line(tool, exe) {
        Ok(line) if line.contains(tool.version()) => ToolState::Installed,
        Ok(line) if line.is_empty() => {
            ToolState::Stale(format!("`{}` printed nothing", tool.version_flag()))
        }
        Ok(line) | Err(line) => ToolState::Stale(line),
    }
}

/// The first stdout line of `<exe> --version`, trimmed; or why it failed.
fn version_line(tool: Tool, exe: &Path) -> Result<String, String> {
    let flag = tool.version_flag();
    let out = Command::new(exe)
        .arg(flag)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("`{flag}` failed to run: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let first = stderr.lines().next().unwrap_or("").trim();
        return Err(if first.is_empty() {
            format!("`{flag}` failed ({})", out.status)
        } else {
            format!("`{flag}` failed ({}): {first}", out.status)
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_owned())
}

/// Whether this process can create entries in `dir`, or create `dir` itself
/// when it does not exist yet. Tested by creating an unnamed temp file in
/// `dir` or its nearest existing ancestor, since permission bits alone miss
/// ownership, ACLs and read-only mounts.
fn can_create_in(dir: &Path) -> bool {
    dir.ancestors()
        .find(|a| a.exists())
        .is_some_and(|a| a.is_dir() && tempfile::tempfile_in(a).is_ok())
}

/// `base` joined with `rel`, a relative path with `/` separators. On Windows
/// a `base` spelled with `/` only (the default `C:/cmake_ext_libs`) keeps `/`
/// throughout, so the path written to sb.config.yml reads as one.
fn join_rel(base: &Path, rel: &str) -> PathBuf {
    if cfg!(windows) {
        if let Some(b) = base.to_str().filter(|b| !b.contains('\\')) {
            return PathBuf::from(format!("{}/{rel}", b.trim_end_matches('/')));
        }
    }
    let mut out = base.to_path_buf();
    out.extend(rel.split('/'));
    out
}

/// `s` as one POSIX shell word: as is when it needs no quoting, else in
/// single quotes.
fn sh_quote(s: &str) -> String {
    let plain = !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/._-+,:@%=".contains(&b));
    if plain {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// Move the folder `from` to `to`, which must not exist: a rename, or, when
/// that fails (the system temp folder and /opt are usually on different
/// filesystems), a recursive copy after which `from` is removed. A failed
/// copy removes the partial `to`.
fn move_dir(from: &Path, to: &Path) -> Result<()> {
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    fs::create_dir(to).with_context(|| format!("creating {}", to.display()))?;
    if let Err(e) = copy_tree(from, to) {
        let _ = fs::remove_dir_all(to);
        return Err(e.context(format!("moving {} to {}", from.display(), to.display())));
    }
    let _ = fs::remove_dir_all(from);
    Ok(())
}

/// Copy everything in the folder `from` into the existing folder `to`. On
/// Unix, `fs::copy` carries each file's permission bits along.
fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    let entries = fs::read_dir(from).with_context(|| format!("listing {}", from.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("listing {}", from.display()))?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        let kind = entry
            .file_type()
            .with_context(|| format!("reading {}", src.display()))?;
        if kind.is_dir() {
            fs::create_dir(&dst).with_context(|| format!("creating {}", dst.display()))?;
            copy_tree(&src, &dst)?;
        } else if kind.is_file() {
            fs::copy(&src, &dst)
                .with_context(|| format!("copying {} to {}", src.display(), dst.display()))?;
        } else {
            bail!("unexpected entry {} (not a file or folder)", src.display());
        }
    }
    Ok(())
}

/// Set `path` to mode 0755: zips carry no reliable exec bit.
#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .with_context(|| format!("making {} executable", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nearest ancestor of `from` (inclusive) holding a file called `name`,
    /// as in `crates/sb-doctor/build.rs`.
    fn find_up(from: &Path, name: &str) -> Option<PathBuf> {
        from.ancestors().map(|d| d.join(name)).find(|p| p.is_file())
    }

    /// The value of `"<want>"` in the flat `versions.json`, scanned like
    /// `crates/sb-doctor/build.rs` does.
    fn declared_version(json: &str, want: &str) -> Option<String> {
        let key = format!("\"{want}\"");
        let rest = json.split_once(&key)?.1;
        let rest = rest.trim_start().strip_prefix(':')?.trim_start();
        let rest = rest.strip_prefix('"')?;
        let (v, _) = rest.split_once('"')?;
        Some(v.to_owned())
    }

    #[test]
    fn pins_match_versions_json() {
        let path = find_up(Path::new(env!("CARGO_MANIFEST_DIR")), "versions.json")
            .expect("versions.json in or above the crate");
        let json = fs::read_to_string(&path).unwrap();
        for (name, pinned) in [("protoc", PROTOC_VERSION), ("flatc", FLATC_VERSION)] {
            assert_eq!(
                declared_version(&json, name).as_deref(),
                Some(pinned),
                "{} and sb_update::tools disagree on {name}; run /sb-deps",
                path.display()
            );
        }
        // The asset table spells each version out; it must follow the consts.
        for (platform, s) in ASSETS {
            let v = s.tool.version();
            assert!(
                s.url.ends_with(&format!("/v{v}/{}", s.asset)),
                "{platform} {}: {} is not release v{v}",
                s.tool.name(),
                s.url
            );
            assert!(
                s.dir.contains(v),
                "{platform} {}: {} lacks {v}",
                s.tool.name(),
                s.dir
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn privilege_hint_is_the_documented_line() {
        let dirs = [
            PathBuf::from("/opt/protobuf"),
            PathBuf::from("/opt/flatbuffers"),
        ];
        assert_eq!(
            privilege_hint(&dirs),
            r#"sudo mkdir -p /opt/protobuf /opt/flatbuffers && sudo chown "$USER" /opt/protobuf /opt/flatbuffers"#
        );
    }
}
