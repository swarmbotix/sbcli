//! L5 native stop: `sb stop m` closes every `m-<id>` window, leaves other
//! modules' windows alone, then runs `scripts.stop` inline.

mod common;
use common::l5_sandbox::yaml_path;
use common::{L5Sandbox, list_windows, tmux_launch_supported};

#[test]
fn level5_native_stop_closes_instances_and_runs_stop_script() {
    if !tmux_launch_supported() {
        eprintln!("skipping: no tmux able to run a launch pane (see installguide_windows.md §9.5)");
        return;
    }
    let mut sb = L5Sandbox::with_workspace("nativestop");
    let sentinel = sb.tempdir().join("m-stopped.txt");
    let m = sb.native(
        "m",
        &format!(
            "module: m\n\
             language: python\n\
             scripts:\n  \
               start: sleep 30\n  \
               stop: |-\n    \
                 sh -c 'printf stopped > \"{}\"'\n",
            sentinel.display()
        ),
    );
    let keep = sb.native(
        "keep",
        "module: keep\nlanguage: python\nscripts:\n  start: sleep 30\n",
    );
    sb.write_installed_flow(&format!(
        "modules:\n  m:\n    path: {}\n    instances: [a, b]\n  keep: {}\n",
        yaml_path(&m.manifest()),
        yaml_path(&keep.manifest())
    ));

    let session = format!("sb-{}", sb.workspace);
    let out = sb.cmd().arg("up").output().unwrap();
    assert!(
        out.status.success(),
        "sb up failed: {}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let mut windows = list_windows(&session);
    windows.sort();
    assert_eq!(windows, ["keep", "m-a", "m-b"]);

    let out = sb.cmd().args(["stop", "m"]).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "sb stop m failed: {stdout}\n{}",
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        stdout.contains("stopped m: closed 2 window(s)"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("ran scripts.stop for m"),
        "stdout: {stdout}"
    );

    assert_eq!(
        list_windows(&session),
        ["keep"],
        "only the other module's window should remain"
    );
    // scripts.stop runs inline, so its effect is there when `sb stop` returns.
    let body = std::fs::read_to_string(&sentinel).expect("scripts.stop never wrote its sentinel");
    assert_eq!(body, "stopped");
}
