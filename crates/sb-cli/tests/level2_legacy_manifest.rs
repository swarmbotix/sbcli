//! sb 0.7.0 no longer reads the 0.5 names. `sb install` refuses a repo
//! whose flow is still `flow.yaml`, and, once the flow is renamed, one
//! whose module still has `sb.dev.yml`; each refusal carries the rename
//! hint on stderr. Nothing in the repo is renamed and no workspace is
//! installed.

mod common;
use common::WsSandbox;

use sb_core::{FLOW_FILE, MODULE_FILE, REMOVED_FLOW_FILE, REMOVED_MODULE_FILE};

#[test]
fn legacy_names_are_refused() {
    let s = WsSandbox::new();
    let repo = s.tempdir().join("legacy_repo");
    let module = repo.join("pubber");
    for (path, body) in [
        (
            repo.join(REMOVED_FLOW_FILE),
            "name: ws_legacy\nmodules:\n  pubber: pubber\n",
        ),
        (
            module.join(REMOVED_MODULE_FILE),
            "module: pubber\nlanguage: rust\n",
        ),
        (
            module.join("runscript.bash"),
            "#!/usr/bin/env bash\nexit 0\n",
        ),
    ] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
    let install = || s.cmd().arg("install").arg(&repo).output().unwrap();

    // 1. The flow name.
    let out = install();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "sb install accepted flow.yaml\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("flow.yaml is no longer read since 0.7.0"),
        "{stderr}"
    );

    // 2. The manifest name, once the flow carries its 0.6 name.
    std::fs::rename(repo.join(REMOVED_FLOW_FILE), repo.join(FLOW_FILE)).unwrap();
    let out = install();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "sb install accepted sb.dev.yml\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("sb.dev.yml is no longer read since 0.7.0"),
        "{stderr}"
    );

    // Nothing in the repo is renamed or added, and nothing is installed.
    assert!(module.join(REMOVED_MODULE_FILE).is_file());
    assert!(!module.join(MODULE_FILE).exists());
    assert!(
        !s.sb_home()
            .join("workspaces")
            .join("ws_legacy")
            .exists()
    );
}
