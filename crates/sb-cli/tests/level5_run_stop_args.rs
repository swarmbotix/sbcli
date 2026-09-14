//! L5 — `sb run <module> [args...]` forwards trailing args to
//! `runscript.bash`, and `sb stop <module> [args...]` mirrors it for
//! `stopscript.bash`. Both share the `sb-<workspace>` tmux session and
//! the idempotent respawn semantics covered by TDD #4.
//!
//! These tests prove the pass-through contract end-to-end: the
//! runscript writes its argv to a sentinel file, the test reads back
//! the file and asserts the args round-tripped intact.

mod common;
use common::{L5Sandbox, list_windows, tmux_launch_supported};

/// Poll a sentinel file (written by the runscript via tmux) until it
/// appears or the wait expires. Tmux launches the pane asynchronously,
/// so a strict synchronous assertion would race.
fn wait_for_file(path: &std::path::Path, max_ms: u64) -> Option<String> {
    let step_ms = 50u64;
    let mut waited = 0u64;
    while waited < max_ms {
        if let Ok(body) = std::fs::read_to_string(path) {
            if !body.is_empty() {
                return Some(body);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(step_ms));
        waited += step_ms;
    }
    None
}

#[test]
fn level5_run_forwards_args_to_runscript() {
    if !tmux_launch_supported() {
        eprintln!(
            "skipping: no tmux able to run a POSIX launch pane (see installguide_windows.md §9.5)"
        );
        return;
    }
    let mut sb = L5Sandbox::with_workspace("argpass");
    let sentinel = sb.tempdir().join("alpha-args.txt");
    let body = format!(
        "#!/usr/bin/env bash\nprintf '%s\\n' \"$@\" > {} \nsleep 30\n",
        shell_escape(&sentinel.display().to_string())
    );
    sb.adopt("alpha", Some(&body));

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
        .expect("runscript never wrote sentinel — args never reached the pane");
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
fn level5_stop_executes_stopscript() {
    if !tmux_launch_supported() {
        eprintln!(
            "skipping: no tmux able to run a POSIX launch pane (see installguide_windows.md §9.5)"
        );
        return;
    }
    let mut sb = L5Sandbox::with_workspace("stopexec");
    let sentinel = sb.tempdir().join("alpha-stop.txt");
    // Runscript: long-running sleep so we have something to "stop".
    sb.adopt("alpha", Some("#!/usr/bin/env bash\nsleep 30\n"));
    // Stopscript: writes its argv to the sentinel.
    let stop_body = format!(
        "#!/usr/bin/env bash\nprintf '%s\\n' \"stopped\" \"$@\" > {} \n",
        shell_escape(&sentinel.display().to_string())
    );
    sb.write_stopscript("alpha", &stop_body);

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

    let lines = wait_for_file(&sentinel, 5_000).expect("stopscript never wrote sentinel");
    let got: Vec<&str> = lines.lines().collect();
    assert_eq!(
        got,
        vec!["stopped", "--graceful"],
        "stopscript argv: {got:?}"
    );
}

#[test]
fn level5_stop_blames_missing_stopscript() {
    let mut sb = L5Sandbox::with_workspace("nostop");
    sb.adopt("alpha", Some("#!/usr/bin/env bash\nsleep 30\n"));
    // No stopscript written.
    let out = sb.cmd().args(["stop", "alpha"]).output().unwrap();
    assert!(
        !out.status.success(),
        "sb stop alpha should fail without stopscript"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("alpha"),
        "want module name in stderr: {stderr}"
    );
    assert!(
        stderr.contains("stopscript"),
        "want 'stopscript' in stderr: {stderr}"
    );
}

#[test]
fn level5_stop_unknown_module_errors() {
    let mut sb = L5Sandbox::with_workspace("stopnope");
    sb.adopt("alpha", Some("#!/usr/bin/env bash\nsleep 30\n"));
    let out = sb.cmd().args(["stop", "nope"]).output().unwrap();
    assert!(!out.status.success(), "sb stop nope should fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("nope"), "got: {stderr}");
}

/// Single-quote `s` for embedding in a shell heredoc / script body.
/// Mirrors `sb-launch`'s internal quoting; we duplicate it here so the
/// test crate doesn't take a dependency on `sb-launch`'s private fn.
fn shell_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}
