//! `sb update`: replace the installed `sb` with another release.
//!
//! [`plan`] decides what to install and refuses early (wrong binary, already
//! at that version) so the CLI can ask before anything is downloaded;
//! [`apply`] downloads, verifies, unpacks and runs the package's installer;
//! [`update`] is the two in a row.

use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow, bail};
use semver::Version;
use sha2::{Digest, Sha256};

use crate::cache;
use crate::fetch::Fetcher;
use crate::notice::NO_CHECK_ENV;
use crate::util::plain_path;
use crate::version;

/// Environment variable that lets `sb update` run from any binary when set
/// to `1`. Tests only: it is how a cargo-built `sb` updates a sandbox.
pub const ALLOW_ANY_EXE_ENV: &str = "SB_UPDATE_ALLOW_ANY_EXE";

/// How many trailing installer output lines an outcome or error carries.
pub const TAIL_LINES: usize = 20;

const BIN_NAME: &str = if cfg!(windows) { "sb.exe" } else { "sb" };
const STALE_BIN_NAME: &str = "sb.exe.old";
const INSTALLER: &str = if cfg!(windows) {
    "install.ps1"
} else {
    "install.sh"
};

/// Options for [`plan`] and [`update`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdateOptions {
    /// Install this release rather than the latest (`sb update --version`).
    pub version: Option<Version>,
    /// Reinstall even when the target is the running version.
    pub force: bool,
}

/// What [`apply`] is about to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdatePlan {
    /// The running version.
    pub from: Version,
    /// The version to install.
    pub to: Version,
    /// `to` is older than `from`. Allowed; the CLI warns.
    pub downgrade: bool,
    /// `to` is the latest release, as opposed to one chosen with
    /// [`UpdateOptions::version`].
    pub to_is_latest: bool,
    /// This build's platform (`linux-x86_64`, ...).
    pub platform: &'static str,
    /// Release asset file name, `swarmbotix-<to>-<platform>.zip`.
    pub asset: String,
    /// Where the asset is downloaded from.
    pub asset_url: String,
}

/// What [`apply`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateOutcome {
    pub from: Version,
    pub to: Version,
    pub downgrade: bool,
    /// The installer's last lines of output (stdout and stderr interleaved),
    /// at most [`TAIL_LINES`].
    pub installer_output_tail: Vec<String>,
}

/// `sb update`: [`plan`], then [`apply`].
pub fn update(home: &Path, fetcher: &dyn Fetcher, opts: &UpdateOptions) -> Result<UpdateOutcome> {
    let plan = plan(home, fetcher, opts)?;
    apply(home, fetcher, &plan)
}

/// Decide what `sb update` would install. Asks `fetcher` for the latest
/// release unless [`UpdateOptions::version`] names one.
///
/// Refuses when the running binary is not `<home>/bin/sb` (a dev build, or
/// a copy elsewhere) unless [`ALLOW_ANY_EXE_ENV`] is `1`, and when the
/// target is the running version without [`UpdateOptions::force`].
pub fn plan(home: &Path, fetcher: &dyn Fetcher, opts: &UpdateOptions) -> Result<UpdatePlan> {
    if std::env::var_os(ALLOW_ANY_EXE_ENV).is_none_or(|v| v != "1") {
        let exe = std::env::current_exe().context("locating the running sb")?;
        check_installed_exe(home, &exe)?;
    }
    plan_from(fetcher, opts, version::installed())
}

fn plan_from(fetcher: &dyn Fetcher, opts: &UpdateOptions, from: Version) -> Result<UpdatePlan> {
    let platform = version::arch()?;
    let (to, to_is_latest) = if let Some(v) = &opts.version {
        (v.clone(), false)
    } else {
        let tag = fetcher
            .latest_tag()
            .context("looking up the latest sb release")?;
        (version::parse_tag(&tag)?, true)
    };
    if to == from && !opts.force {
        bail!("already at {from} (use --force to reinstall)");
    }
    let asset = version::asset_name(&to, platform);
    let asset_url = fetcher.asset_url(&to, &asset);
    Ok(UpdatePlan {
        downgrade: to < from,
        from,
        to,
        to_is_latest,
        platform,
        asset,
        asset_url,
    })
}

