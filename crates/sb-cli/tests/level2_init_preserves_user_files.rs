//! L2 TDD test #5 — `sb init` preserves user files byte-for-byte.
//!
//! Matrix across Rust / Python / C++ / Flutter / Unity.

mod common;
use common::WsSandbox;
use std::path::Path;

fn setup_active(s: &WsSandbox) {
    s.cmd().args(["ws", "create", "demo"]).output().unwrap();
    s.cmd().args(["ws", "set", "demo"]).output().unwrap();
}

fn assert_files_unchanged(originals: &[(std::path::PathBuf, Vec<u8>)]) {
    for (p, expected) in originals {
        let actual =
            std::fs::read(p).unwrap_or_else(|e| panic!("re-read of {} failed: {e}", p.display()));
        assert_eq!(
            actual.as_slice(),
            expected.as_slice(),
            "{} was modified",
            p.display()
        );
    }
}

fn assert_artifacts_present(project: &Path, io_subpath: &str) {
    let manifest = sb_core::MODULE_FILE;
    assert!(project.join(manifest).exists(), "{manifest} missing");
    assert!(
        !project.join("runscript.bash").exists(),
        "sb init wrote runscript.bash"
    );
    assert!(project.join(io_subpath).is_dir(), "{io_subpath} missing");
}

fn snapshot(paths: &[&Path]) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    paths
        .iter()
        .map(|p| (p.to_path_buf(), std::fs::read(p).unwrap()))
        .collect()
}

#[test]
fn unity_project_assets_untouched() {
    let s = WsSandbox::new();
    setup_active(&s);
    let project = s.tempdir().join("teleop_unity");
    let assets = project.join("Assets").join("Scripts");
    std::fs::create_dir_all(&assets).unwrap();
    let player_cs = assets.join("Player.cs");
    std::fs::write(&player_cs, "// user Unity script\n").unwrap();
    let scene = project.join("Assets").join("Main.unity");
    std::fs::write(&scene, "%YAML 1.1\n").unwrap();

    let before = snapshot(&[&player_cs, &scene]);
    let out = s
        .cmd()
        .args(["init", "--unity"])
        .arg(&project)
        .output()
        .unwrap();
    assert!(out.status.success(), "init failed: {out:?}");

    assert_files_unchanged(&before);
    assert_artifacts_present(&project, "Assets/Scripts/swarmbotix_io");
}
