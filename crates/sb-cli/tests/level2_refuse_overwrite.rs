//! L2 TDD test #10 — refuse-to-overwrite vs `--force`.

mod common;
use common::WsSandbox;

const HAND_EDITED_MANIFEST: &str = "\
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
    // `runscript.bash` is an ordinary user file to sb since 0.7.0; init
    // must leave it alone like any other.
    let manifest = project.join(sb_core::MODULE_FILE);
    let runscript = project.join("runscript.bash");
    std::fs::write(&manifest, HAND_EDITED_MANIFEST).unwrap();
    std::fs::write(&runscript, "#!/usr/bin/env bash\necho user-script\n").unwrap();

    let out = s.cmd().args(["init"]).arg(&project).output().unwrap();
    assert!(out.status.success(), "bare init failed: {out:?}");

    assert_eq!(
        std::fs::read_to_string(&manifest).unwrap(),
        HAND_EDITED_MANIFEST
    );
    assert_eq!(
        std::fs::read_to_string(&runscript).unwrap(),
        "#!/usr/bin/env bash\necho user-script\n"
    );

    let installed = s
        .sb_home()
        .join("workspaces")
        .join("demo")
        .join(sb_core::WORKSPACE_FILE);
    let ws_body = std::fs::read_to_string(&installed).unwrap();
    assert!(
        ws_body.contains("pubber"),
        "installed workspace missing pubber: {ws_body}"
    );
}
