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
        .join(sb_core::FLOW_FILE);
    let flow_body = std::fs::read_to_string(&flow).unwrap();
    assert!(
        flow_body.contains("pubber"),
        "installed flow missing pubber: {flow_body}"
    );
}
