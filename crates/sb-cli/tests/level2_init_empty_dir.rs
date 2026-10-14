//! L2 TDD test #3 — `sb init --rust ./pubber` writes `sb.module.yaml` and
//! an empty IO directory (no runscript), and registers the module in the
//! active workspace's installed `sb.workspace.yaml` (never in a flow).

mod common;
use common::WsSandbox;

#[test]
fn init_empty_dir_writes_manifest_and_io_dir_and_registers() {
    let s = WsSandbox::new();

    // Create + activate workspace `demo`.
    s.cmd().args(["ws", "create", "demo"]).output().unwrap();
    s.cmd().args(["ws", "set", "demo"]).output().unwrap();

    // Empty project dir under the sandbox tempdir.
    let project = s.tempdir().join("pubber");
    std::fs::create_dir_all(&project).unwrap();

    let out = s
        .cmd()
        .args(["init", "--rust"])
        .arg(&project)
        .output()
        .unwrap();
    assert!(out.status.success(), "init failed: {out:?}");

    // 1. sb.module.yaml exists with right shape; no legacy file is written.
    let manifest = project.join(sb_core::MODULE_FILE);
    assert!(manifest.exists(), "{} missing", sb_core::MODULE_FILE);
    let body = std::fs::read_to_string(&manifest).unwrap();
    assert!(body.contains("module: pubber"), "body: {body}");
    assert!(body.contains("language: rust"), "body: {body}");
    assert!(body.contains("io_dir: swarmbotix_io"), "body: {body}");
    assert!(!project.join(sb_core::REMOVED_MODULE_FILE).exists());

    // 2. No runscript.bash: sb launches the module natively.
    let rs = project.join("runscript.bash");
    assert!(!rs.exists(), "sb init wrote runscript.bash");

    // 3. swarmbotix_io/ exists as empty dir.
    let io = project.join("swarmbotix_io");
    assert!(io.is_dir(), "swarmbotix_io/ missing");
    assert!(
        std::fs::read_dir(&io).unwrap().next().is_none(),
        "swarmbotix_io/ should be empty"
    );

    // 4. The installed workspace has pubber → <abs>/sb.module.yaml; the
    //    default flow is untouched.
    let installed = s.sb_home().join("workspaces").join("demo");
    let ws_body = std::fs::read_to_string(installed.join(sb_core::WORKSPACE_FILE)).unwrap();
    let canon_manifest = std::fs::canonicalize(&manifest).unwrap();
    let workspace: sb_core::Workspace = serde_yaml::from_str(&ws_body).unwrap();
    assert_eq!(
        workspace.modules.get("pubber"),
        Some(&canon_manifest),
        "installed workspace should register pubber at its manifest: {ws_body}"
    );
    let flow_body =
        std::fs::read_to_string(installed.join("flows").join(sb_core::DEFAULT_FLOW_FILE)).unwrap();
    assert!(
        !flow_body.contains("pubber"),
        "sb init must not add a node to a flow: {flow_body}"
    );
}
