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
    assert!(project.join("sb.dev.yml").exists(), "sb.dev.yml missing");
    assert!(project.join("runscript.bash").exists(), "runscript missing");
    assert!(project.join(io_subpath).is_dir(), "{io_subpath} missing");
}

fn snapshot(paths: &[&Path]) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    paths
        .iter()
        .map(|p| (p.to_path_buf(), std::fs::read(p).unwrap()))
        .collect()
}

#[test]
fn rust_project_files_untouched() {
    let s = WsSandbox::new();
    setup_active(&s);
    let project = s.tempdir().join("camera_rs");
    std::fs::create_dir_all(project.join("src")).unwrap();
    let cargo_toml = project.join("Cargo.toml");
    let main_rs = project.join("src").join("main.rs");
    std::fs::write(
        &cargo_toml,
        "[package]\nname = \"camera_rs\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(&main_rs, "fn main() { println!(\"hi\"); }\n").unwrap();

    let before = snapshot(&[&cargo_toml, &main_rs]);
    let out = s
        .cmd()
        .args(["init", "--rust"])
        .arg(&project)
        .output()
        .unwrap();
    assert!(out.status.success(), "init failed: {out:?}");

    assert_files_unchanged(&before);
    assert_artifacts_present(&project, "swarmbotix_io");
}

#[test]
fn python_project_files_untouched() {
    let s = WsSandbox::new();
    setup_active(&s);
    let project = s.tempdir().join("detector_py");
    std::fs::create_dir_all(&project).unwrap();
    let pyproject = project.join("pyproject.toml");
    let main_py = project.join("main.py");
    std::fs::write(
        &pyproject,
        "[project]\nname = \"detector\"\nversion = \"0.1\"\n",
    )
    .unwrap();
    std::fs::write(&main_py, "print('hi')\n").unwrap();

    let before = snapshot(&[&pyproject, &main_py]);
    let out = s
        .cmd()
        .args(["init", "--python"])
        .arg(&project)
        .output()
        .unwrap();
    assert!(out.status.success(), "init failed: {out:?}");

    assert_files_unchanged(&before);
    assert_artifacts_present(&project, "swarmbotix_io");
}

#[test]
fn cpp_project_files_untouched() {
    let s = WsSandbox::new();
    setup_active(&s);
    let project = s.tempdir().join("planner_cpp");
    std::fs::create_dir_all(&project).unwrap();
    let cmakelists = project.join("CMakeLists.txt");
    std::fs::write(
        &cmakelists,
        "cmake_minimum_required(VERSION 3.20)\nproject(planner)\n",
    )
    .unwrap();
    let main_cpp = project.join("main.cpp");
    std::fs::write(&main_cpp, "int main() { return 0; }\n").unwrap();

    let before = snapshot(&[&cmakelists, &main_cpp]);
    let out = s
        .cmd()
        .args(["init", "--cpp"])
        .arg(&project)
        .output()
        .unwrap();
    assert!(out.status.success(), "init failed: {out:?}");

    assert_files_unchanged(&before);
    assert_artifacts_present(&project, "swarmbotix_io");
}

#[test]
fn flutter_project_files_untouched() {
    let s = WsSandbox::new();
    setup_active(&s);
    let project = s.tempdir().join("tablet_flutter");
    std::fs::create_dir_all(project.join("lib")).unwrap();
    let pubspec = project.join("pubspec.yaml");
    std::fs::write(&pubspec, "name: tablet\nversion: 0.1.0\n").unwrap();
    let main_dart = project.join("lib").join("main.dart");
    std::fs::write(&main_dart, "void main() {}\n").unwrap();

    let before = snapshot(&[&pubspec, &main_dart]);
    let out = s
        .cmd()
        .args(["init", "--flutter"])
        .arg(&project)
        .output()
        .unwrap();
    assert!(out.status.success(), "init failed: {out:?}");

    assert_files_unchanged(&before);
    assert_artifacts_present(&project, "swarmbotix_io");
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
