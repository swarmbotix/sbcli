//! `sb update` and the `sb --version` notice end to end through the real
//! binary, with no network: `SB_RELEASE_BASE=file:///<dir>` serves a release
//! zip built here (a bash `bin/sb` that only knows `--version`, plus an
//! `install.sh` that copies it into `$SB_HOME`), and `SB_UPDATE_ALLOW_ANY_EXE=1`
//! lets the cargo-built `sb` update the sandbox's `<sb_home>`.
//!
//! Unix only: the fixture installer is a bash script, and Windows runs
//! `install.ps1`. Each test returns early on Windows.

mod common;
use common::Sandbox;

use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;

const INSTALLED: &str = env!("CARGO_PKG_VERSION");

/// Copies `bin/sb` and `VERSION` into `$SB_HOME`, like the real installer.
const FIXTURE_INSTALLER: &str = r#"#!/usr/bin/env bash
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
[ "${1:-}" = "--yes" ] || { echo "fixture installer: run with --yes" >&2; exit 2; }
: "${SB_HOME:?SB_HOME must be set}"
mkdir -p "$SB_HOME/bin"
cp "$here/bin/sb" "$SB_HOME/bin/sb"
chmod 0755 "$SB_HOME/bin/sb"
cp "$here/VERSION" "$SB_HOME/VERSION"
echo "fixture installed into $SB_HOME"
"#;

fn skip_on_windows() -> bool {
    if cfg!(windows) {
        eprintln!("skipping: the fixture installer is bash; Windows runs install.ps1");
    }
    cfg!(windows)
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn describe(out: &Output) -> String {
    format!(
        "status {:?}\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        text(&out.stdout),
        text(&out.stderr)
    )
}

/// `INSTALLED` with the patch number bumped: always newer than this build.
fn next_version() -> String {
    let core = INSTALLED.split(['-', '+']).next().unwrap();
    let parts: Vec<u64> = core.split('.').map(|p| p.parse().unwrap()).collect();
    format!("{}.{}.{}", parts[0], parts[1], parts[2] + 1)
}

fn platform() -> &'static str {
    sb_update::arch().expect("tests run on a released platform")
}

/// A folder of releases as `SB_RELEASE_BASE=file:///<dir>` serves them.
struct Releases {
    dir: tempfile::TempDir,
}

impl Releases {
    fn new() -> Self {
        Self {
            dir: tempfile::TempDir::new().unwrap(),
        }
    }

    fn base(&self) -> String {
        format!("file://{}", self.dir.path().display())
    }

    fn set_latest(&self, version: &str) {
        fs::write(self.dir.path().join("latest.txt"), format!("{version}\n")).unwrap();
    }

    fn asset_url(&self, version: &str) -> String {
        format!("{}/swarmbotix-{version}-{}.zip", self.base(), platform())
    }

    /// Add release `version`: the zip and its `.sha256`, laid out like a
    /// real one (one top folder with bin/, documents/, VERSION, install.sh).
    fn publish(&self, version: &str) -> PathBuf {
        let stem = format!("swarmbotix-{version}-{}", platform());
        let asset = format!("{stem}.zip");
        let zip_path = self.dir.path().join(&asset);
        let mut zw = zip::ZipWriter::new(File::create(&zip_path).unwrap());
        let file = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o644);
        let exe = file.unix_permissions(0o755);
        for d in ["", "bin/", "documents/", "messages/"] {
            zw.add_directory(format!("{stem}/{d}"), file).unwrap();
        }
        let fake_sb = format!(
            "#!/usr/bin/env bash\n\
             if [ \"${{1:-}}\" = \"--version\" ]; then echo \"sb {version}\"; exit 0; fi\n\
             exit 1\n"
        );
        let version_file = format!("version:    {version}\nplatform:   {}\n", platform());
        let entries = [
            ("bin/sb", fake_sb.as_str(), exe),
            ("documents/sbcli.md", "# sb\n", file),
            ("VERSION", version_file.as_str(), file),
            ("install.sh", FIXTURE_INSTALLER, exe),
        ];
        for (name, body, opts) in entries {
            zw.start_file(format!("{stem}/{name}"), opts).unwrap();
            zw.write_all(body.as_bytes()).unwrap();
        }
        zw.finish().unwrap();

        let mut hex = String::with_capacity(64);
        for byte in Sha256::digest(fs::read(&zip_path).unwrap()) {
            write!(hex, "{byte:02x}").unwrap();
        }
        fs::write(
            self.dir.path().join(format!("{asset}.sha256")),
            format!("{hex}  {asset}\n"),
        )
        .unwrap();
        zip_path
    }
}

/// `sb` in the sandbox, with every update-related variable from the
/// developer's shell removed so each test states exactly what it sets.
fn sb(s: &Sandbox) -> Command {
    let mut c = s.cmd();
    c.env_remove("SB_RELEASE_BASE")
        .env_remove("SB_NO_UPDATE_CHECK")
        .env_remove("SB_UPDATE_ALLOW_ANY_EXE")
        .stdin(Stdio::null());
    c
}

/// `sb` pointed at `rel`, allowed to update the sandbox.
fn sb_offline(s: &Sandbox, rel: &Releases) -> Command {
    let mut c = sb(s);
    c.env("SB_RELEASE_BASE", rel.base())
        .env("SB_UPDATE_ALLOW_ANY_EXE", "1");
    c
}

