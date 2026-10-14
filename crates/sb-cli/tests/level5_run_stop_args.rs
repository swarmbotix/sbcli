//! L5 — `sb run <module> [args...]` forwards trailing args to the
//! module's `scripts.start` (where it says `{args}`), and `sb stop
//! <module> [args...]` mirrors it for `scripts.stop`. Both share the
//! `sb-<workspace>` tmux session and the idempotent respawn semantics
//! covered by TDD #4.
//!
//! These tests prove the pass-through contract end-to-end: the command
//! writes its argv to a sentinel file, the test reads back the file and
//! asserts the args round-tripped intact.

mod common;
use common::l5_sandbox::{wait_for_file, yaml_path};
use common::{L5Sandbox, list_windows, tmux_launch_supported};

#[test]
fn level5_run_forwards_args_to_start() {
    if !tmux_launch_supported() {
        eprintln!(
            "skipping: no tmux able to run a POSIX launch pane (see installguide_windows.md §9.5)"
        );
        return;
    }
    let mut sb = L5Sandbox::with_workspace("argpass");
    let sentinel = sb.tempdir().join("alpha-args.txt");
    // argv, not shell: sb splits this into `sh`, `-c`, `<script>`, `sh`,
    // then splices the passthrough args at `{args}`; the script sees them
    // as "$@" ($0 is the second `sh`).
    let manifest = format!(
        "module: alpha\n\
         language: python\n\
         scripts:\n  \
           start: |-\n    \
             sh -c 'printf \"%s\\n\" \"$@\" > \"{}\"; exec sleep 30' sh {{args}}\n",
        sentinel.display()
    );
    let alpha = sb.native("alpha", &manifest);
    sb.write_installed_flow(&format!(
        "modules:\n  alpha: {}\n",
        yaml_path(&alpha.manifest())
    ));

    let out = sb
        .cmd()
        .args(["run", "alpha", "--id", "sb01", "extra arg"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "sb run alpha --id sb01 'extra arg' failed: {out:?}"
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("args:"),
        "stdout should mention args: {stdout}"
    );
    assert!(stdout.contains("--id"), "stdout should echo --id: {stdout}");

    let lines = wait_for_file(&sentinel, 5_000)
        .expect("scripts.start never wrote sentinel — args never reached the pane");
    let got: Vec<&str> = lines.lines().collect();
    assert_eq!(
        got,
        vec!["--id", "sb01", "extra arg"],
        "argv mismatch: {got:?}"
    );

    // Window is named after the module, regardless of args.
    let windows = list_windows(&format!("sb-{}", sb.workspace));
    assert_eq!(windows, vec!["alpha".to_string()]);
}

#[test]
fn level5_stop_passes_args_to_scripts_stop() {
    if !tmux_launch_supported() {
        eprintln!(
            "skipping: no tmux able to run a POSIX launch pane (see installguide_windows.md §9.5)"
        );
        return;
    }
    let mut sb = L5Sandbox::with_workspace("stopexec");
    let sentinel = sb.tempdir().join("alpha-stop.txt");
    // start: a long-running sleep so there is something to stop.
    // stop: writes its argv to the sentinel; the passthrough args land at
    // `{args}`.
    let manifest = format!(
        "module: alpha\n\
         language: python\n\
         scripts:\n  \
           start: sleep 30\n  \
           stop: |-\n    \
             sh -c 'printf \"%s\\n\" stopped \"$@\" > \"{}\"' sh {{args}}\n",
        sentinel.display()
    );
    let alpha = sb.native("alpha", &manifest);
    sb.write_installed_flow(&format!(
        "modules:\n  alpha: {}\n",
        yaml_path(&alpha.manifest())
    ));

    // Start the module, then stop it with a passthrough arg.
    let out = sb.cmd().args(["run", "alpha"]).output().unwrap();
    assert!(out.status.success(), "sb run alpha failed: {out:?}");

    let out = sb
        .cmd()
        .args(["stop", "alpha", "--graceful"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "sb stop alpha --graceful failed: {out:?}"
    );

    let lines = wait_for_file(&sentinel, 5_000).expect("scripts.stop never wrote sentinel");
    let got: Vec<&str> = lines.lines().collect();
    assert_eq!(
        got,
        vec!["stopped", "--graceful"],
        "scripts.stop argv: {got:?}"
    );
}
