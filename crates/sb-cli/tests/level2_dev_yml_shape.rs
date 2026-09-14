//! L2 TDD test #7 — `sb.dev.yml` shape per language (serde round-trip).

mod common;
use common::WsSandbox;
use sb_core::{Language, ModuleDevConfig};
use std::path::Path;

fn setup_active(s: &WsSandbox) {
    s.cmd().args(["ws", "create", "demo"]).output().unwrap();
    s.cmd().args(["ws", "set", "demo"]).output().unwrap();
}

fn init_at(s: &WsSandbox, project: &Path, flag: &str) {
    let out = s.cmd().args(["init", flag]).arg(project).output().unwrap();
    assert!(out.status.success(), "init {flag} failed: {out:?}");
}

fn parse_dev_yml(project: &Path) -> ModuleDevConfig {
    let body = std::fs::read_to_string(project.join("sb.dev.yml")).unwrap();
    serde_yaml::from_str(&body).expect("sb.dev.yml must parse into ModuleDevConfig")
}

#[test]
fn rust_shape() {
    let s = WsSandbox::new();
    setup_active(&s);
    let p = s.tempdir().join("camera_rs");
    std::fs::create_dir_all(&p).unwrap();
    init_at(&s, &p, "--rust");
    let cfg = parse_dev_yml(&p);
    assert_eq!(cfg.module, "camera_rs");
    assert_eq!(cfg.language, Language::Rust);
    assert_eq!(cfg.io_dir, std::path::PathBuf::from("swarmbotix_io"));
    assert!(cfg.publishers.is_empty());
    assert!(cfg.subscribers.is_empty());
}

#[test]
fn python_shape() {
    let s = WsSandbox::new();
    setup_active(&s);
    let p = s.tempdir().join("detector");
    std::fs::create_dir_all(&p).unwrap();
    init_at(&s, &p, "--python");
    let cfg = parse_dev_yml(&p);
    assert_eq!(cfg.language, Language::Python);
    assert_eq!(cfg.io_dir, std::path::PathBuf::from("swarmbotix_io"));
}

#[test]
fn cpp_shape() {
    let s = WsSandbox::new();
    setup_active(&s);
    let p = s.tempdir().join("planner");
    std::fs::create_dir_all(&p).unwrap();
    init_at(&s, &p, "--cpp");
    let cfg = parse_dev_yml(&p);
    assert_eq!(cfg.language, Language::Cpp);
    assert_eq!(cfg.io_dir, std::path::PathBuf::from("swarmbotix_io"));
}

#[test]
fn flutter_shape() {
    let s = WsSandbox::new();
    setup_active(&s);
    let p = s.tempdir().join("tablet");
    std::fs::create_dir_all(&p).unwrap();
    init_at(&s, &p, "--flutter");
    let cfg = parse_dev_yml(&p);
    assert_eq!(cfg.language, Language::Flutter);
    assert_eq!(cfg.io_dir, std::path::PathBuf::from("swarmbotix_io"));
}

#[test]
fn unity_shape_io_dir_under_assets() {
    let s = WsSandbox::new();
    setup_active(&s);
    let p = s.tempdir().join("teleop");
    std::fs::create_dir_all(p.join("Assets").join("Scripts")).unwrap();
    init_at(&s, &p, "--unity");
    let cfg = parse_dev_yml(&p);
    assert_eq!(cfg.language, Language::Unity);
    assert_eq!(
        cfg.io_dir,
        std::path::PathBuf::from("Assets/Scripts/swarmbotix_io")
    );
}
