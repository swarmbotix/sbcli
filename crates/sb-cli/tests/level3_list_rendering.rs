//! L3 TDD #8 — `sb list` / `sb pub list` / `sb sub list` rendering.

mod common;
use common::L3Sandbox;

#[test]
fn sb_list_shows_both_publishers_and_subscribers() {
    let s = L3Sandbox::rust("dual");
    let _ = s
        .cmd_in_module()
        .args([
            "pub",
            "add",
            "-m",
            "ros2/std/StringStamped",
            "hello",
            "--zenoh",
        ])
        .output()
        .unwrap();
    let _ = s
        .cmd_in_module()
        .args([
            "sub",
            "add",
            "-m",
            "ros2/std/ImageStamped",
            "image_raw",
            "--iox2",
        ])
        .output()
        .unwrap();

    let out = s.cmd_in_module().args(["list"]).output().unwrap();
    assert!(out.status.success(), "sb list failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("role"), "header missing: {stdout}");
    assert!(stdout.contains("hello"), "pub missing: {stdout}");
    assert!(stdout.contains("image_raw"), "sub missing: {stdout}");
    // Columns: type + transport
    assert!(
        stdout.contains("ros2/std/StringStamped"),
        "type missing: {stdout}"
    );
    assert!(stdout.contains("zenoh"), "transport missing: {stdout}");
    assert!(stdout.contains("iceoryx2"), "transport missing: {stdout}");
}

#[test]
fn sb_pub_list_filters_to_publishers_only() {
    let s = L3Sandbox::rust("dual");
    let _ = s
        .cmd_in_module()
        .args([
            "pub",
            "add",
            "-m",
            "ros2/std/StringStamped",
            "hello",
            "--zenoh",
        ])
        .output()
        .unwrap();
    let _ = s
        .cmd_in_module()
        .args([
            "sub",
            "add",
            "-m",
            "ros2/std/ImageStamped",
            "image_raw",
            "--iox2",
        ])
        .output()
        .unwrap();

    let out = s.cmd_in_module().args(["pub", "list"]).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("hello"));
    assert!(
        !stdout.contains("image_raw"),
        "sb pub list should not show subscribers:\n{stdout}"
    );
}

#[test]
fn sb_sub_list_filters_to_subscribers_only() {
    let s = L3Sandbox::rust("dual");
    let _ = s
        .cmd_in_module()
        .args([
            "pub",
            "add",
            "-m",
            "ros2/std/StringStamped",
            "hello",
            "--zenoh",
        ])
        .output()
        .unwrap();
    let _ = s
        .cmd_in_module()
        .args([
            "sub",
            "add",
            "-m",
            "ros2/std/ImageStamped",
            "image_raw",
            "--iox2",
        ])
        .output()
        .unwrap();

    let out = s.cmd_in_module().args(["sub", "list"]).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("image_raw"));
    assert!(
        !stdout.contains("hello"),
        "sb sub list should not show publishers:\n{stdout}"
    );
}

#[test]
fn empty_module_lists_render_explanation() {
    let s = L3Sandbox::rust("empty");
    let out = s.cmd_in_module().args(["list"]).output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("no publishers") || stdout.contains("(no"),
        "empty list should say so: {stdout}"
    );
}
