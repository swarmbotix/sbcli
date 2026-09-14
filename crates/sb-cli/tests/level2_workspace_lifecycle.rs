//! L2 TDD test #1 — full workspace lifecycle round-trip.
//!
//! Driven by `sb.config.yml`: each `WsSandbox` writes a temp config
//! that points `sb_home_dir` at a tempdir, then sets `$SB_CONFIG` —
//! no `$HOME` override.

mod common;
use common::WsSandbox;

#[test]
fn workspace_lifecycle_round_trip() {
    let s = WsSandbox::new();
    let workspaces = s.sb_home().join("workspaces");
    let active = s.sb_home().join("active");

    // create ws1
    let out = s.cmd().args(["ws", "create", "ws1"]).output().unwrap();
    assert!(out.status.success(), "ws create ws1 failed: {out:?}");
    assert!(workspaces.join("ws1").is_dir());
    assert!(workspaces.join("ws1").join("flow.yaml").exists());

    // list shows ws1
    let out = s.cmd().args(["ws", "list"]).output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("ws1"));

    // set ws1 → active marker
    let out = s.cmd().args(["ws", "set", "ws1"]).output().unwrap();
    assert!(out.status.success(), "ws set ws1 failed: {out:?}");
    assert_eq!(std::fs::read_to_string(&active).unwrap().trim(), "ws1");

    // list now shows the * marker for ws1
    let out = s.cmd().args(["ws", "list"]).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout
        .lines()
        .find(|l| l.contains("ws1"))
        .expect("ws1 line missing");
    assert!(
        line.trim_start().starts_with('*'),
        "expected active marker on ws1: {line:?}"
    );

    // create + set ws2
    let out = s.cmd().args(["ws", "create", "ws2"]).output().unwrap();
    assert!(out.status.success(), "ws create ws2 failed: {out:?}");
    let out = s.cmd().args(["ws", "set", "ws2"]).output().unwrap();
    assert!(out.status.success(), "ws set ws2 failed: {out:?}");
    assert_eq!(std::fs::read_to_string(&active).unwrap().trim(), "ws2");

    // delete ws2 → active cleared with warning
    let out = s.cmd().args(["ws", "delete", "ws2"]).output().unwrap();
    assert!(out.status.success(), "ws delete ws2 failed: {out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("warning"), "expected warning: {stderr}");
    let active_after = std::fs::read_to_string(&active).unwrap();
    assert!(
        active_after.trim().is_empty(),
        "active should be cleared, got {active_after:?}"
    );
    assert!(!workspaces.join("ws2").exists());

    // delete ws1 → no warning (it wasn't active anymore)
    let out = s.cmd().args(["ws", "delete", "ws1"]).output().unwrap();
    assert!(out.status.success(), "ws delete ws1 failed: {out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("warning"), "no warning expected: {stderr}");
    assert!(!workspaces.join("ws1").exists());
}

#[test]
fn ws_set_unknown_workspace_errors() {
    let s = WsSandbox::new();
    let out = s.cmd().args(["ws", "set", "ghost"]).output().unwrap();
    assert!(
        !out.status.success(),
        "setting a non-existent workspace must fail"
    );
}

#[test]
fn ws_list_with_no_workspaces_is_ok() {
    let s = WsSandbox::new();
    let out = s.cmd().args(["ws", "list"]).output().unwrap();
    assert!(out.status.success(), "empty ws list should succeed");
}