/// Install `plan.to` into `home`:
///
/// 1. download the zip and its `.sha256` into a fresh folder in the system
///    temp (never under `home`: the installer refuses to run from there),
/// 2. check the zip against the checksum,
/// 3. unpack it and check for the one `swarmbotix-<to>-<platform>/` folder,
/// 4. run its installer non-interactively with `SB_HOME=<home>`,
/// 5. run the new `<home>/bin/sb --version` and require `sb <to>`,
/// 6. record `to` in the update cache when it is the latest release.
///
/// The temp folder is deleted on return, whether or not the update worked.
pub fn apply(home: &Path, fetcher: &dyn Fetcher, plan: &UpdatePlan) -> Result<UpdateOutcome> {
    let tmp = tempfile::tempdir().context("creating a temporary folder for the download")?;
    ensure_outside(tmp.path(), home)?;

    let zip_path = tmp.path().join(&plan.asset);
    fetcher
        .download(&plan.asset_url, &zip_path)
        .with_context(|| {
            if plan.to_is_latest {
                format!("downloading {}", plan.asset)
            } else {
                format!(
                    "downloading {} (is {} a published release?)",
                    plan.asset, plan.to
                )
            }
        })?;
    let sum_name = format!("{}.sha256", plan.asset);
    let sum_path = tmp.path().join(&sum_name);
    fetcher
        .download(&fetcher.asset_url(&plan.to, &sum_name), &sum_path)
        .with_context(|| format!("downloading {sum_name}"))?;
    verify_sha256(&zip_path, &sum_path)?;

    let unpacked = tmp.path().join("unpacked");
    unzip(&zip_path, &unpacked)?;
    let pkg = package_root(&unpacked, &version::package_stem(&plan.to, plan.platform))?;

    let tail = run_installer(home, &pkg, &tmp.path().join("installer.log"))?;
    verify_installed(home, &plan.to)?;
    refresh_cache(home, plan);

    Ok(UpdateOutcome {
        from: plan.from.clone(),
        to: plan.to.clone(),
        downgrade: plan.downgrade,
        installer_output_tail: tail,
    })
}

/// Delete `<home>/bin/sb.exe.old`, the binary a Windows `sb update` renamed
/// out of the way (a running `.exe` can be renamed but not overwritten).
/// Cheap enough for every start. Errors are ignored: the file stays locked
/// while the process that was updated is still running, and a later start
/// tries again.
pub fn cleanup_stale_binary(home: &Path) {
    let _ = fs::remove_file(stale_binary(home));
}

/// `<home>/bin/sb`, or `sb.exe` on Windows.
fn installed_binary(home: &Path) -> PathBuf {
    home.join("bin").join(BIN_NAME)
}

fn stale_binary(home: &Path) -> PathBuf {
    home.join("bin").join(STALE_BIN_NAME)
}

/// Refuse unless `exe` is the installed `<home>/bin/sb` (symlinks resolved).
fn check_installed_exe(home: &Path, exe: &Path) -> Result<()> {
    let installed = installed_binary(home);
    let exe_real = exe.canonicalize().map(|p| plain_path(&p));
    let installed_real = installed.canonicalize().map(|p| plain_path(&p));
    if let (Ok(a), Ok(b)) = (&exe_real, &installed_real) {
        if a == b {
            return Ok(());
        }
    }
    let shown = exe_real.unwrap_or_else(|_| exe.to_path_buf());
    bail!(
        "this sb is {}, not the installed {}; `sb update` only updates an installed sb \
         (a dev build updates itself with cargo)",
        shown.display(),
        installed.display()
    )
}

/// The installer refuses a package unpacked inside `SB_HOME`; say why up
/// front rather than relay its error.
fn ensure_outside(tmp: &Path, home: &Path) -> Result<()> {
    if let (Ok(t), Ok(h)) = (tmp.canonicalize(), home.canonicalize()) {
        if t.starts_with(&h) {
            bail!(
                "the system temp folder {} is inside {}, where the installer will not run; \
                 point TMPDIR (TEMP on Windows) somewhere else",
                plain_path(&t).display(),
                plain_path(&h).display()
            );
        }
    }
    Ok(())
}

