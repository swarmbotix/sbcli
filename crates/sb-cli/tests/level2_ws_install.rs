//! `sb install <folder|url>` on a workspace package (a root
//! `sb.workspace.yaml` plus its flow files), and `sb ws uninstall`.

mod common;
use common::WsSandbox;

use std::path::{Path, PathBuf};
use std::process::Output;

use sb_core::{DEFAULT_FLOW_FILE, MODULE_FILE, WORKSPACE_FILE, Workspace};

const WORKSPACE: &str = "\
name: ws_fix
modules:
  pubber: pubber
  calib: calib
messages:
  custom:
    style: ros2
    path: messages/custom
    backends: [proto]
";

/// The default flow runs pubber; calib runs in its own flow.
const FLOW: &str = "nodes: [pubber]\n";
const CALIB_FLOW: &str = "nodes: [calib]\n";

const PING_V1: &str = "syntax = \"proto3\";\n\npackage swarmbotix.custom;\n\n\
                       message Ping {\n  uint32 seq = 1;\n}\n";

/// A workspace repository at `<tempdir>/<dir>`: two modules, two flows
/// (`default` runs pubber, `calib` runs calib), and one message source
/// holding `custom/Ping.proto`.
fn fixture_repo(s: &WsSandbox, dir: &str) -> PathBuf {
    let repo = s.tempdir().join(dir);
    for (path, body) in [
        (repo.join(WORKSPACE_FILE), WORKSPACE),
        (repo.join(DEFAULT_FLOW_FILE), FLOW),
        (repo.join("calib.flow.yaml"), CALIB_FLOW),
        (
            repo.join("pubber").join(MODULE_FILE),
            "module: pubber\nlanguage: rust\n",
        ),
        (
            repo.join("calib").join(MODULE_FILE),
            "module: calib\nlanguage: python\n",
        ),
        (repo.join("messages/custom/Ping.proto"), PING_V1),
    ] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
    repo
}

