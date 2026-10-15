//! `sb tools install` end to end through the real binary, with no network:
//! `SB_TOOLS_BASE=file:///<dir>` serves fake protoc / flatc zips built here
//! (sh scripts that print the pinned version for `--version`), each with a
//! `.sha256` sidecar that stands in for the pinned digest. The install goes
//! under a temporary `--prefix`.
//!
//! The sandbox's `$SB_CONFIG` is the highest config layer and inherits the dev
//! box's `protoc:` / `flatc:`; both are stripped, so the doctor rows read the
//! keys that `sb tools install` writes into the global sb.config.yml.
//!
//! Unix only: the fake executables are sh scripts. The test returns early on
//! Windows.

mod common;
use common::Sandbox;

use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use sb_update::tools::{self, Tool};
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;

fn skip_on_windows() -> bool {
    if cfg!(windows) {
        eprintln!("skipping: the fake protoc and flatc are sh scripts");
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

/// A stand-in for `tool` that answers any arguments with the first line the
/// real one prints for `--version`.
fn fake_exe(tool: Tool) -> String {
    let line = match tool {
        Tool::Protoc => format!("libprotoc {}", tool.version()),
        Tool::Flatc => format!("flatc version {}", tool.version()),
    };
    format!("#!/bin/sh\necho \"{line}\"\n")
}

/// Write `<dir>/<asset>`, a zip holding `body` at `exe` (mode 0755), and its
/// `<asset>.sha256` sidecar with the real digest of that zip.
fn publish(dir: &Path, asset: &str, exe: &str, body: &str) {
    let zip_path = dir.join(asset);
    let mut zw = zip::ZipWriter::new(File::create(&zip_path).unwrap());
    let opts = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o755);
    if let Some((parent, _)) = exe.rsplit_once('/') {
        zw.add_directory(format!("{parent}/"), opts).unwrap();
    }
    zw.start_file(exe, opts).unwrap();
    zw.write_all(body.as_bytes()).unwrap();
    zw.finish().unwrap();

    let mut hex = String::with_capacity(64);
    for byte in Sha256::digest(fs::read(&zip_path).unwrap()) {
        write!(hex, "{byte:02x}").unwrap();
    }
    fs::write(
        dir.join(format!("{asset}.sha256")),
        format!("{hex}  {asset}\n"),
    )
    .unwrap();
}

#[test]
fn tools_install_puts_the_tools_under_the_prefix_and_in_the_config() {
    if skip_on_windows() {
        return;
    }
    let s = Sandbox::new();
    let body: String = fs::read_to_string(&s.config_path)
        .unwrap()
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            !t.starts_with("protoc:") && !t.starts_with("flatc:")
        })
        .map(|l| format!("{l}\n"))
        .collect();
    fs::write(&s.config_path, body).unwrap();

    // Assets named as this platform's real ones; a tool with no prebuilt
    // asset here (flatc on linux-aarch64) is left out.
    let assets = tempfile::TempDir::new().unwrap();
    let specs: Vec<_> = Tool::ALL
        .into_iter()
        .filter_map(|tool| tools::spec(tool).ok())
        .collect();
    assert!(!specs.is_empty(), "this platform has a prebuilt protoc");
    for spec in &specs {
        publish(assets.path(), spec.asset, spec.exe, &fake_exe(spec.tool));
    }

    // What the sudo line of the real flow does: the parents exist and are
    // writable before the install runs.
    let prefix = s.home().join("opt");
    for spec in &specs {
        let top = spec.dir.split('/').next().unwrap();
        fs::create_dir_all(prefix.join(top)).unwrap();
    }

    let base = format!("file://{}", assets.path().display());
    let run = || {
        s.cmd()
            .env("SB_TOOLS_BASE", &base)
            .stdin(Stdio::null())
            .args(["tools", "install", "--yes", "--prefix"])
            .arg(&prefix)
            .output()
            .unwrap()
    };

    let out = run();
    assert!(out.status.success(), "{}", describe(&out));
    let stdout = text(&out.stdout);
    let global_path = s.sb_home().join("sb.config.yml");
    let global_text = fs::read_to_string(&global_path).unwrap();
    let global: serde_yaml::Value = serde_yaml::from_str(&global_text).unwrap();
    for spec in &specs {
        let name = spec.tool.name();
        let version = spec.tool.version();
        let exe = prefix.join(spec.dir).join(spec.exe);
        let shown = exe.display();
        // The table row: `{:<10}: {:<10} {:<12} -> {}`.
        let row = format!("{name:<10}: {version:<10} missing      -> {shown}");
        assert!(stdout.contains(&row), "{}", describe(&out));
        let done = format!("installed: {name} {version} -> {shown}");
        assert!(stdout.contains(&done), "{}", describe(&out));
        assert!(exe.is_file(), "{shown} was not installed");
        let ran = Command::new(&exe)
            .arg("--version")
            .output()
            .unwrap_or_else(|e| panic!("running {shown}: {e}"));
        assert!(
            ran.status.success() && text(&ran.stdout).contains(version),
            "{shown}: {}",
            describe(&ran)
        );
        assert_eq!(
            global.get(name).and_then(serde_yaml::Value::as_str),
            exe.to_str(),
            "`{name}` in the global sb.config.yml"
        );
        assert!(
            stdout.contains(&format!("[OK]   {name}")),
            "{}",
            describe(&out)
        );
    }

    // Everything is at its pin now: nothing is downloaded or replaced.
    let out = run();
    assert!(out.status.success(), "{}", describe(&out));
    let stdout = text(&out.stdout);
    assert!(stdout.contains("nothing to do"), "{}", describe(&out));
    for spec in &specs {
        assert!(
            stdout.contains(&format!("[OK]   {}", spec.tool.name())),
            "{}",
            describe(&out)
        );
    }
}