/// Check `file` against the digest in `sidecar`; on a mismatch delete both
/// and fail.
fn verify_sha256(file: &Path, sidecar: &Path) -> Result<()> {
    let text =
        fs::read_to_string(sidecar).with_context(|| format!("reading {}", sidecar.display()))?;
    let expected =
        parse_sidecar(&text).with_context(|| format!("reading {}", sidecar.display()))?;
    let actual = sha256_hex(file)?;
    if actual != expected {
        let _ = fs::remove_file(file);
        let _ = fs::remove_file(sidecar);
        let name = file.file_name().map_or_else(
            || file.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        bail!(
            "sha256 mismatch for {name}: the release says {expected}, the download is {actual}; \
             deleted the download"
        );
    }
    Ok(())
}

/// The digest in a `.sha256` file: its first whitespace-separated token,
/// lowercased. Accepts `sha256sum` output (`<hex>  <name>`), a bare digest,
/// CRLF line ends, a UTF-8 BOM, and `sha256sum`'s `\` escape marker.
pub(crate) fn parse_sidecar(text: &str) -> Result<String> {
    let token = text
        .trim_start_matches('\u{feff}')
        .split_whitespace()
        .next()
        .ok_or_else(|| anyhow!("the checksum file is empty"))?;
    let token = token.strip_prefix('\\').unwrap_or(token);
    if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("the checksum file does not start with a sha256 digest (found `{token}`)");
    }
    Ok(token.to_ascii_lowercase())
}

pub(crate) fn sha256_hex(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut hasher = Sha256::new();
    io::copy(&mut file, &mut hasher).with_context(|| format!("reading {}", path.display()))?;
    let mut hex = String::with_capacity(64);
    for byte in hasher.finalize() {
        let _ = write!(hex, "{byte:02x}");
    }
    Ok(hex)
}

pub(crate) fn unzip(zip_path: &Path, dest: &Path) -> Result<()> {
    let file = File::open(zip_path).with_context(|| format!("opening {}", zip_path.display()))?;
    let mut archive = zip::ZipArchive::new(BufReader::new(file))
        .with_context(|| format!("{} is not a readable zip", zip_path.display()))?;
    archive
        .extract(dest)
        .with_context(|| format!("unpacking {}", zip_path.display()))
}

/// The package folder: the single entry of `unpacked`, named `stem`, and
/// holding the installer for this platform.
fn package_root(unpacked: &Path, stem: &str) -> Result<PathBuf> {
    let mut found: Vec<String> = fs::read_dir(unpacked)
        .with_context(|| format!("listing {}", unpacked.display()))?
        .filter_map(Result::ok)
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.path().is_dir() {
                format!("{name}/")
            } else {
                name
            }
        })
        .collect();
    found.sort();
    let want = format!("{stem}/");
    if found != [want.as_str()] {
        let what = if found.is_empty() {
            "nothing".to_owned()
        } else {
            found.join(", ")
        };
        bail!("unexpected zip layout: expected one top folder {want}, found {what}");
    }
    let root = unpacked.join(stem);
    if !root.join(INSTALLER).is_file() {
        bail!("unexpected zip layout: {want} has no {INSTALLER}");
    }
    Ok(root)
}

/// Run the package's installer from `pkg`, non-interactively, with
/// `SB_HOME=<home>`. Its stdout and stderr go to one file at `log` (so they
/// stay interleaved); the last [`TAIL_LINES`] lines are returned, or carried
/// by the error when it fails.
fn run_installer(home: &Path, pkg: &Path, log: &Path) -> Result<Vec<String>> {
    let out = File::create(log).with_context(|| format!("creating {}", log.display()))?;
    let err = out
        .try_clone()
        .with_context(|| format!("opening {} twice", log.display()))?;
    let mut cmd = installer_command();
    cmd.current_dir(pkg)
        .env("SB_HOME", home)
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err);

    let parked = if cfg!(windows) {
        park_running_binary(home)?
    } else {
        false
    };
    let status = cmd.status().with_context(|| {
        format!(
            "starting {} for {INSTALLER}",
            cmd.get_program().to_string_lossy()
        )
    });
    let tail = tail_lines(&fs::read(log).unwrap_or_default(), TAIL_LINES);
    let failure = match status {
        Ok(s) if s.success() => None,
        Ok(s) => Some(anyhow!(
            "{INSTALLER} failed ({s}); {}",
            describe_tail(&tail)
        )),
        Err(e) => Some(e),
    };
    if let Some(e) = failure {
        if parked {
            unpark_binary(home);
        }
        return Err(e);
    }
    Ok(tail)
}

