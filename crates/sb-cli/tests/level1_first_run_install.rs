//! L1 TDD test #5 — install-on-first-run.
//!
//! Fresh $HOME → first `sb message list` populates `std/*` from the
//! embedded bundle. Second call is a no-op. User edits never clobbered.
//!
//! The bundle installs into the **ros2** style's vault, which is where
//! `std/` lives; [`common::SANDBOX_STYLE`] is what the sandbox activates.

mod common;
use common::{Sandbox, protoc_available};

#[test]
fn first_run_populates_std_then_idempotent_then_preserves_user_edits() {
    let s = Sandbox::new();
    let messages_std = s.vault_dir().join("std");

    assert!(!messages_std.exists(), "precondition: std/ absent");

    // First run installs.
    let out = s.cmd().args(["message", "list"]).output().unwrap();
    assert!(out.status.success(), "first run failed: {out:?}");
    assert!(messages_std.exists(), "std/ should now exist");
    assert!(messages_std.join("Header.proto").exists());
    assert!(messages_std.join("String.proto").exists());
    assert!(messages_std.join("StringStamped.proto").exists());

    // Capture current Header.proto content + edit it.
    let header_path = messages_std.join("Header.proto");
    std::fs::write(&header_path, "// user-edited Header\n").unwrap();

    // Second run is a no-op AND preserves the edit.
    let out = s.cmd().args(["message", "list"]).output().unwrap();
    assert!(out.status.success(), "second run failed: {out:?}");
    let after = std::fs::read_to_string(&header_path).unwrap();
    assert_eq!(
        after, "// user-edited Header\n",
        "user edits to std/* must not be clobbered"
    );
}

#[test]
fn message_new_then_list_then_rm_round_trip() {
    let s = Sandbox::new();
    // `message new` auto-compiles the new .proto across every backend, so it
    // exits non-zero on a host with no protoc configured. Same guard the
    // other compile-dependent L1 tests use.
    if !protoc_available(&s) {
        eprintln!("protoc not configured on this host — skipping");
        return;
    }
    let out = s
        .cmd()
        .args(["message", "new", "swarmbotix/custom/Foo"])
        .output()
        .unwrap();
    assert!(out.status.success(), "new failed: {out:?}");

    let out = s.cmd().args(["message", "list"]).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("custom/"));
    assert!(stdout.contains("Foo"));

    let out = s
        .cmd()
        .args(["message", "rm", "swarmbotix/custom/Foo"])
        .output()
        .unwrap();
    assert!(out.status.success(), "rm failed: {out:?}");

    let out = s.cmd().args(["message", "list"]).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.contains("Foo"));
}

#[test]
fn message_rm_refuses_std() {
    let s = Sandbox::new();
    // populate first
    s.cmd().args(["message", "list"]).output().unwrap();
    let out = s
        .cmd()
        .args(["message", "rm", "ros2/std/Header"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "rm std/Header should fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("std/"),
        "stderr should mention std/: {stderr}"
    );
}
