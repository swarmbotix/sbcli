//! L4 TDD #3 (lite) + done-when item #1 — `sb topic list` against a live
//! Zenoh publisher running on the wire format an L3 user would emit
//! (`*Stamped` protobuf with Header at field 1).
//!
//! Goes through the actual CLI binary so the argv parsing, JSON output
//! shape, and sb-discover wiring all get exercised together. The truly
//! end-to-end version that compiles + runs an L3-scaffolded publisher
//! lives at the examples/ level (mirroring L3's e2e gates).

mod common;
use common::Sandbox;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use prost::Message;
use sb_discover::{HeaderProbe, MetadataProbe, StampedProbe};
use zenoh::Wait;

fn stamped_bytes(msg_type: &str, rate: f64, topic_full: &str) -> Vec<u8> {
    let probe = StampedProbe {
        header: Some(HeaderProbe {
            timestamp_ns: 1_700_000_000_000_000_000,
            frame_id: "world".into(),
            metadata: Some(MetadataProbe {
                topic_full_name: topic_full.into(),
                msg_type: msg_type.into(),
                msg_freq_desired: rate,
            }),
        }),
    };
    let mut buf = Vec::new();
    probe.encode(&mut buf).unwrap();
    buf
}

#[test]
#[ignore = "live zenoh — run with --ignored"]
fn sb_topic_list_finds_running_stamped_publisher() {
    let key = "test/lvl4/cli/list_e2e";
    let stop = Arc::new(AtomicBool::new(false));
    let stop_t = Arc::clone(&stop);
    let handle = thread::spawn(move || {
        let session = zenoh::open(zenoh::Config::default()).wait().unwrap();
        let pubr = session.declare_publisher(key).wait().unwrap();
        let payload = stamped_bytes("ros2/std/StringStamped", 10.0, "/test/lvl4/cli/list_e2e");
        while !stop_t.load(Ordering::Relaxed) {
            let _ = pubr.put(payload.clone()).wait();
            thread::sleep(Duration::from_millis(100));
        }
        let _ = pubr.undeclare().wait();
        let _ = session.close().wait();
    });

    thread::sleep(Duration::from_millis(200));

    let sandbox = Sandbox::new();
    let out = sandbox
        .cmd()
        .args(["topic", "list", "--transport", "zenoh", "--json"])
        .output()
        .expect("sb topic list");
    stop.store(true, Ordering::Relaxed);
    let _ = handle.join();

    assert!(
        out.status.success(),
        "sb topic list failed: stderr={:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let found = stdout
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find(|v| v["topic"].as_str() == Some(key))
        .unwrap_or_else(|| panic!("publisher not found in: {stdout}"));
    assert_eq!(found["transport"], "zenoh");
    assert_eq!(found["schema"], "ros2/std/StringStamped");
    assert_eq!(found["rate_hz"], 10.0);
}

#[test]
#[ignore = "live zenoh — run with --ignored"]
fn sb_topic_list_table_renders_running_publisher() {
    let key = "test/lvl4/cli/list_table";
    let stop = Arc::new(AtomicBool::new(false));
    let stop_t = Arc::clone(&stop);
    let handle = thread::spawn(move || {
        let session = zenoh::open(zenoh::Config::default()).wait().unwrap();
        let pubr = session.declare_publisher(key).wait().unwrap();
        let payload = stamped_bytes("ros2/std/StringStamped", 5.0, "/test/lvl4/cli/list_table");
        while !stop_t.load(Ordering::Relaxed) {
            let _ = pubr.put(payload.clone()).wait();
            thread::sleep(Duration::from_millis(100));
        }
        let _ = pubr.undeclare().wait();
        let _ = session.close().wait();
    });

    thread::sleep(Duration::from_millis(200));
    let sandbox = Sandbox::new();
    let out = sandbox
        .cmd()
        .args(["topic", "list", "--transport", "zenoh"])
        .output()
        .expect("sb topic list");
    stop.store(true, Ordering::Relaxed);
    let _ = handle.join();

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(key), "topic missing from table: {stdout}");
    assert!(
        stdout.contains("ros2/std/StringStamped"),
        "schema missing from table: {stdout}"
    );
    assert!(
        stdout.contains("5.0 Hz"),
        "rate missing from table: {stdout}"
    );
}
