//! L5 TDD #3 — every L5 verb errors with an actionable message when
//! no workspace is active.

mod common;
use common::{WsSandbox, has_session};

#[test]
fn up_errors_when_no_active_workspace() {
    let ws = WsSandbox::new();
    // No `sb ws set` call — `~/.swarmbotix/active` is empty.
    let out = ws.cmd().arg("up").output().unwrap();
    assert!(
        !out.status.success(),
        "sb up should fail without an active workspace"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no active workspace"), "got: {stderr}");
    assert!(
        stderr.contains("sb ws"),
        "want suggestion to set workspace: {stderr}"
    );
}

#[test]
fn down_errors_when_no_active_workspace() {
    let ws = WsSandbox::new();
    let out = ws.cmd().arg("down").output().unwrap();
    assert!(
        !out.status.success(),
        "sb down should fail without an active workspace"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no active workspace"), "got: {stderr}");
}

#[test]
fn attach_errors_when_no_active_workspace() {
    let ws = WsSandbox::new();
    let out = ws.cmd().arg("attach").output().unwrap();
    assert!(
        !out.status.success(),
        "sb attach should fail without an active workspace"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no active workspace"), "got: {stderr}");
}

#[test]
fn up_with_no_modules_in_workspace_errors_clearly() {
    let ws = WsSandbox::new();
    let out = ws.cmd().args(["ws", "create", "demo"]).output().unwrap();
    assert!(out.status.success(), "ws create demo: {out:?}");
    let out = ws.cmd().args(["ws", "set", "demo"]).output().unwrap();
    assert!(out.status.success(), "ws set demo: {out:?}");

    let out = ws.cmd().arg("up").output().unwrap();
    assert!(!out.status.success(), "sb up should fail with no modules");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no modules"), "got: {stderr}");

    // No session created either way.
    assert!(!has_session("sb-demo"));
}
