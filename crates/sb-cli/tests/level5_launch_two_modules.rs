//! L5 TDD #1 — `sb up` launches expected panes.
//!
//! 2-module workspace, both native modules whose start command sleeps.
//! `sb up` → tmux list-windows reports 2 windows, one per module.
//! `sb down` → tmux has-session returns non-zero.
//!
//! `sb up <flow>` launches only that flow's nodes and records it as the
//! active flow, which `sb run` then reads.

mod common;
use common::{L5Sandbox, has_session, list_windows, tmux_launch_supported};

/// A python module whose start command sleeps, so tmux keeps the pane alive
/// long enough for has-session / list-windows to observe it.
fn sleeper(name: &str) -> String {
    format!("module: {name}\nlanguage: python\nscripts:\n  start: sleep 30\n")
}

#[test]
fn level5_up_two_modules_then_down() {
    if !tmux_launch_supported() {
        eprintln!(
            "skipping: no tmux able to run a POSIX launch pane (see installguide_windows.md §9.5)"
        );
        return;
    }
    let mut sb = L5Sandbox::empty();
    let alpha = sb.native("alpha", &sleeper("alpha"));
    let beta = sb.native("beta", &sleeper("beta"));
    sb.write_installed(&[&alpha, &beta], "nodes: [alpha, beta]\n");

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
fn level5_up_named_flow_then_run_reads_the_active_flow() {
    if !tmux_launch_supported() {
        eprintln!(
            "skipping: no tmux able to run a POSIX launch pane (see installguide_windows.md §9.5)"
        );
        return;
    }
    let mut sb = L5Sandbox::with_workspace("upflow");
    let alpha = sb.native("alpha", &sleeper("alpha"));
    let beta = sb.native("beta", &sleeper("beta"));
    let gamma = sb.native("gamma", &sleeper("gamma"));
    // `default` runs alpha and beta; `calib` runs gamma alone.
    sb.write_installed(&[&alpha, &beta, &gamma], "nodes: [alpha, beta]\n");
    sb.write_installed_flow("calib", "nodes: [gamma]\n");

    let session = format!("sb-{}", sb.workspace);
    assert!(!has_session(&session), "stale session before sb up");

    let out = sb.cmd().args(["up", "calib"]).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "sb up calib failed: {stdout}\n{}",
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        stdout.starts_with(&format!(
            "flow calib: launched 1 window(s) in tmux session \"{session}\"\n"
        )),
        "stdout: {stdout}"
    );
    assert_eq!(list_windows(&session), ["gamma"], "only calib's node runs");

    // `sb run` acts in the active flow, calib: its node restarts in place ...
    let out = sb.cmd().args(["run", "gamma"]).output().unwrap();
    assert!(out.status.success(), "sb run gamma failed: {out:?}");
    assert_eq!(list_windows(&session), ["gamma"]);

    // ... and a node of `default` only is refused.
    let out = sb.cmd().args(["run", "alpha"]).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "sb run alpha should fail: {out:?}");
    assert!(
        stderr.contains("module alpha is not a node of flow calib"),
        "stderr: {stderr}"
    );
    assert_eq!(list_windows(&session), ["gamma"], "nothing else launched");

    let out = sb.cmd().arg("down").output().unwrap();
    assert!(out.status.success(), "sb down failed: {out:?}");
    assert!(!has_session(&session));
}
