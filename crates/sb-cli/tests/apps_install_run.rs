//! App packages end to end through the real binary: `sb app init` writes a
//! package, `sb install <dir>` registers it in the sandboxed sb_home,
//! `sb <name> args...` runs it from the caller's cwd with the args verbatim
//! and passes its exit code on, and `sb app list|info|remove` manage it.
//!
//! On Unix `sb <name>` execs the entry in place of itself, so a successful
//! run can only be observed from outside the process, as these tests do.

mod common;
use common::WsSandbox;
use std::path::{Path, PathBuf};
use std::process::Output;
#[cfg(unix)]
use std::process::Stdio;

/// Entry that reports its args, cwd, app env and pid, and exits with `$2`
/// when called as `fail <code>`.
#[cfg(unix)]
const ECHO_ENTRY: &str = r#"#!/usr/bin/env bash
echo "args: $*"
echo "cwd: $PWD"
echo "app: $SB_APP_NAME"
echo "pid: $$"
if [[ "${1:-}" == "fail" ]]; then exit "$2"; fi
"#;

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn ok(out: &Output, what: &str) -> String {
    assert!(
        out.status.success(),
        "{what} failed\nstdout:\n{}\nstderr:\n{}",
        text(&out.stdout),
        text(&out.stderr)
    );
    text(&out.stdout)
}

fn mkdir(p: &Path) -> PathBuf {
    std::fs::create_dir_all(p).unwrap();
    p.to_path_buf()
}

#[cfg(unix)]
#[test]
fn init_install_run_list_remove() {
    if !common::bash_available() {
        eprintln!("skipping: no bash");
        return;
    }
    let s = WsSandbox::new();
    let pkg = mkdir(&s.tempdir().join("demo-pkg"));

    let out = s
        .cmd()
        .args(["app", "init"])
        .arg(&pkg)
        .args([
            "--name",
            "demoapp",
            "--kind",
            "none",
            "--description",
            "Demo app",
        ])
        .output()
        .unwrap();
    let stdout = ok(&out, "sb app init");
    assert!(stdout.contains("created app package demoapp"), "{stdout}");
    assert!(pkg.join("sb.app.yml").is_file());
    assert!(pkg.join("run.bash").is_file());
    // Overwriting keeps the stub's executable bit.
    std::fs::write(pkg.join("run.bash"), ECHO_ENTRY).unwrap();

    let out = s.cmd().arg("install").arg(&pkg).output().unwrap();
    let stdout = ok(&out, "sb install");
    assert!(stdout.contains("installed app demoapp 0.1.0"), "{stdout}");
    let registry = std::fs::read_to_string(s.sb_home().join("apps.yml")).unwrap();
    assert!(registry.contains("demoapp:"), "{registry}");

    let work = mkdir(&s.tempdir().join("work"));
    let child = s
        .cmd()
        .args(["demoapp", "a", "b c", "--flag"])
        .current_dir(&work)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let sb_pid = child.id().to_string();
    let out = child.wait_with_output().unwrap();
    let stdout = ok(&out, "sb demoapp");
    assert!(
        stdout
            .lines()
            .any(|l| l.strip_prefix("pid: ") == Some(&sb_pid)),
        "sb must exec the app in place of itself (app pid == sb pid {sb_pid}):\n{stdout}"
    );
    assert!(stdout.contains("args: a b c --flag"), "{stdout}");
    assert!(stdout.contains("app: demoapp"), "{stdout}");
    let cwd = stdout
        .lines()
        .find_map(|l| l.strip_prefix("cwd: "))
        .expect("cwd line");
    assert_eq!(
        Path::new(cwd).canonicalize().unwrap(),
        work.canonicalize().unwrap(),
        "the app must run in the caller's cwd, not the package folder"
    );

    let out = s.cmd().args(["demoapp", "fail", "3"]).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(3),
        "exit code must pass through\nstderr:\n{}",
        text(&out.stderr)
    );
    let stdout = text(&out.stdout);
    assert!(stdout.contains("args: fail 3"), "{stdout}");

    let stdout = ok(
        &s.cmd().args(["app", "list"]).output().unwrap(),
        "sb app list",
    );
    let row = stdout
        .lines()
        .find(|l| l.starts_with("demoapp"))
        .unwrap_or_else(|| panic!("no demoapp row:\n{stdout}"));
    assert!(
        row.contains("0.1.0") && row.contains("none") && row.contains("path"),
        "{row}"
    );

    let stdout = ok(
        &s.cmd().args(["app", "info", "demoapp"]).output().unwrap(),
        "sb app info",
    );
    assert!(stdout.contains("Demo app"), "{stdout}");

    let stdout = ok(
        &s.cmd().args(["app", "remove", "demoapp"]).output().unwrap(),
        "sb app remove",
    );
    assert!(stdout.contains("removed app demoapp"), "{stdout}");
    assert!(
        pkg.join("sb.app.yml").is_file(),
        "in-place package must survive"
    );

    let stdout = ok(
        &s.cmd().args(["app", "list"]).output().unwrap(),
        "sb app list",
    );
    assert!(stdout.contains("no apps installed"), "{stdout}");
    let out = s.cmd().arg("demoapp").output().unwrap();
    assert!(!out.status.success());
    let stderr = text(&out.stderr);
    assert!(
        stderr.contains("not an sb command or an installed app"),
        "{stderr}"
    );
}

#[test]
fn app_init_refuses_builtin_names_and_existing_manifest() {
    let s = WsSandbox::new();
    let pkg = mkdir(&s.tempdir().join("pkg"));

    let out = s
        .cmd()
        .args(["app", "init"])
        .arg(&pkg)
        .args(["--name", "run"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        text(&out.stderr).contains("built-in"),
        "{}",
        text(&out.stderr)
    );
    assert!(!pkg.join("sb.app.yml").exists());

    let init = |extra: &[&str]| {
        s.cmd()
            .args(["app", "init"])
            .arg(&pkg)
            .args(["--name", "fine_name"])
            .args(extra)
            .output()
            .unwrap()
    };
    ok(&init(&[]), "first sb app init");
    let again = init(&[]);
    assert!(!again.status.success());
    assert!(
        text(&again.stderr).contains("--force"),
        "{}",
        text(&again.stderr)
    );
    ok(&init(&["--force"]), "sb app init --force");
}
