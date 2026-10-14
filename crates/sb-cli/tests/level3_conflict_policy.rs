//! L3 — conflict policy on `sb io add pub` / `sb io add sub`.
//!
//! Three knobs:
//!   - default (no flag, non-TTY in tests) → error,
//!   - `--force` → remove colliding entries first,
//!   - `--skip-existing` → no-op.

mod common;
use common::L3Sandbox;

#[test]
fn non_tty_default_still_errors_with_hint() {
    // No flag + non-TTY (the default test environment has no controlling
    // terminal) should produce the canonical error pointing at the flags.
    let s = L3Sandbox::rust("pubber");
    let _ = s
        .cmd_in_module()
        .args([
            "io",
            "add",
            "pub",
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
            "io",
            "add",
            "pub",
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
