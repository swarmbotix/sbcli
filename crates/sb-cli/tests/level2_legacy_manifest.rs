//! A 0.5 workspace (`flow.yaml`, `sb.dev.yml`, `runscript.bash`) still
//! installs under 0.6, unchanged on disk, and `sb install` announces each
//! legacy file name as deprecated on stderr.

mod common;
use common::WsSandbox;

use sb_core::{FLOW_FILE, LEGACY_FLOW_FILE, LEGACY_MODULE_FILE, MODULE_FILE, WorkspaceFlow};

#[test]
fn legacy_names_install_with_deprecation_warnings() {
    let s = WsSandbox::new();
    let repo = s.tempdir().join("legacy_repo");
    let module = repo.join("pubber");
    for (path, body) in [
        (
            repo.join(LEGACY_FLOW_FILE),
            "name: ws_legacy\nmodules:\n  pubber: pubber\n",
        ),
        (
            module.join(LEGACY_MODULE_FILE),
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

    let out = s.cmd().arg("install").arg(&repo).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "sb install failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.starts_with("-- workspace ws_legacy: created, active\n"),
        "{stdout}"
    );
    assert!(stderr.contains("sb.dev.yml is deprecated"), "{stderr}");
    assert!(stderr.contains("flow.yaml is deprecated"), "{stderr}");

    // The installed copy takes the new name and points at the legacy
    // manifest, which is used as is.
    let installed = s
        .sb_home()
        .join("workspaces")
        .join("ws_legacy")
        .join(FLOW_FILE);
    let flow: WorkspaceFlow =
        serde_yaml::from_str(&std::fs::read_to_string(installed).unwrap()).unwrap();
    assert_eq!(
        flow.modules["pubber"].path,
        module.canonicalize().unwrap().join(LEGACY_MODULE_FILE)
    );

    // Nothing in the repo is renamed or added.
    assert!(repo.join(LEGACY_FLOW_FILE).is_file());
    assert!(!repo.join(FLOW_FILE).exists());
    assert!(module.join(LEGACY_MODULE_FILE).is_file());
    assert!(!module.join(MODULE_FILE).exists());
}