/// `bash install.sh --yes`, or on Windows
/// `powershell -NoProfile -ExecutionPolicy Bypass -File install.ps1 -Yes`.
fn installer_command() -> Command {
    if cfg!(windows) {
        let mut cmd = Command::new("powershell");
        cmd.args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            INSTALLER,
            "-Yes",
        ]);
        cmd
    } else {
        let mut cmd = Command::new("bash");
        cmd.args([INSTALLER, "--yes"]);
        cmd
    }
}

/// Windows: rename the running `<home>/bin/sb.exe` to `sb.exe.old` so the
/// installer can write a new one. True if there was a binary to move.
fn park_running_binary(home: &Path) -> Result<bool> {
    let bin = installed_binary(home);
    if !bin.exists() {
        return Ok(false);
    }
    let old = stale_binary(home);
    if old.exists() {
        fs::remove_file(&old).with_context(|| {
            format!(
                "removing {} left by an earlier update; close any sb still running from it \
                 and try again",
                old.display()
            )
        })?;
    }
    fs::rename(&bin, &old)
        .with_context(|| format!("renaming {} to {}", bin.display(), old.display()))?;
    Ok(true)
}

/// Undo [`park_running_binary`] after a failed install, unless the
/// installer already put a new binary in place.
fn unpark_binary(home: &Path) {
    let bin = installed_binary(home);
    if !bin.exists() {
        let _ = fs::rename(stale_binary(home), &bin);
    }
}

/// Run the freshly installed binary and require `sb <to>` as its first line.
fn verify_installed(home: &Path, to: &Version) -> Result<()> {
    let bin = installed_binary(home);
    let out = Command::new(&bin)
        .arg("--version")
        .env(NO_CHECK_ENV, "1")
        .env("SB_HOME", home)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("running {} --version", bin.display()))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let first = stdout.lines().next().unwrap_or("").trim();
    let want = format!("sb {to}");
    if first != want {
        let got = if first.is_empty() {
            "nothing".to_owned()
        } else {
            format!("`{first}`")
        };
        bail!(
            "installed binary reports {got}, expected `{want}` ({})",
            bin.display()
        );
    }
    Ok(())
}

/// Record `to` as the latest release when it is known to be: it came from
/// the release feed, or it is newer than what the cache last saw. A pinned
/// older version says nothing about the latest one and leaves the cache be.
fn refresh_cache(home: &Path, plan: &UpdatePlan) {
    let newer = cache::read(home)
        .and_then(|c| c.latest_version())
        .is_some_and(|seen| plan.to > seen);
    if plan.to_is_latest || newer {
        let _ = cache::write(home, &plan.to);
    }
}

/// The last `n` lines of `bytes`, decoded leniently, trailing blanks dropped.
fn tail_lines(bytes: &[u8], n: usize) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<&str> = text.lines().collect();
    let end = lines
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .map_or(0, |i| i + 1);
    let start = end.saturating_sub(n);
    lines[start..end]
        .iter()
        .map(|l| l.trim_end().to_owned())
        .collect()
}

