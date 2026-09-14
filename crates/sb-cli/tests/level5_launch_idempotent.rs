//! L5 TDD #4 — `sb run <module>` is idempotent.
//!
//! Launches exactly one pane on first call; re-running with the same
//! module name kills the existing window and creates a fresh one
//! instead of duplicating.

mod common;
use common::{L5Sandbox, list_windows, tmux_launch_supported};

#[test]
fn level5_run_idempotent() {
    if !tmux_launch_supported() {
        eprintln!(
            "skipping: no tmux able to run a POSIX launch pane (see installguide_windows.md §9.5)"
        );
        return;
    }
    let mut sb = L5Sandbox::empty();
    sb.adopt("alpha", Some("#!/usr/bin/env bash\nsleep 30\n"));
    sb.adopt("beta", Some("#!/usr/bin/env bash\nsleep 30\n"));

    let session = format!("sb-{}", sb.workspace);

    // First run — session created, one window for `alpha`.
    let out = sb.cmd().args(["run", "alpha"]).output().unwrap();
    assert!(out.status.success(), "first sb run alpha failed: {out:?}");
    let windows = list_windows(&session);
    assert_eq!(windows, vec!["alpha".to_string()]);

    // Second run — window killed + re-created, still exactly one `alpha`.
    let out = sb.cmd().args(["run", "alpha"]).output().unwrap();
    assert!(out.status.success(), "second sb run alpha failed: {out:?}");
    let windows = list_windows(&session);
    assert_eq!(
        windows,
        vec!["alpha".to_string()],
        "duplicate windows after re-run: {windows:?}"
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("restarted"),
        "second run should mention restart, got: {stdout}"
    );

    // Add a second module's window via run; both should coexist.
    let out = sb.cmd().args(["run", "beta"]).output().unwrap();
    assert!(out.status.success(), "sb run beta failed: {out:?}");
    let mut windows = list_windows(&session);
    windows.sort();
    assert_eq!(windows, vec!["alpha".to_string(), "beta".to_string()]);
}

#[test]
fn level5_run_unknown_module_errors() {
    let mut sb = L5Sandbox::empty();
    sb.adopt("alpha", Some("#!/usr/bin/env bash\nsleep 30\n"));
    let out = sb.cmd().args(["run", "nope"]).output().unwrap();
    assert!(!out.status.success(), "sb run nope should fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("nope"), "got: {stderr}");
    assert!(
        stderr.contains("flow.yaml") || stderr.contains("not registered"),
        "got: {stderr}"
    );
}
