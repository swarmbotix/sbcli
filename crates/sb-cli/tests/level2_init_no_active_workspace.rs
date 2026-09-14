//! L2 TDD test #2 — `sb init` with no active workspace must error
//! actionably without touching the project or any flow.yaml.

mod common;
use common::WsSandbox;

#[test]
fn init_with_no_active_workspace_errors_and_writes_nothing() {
    let s = WsSandbox::new();
    let project = s.tempdir().join("pubber");
    std::fs::create_dir_all(&project).unwrap();

    // No `sb ws create / sb ws set` here — the active marker must be missing.
    let active = s.sb_home().join("active");
    assert!(!active.exists(), "precondition: no active marker");

    let out = s
        .cmd()
        .args(["init", "--rust"])
        .arg(&project)
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "init must fail with no active workspace"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("sb ws"),
        "error should suggest `sb ws set`: {stderr}"
    );

    // Nothing written into the project.
    assert!(!project.join("sb.dev.yml").exists());
    assert!(!project.join("runscript.bash").exists());
    assert!(!project.join("swarmbotix_io").exists());
}