/// Whether the sandbox config names a protoc that exists; the message
/// compile step needs it.
fn protoc_configured(s: &WsSandbox) -> bool {
    let cfg = std::fs::read_to_string(s.config_path()).unwrap_or_default();
    cfg.lines().any(|l| {
        l.trim_start()
            .strip_prefix("protoc:")
            .and_then(|rest| rest.split_whitespace().next())
            .is_some_and(|p| Path::new(p).is_file())
    })
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn ok(out: &Output, what: &str) -> String {
    assert!(
        out.status.success(),
        "{what} failed\nstdout:\n{}\nstderr:\n{}",
        text(&out.stdout),
        text(&out.stderr)
    );
    text(&out.stdout)
}

fn installed(s: &WsSandbox, name: &str) -> Workspace {
    let body = std::fs::read_to_string(
        s.sb_home()
            .join("workspaces")
            .join(name)
            .join(WORKSPACE_FILE),
    )
    .unwrap();
    serde_yaml::from_str(&body).unwrap()
}

#[test]
fn install_workspace_folder_then_reinstall_then_uninstall_clean() {
    let s = WsSandbox::new();
    if !protoc_configured(&s) {
        eprintln!("skipping: no protoc in the sandbox config");
        return;
    }
    let repo = fixture_repo(&s, "ws_repo");
    let canon = repo.canonicalize().unwrap();

    let out = s.cmd().arg("install").arg(&repo).output().unwrap();
    let stdout = ok(&out, "sb install <folder>");
    assert_eq!(
        stdout,
        "-- workspace ws_fix: created, active\n\
         -- modules: 2 registered\n\
         -- flows: default, calib\n\
         -- messages: ros2 <- messages/custom: 1 imported, 0 unchanged, 0 conflicts; \
         compiled [proto]\n\
         -- io: calib 0 written, 0 unchanged\n\
         -- io: pubber 0 written, 0 unchanged\n\
         done. next: sb up\n"
    );

    // Installed copy: absolute paths and provenance in sb.workspace.yaml,
    // each flow copied as it is under flows/.
    let workspace = installed(&s, "ws_fix");
    assert_eq!(workspace.name.as_deref(), Some("ws_fix"));
    assert_eq!(
        workspace.modules["pubber"],
        canon.join("pubber").join(MODULE_FILE)
    );
    assert!(workspace.modules.values().all(|p| p.is_absolute()));
    assert_eq!(
        workspace.messages[0].path,
        canon.join("messages").join("custom")
    );
    let src = workspace
        .source
        .clone()
        .expect("installed copy records its source");
    assert_eq!(src.repo_workspace, canon.join(WORKSPACE_FILE));
    assert_eq!(src.url, None);
    let flows = s.sb_home().join("workspaces/ws_fix/flows");
    assert_eq!(
        std::fs::read_to_string(flows.join(DEFAULT_FLOW_FILE)).unwrap(),
        FLOW
    );
    assert_eq!(
        std::fs::read_to_string(flows.join("calib.flow.yaml")).unwrap(),
        CALIB_FLOW
    );
    // Nothing deprecated in this repo, so nothing is announced.
    assert!(
        !text(&out.stderr).contains("is deprecated since"),
        "{}",
        text(&out.stderr)
    );

    // Active, and the message is in the configured vault.
    let active = std::fs::read_to_string(s.sb_home().join("active")).unwrap();
    assert_eq!(active.trim(), "ws_fix");
    let vault_copy = s
        .sb_home()
        .join("messages/ros2/message_definitions/custom/Ping.proto");
    assert_eq!(std::fs::read_to_string(&vault_copy).unwrap(), PING_V1);

    // A second install is an update that changes nothing.
    let out = s.cmd().arg("install").arg(&repo).output().unwrap();
    let stdout = ok(&out, "second sb install");
    assert!(
        stdout.starts_with("-- workspace ws_fix: updated, active\n"),
        "{stdout}"
    );
    assert!(
        stdout.contains("ros2 <- messages/custom: 0 imported, 1 unchanged, 0 conflicts"),
        "{stdout}"
    );
    let again = installed(&s, "ws_fix");
    assert_eq!(again.modules, workspace.modules);

    // A changed message conflicts until --force.
    std::fs::write(
        repo.join("messages/custom/Ping.proto"),
        PING_V1.replace("uint32 seq = 1;", "uint32 seq = 1;\n  string note = 2;"),
    )
    .unwrap();
    let out = s.cmd().arg("install").arg(&repo).output().unwrap();
    assert!(!out.status.success(), "a conflict must fail the install");
    assert!(
        text(&out.stdout).contains("1 conflicts; not compiled"),
        "{out:?}"
    );
    assert!(text(&out.stderr).contains("--force"), "{out:?}");
    assert_eq!(std::fs::read_to_string(&vault_copy).unwrap(), PING_V1);
    let out = s
        .cmd()
        .arg("install")
        .arg(&repo)
        .arg("--force")
        .output()
        .unwrap();
    ok(&out, "sb install --force");
    assert!(
        std::fs::read_to_string(&vault_copy)
            .unwrap()
            .contains("note")
    );

    // The workspace checks clean.
    let out = s.cmd().args(["ws", "check"]).output().unwrap();
    let stdout = ok(&out, "sb ws check");
    assert!(!stdout.contains("[FAIL]"), "{stdout}");

    // Uninstall --clean: workspace dir and io dirs go, the repo stays.
    std::fs::create_dir_all(repo.join("pubber/swarmbotix_io/publishers")).unwrap();
    let out = s
        .cmd()
        .args(["ws", "uninstall", "ws_fix", "--clean"])
        .output()
        .unwrap();
    let stdout = ok(&out, "sb ws uninstall --clean");
    assert!(
        stdout.starts_with("uninstalled workspace ws_fix"),
        "{stdout}"
    );
    assert!(!s.sb_home().join("workspaces").join("ws_fix").exists());
    assert!(!repo.join("pubber/swarmbotix_io").exists());
    assert!(repo.join(WORKSPACE_FILE).is_file());
    let active = std::fs::read_to_string(s.sb_home().join("active")).unwrap();
    assert!(active.trim().is_empty(), "{active:?}");
}

#[test]
fn install_from_git_url_clones_into_repos_and_updates_in_place() {
    let s = WsSandbox::new();
    if !protoc_configured(&s) || which::which("git").is_err() {
        eprintln!("skipping: needs protoc in the sandbox config and git on PATH");
        return;
    }
    let repo = fixture_repo(&s, "upstream");
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    };
    git(&["init", "-q"]);
    git(&["add", "-A"]);
    git(&["commit", "-qm", "init"]);
    let url = format!("file://{}", repo.canonicalize().unwrap().display());

    let out = s.cmd().args(["install", &url]).output().unwrap();
    let stdout = ok(&out, "sb install <url>");
    assert!(
        stdout.starts_with("-- workspace ws_fix: created, active\n"),
        "{stdout}"
    );
    let clone = s.sb_home().join("repos").join("ws_fix");
    assert!(
        clone.join(WORKSPACE_FILE).is_file(),
        "no clone at {}",
        clone.display()
    );
    let src = installed(&s, "ws_fix").source.unwrap();
    assert_eq!(src.url.as_deref(), Some(url.as_str()));
    assert!(
        src.repo_workspace
            .starts_with(clone.canonicalize().unwrap())
    );

    // Installing the same URL again pulls the clone and reinstalls.
    let out = s.cmd().args(["install", &url]).output().unwrap();
    let stdout = ok(&out, "second sb install <url>");
    assert!(
        stdout.starts_with("-- workspace ws_fix: updated, active\n"),
        "{stdout}"
    );

    // --clean also removes the clone sb made.
    let out = s
        .cmd()
        .args(["ws", "uninstall", "ws_fix", "--clean"])
        .output()
        .unwrap();
    let stdout = ok(&out, "sb ws uninstall --clean");
    assert!(stdout.contains("removed clone"), "{stdout}");
    assert!(!clone.exists());
    assert!(
        repo.join(WORKSPACE_FILE).is_file(),
        "the upstream repo is untouched"
    );
}
