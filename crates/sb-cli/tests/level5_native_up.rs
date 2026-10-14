//! L5 native launch: `sb up` builds first, then opens one tmux window per
//! instance, with no `runscript.bash` anywhere.
//!
//! 1. A module whose `sb.module.yaml` declares `scripts.start`, with two
//!    instances in the flow, comes up as windows `m-a` and `m-b`; each pane
//!    writes a sentinel holding its `$SB_INSTANCE_ID`, proving sb exported
//!    the variable into the pane.
//! 2. A module whose `scripts.build` fails makes `sb up` fail before tmux
//!    is touched: no session is left behind.

mod common;
use common::l5_sandbox::{wait_for_file, yaml_path};
use common::{L5Sandbox, has_session, list_windows, tmux_launch_supported};

#[test]
fn level5_native_up_opens_one_window_per_instance() {
    if !tmux_launch_supported() {
        eprintln!("skipping: no tmux able to run a launch pane (see installguide_windows.md §9.5)");
        return;
    }
    let mut sb = L5Sandbox::with_workspace("nativeup");
    let out_dir = sb.tempdir().join("sentinels");
    std::fs::create_dir_all(&out_dir).unwrap();
    // argv, not shell: sb splits this into `sh`, `-c`, `<script>` and
    // appends `--id <id>`; `sh` then sees them as $0 and $1.
    let manifest = format!(
        "module: m\n\
         language: python\n\
         scripts:\n  \
           start: |-\n    \
             sh -c 'printf %s \"$SB_INSTANCE_ID\" > \"{}/m-$SB_INSTANCE_ID.txt\"; exec sleep 30'\n",
        out_dir.display()
    );
    let m = sb.native("m", &manifest);
    sb.write_installed_flow(&format!(
        "modules:\n  m:\n    path: {}\n    instances: [a, b]\n",
        yaml_path(&m.manifest())
    ));

    let session = format!("sb-{}", sb.workspace);
    assert!(!has_session(&session), "stale session before sb up");

    let out = sb.cmd().arg("up").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "sb up failed: {stdout}\n{}",
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        stdout.contains(&format!("launched 2 window(s) in tmux session \"{session}\"")),
        "stdout: {stdout}"
    );

    let mut windows = list_windows(&session);
    windows.sort();
    assert_eq!(windows, ["m-a", "m-b"], "one window per instance");

    for id in ["a", "b"] {
        let sentinel = out_dir.join(format!("m-{id}.txt"));
        let got = wait_for_file(&sentinel, 5_000)
            .unwrap_or_else(|| panic!("pane m-{id} never wrote {}", sentinel.display()));
        assert_eq!(got, id, "SB_INSTANCE_ID seen by pane m-{id}");
    }

    let out = sb.cmd().arg("down").output().unwrap();
    assert!(out.status.success(), "sb down failed: {out:?}");
    assert!(!has_session(&session), "session still present after sb down");
}

#[test]
fn level5_native_up_failed_build_leaves_no_session() {
    if !tmux_launch_supported() {
        eprintln!("skipping: no tmux able to run a launch pane (see installguide_windows.md §9.5)");
        return;
    }
    let mut sb = L5Sandbox::with_workspace("nativebuild");
    let good = sb.native(
        "m",
        "module: m\nlanguage: python\nscripts:\n  start: sleep 30\n",
    );
    // Quoted: a bare `false` is a YAML boolean, not the command.
    let bad = sb.native(
        "bad",
        "module: bad\nlanguage: python\nscripts:\n  build: \"false\"\n  start: sleep 30\n",
    );
    sb.write_installed_flow(&format!(
        "modules:\n  m: {}\n  bad: {}\n",
        yaml_path(&good.manifest()),
        yaml_path(&bad.manifest())
    ));

    let session = format!("sb-{}", sb.workspace);
    let out = sb.cmd().arg("up").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !out.status.success(),
        "sb up should fail when a build fails: {stdout}"
    );
    assert!(
        stdout.contains("building bad: false"),
        "the failing build should be announced: {stdout}"
    );
    assert!(
        !has_session(&session),
        "a failed build must not leave a tmux session"
    );
}
