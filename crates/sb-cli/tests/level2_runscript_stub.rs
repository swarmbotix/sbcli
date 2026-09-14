//! L2 TDD test #8 — runscript.bash stub contents.

mod common;
use common::WsSandbox;
use std::path::Path;

fn setup_active(s: &WsSandbox) {
    s.cmd().args(["ws", "create", "demo"]).output().unwrap();
    s.cmd().args(["ws", "set", "demo"]).output().unwrap();
}

fn init_and_read_runscript(s: &WsSandbox, project: &Path, flag: &str) -> String {
    let out = s.cmd().args(["init", flag]).arg(project).output().unwrap();
    assert!(out.status.success(), "init {flag} failed: {out:?}");
    std::fs::read_to_string(project.join("runscript.bash")).unwrap()
}

fn assert_universal_stub_shape(body: &str, runscript_path: &Path, module: &str) {
    assert!(
        body.starts_with("#!/usr/bin/env bash\n"),
        "shebang missing: {body:?}"
    );

    // The script must pin its own cwd so it works no matter who calls
    // it (`sb run`, manual, IDE, cron, …).
    assert!(
        body.contains(r#"cd -- "$(dirname -- "${BASH_SOURCE[0]}")""#),
        "missing cd-to-script-dir guard:\n{body}"
    );
    assert!(
        body.contains("set -euo pipefail"),
        "missing safe-bash flags:\n{body}"
    );

    // Standalone-tmux block: re-exec under a per-instance tmux session
    // when invoked outside `sb up`'s own tmux. Session name is the
    // module identifier suffixed with `-dev` for the no-id case, and
    // `<module>-<id>-dev` when `--id <id>` is in "$@" — that way
    // `bash runscript.bash --id 1` and `--id 2` land in different
    // sessions and actually run as two processes (otherwise tmux's
    // `-A` flag would silently reattach the second call to the first
    // session and discard the new launch command). Honors the
    // `SB_NO_TMUX=1` escape hatch.
    assert!(
        body.contains(
            r#"if [[ -z "${TMUX:-}" ]] && [[ "${SB_NO_TMUX:-0}" != "1" ]] && command -v tmux"#
        ),
        "missing tmux re-exec guard (with SB_NO_TMUX escape):\n{body}"
    );
    let expected_session = format!(r#"__sb_session="{module}${{__sb_id_suffix}}-dev""#);
    assert!(
        body.contains(&expected_session),
        "missing per-instance tmux session-name assembly {expected_session:?}:\n{body}"
    );
    assert!(
        body.contains(r#"exec tmux new -A -s "${__sb_session}""#),
        "missing tmux re-exec line that uses the assembled session name:\n{body}"
    );
    // The session-suffix extractor must walk "$@" looking for `--id`
    // and bind its value as `-<id>`; without that, `--id 2` would not
    // alter the session name and both invocations would collide.
    assert!(
        body.contains(r#"if [[ "$__sb_prev" == "--id" ]]"#)
            && body.contains(r#"__sb_id_suffix="-$__arg""#),
        "missing --id extractor for per-instance session naming:\n{body}"
    );
    // Keep-pane-alive: capture exit code and pause for a keypress so
    // build errors / crash output stay readable. Outer wrapper traps
    // SIGINT (so Ctrl+C doesn't kill the wrapper itself), inner
    // subshell resets the trap so the app gets the signal normally.
    assert!(
        body.contains("press any key to close pane") && body.contains("read -n 1"),
        "missing keep-pane-alive wrapper around the inner bash:\n{body}"
    );
    assert!(
        body.contains("trap '' INT") && body.contains("trap - INT"),
        "missing trap-ignore-INT + subshell-reset pattern (Ctrl+C must \
         stop the app without killing the wrapper):\n{body}"
    );

    // Each example must be a `# `-commented runnable bash line — so
    // when the user strips the leading `# ` they get something that
    // actually executes. The language label is a trailing `# Lang`
    // comment, not a prefix like `Rust: cargo run` (which would try
    // to execute `Rust:` as a command). Examples that launch a binary
    // end with `"$@"` so per-instance flags reach the app's main().
    for line in [
        "# cargo run --release -- \"$@\"",
        "# python main.py \"$@\"",
        "# cmake --build build && exec ./build/app \"$@\"",
        "# flutter run -d linux",
        "# : 'no command — Unity launches from the editor'",
    ] {
        assert!(
            body.contains(line),
            "missing example: {line:?}\n--- body:\n{body}"
        );
    }
    // Args from the outer script must reach the inner tmux-launched
    // bash — otherwise `bash runscript.bash --id 2` loses the `--id 2`.
    assert!(
        body.contains("__sb_args") && body.contains(r#"for __arg in "$@""#),
        "missing $@ forwarding into the tmux re-exec:\n{body}"
    );

    assert!(body.contains("exit 1"), "body should `exit 1`: {body}");
    assert!(body.contains("TODO"), "body should contain TODO: {body}");
    let abs = runscript_path.display().to_string();
    assert!(
        body.contains(&abs),
        "body should embed the absolute runscript path {abs:?}: {body}"
    );
}

#[cfg(unix)]
fn assert_executable_bit(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path).unwrap().permissions().mode();
    assert!(
        mode & 0o111 != 0,
        "runscript should be executable (mode {mode:o})"
    );
}

#[cfg(not(unix))]
fn assert_executable_bit(_path: &Path) {}

#[test]
fn runscript_stub_matches_golden_for_every_language() {
    for (flag, project_name) in [
        ("--rust", "p_rust"),
        ("--python", "p_py"),
        ("--cpp", "p_cpp"),
        ("--flutter", "p_flutter"),
    ] {
        let s = WsSandbox::new();
        setup_active(&s);
        let project = s.tempdir().join(project_name);
        std::fs::create_dir_all(&project).unwrap();
        let body = init_and_read_runscript(&s, &project, flag);
        let rs_path = std::fs::canonicalize(project.join("runscript.bash")).unwrap();
        assert_universal_stub_shape(&body, &rs_path, project_name);
        assert_executable_bit(&rs_path);
    }
}

#[test]
fn runscript_stub_for_unity() {
    let s = WsSandbox::new();
    setup_active(&s);
    let project = s.tempdir().join("teleop");
    std::fs::create_dir_all(project.join("Assets").join("Scripts")).unwrap();
    let body = init_and_read_runscript(&s, &project, "--unity");
    let rs_path = std::fs::canonicalize(project.join("runscript.bash")).unwrap();
    assert_universal_stub_shape(&body, &rs_path, "teleop");
    assert_executable_bit(&rs_path);
}
