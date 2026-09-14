//! L5 TDD #5 — `sb attach` / `sb down` round-trip.
//!
//! In non-interactive mode (cargo test → not a TTY) `sb attach` prints
//! the session name + exits 0. After `sb down`, attach errors with a
//! "no session" message.

mod common;
use common::{L5Sandbox, has_session, tmux_launch_supported};

#[test]
fn level5_attach_reports_session_then_down_clears_it() {
    if !tmux_launch_supported() {
        eprintln!(
            "skipping: no tmux able to run a POSIX launch pane (see installguide_windows.md §9.5)"
        );
        return;
    }
    let mut sb = L5Sandbox::empty();
    sb.adopt("alpha", Some("#!/usr/bin/env bash\nsleep 30\n"));

    let session = format!("sb-{}", sb.workspace);

    // sb up — start the session.
    let out = sb.cmd().arg("up").output().unwrap();
    assert!(out.status.success(), "sb up failed: {out:?}");

    // sb attach in non-interactive mode — should print session name + exit 0.
    let out = sb.cmd().arg("attach").output().unwrap();
    assert!(
        out.status.success(),
        "sb attach (non-tty) should report session + exit 0: {out:?}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(&session),
        "want session name {session:?} in stdout: {stdout}"
    );

    // sb down — clears session.
    let out = sb.cmd().arg("down").output().unwrap();
    assert!(out.status.success(), "sb down failed: {out:?}");
    assert!(!has_session(&session));

    // sb attach after down — errors with "no session" (and an actionable hint).
    let out = sb.cmd().arg("attach").output().unwrap();
    assert!(!out.status.success(), "sb attach should fail after sb down");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no tmux session"),
        "want 'no tmux session' hint, got: {stderr}"
    );
    assert!(
        stderr.contains("sb up"),
        "want 'run sb up first' suggestion, got: {stderr}"
    );
}
