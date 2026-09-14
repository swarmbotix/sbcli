//! L3 TDD #2 — validation errors:
//!   - unknown MsgType
//!   - duplicate topic

mod common;
use common::L3Sandbox;

#[test]
fn unqualified_msg_type_teaches_the_three_segment_form() {
    // A 2-segment name is not an identity: two styles may each define an
    // `images/Image`. The error has to say so, not just "not found".
    let s = L3Sandbox::rust("pubber");
    let out = s
        .cmd_in_module()
        .args(["pub", "add", "-m", "ghost/Nope", "hello", "--zenoh"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "unqualified msg type must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("ghost/Nope"), "stderr: {stderr}");
    assert!(
        stderr.contains("<style>/<namespace>/<Leaf>"),
        "stderr should name the required form: {stderr}"
    );
    assert!(
        stderr.contains("sb message list"),
        "stderr should hint at sb message list: {stderr}"
    );
}

#[test]
fn unknown_msg_type_errors_with_actionable_message() {
    let s = L3Sandbox::rust("pubber");
    let out = s
        .cmd_in_module()
        .args(["pub", "add", "-m", "ros2/ghost/Nope", "hello", "--zenoh"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "unknown msg type must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("ros2/ghost/Nope"), "stderr: {stderr}");
    assert!(
        stderr.contains("vault"),
        "stderr should mention vault: {stderr}"
    );
    assert!(
        stderr.contains("sb message list"),
        "stderr should hint at sb message list: {stderr}"
    );

    // sb.dev.yml unchanged (still has empty publishers).
    let body = std::fs::read_to_string(s.dev_yml()).unwrap();
    assert!(
        !body.contains("ghost"),
        "dev_yml should not contain the rejected publisher:\n{body}"
    );
}

#[test]
fn duplicate_topic_is_rejected_with_pub_name() {
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

    // Second publisher on the same topic — must fail.
    let out = s
        .cmd_in_module()
        .args([
            "pub",
            "add",
            "-m",
            "ros2/std/StringStamped",
            "hello",
            "--name",
            "hello2",
            "--zenoh",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success(), "duplicate topic must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("hello"), "stderr: {stderr}");
    // New wording surfaces the colliding entry and points at the flags
    // that resolve the conflict.
    assert!(
        stderr.contains("blocked by") && stderr.contains("same topic"),
        "stderr should explain collision: {stderr}"
    );
    assert!(
        stderr.contains("--force") && stderr.contains("--skip-existing"),
        "stderr should hint at the conflict flags: {stderr}"
    );
}

#[test]
fn duplicate_publisher_name_is_rejected() {
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
    // Different topic, same name → still a clash on the identifier.
    let out = s
        .cmd_in_module()
        .args([
            "pub",
            "add",
            "-m",
            "ros2/std/StringStamped",
            "world",
            "--name",
            "hello",
            "--zenoh",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success(), "duplicate publisher name must fail");
}
