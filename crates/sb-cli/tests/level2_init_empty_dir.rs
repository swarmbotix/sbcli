//! L2 TDD test #3 — `sb init --rust ./pubber` writes `sb.module.yaml` and
//! an empty IO directory (no runscript), and registers the module in the
//! active workspace's installed flow.

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

    // 4. The installed flow has pubber → <abs>/sb.module.yaml.
    let flow = s
        .sb_home()
        .join("workspaces")
        .join("demo")
        .join(sb_core::FLOW_FILE);
    let flow_body = std::fs::read_to_string(&flow).unwrap();
    assert!(
        flow_body.contains("pubber"),
        "installed flow missing pubber: {flow_body}"
    );
    let canon_manifest = std::fs::canonicalize(&manifest).unwrap();
    assert!(
        flow_body.contains(canon_manifest.to_str().unwrap()),
        "installed flow missing absolute path to the manifest: {flow_body}"
    );
}
