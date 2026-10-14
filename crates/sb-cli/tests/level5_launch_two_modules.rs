//! L5 TDD #1 — `sb up` launches expected panes.
//!
//! 2-module workspace, both native modules whose start command sleeps.
//! `sb up` → tmux list-windows reports 2 windows, one per module.
//! `sb down` → tmux has-session returns non-zero.

mod common;
use common::l5_sandbox::yaml_path;
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
    // Both modules just sleep, so tmux keeps the pane alive long enough
    // for has-session / list-windows to observe it.
    let alpha = sb.native(
        "alpha",
        "module: alpha\nlanguage: python\nscripts:\n  start: sleep 30\n",
    );
    let beta = sb.native(
        "beta",
        "module: beta\nlanguage: python\nscripts:\n  start: sleep 30\n",
    );
    sb.write_installed_flow(&format!(
        "modules:\n  alpha: {}\n  beta: {}\n",
        yaml_path(&alpha.manifest()),
        yaml_path(&beta.manifest())
    ));

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