fn write_cache(sb_home: &Path, latest: &str, checked_at_unix: u64) {
    fs::create_dir_all(sb_home).unwrap();
    fs::write(
        sb_home.join("update-check.json"),
        format!(
            r#"{{"latest":"{latest}","checked_at":"(test)","checked_at_unix":{checked_at_unix}}}"#
        ),
    )
    .unwrap();
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[test]
fn check_prints_five_lines_and_exits_10_when_behind() {
    if skip_on_windows() {
        return;
    }
    let s = Sandbox::new();
    let rel = Releases::new();
    let next = next_version();
    rel.publish(&next);
    rel.set_latest(&next);

    let out = sb_offline(&s, &rel)
        .args(["update", "--check"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(10), "{}", describe(&out));
    let expected = [
        format!("installed : {INSTALLED}"),
        format!("latest    : {next}"),
        format!("platform  : {}", platform()),
        format!("asset     : {}", rel.asset_url(&next)),
        "status    : update available   run `sb update`".to_owned(),
    ];
    assert_eq!(
        text(&out.stdout).lines().collect::<Vec<_>>(),
        expected,
        "{}",
        describe(&out)
    );

    rel.set_latest(INSTALLED);
    let out = sb_offline(&s, &rel)
        .args(["update", "--check"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    assert!(
        text(&out.stdout).contains("status    : up to date\n"),
        "{}",
        describe(&out)
    );

    rel.set_latest("0.0.1");
    let out = sb_offline(&s, &rel)
        .args(["update", "--check"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    assert!(
        text(&out.stdout).contains("status    : ahead"),
        "{}",
        describe(&out)
    );
}

#[test]
fn update_installs_the_release_into_sb_home() {
    if skip_on_windows() {
        return;
    }
    let s = Sandbox::new();
    let rel = Releases::new();
    let next = next_version();
    rel.publish(&next);
    rel.set_latest(&next);

    // No terminal and no --yes: refused before anything is downloaded.
    let out = sb_offline(&s, &rel).arg("update").output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert!(
        text(&out.stderr).contains("not a terminal; pass --yes"),
        "{}",
        describe(&out)
    );

    let out = sb_offline(&s, &rel)
        .args(["update", "-y"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    let stdout = text(&out.stdout);
    assert!(
        stdout.contains(&format!("sb {INSTALLED} -> {next}\n")),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!("updated: sb {next}\n")),
        "{stdout}"
    );

    let installed = s.sb_home().join("bin").join("sb");
    let out = Command::new(&installed).arg("--version").output().unwrap();
    assert_eq!(text(&out.stdout).trim(), format!("sb {next}"));
    assert!(s.sb_home().join("VERSION").is_file());
}

#[test]
fn update_refuses_a_binary_that_is_not_the_installed_one() {
    if skip_on_windows() {
        return;
    }
    let s = Sandbox::new();
    let rel = Releases::new();
    rel.set_latest(&next_version());

    let mut cmd = sb_offline(&s, &rel);
    cmd.env_remove("SB_UPDATE_ALLOW_ANY_EXE");
    let out = cmd.args(["update", "-y"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    let stderr = text(&out.stderr);
    assert!(
        stderr.contains("not the installed")
            && stderr.contains("`sb update` only updates an installed sb"),
        "{stderr}"
    );
    assert!(!s.sb_home().join("bin").exists(), "nothing was installed");
}

#[test]
fn version_prints_one_line_when_the_check_is_off() {
    if skip_on_windows() {
        return;
    }
    let s = Sandbox::new();
    write_cache(&s.sb_home(), "99.0.0", now_unix());
    let out = sb(&s)
        .arg("--version")
        .env("SB_NO_UPDATE_CHECK", "1")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(text(&out.stdout), format!("sb {INSTALLED}\n"));
}

#[test]
fn version_notice_comes_from_a_fresh_cache_without_network() {
    if skip_on_windows() {
        return;
    }
    let s = Sandbox::new();
    let next = next_version();
    // Any fetch would go to the discard port and fail fast; the cache must
    // answer before one is attempted.
    let dead_base = "http://127.0.0.1:9/";

    write_cache(&s.sb_home(), &next, now_unix());
    let out = sb(&s)
        .arg("--version")
        .env("SB_RELEASE_BASE", dead_base)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        text(&out.stdout),
        format!("sb {INSTALLED}\nupdate available: {next}   run `sb update`\n")
    );

    write_cache(&s.sb_home(), INSTALLED, now_unix());
    let out = sb(&s)
        .arg("--version")
        .env("SB_RELEASE_BASE", dead_base)
        .output()
        .unwrap();
    assert_eq!(text(&out.stdout), format!("sb {INSTALLED}\n(latest)\n"));

    // A stale cache does go to the network; the failure must be silent.
    write_cache(&s.sb_home(), &next, 0);
    let started = Instant::now();
    let out = sb(&s)
        .arg("--version")
        .env("SB_RELEASE_BASE", dead_base)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(text(&out.stdout), format!("sb {INSTALLED}\n"));
    assert_eq!(text(&out.stderr), "", "a failed check prints nothing");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the check must give up quickly ({:?})",
        started.elapsed()
    );
}
