//! L2 — `sb init --docker` emits a docker-flavored runscript from
//! `flow.yaml::instances`, fails actionably on missing instances, is
//! force-regeneratable, and surfaces a staleness warning to stderr.

mod common;
use common::{WsSandbox, bash_available, bash_script};
use std::path::Path;

fn setup_active_with_instances(s: &WsSandbox, module: &str, project: &Path) {
    s.cmd().args(["ws", "create", "demo"]).output().unwrap();
    s.cmd().args(["ws", "set", "demo"]).output().unwrap();
    // Pre-seed flow.yaml with two instances so `init --docker` can
    // resolve them. The module entry itself is appended by `sb init`,
    // but `instances:` is hand-edited (no mgmt subcommand at this stage).
    let dev_yml = project.join("sb.dev.yml");
    let body = format!(
        "modules:\n  {module}: {dev}\n\
         instances:\n  {module}:\n    \
         - id: left\n      label: Left Camera\n      docker:\n        image: cam:latest\n        env:\n          CAM_SERIAL: A123\n          LOG_LEVEL: info\n        devices:\n        - /dev/video0\n        gpus: all\n    \
         - id: right\n      docker:\n        image: cam:latest\n        env:\n          CAM_SERIAL: B456\n        devices:\n        - /dev/video1\n",
        module = module,
        dev = dev_yml.display(),
    );
    let flow_path = s
        .sb_home()
        .join("workspaces")
        .join("demo")
        .join("flow.yaml");
    std::fs::write(&flow_path, body).unwrap();
}

#[test]
fn init_docker_emits_runscript_with_branches() {
    let s = WsSandbox::new();
    let project = s.tempdir().join("cam_gige_ht");
    std::fs::create_dir_all(&project).unwrap();
    setup_active_with_instances(&s, "cam_gige_ht", &project);

    let out = s
        .cmd()
        .args(["init", "--cpp", "--docker"])
        .arg(&project)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "init --docker failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    let rs = project.join("runscript.bash");
    let body = std::fs::read_to_string(&rs).unwrap();
    // Header marker for the docker variant.
    assert!(
        body.contains("regenerate with: sb init --docker --force"),
        "missing docker header:\n{body}"
    );
    // Both case branches present.
    assert!(body.contains("'left')"), "left branch missing:\n{body}");
    assert!(body.contains("'right')"), "right branch missing:\n{body}");
    // Container names use <module>-<id>.
    assert!(body.contains("--name 'cam_gige_ht-left'"));
    assert!(body.contains("--name 'cam_gige_ht-right'"));
    // Env / device / gpu flags baked in.
    assert!(body.contains("'CAM_SERIAL=A123'"));
    assert!(body.contains("'CAM_SERIAL=B456'"));
    assert!(body.contains("--device '/dev/video0'"));
    assert!(body.contains("--device '/dev/video1'"));
    assert!(body.contains("--gpus 'all'"));
    // Image then "$@".
    assert!(body.contains("'cam:latest' \"$@\""));
    // Known-id list in error message.
    assert!(body.contains("known: left right"));
    // Staleness check embedded.
    assert!(body.contains("instances may be stale"));
    // Executable bit set.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&rs).unwrap().permissions().mode();
        assert!(
            mode & 0o111 != 0,
            "runscript not executable (mode {mode:o})"
        );
    }
}

#[test]
fn init_docker_requires_instances_block() {
    let s = WsSandbox::new();
    let project = s.tempdir().join("cam_gige_ht");
    std::fs::create_dir_all(&project).unwrap();
    s.cmd().args(["ws", "create", "demo"]).output().unwrap();
    s.cmd().args(["ws", "set", "demo"]).output().unwrap();
    // Note: no `instances:` block pre-seeded.

    let out = s
        .cmd()
        .args(["init", "--cpp", "--docker"])
        .arg(&project)
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "should have failed without instances"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--docker requires at least one instance"),
        "missing actionable error in stderr: {stderr}"
    );
    assert!(
        stderr.contains("flow.yaml") && stderr.contains("sbcli_docker_runscript.md"),
        "error should reference flow.yaml and the consuming doc: {stderr}"
    );
    // Runscript must NOT have been written.
    assert!(!project.join("runscript.bash").exists());
}

