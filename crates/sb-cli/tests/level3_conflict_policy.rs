//! L3 — conflict policy on `sb pub add` / `sb sub add`.
//!
//! Three knobs:
//!   - default (no flag, non-TTY in tests) → error,
//!   - `--force` → remove colliding entries first,
//!   - `--skip-existing` → no-op.

mod common;
use common::L3Sandbox;

#[test]
fn pub_add_force_overwrites_existing_with_same_topic() {
    let s = L3Sandbox::rust("pubber");
    // First add — succeeds.
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
    assert!(out.status.success(), "first add: {out:?}");
    let first_body = std::fs::read_to_string(s.io_dir().join("publishers/hello.rs")).unwrap();
    assert!(first_body.contains("StringStamped"));

    // Re-add same topic with a different MsgType + --force.
    let out = s
        .cmd_in_module()
        .args([
            "pub",
            "add",
            "-m",
            "ros2/std/ImageStamped",
            "hello",
            "--zenoh",
            "--force",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "--force should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("overwrote"),
        "stdout should report overwrite: {stdout}"
    );

    // sb.dev.yml now reflects the new MsgType (exactly one publisher).
    let dev = std::fs::read_to_string(s.dev_yml()).unwrap();
    assert!(
        dev.contains("type: ros2/std/ImageStamped"),
        "dev_yml: {dev}"
    );
    assert!(
        !dev.contains("type: ros2/std/StringStamped"),
        "dev_yml should not retain the old type:\n{dev}"
    );
    // Only one `name: hello` line — no duplicates.
    let count = dev.matches("name: hello").count();
    assert_eq!(
        count, 1,
        "expected exactly one hello entry, got {count}:\n{dev}"
    );

    // Regenerated file references the new type.
    let body = std::fs::read_to_string(s.io_dir().join("publishers/hello.rs")).unwrap();
    assert!(body.contains("ImageStamped"), "regenerated file: {body}");
}

#[test]
fn pub_add_skip_existing_is_noop_on_collision() {
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
    let before = std::fs::read_to_string(s.dev_yml()).unwrap();

    let out = s
        .cmd_in_module()
        .args([
            "pub",
            "add",
            "-m",
            "ros2/std/ImageStamped",
            "hello",
            "--zenoh",
            "--skip-existing",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "--skip-existing should succeed: {out:?}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("skipped"),
        "stdout should say skipped: {stdout}"
    );

    // dev.yml unchanged.
    let after = std::fs::read_to_string(s.dev_yml()).unwrap();
    assert_eq!(before, after, "dev_yml mutated despite --skip-existing");
}

#[test]
fn no_collision_then_skip_existing_still_adds() {
    // --skip-existing should only no-op when there IS a collision.
    // Adding a fresh entry should still succeed.
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
            "--skip-existing",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "fresh --skip-existing add: {out:?}");
    assert!(s.io_dir().join("publishers/hello.rs").exists());
}

#[test]
fn pub_add_force_and_skip_are_mutually_exclusive() {
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
            "--force",
            "--skip-existing",
        ])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "--force + --skip-existing together must fail"
    );
}

#[test]
fn sub_add_force_clears_publisher_on_same_topic() {
    // Cross-role collision: publisher on `chat`, then `sub add chat
    // --force` should remove the publisher.
    let s = L3Sandbox::rust("dual");
    let _ = s
        .cmd_in_module()
        .args([
            "pub",
            "add",
            "-m",
            "ros2/std/StringStamped",
            "chat",
            "--zenoh",
        ])
        .output()
        .unwrap();
    assert!(s.io_dir().join("publishers/chat.rs").exists());

    let out = s
        .cmd_in_module()
        .args([
            "sub",
            "add",
            "-m",
            "ros2/std/StringStamped",
            "chat",
            "--zenoh",
            "--force",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "cross-role --force: {out:?}");

    // Publisher file gone, subscriber file present.
    assert!(!s.io_dir().join("publishers/chat.rs").exists());
    assert!(s.io_dir().join("subscribers/chat.rs").exists());

    let dev = std::fs::read_to_string(s.dev_yml()).unwrap();
    // The publishers list is now empty — serde skips empty Vecs, so the
    // key is absent. Asserting the publisher entry is gone is enough.
    assert!(
        !dev.contains("publishers:"),
        "dev_yml should not retain a publishers section:\n{dev}"
    );
    assert!(
        dev.contains("subscribers:") && dev.contains("name: chat"),
        "subscriber should be present:\n{dev}"
    );
}

#[test]
fn non_tty_default_still_errors_with_hint() {
    // No flag + non-TTY (the default test environment has no controlling
    // terminal) should produce the canonical error pointing at the flags.
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

    let out = s
        .cmd_in_module()
        .args([
            "pub",
            "add",
            "-m",
            "ros2/std/ImageStamped",
            "hello",
            "--zenoh",
        ])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "default collision must error: {out:?}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--force") && stderr.contains("--skip-existing"),
        "stderr should hint at flags: {stderr}"
    );
}
