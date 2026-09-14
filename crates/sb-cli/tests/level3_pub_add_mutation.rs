//! L3 TDD #1 (partial) + #7 — `sb pub add` mutates `sb.dev.yml` and
//! drops the language file under `<io_dir>/publishers/`.
//!
//! Golden file matching is split into [level3_codegen_rust.rs](./level3_codegen_rust.rs)
//! per language. This file proves the mutation+emit pipeline end-to-end.

mod common;
use common::L3Sandbox;

#[test]
fn pub_add_appends_to_dev_yml_and_emits_file() {
    let s = L3Sandbox::rust("pubber");

    let out = s
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
    assert!(
        out.status.success(),
        "pub add failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    // 1. sb.dev.yml has the publisher.
    let dev_body = std::fs::read_to_string(s.dev_yml()).unwrap();
    assert!(dev_body.contains("name: hello"), "dev_yml: {dev_body}");
    assert!(dev_body.contains("topic: hello"), "dev_yml: {dev_body}");
    assert!(
        dev_body.contains("type: ros2/std/StringStamped"),
        "dev_yml: {dev_body}"
    );
    assert!(dev_body.contains("transport: zenoh"), "dev_yml: {dev_body}");

    // 2. Generated file exists.
    let pub_file = s.io_dir().join("publishers").join("hello.rs");
    assert!(
        pub_file.is_file(),
        "missing generated file: {}",
        pub_file.display()
    );

    // 3. File references the topic and message type.
    let body = std::fs::read_to_string(&pub_file).unwrap();
    assert!(body.contains("zenoh"), "body: {body}");
    assert!(body.contains("HelloPublisher"), "body: {body}");
    assert!(body.contains("StringStamped"), "body: {body}");
    // Zenoh topic = full path minus leading slash, includes transport segment.
    assert!(
        body.contains("dev01/demo/pubber/zenoh/hello"),
        "expected qualified topic in body:\n{body}"
    );
}

#[test]
fn pub_add_then_rm_drops_dev_yml_entry_and_file() {
    let s = L3Sandbox::rust("pubber");
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
    let pub_file = s.io_dir().join("publishers").join("hello.rs");
    assert!(pub_file.exists());

    let out = s
        .cmd_in_module()
        .args(["pub", "rm", "hello"])
        .output()
        .unwrap();
    assert!(out.status.success(), "pub rm failed: {out:?}");

    assert!(!pub_file.exists(), "publisher file should be gone");
    let body = std::fs::read_to_string(s.dev_yml()).unwrap();
    assert!(
        !body.contains("name: hello"),
        "publisher should be removed from dev_yml:\n{body}"
    );
}

#[test]
fn pub_edit_rewrites_file_with_new_topic() {
    let s = L3Sandbox::rust("pubber");
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
    let pub_file = s.io_dir().join("publishers").join("hello.rs");
    let before = std::fs::read_to_string(&pub_file).unwrap();
    assert!(before.contains("hello"));

    let out = s
        .cmd_in_module()
        .args(["pub", "edit", "hello", "-t", "world"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "pub edit failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let after = std::fs::read_to_string(&pub_file).unwrap();
    assert!(
        after.contains("dev01/demo/pubber/zenoh/world"),
        "file should reference new topic:\n{after}"
    );

    let dev = std::fs::read_to_string(s.dev_yml()).unwrap();
    assert!(dev.contains("topic: world"), "dev_yml: {dev}");
}

#[test]
fn sub_add_emits_subscriber_file() {
    let s = L3Sandbox::rust("subber");
    let out = s
        .cmd_in_module()
        .args([
            "sub",
            "add",
            "-m",
            "ros2/std/StringStamped",
            "hello",
            "--zenoh",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "sub add failed: {out:?}");
    let sub_file = s.io_dir().join("subscribers").join("hello.rs");
    assert!(sub_file.is_file(), "missing: {}", sub_file.display());
    let body = std::fs::read_to_string(&sub_file).unwrap();
    assert!(body.contains("HelloSubscriber"), "body: {body}");
    assert!(body.contains("declare_subscriber"), "body: {body}");
}
