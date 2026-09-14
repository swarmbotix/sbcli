//! L1 TDD test #6 — `sb message compile` invokes protoc, producing a
//! non-empty FileDescriptorSet. With a bogus protoc path the command
//! fails non-zero with a message naming the path.

mod common;
use common::{Sandbox, protoc_available};

#[test]
fn message_compile_produces_descriptor_set() {
    let s = Sandbox::new();
    if !protoc_available(&s) {
        eprintln!("protoc not configured on this host — skipping");
        return;
    }
    // Populate std/*.
    s.cmd().args(["message", "list"]).output().unwrap();

    let out = s
        .cmd()
        .args(["message", "compile", "ros2/std/Header"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "compile failed: stdout={stdout} stderr={stderr}"
    );

    let descriptor = s.vault_dir().join(".cache").join("descriptor.bin");
    assert!(descriptor.exists(), "descriptor file not produced");
    let bytes = std::fs::read(&descriptor).unwrap();
    assert!(!bytes.is_empty(), "descriptor file empty");
}

#[test]
fn message_compile_fails_with_bogus_protoc_path() {
    let s = Sandbox::new();
    // Force a bad protoc path. Goes through `write_config` so the sandbox
    // pins (`sb_home_dir`, `messages_root`, `message_style`) are re-applied
    // — a bare `fs::write` here would drop them and resolve the vault
    // against the developer's real `~/.swarmbotix/`.
    s.write_config("protoc: /definitely/not/here/protoc\n");
    // Make sure std/* is on disk so the compile target exists.
    s.cmd().args(["message", "list"]).output().unwrap();

    let out = s
        .cmd()
        .args(["message", "compile", "ros2/std/Header"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "compile should fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("protoc"),
        "stderr must name protoc: {stderr}"
    );
}