#[test]
fn init_docker_force_regenerates_from_updated_flow() {
    let s = WsSandbox::new();
    let project = s.tempdir().join("cam_gige_ht");
    std::fs::create_dir_all(&project).unwrap();
    setup_active_with_instances(&s, "cam_gige_ht", &project);

    let out = s
        .cmd()
        .args(["init", "--cpp", "--docker"])
        .arg(&project)
        .output()
        .unwrap();
    assert!(out.status.success(), "first init: {out:?}");
    let v1 = std::fs::read_to_string(project.join("runscript.bash")).unwrap();
    assert!(v1.contains("'right')"));

    // Rewrite flow.yaml with only `left`.
    let dev_yml = project.join("sb.dev.yml");
    let body = format!(
        "modules:\n  cam_gige_ht: {dev}\n\
         instances:\n  cam_gige_ht:\n    \
         - id: left\n      docker:\n        image: cam:v2\n",
        dev = dev_yml.display(),
    );
    let flow_path = s
        .sb_home()
        .join("workspaces")
        .join("demo")
        .join("flow.yaml");
    std::fs::write(&flow_path, body).unwrap();

    // Without --force, no rewrite (runscript already exists).
    let out2 = s
        .cmd()
        .args(["init", "--cpp", "--docker"])
        .arg(&project)
        .output()
        .unwrap();
    assert!(out2.status.success(), "second init (no force): {out2:?}");
    let v2 = std::fs::read_to_string(project.join("runscript.bash")).unwrap();
    assert_eq!(v1, v2, "runscript should be unchanged without --force");

    // With --force, regenerate from new flow.yaml.
    let out3 = s
        .cmd()
        .args(["init", "--cpp", "--docker", "--force"])
        .arg(&project)
        .output()
        .unwrap();
    assert!(out3.status.success(), "third init (force): {out3:?}");
    let v3 = std::fs::read_to_string(project.join("runscript.bash")).unwrap();
    assert!(v3.contains("'left')"));
    assert!(
        !v3.contains("'right')"),
        "right should be gone after --force regen"
    );
    assert!(v3.contains("'cam:v2'"), "new image should appear");
}

#[test]
fn docker_runscript_unknown_id_exits_nonzero_with_help() {
    let s = WsSandbox::new();
    let project = s.tempdir().join("cam_gige_ht");
    std::fs::create_dir_all(&project).unwrap();
    setup_active_with_instances(&s, "cam_gige_ht", &project);

    let init = s
        .cmd()
        .args(["init", "--cpp", "--docker"])
        .arg(&project)
        .output()
        .unwrap();
    assert!(init.status.success());

    // Run the runscript directly; skip the tmux wrap so this is hermetic.
    // The docker variant doesn't have a tmux wrap, but SB_NO_TMUX is also
    // harmless if no wrap is present.
    if !bash_available() {
        eprintln!("no Git Bash on this host — skipping runscript execution");
        return;
    }
    let rs = project.join("runscript.bash");
    let out = bash_script(&rs)
        .args(["--id", "ghost"])
        .env("SB_NO_TMUX", "1")
        .output()
        .unwrap();
    assert!(!out.status.success(), "should fail on unknown id");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unknown --id 'ghost'") && stderr.contains("known: left right"),
        "stderr did not name the unknown id and known list: {stderr}"
    );
}

#[test]
fn docker_runscript_missing_id_exits_nonzero() {
    let s = WsSandbox::new();
    let project = s.tempdir().join("cam_gige_ht");
    std::fs::create_dir_all(&project).unwrap();
    setup_active_with_instances(&s, "cam_gige_ht", &project);

    let init = s
        .cmd()
        .args(["init", "--cpp", "--docker"])
        .arg(&project)
        .output()
        .unwrap();
    assert!(init.status.success());

    if !bash_available() {
        eprintln!("no Git Bash on this host — skipping runscript execution");
        return;
    }
    let rs = project.join("runscript.bash");
    let out = bash_script(&rs).env("SB_NO_TMUX", "1").output().unwrap();
    assert!(!out.status.success(), "should fail without --id");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--id <name> required") && stderr.contains("known: left right"),
        "stderr should explain required --id: {stderr}"
    );
}

#[test]
fn docker_runscript_warns_when_flow_yaml_is_newer() {
    let s = WsSandbox::new();
    let project = s.tempdir().join("cam_gige_ht");
    std::fs::create_dir_all(&project).unwrap();
    setup_active_with_instances(&s, "cam_gige_ht", &project);

    let init = s
        .cmd()
        .args(["init", "--cpp", "--docker"])
        .arg(&project)
        .output()
        .unwrap();
    assert!(init.status.success());

    if !bash_available() {
        eprintln!("no Git Bash on this host — skipping runscript execution");
        return;
    }
    // Touch flow.yaml so its mtime is strictly newer than the runscript's.
    let flow_path = s
        .sb_home()
        .join("workspaces")
        .join("demo")
        .join("flow.yaml");
    let rs = project.join("runscript.bash");
    // Ensure flow.yaml mtime > rs mtime: rewrite flow.yaml after a tiny sleep
    // (filesystem mtime resolution is 1s on some systems; on ext4 it's ns).
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let body = std::fs::read_to_string(&flow_path).unwrap();
    std::fs::write(&flow_path, body).unwrap();

    // Run the script with an unknown id so it exits before invoking docker
    // (which may not be installed in the test environment). The staleness
    // check fires before the --id branch dispatch.
    let out = bash_script(&rs)
        .args(["--id", "ghost"])
        .env("SB_NO_TMUX", "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("instances may be stale") && stderr.contains("sb init --docker --force"),
        "expected staleness warning in stderr: {stderr}"
    );
}
