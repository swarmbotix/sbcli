//! L2 TDD test #9 — Unity io_dir lives under `Assets/Scripts/`.

mod common;
use common::WsSandbox;

#[test]
fn unity_io_dir_under_assets_scripts() {
    let s = WsSandbox::new();
    s.cmd().args(["ws", "create", "demo"]).output().unwrap();
    s.cmd().args(["ws", "set", "demo"]).output().unwrap();

    let project = s.tempdir().join("teleop");
    let assets_scripts = project.join("Assets").join("Scripts");
    std::fs::create_dir_all(&assets_scripts).unwrap();

    let player = assets_scripts.join("Player.cs");
    let scene = project.join("Assets").join("Main.unity");
    std::fs::write(&player, "// player\n").unwrap();
    std::fs::write(&scene, "scene\n").unwrap();

    let out = s
        .cmd()
        .args(["init", "--unity"])
        .arg(&project)
        .output()
        .unwrap();
    assert!(out.status.success(), "init failed: {out:?}");

    let dev = project.join("sb.dev.yml");
    assert!(dev.exists());
    let body = std::fs::read_to_string(&dev).unwrap();
    assert!(
        body.contains("io_dir: Assets/Scripts/swarmbotix_io"),
        "unity io_dir wrong: {body}"
    );

    let io = project.join("Assets").join("Scripts").join("swarmbotix_io");
    assert!(io.is_dir(), "expected {} to be a directory", io.display());

    assert_eq!(std::fs::read_to_string(&player).unwrap(), "// player\n");
    assert_eq!(std::fs::read_to_string(&scene).unwrap(), "scene\n");
}
