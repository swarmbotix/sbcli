//! L2 TDD test #3 — `sb init --rust ./pubber` writes the three artifacts
//! and registers the module in the active workspace's flow.yaml.

mod common;
use common::WsSandbox;

#[test]
fn init_empty_dir_writes_three_artifacts_and_registers() {
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

    // 1. sb.dev.yml exists with right shape.
    let dev_yml = project.join("sb.dev.yml");
    assert!(dev_yml.exists(), "sb.dev.yml missing");
    let body = std::fs::read_to_string(&dev_yml).unwrap();
    assert!(body.contains("module: pubber"), "body: {body}");
    assert!(body.contains("language: rust"), "body: {body}");
    assert!(body.contains("io_dir: swarmbotix_io"), "body: {body}");

    // 2. runscript.bash exists + executable bit set.
    let rs = project.join("runscript.bash");
    assert!(rs.exists(), "runscript.bash missing");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&rs).unwrap().permissions().mode();
        assert!(
            mode & 0o111 != 0,
            "runscript not executable (mode {mode:o})"
        );
    }

    // 3. swarmbotix_io/ exists as empty dir.
    let io = project.join("swarmbotix_io");
    assert!(io.is_dir(), "swarmbotix_io/ missing");
    assert!(
        std::fs::read_dir(&io).unwrap().next().is_none(),
        "swarmbotix_io/ should be empty"
    );

    // 4. flow.yaml has pubber → <abs>/sb.dev.yml.
    let flow = s
        .sb_home()
        .join("workspaces")
        .join("demo")
        .join("flow.yaml");
    let flow_body = std::fs::read_to_string(&flow).unwrap();
    assert!(
        flow_body.contains("pubber"),
        "flow.yaml missing pubber: {flow_body}"
    );
    let canon_dev = std::fs::canonicalize(&dev_yml).unwrap();
    assert!(
        flow_body.contains(canon_dev.to_str().unwrap()),
        "flow.yaml missing absolute path to sb.dev.yml: {flow_body}"
    );
}
