//! L5 TDD #1 — `sb up` launches expected panes.
//!
//! 2-module workspace, both with valid runscripts. `sb up` → tmux
//! list-windows reports 2 windows, one per module. `sb down` → tmux
//! has-session returns non-zero.

mod common;
use common::{L5Sandbox, has_session, list_windows, tmux_launch_supported};

#[test]
fn level5_up_two_modules_then_down() {
    if !tmux_launch_supported() {
        eprintln!(
            "skipping: no tmux able to run a POSIX launch pane (see installguide_windows.md §9.5)"
        );
        return;
    }
    let mut sb = L5Sandbox::empty();
    // Both runscripts are no-ops that just sleep, so tmux keeps the
    // pane alive long enough for has-session / list-windows to observe it.
    sb.adopt("alpha", Some("#!/usr/bin/env bash\nsleep 30\n"));
    sb.adopt("beta", Some("#!/usr/bin/env bash\nsleep 30\n"));

    let session = format!("sb-{}", sb.workspace);
    // Make sure no leftover from a previous run.
    assert!(!has_session(&session), "stale session before sb up");

    let out = sb.cmd().arg("up").output().unwrap();
    assert!(
        out.status.success(),
        "sb up failed: {}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    assert!(has_session(&session), "session not created by sb up");

    let mut windows = list_windows(&session);
    windows.sort();
    assert_eq!(
        windows,
        vec!["alpha".to_string(), "beta".to_string()],
        "expected one window per module"
    );

    // Tear down.
    let out = sb.cmd().arg("down").output().unwrap();
    assert!(out.status.success(), "sb down failed: {out:?}");
    assert!(
        !has_session(&session),
        "session still present after sb down"
    );
}

#[test]
fn level5_down_when_no_session_is_a_noop() {
    if !tmux_launch_supported() {
        eprintln!(
            "skipping: no tmux able to run a POSIX launch pane (see installguide_windows.md §9.5)"
        );
        return;
    }
    let sb = L5Sandbox::empty();
    let out = sb.cmd().arg("down").output().unwrap();
    assert!(
        out.status.success(),
        "sb down should succeed when no session exists: {out:?}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("nothing to do") || stdout.contains("no tmux session"),
        "want friendly no-op message, got: {stdout}"
    );
}
