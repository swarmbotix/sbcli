//! L2 TDD test #10 — refuse-to-overwrite vs `--force`.

mod common;
use common::WsSandbox;

const HAND_EDITED_DEV_YML: &str = "\
# user-added comment that bare init must preserve
module: pubber
language: rust
root: /will/be/replaced/by/force
io_dir: swarmbotix_io
publishers: []
subscribers: []
";

#[test]
fn bare_init_preserves_existing_files_but_registers() {
    let s = WsSandbox::new();
    s.cmd().args(["ws", "create", "demo"]).output().unwrap();
    s.cmd().args(["ws", "set", "demo"]).output().unwrap();

    let project = s.tempdir().join("pubber");
    std::fs::create_dir_all(&project).unwrap();
    let dev_yml = project.join("sb.dev.yml");
    let runscript = project.join("runscript.bash");
    std::fs::write(&dev_yml, HAND_EDITED_DEV_YML).unwrap();
    std::fs::write(&runscript, "#!/usr/bin/env bash\necho user-script\n").unwrap();

    let out = s.cmd().args(["init"]).arg(&project).output().unwrap();
    assert!(out.status.success(), "bare init failed: {out:?}");

    assert_eq!(
        std::fs::read_to_string(&dev_yml).unwrap(),
        HAND_EDITED_DEV_YML
    );
    assert_eq!(
        std::fs::read_to_string(&runscript).unwrap(),
        "#!/usr/bin/env bash\necho user-script\n"
    );

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
}

#[test]
fn force_rewrites_dev_yml_and_runscript() {
    let s = WsSandbox::new();
    s.cmd().args(["ws", "create", "demo"]).output().unwrap();
    s.cmd().args(["ws", "set", "demo"]).output().unwrap();

    let project = s.tempdir().join("pubber");
    std::fs::create_dir_all(&project).unwrap();
    let dev_yml = project.join("sb.dev.yml");
    let runscript = project.join("runscript.bash");
    std::fs::write(&dev_yml, HAND_EDITED_DEV_YML).unwrap();
    std::fs::write(&runscript, "#!/usr/bin/env bash\necho user-script\n").unwrap();

    let out = s
        .cmd()
        .args(["init", "--force", "--rust"])
        .arg(&project)
        .output()
        .unwrap();
    assert!(out.status.success(), "force init failed: {out:?}");

    let dev_body = std::fs::read_to_string(&dev_yml).unwrap();
    assert!(
        !dev_body.contains("user-added comment that bare init must preserve"),
        "user comment should be gone after --force"
    );
    let canon = std::fs::canonicalize(&project).unwrap();
    assert!(
        dev_body.contains(canon.to_str().unwrap()),
        "rewritten dev_yml should carry real abs root: {dev_body}"
    );

    let rs_body = std::fs::read_to_string(&runscript).unwrap();
    assert!(
        rs_body.contains("TODO"),
        "runscript should be canonical stub: {rs_body}"
    );
    assert!(!rs_body.contains("echo user-script"));
}
