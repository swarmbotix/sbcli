//! L2 TDD test #6 — language-flag handling.

mod common;
use common::WsSandbox;

fn setup_active(s: &WsSandbox) {
    s.cmd().args(["ws", "create", "demo"]).output().unwrap();
    s.cmd().args(["ws", "set", "demo"]).output().unwrap();
}

#[test]
fn case_a_no_dev_yml_no_flag_errors() {
    let s = WsSandbox::new();
    setup_active(&s);
    let project = s.tempdir().join("pubber");
    std::fs::create_dir_all(&project).unwrap();

    let out = s.cmd().args(["init"]).arg(&project).output().unwrap();
    assert!(
        !out.status.success(),
        "init without a language flag must fail"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--rust") && stderr.contains("--python"),
        "error should list language flags: {stderr}"
    );
    assert!(
        !project.join("sb.dev.yml").exists(),
        "nothing should be written"
    );
}

#[test]
fn case_b_mismatched_language_flag_errors() {
    let s = WsSandbox::new();
    setup_active(&s);
    let project = s.tempdir().join("pubber");
    std::fs::create_dir_all(&project).unwrap();

    let out = s
        .cmd()
        .args(["init", "--rust"])
        .arg(&project)
        .output()
        .unwrap();
    assert!(out.status.success(), "first init failed: {out:?}");

    let out = s
        .cmd()
        .args(["init", "--python"])
        .arg(&project)
        .output()
        .unwrap();
    assert!(!out.status.success(), "init --python over rust must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("language tag mismatch"),
        "expected 'language tag mismatch': {stderr}"
    );
}

#[test]
fn case_c_existing_dev_yml_no_flag_succeeds() {
    let s = WsSandbox::new();
    setup_active(&s);
    let project = s.tempdir().join("pubber");
    std::fs::create_dir_all(&project).unwrap();

    let out = s
        .cmd()
        .args(["init", "--rust"])
        .arg(&project)
        .output()
        .unwrap();
    assert!(out.status.success(), "first init failed: {out:?}");

    let out = s.cmd().args(["init"]).arg(&project).output().unwrap();
    assert!(
        out.status.success(),
        "second init without flag failed: {out:?}"
    );
}
