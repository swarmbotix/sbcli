//! L3 TDD #4 — transport defaulting.
//!
//! With no `--zenoh` / `--iox2`, `sb pub add` reads the default
//! transport from sb.config.yml's `transport_on_device`. Explicit flag
//! always wins.

mod common;
use common::L3Sandbox;

#[test]
fn sb_config_override_flips_default() {
    let s = L3Sandbox::rust("pubber");
    // Rewrite the test sb.config.yml to flip transport_on_device → zenoh.
    let cfg_path = s.ws.config_path().to_path_buf();
    let orig = std::fs::read_to_string(&cfg_path).unwrap();
    // Drop any existing transport_on_device line and append the override.
    let mut filtered: String = orig
        .lines()
        .filter(|l| !l.trim_start().starts_with("transport_on_device:"))
        .collect::<Vec<_>>()
        .join("\n");
    if !filtered.ends_with('\n') {
        filtered.push('\n');
    }
    filtered.push_str("transport_on_device: zenoh\n");
    std::fs::write(&cfg_path, filtered).unwrap();

    let out = s
        .cmd_in_module()
        .args(["pub", "add", "-m", "ros2/std/StringStamped", "hello"])
        .output()
        .unwrap();
    assert!(out.status.success(), "pub add failed: {out:?}");
    let body = std::fs::read_to_string(s.dev_yml()).unwrap();
    assert!(
        body.contains("transport: zenoh"),
        "flipped default should give zenoh:\n{body}"
    );
}
