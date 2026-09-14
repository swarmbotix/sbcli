//! L3 TDD #3 — module resolution: cwd, sibling-of-cwd, workspace `flow.yaml`.

mod common;
use common::{L3Sandbox, WsSandbox};

#[test]
fn bare_pub_list_in_cwd_works() {
    let s = L3Sandbox::rust("pubber");
    let out = s.cmd_in_module().args(["pub", "list"]).output().unwrap();
    assert!(out.status.success(), "bare pub list failed: {out:?}");
}

#[test]
fn bare_pub_list_in_parent_without_dev_yml_errors() {
    let s = L3Sandbox::rust("pubber");
    let out = s.cmd_in_tempdir().args(["pub", "list"]).output().unwrap();
    assert!(
        !out.status.success(),
        "running from parent without sb.dev.yml must fail"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no sb.dev.yml"),
        "stderr should explain: {stderr}"
    );
}

#[test]
fn module_flag_resolves_child_of_cwd() {
    let s = L3Sandbox::rust("pubber");
    // Run with cwd = tempdir (parent of `pubber`), use --module pubber.
    let out = s
        .cmd_in_tempdir()
        .args(["pub", "list", "--module", "pubber"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "--module pubber should find the sibling: {out:?}"
    );
}

#[test]
fn module_flag_resolves_via_flow_yaml_from_unrelated_cwd() {
    let s = L3Sandbox::rust("pubber");
    // Run from a totally unrelated cwd — sibling lookup fails, flow.yaml hit.
    let other = s.ws.tempdir().join("unrelated");
    std::fs::create_dir_all(&other).unwrap();
    let mut cmd = s.ws.cmd();
    cmd.current_dir(&other);
    cmd.args(["pub", "list", "--module", "pubber"]);
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "flow.yaml lookup should resolve pubber from {}: stderr={}",
        other.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn unknown_module_errors_with_paths_searched() {
    let ws = WsSandbox::new();
    let _ = ws.cmd().args(["ws", "create", "demo"]).output().unwrap();
    let _ = ws.cmd().args(["ws", "set", "demo"]).output().unwrap();

    let out = ws
        .cmd()
        .args(["pub", "list", "--module", "ghost"])
        .current_dir(ws.tempdir())
        .output()
        .unwrap();
    assert!(!out.status.success(), "unknown --module must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("ghost"), "stderr: {stderr}");
    assert!(
        stderr.contains("Searched") || stderr.contains("searched"),
        "stderr should mention searched paths: {stderr}"
    );
}
