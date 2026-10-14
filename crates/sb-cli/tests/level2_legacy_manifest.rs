//! Old repository shapes on `sb install`.
//!
//! sb 0.7.0 no longer reads the 0.5 names: `sb install` refuses a repo
//! whose flow is still `flow.yaml`, and, once the flow is renamed, one
//! whose module still has `sb.dev.yml`; each refusal carries the rename
//! hint on stderr, and nothing in the repo is renamed or installed.
//!
//! sb 0.8.0 still installs a `sb.flow.yaml` in the 0.7 shape (modules and
//! wiring in one file), converted into a workspace and its default flow,
//! with a deprecation warning on stderr.

mod common;
use common::WsSandbox;

use sb_core::{
    DEFAULT_FLOW_FILE, MODULE_FILE, REMOVED_FLOW_FILE, REMOVED_MODULE_FILE, WORKSPACE_FILE,
};

#[test]
fn legacy_names_are_refused_and_a_0_7_flow_installs_with_a_deprecation() {
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
    let installed = s.sb_home().join("workspaces").join("ws_legacy");

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

    // 2. The manifest name, once the flow carries its 0.7 name.
    std::fs::rename(repo.join(REMOVED_FLOW_FILE), repo.join(DEFAULT_FLOW_FILE)).unwrap();
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
    assert!(!installed.exists());

    // 3. With the manifest renamed, the 0.7 flow installs: converted into
    //    an installed workspace and default flow, deprecated on stderr, the
    //    repo left as it is.
    std::fs::rename(module.join(REMOVED_MODULE_FILE), module.join(MODULE_FILE)).unwrap();
    let out = install();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "sb install of a 0.7 flow failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains(
            "warning: sb.flow.yaml in the 0.7 shape is deprecated since 0.8.0 and will be \
             removed in 0.8.1; split it into sb.workspace.yaml and sb.flow.yaml; see \
             sbcli_workspace.md §Migrating"
        ),
        "{stderr}"
    );
    assert!(stdout.contains("-- flows: default\n"), "{stdout}");
    let workspace: sb_core::Workspace =
        serde_yaml::from_str(&std::fs::read_to_string(installed.join(WORKSPACE_FILE)).unwrap())
            .unwrap();
    assert_eq!(
        workspace.modules.get("pubber"),
        Some(&module.canonicalize().unwrap().join(MODULE_FILE))
    );
    let flow: sb_core::Flow = serde_yaml::from_str(
        &std::fs::read_to_string(installed.join("flows").join(DEFAULT_FLOW_FILE)).unwrap(),
    )
    .unwrap();
    assert_eq!(flow.nodes, ["pubber"]);
    assert!(!repo.join(WORKSPACE_FILE).exists(), "the repo is not split");
}