fn describe_tail(tail: &[String]) -> String {
    if tail.is_empty() {
        return "it printed nothing".to_owned();
    }
    let mut out = String::from("its last lines:");
    for line in tail {
        out.push_str("\n  ");
        out.push_str(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::DirFetcher;
    use std::io::Write;
    use tempfile::TempDir;
    use zip::write::SimpleFileOptions;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    /// A version newer than this build, so an update to it is a real one.
    #[cfg(unix)]
    fn next_version() -> Version {
        let mut next = version::installed();
        next.patch += 1;
        next.pre = semver::Prerelease::EMPTY;
        next
    }

    fn fake_sb(reports: &str) -> String {
        format!(
            "#!/usr/bin/env bash\n\
             if [ \"${{1:-}}\" = \"--version\" ]; then echo \"sb {reports}\"; exit 0; fi\n\
             echo \"fake sb: $*\" >&2\nexit 1\n"
        )
    }

    /// Copies `bin/sb` and `VERSION` into `$SB_HOME`, like the real one.
    const FIXTURE_INSTALLER: &str = r#"#!/usr/bin/env bash
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
[ "${1:-}" = "--yes" ] || { echo "fixture installer: run with --yes" >&2; exit 2; }
: "${SB_HOME:?SB_HOME must be set}"
mkdir -p "$SB_HOME/bin"
cp "$here/bin/sb" "$SB_HOME/bin/sb"
chmod 0755 "$SB_HOME/bin/sb"
cp "$here/VERSION" "$SB_HOME/VERSION"
echo "fixture installed $(basename "$here") into $SB_HOME"
"#;

    /// Write a release of `ver` into `dir`, as a `DirFetcher` serves it:
    /// the zip, its `.sha256`, and `latest.txt`. The zip's `bin/sb` prints
    /// `sb <reports>` for `--version`.
    fn write_release(dir: &Path, ver: &Version, reports: &str, installer: &str) -> PathBuf {
        let platform = version::arch().unwrap();
        let stem = version::package_stem(ver, platform);
        let asset = version::asset_name(ver, platform);
        let zip_path = dir.join(&asset);
        let mut zw = zip::ZipWriter::new(File::create(&zip_path).unwrap());
        let file = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o644);
        let exe = file.unix_permissions(0o755);
        for d in ["", "bin/", "documents/", "messages/"] {
            zw.add_directory(format!("{stem}/{d}"), file).unwrap();
        }
        let entries = [
            ("bin/sb", fake_sb(reports), exe),
            ("documents/sbcli.md", "# sb\n".to_owned(), file),
            (
                "VERSION",
                format!("version:    {ver}\nplatform:   {platform}\n"),
                file,
            ),
            // Named for this platform so the layout check passes everywhere;
            // only Unix tests ever run it.
            (INSTALLER, installer.to_owned(), exe),
        ];
        for (name, body, opts) in entries {
            zw.start_file(format!("{stem}/{name}"), opts).unwrap();
            zw.write_all(body.as_bytes()).unwrap();
        }
        zw.finish().unwrap();
        let digest = sha256_hex(&zip_path).unwrap();
        fs::write(
            dir.join(format!("{asset}.sha256")),
            format!("{digest}  {asset}\n"),
        )
        .unwrap();
        fs::write(dir.join("latest.txt"), format!("{ver}\n")).unwrap();
        zip_path
    }

    fn dir_fetcher(dir: &Path) -> DirFetcher {
        DirFetcher::open(dir.to_path_buf()).unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn wrong_binary_version_is_caught() {
        let rel = TempDir::new().unwrap();
        let home = TempDir::new().unwrap();
        let to = next_version();
        write_release(rel.path(), &to, "0.0.1", FIXTURE_INSTALLER);
        let f = dir_fetcher(rel.path());
        let plan = plan_from(&f, &UpdateOptions::default(), version::installed()).unwrap();

        let err = apply(home.path(), &f, &plan).unwrap_err().to_string();
        assert!(
            err.starts_with(&format!(
                "installed binary reports `sb 0.0.1`, expected `sb {to}`"
            )),
            "{err}"
        );
    }

    #[test]
    fn corrupt_download_is_rejected() {
        let rel = TempDir::new().unwrap();
        let home = TempDir::new().unwrap();
        let to = v("0.2.3");
        let zip_path = write_release(rel.path(), &to, "0.2.3", FIXTURE_INSTALLER);
        fs::OpenOptions::new()
            .append(true)
            .open(&zip_path)
            .unwrap()
            .write_all(b"tampered")
            .unwrap();
        let f = dir_fetcher(rel.path());
        let plan = plan_from(&f, &UpdateOptions::default(), v("0.2.0")).unwrap();
        let err = apply(home.path(), &f, &plan).unwrap_err().to_string();
        assert!(err.contains("sha256 mismatch"), "{err}");
        assert!(!installed_binary(home.path()).exists());
    }
}
