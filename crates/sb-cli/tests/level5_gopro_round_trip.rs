//! L5 TDD #7 — `sb gopro` round-trip.
//!
//! Non-trivial sb.dev.yml (3 pubs, 2 subs, mixed transports) → gopro
//! → parse sb.prd.yml → publishers + subscribers identical to dev
//! (ignoring host-only fields).

mod common;
use common::L5Sandbox;

use sb_core::ModulePrdConfig;

#[test]
fn gopro_preserves_full_pub_sub_lists() {
    let mut sb = L5Sandbox::empty();
    let m = sb.adopt("camera", Some("#!/usr/bin/env bash\nexit 0\n"));

    // Three publishers, two subscribers, mixed transports. Use --force so
    // we don't need to inspect colliders.
    for (msg, topic, tr) in [
        ("ros2/std/StringStamped", "hello", "--zenoh"),
        ("ros2/std/StringStamped", "telemetry", "--zenoh"),
        ("ros2/std/StringStamped", "diag", "--iox2"),
    ] {
        let out = sb
            .cmd()
            .current_dir(&m.root)
            .args(["pub", "add", "-m", msg, topic, tr, "--force"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "sb pub add {topic} failed: {}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
    }
    for (msg, topic, tr) in [
        ("ros2/std/StringStamped", "cmd_vel", "--zenoh"),
        ("ros2/std/StringStamped", "battery", "--iox2"),
    ] {
        let out = sb
            .cmd()
            .current_dir(&m.root)
            .args(["sub", "add", "-m", msg, topic, tr, "--force"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "sb sub add {topic} failed: {}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
    }

    // Now freeze.
    let out = sb
        .cmd()
        .current_dir(&m.root)
        .args(["gopro"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "sb gopro failed: {}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    // Parse both and compare pub/sub lists.
    let dev_body = std::fs::read_to_string(m.root.join("sb.dev.yml")).unwrap();
    let dev: sb_core::ModuleDevConfig = serde_yaml::from_str(&dev_body).unwrap();
    let prd_body = std::fs::read_to_string(m.root.join("sb.prd.yml")).unwrap();
    let prd: ModulePrdConfig = serde_yaml::from_str(&prd_body).unwrap();

    assert_eq!(prd.module, dev.module);
    assert_eq!(prd.publishers, dev.publishers, "publishers list lost data");
    assert_eq!(
        prd.subscribers, dev.subscribers,
        "subscribers list lost data"
    );
    assert_eq!(prd.publishers.len(), 3);
    assert_eq!(prd.subscribers.len(), 2);

    // Mixed transports survive.
    let pub_transports: std::collections::BTreeSet<_> = prd
        .publishers
        .iter()
        .map(|p| p.transport.as_str())
        .collect();
    assert!(pub_transports.contains("zenoh"));
    assert!(pub_transports.contains("iceoryx2"));
}
