//! L1 — `sb config show` / `sb config get` read-only inspection surface.
//!
//! These verbs let user-side LLMs and scripts ask for the merged config
//! that every other `sb` verb sees, without having to replicate the
//! layering rules. See sbcli_config.md §4.3 / §4.4.

use std::process::Command;

use tempfile::TempDir;

/// Spawn `sb` with `$SB_CONFIG` pinned to a fresh test config AND
/// `$HOME` redirected into the tempdir, so the global layer
/// (`~/.swarmbotix/sb.config.yml` on the real host) doesn't bleed in.
fn hermetic_sb(d: &TempDir, cfg: &std::path::Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_sb"));
    c.env("SB_CONFIG", cfg);
    // `$SB_CONFIG` is the highest layer, not an exclusive one — the global
    // `<sb_home>/sb.config.yml` still merges underneath it. Redirecting
    // SB_HOME into the tempdir points that layer at a file that does not
    // exist, which is what actually makes these tests hermetic. `HOME` alone
    // cannot do it: `dirs::home_dir()` consults `$HOME` only on Unix.
    c.env("SB_HOME", d.path().join(".swarmbotix"));
    c.env("HOME", d.path());
    c
}

/// Write a self-contained `sb.config.yml` and return its path + the
/// tempdir holding it (kept alive for the test).
fn isolated_config(body: &str) -> (TempDir, std::path::PathBuf) {
    let d = TempDir::new().unwrap();
    let p = d.path().join("sb.config.yml");
    std::fs::write(&p, body).unwrap();
    (d, p)
}

#[test]
fn show_yaml_contains_resolved_fields() {
    let (d, cfg) = isolated_config(
        "device: dev42\ntransport_on_device: zenoh\nmessage_targets: /opt/targets\n",
    );
    let out = hermetic_sb(&d, &cfg)
        .args(["config", "show"])
        .output()
        .unwrap();
    assert!(out.status.success(), "show failed: {out:?}");
    let s = String::from_utf8(out.stdout).unwrap();
    assert!(s.contains("device: dev42"), "device missing:\n{s}");
    assert!(
        s.contains("transport_on_device: zenoh"),
        "transport missing:\n{s}"
    );
    assert!(
        s.contains("message_targets: /opt/targets"),
        "targets missing:\n{s}"
    );
    // Built-in defaults bleed through (transport_cross_device).
    assert!(
        s.contains("transport_cross_device: zenoh"),
        "built-in cross-device missing:\n{s}"
    );
}

#[test]
fn get_scalar_field_prints_bare_value() {
    let (d, cfg) = isolated_config("device: hello-bot\n");
    let out = hermetic_sb(&d, &cfg)
        .args(["config", "get", "device"])
        .output()
        .unwrap();
    assert!(out.status.success(), "get failed: {out:?}");
    let s = String::from_utf8(out.stdout).unwrap();
    assert_eq!(s.trim(), "hello-bot");
}

#[test]
fn get_unknown_key_lists_known_keys() {
    let (d, cfg) = isolated_config("device: x\n");
    let out = hermetic_sb(&d, &cfg)
        .args(["config", "get", "not_a_real_key"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "bogus key should error: {out:?}");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("not_a_real_key") && stderr.contains("messages_root"),
        "should list known keys in the error:\n{stderr}"
    );
}
