//! L3: CLI guard for a structurally impossible language/transport pair.
//!
//! Drives `sb io add pub --iox2` in a Flutter module and asserts that it fails
//! with the structural-impossibility message (Flutter runs on mobile and
//! sandboxed runtimes with no shared memory, so there is no iceoryx2 path),
//! not a "not yet implemented" one. The same guard covers Unity in
//! `sb-pubsub`; this test exercises it once through the CLI.
//!
//! What each language's generated file contains is covered by the
//! byte-exact goldens in `level3_codegen_*_snapshots.rs`.

mod common;
use common::L3Sandbox;

// ─────────────────────────────────────────────────────────────────────
// Flutter
// ─────────────────────────────────────────────────────────────────────

#[test]
fn flutter_pub_add_iceoryx_errors_as_impossible() {
    let s = L3Sandbox::flutter("pubber");
    let out = s
        .cmd_in_module()
        .args([
            "io",
            "add",
            "pub",
            "-m",
            "ros2/std/ImageStamped",
            "image_raw",
            "--iox2",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success(), "flutter --iox2 must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    // Structural-impossibility wording — not "not yet implemented".
    assert!(
        stderr.contains("no iceoryx2 path")
            || stderr.contains("shared memory isn't available")
            || stderr.contains("shared memory"),
        "stderr should explain structural impossibility, got: {stderr}"
    );
    assert!(
        !stderr.contains("not yet implemented"),
        "stderr should NOT claim 'not yet implemented' for Flutter: {stderr}"
    );
}
