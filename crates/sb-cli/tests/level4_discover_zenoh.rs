//! L4 TDD #1 — live Zenoh discovery.
//!
//! Spin up a real Zenoh publisher in a background thread on a known key
//! and verify [`sb_discover::discover_zenoh`] finds it within the 500 ms
//! default window. Tagged `[z]`.
//!
//! Marked `#[ignore]` because the Zenoh runtime opens UDP multicast and
//! may collide with other localhost peers / iox2 state in CI. Run with
//! `cargo test -p sb-cli --test level4_discover_zenoh -- --ignored`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use sb_discover::{DEFAULT_DISCOVERY_WINDOW, Transport, discover_zenoh};
use zenoh::Wait;

/// Spawn a Zenoh publisher that puts `payload` on `key` every 50 ms
/// until the returned flag flips. Caller owns the lifetime — flip the
/// flag, join the thread, and the session closes cleanly.
fn spawn_publisher(
    key: &'static str,
    payload: Vec<u8>,
) -> (Arc<AtomicBool>, thread::JoinHandle<()>) {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_t = Arc::clone(&stop);
    let handle = thread::spawn(move || {
        let session = match zenoh::open(zenoh::Config::default()).wait() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("publisher zenoh::open failed: {e}");
                return;
            }
        };
        let pubr = match session.declare_publisher(key).wait() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("declare_publisher failed: {e}");
                return;
            }
        };
        while !stop_t.load(Ordering::Relaxed) {
            let _ = pubr.put(payload.clone()).wait();
            thread::sleep(Duration::from_millis(50));
        }
        let _ = pubr.undeclare().wait();
        let _ = session.close().wait();
    });
    (stop, handle)
}

#[test]
#[ignore = "live zenoh — run with --ignored"]
fn discover_zenoh_finds_live_publisher() {
    let (stop, handle) = spawn_publisher("test/lvl4/discover/raw", b"raw-bytes-payload".to_vec());
    // Give the publisher a moment to declare before we open the discovery session.
    thread::sleep(Duration::from_millis(150));

    let entries = discover_zenoh(DEFAULT_DISCOVERY_WINDOW, None, false)
        .expect("discover_zenoh should succeed");
    stop.store(true, Ordering::Relaxed);
    let _ = handle.join();

    let found = entries
        .iter()
        .find(|e| e.topic == "test/lvl4/discover/raw")
        .unwrap_or_else(|| panic!("publisher not found, saw: {entries:#?}"));
    assert_eq!(found.transport, Transport::Zenoh);
    // Raw bytes → Header probe fails → schema and rate stay null.
    assert!(found.schema.is_none(), "raw bytes should leave schema null");
    assert!(found.rate_hz.is_none(), "raw bytes should leave rate null");
}

#[test]
#[ignore = "live zenoh — run with --ignored"]
fn discover_zenoh_keyword_filter_drops_non_matches() {
    let (s1, h1) = spawn_publisher("test/lvl4/filter/keepme", b"x".to_vec());
    let (s2, h2) = spawn_publisher("test/lvl4/filter/dropme", b"x".to_vec());
    thread::sleep(Duration::from_millis(150));

    let entries = discover_zenoh(DEFAULT_DISCOVERY_WINDOW, Some("keepme"), false).unwrap();
    s1.store(true, Ordering::Relaxed);
    s2.store(true, Ordering::Relaxed);
    let _ = h1.join();
    let _ = h2.join();

    let topics: Vec<&str> = entries.iter().map(|e| e.topic.as_str()).collect();
    assert!(
        topics.contains(&"test/lvl4/filter/keepme"),
        "matching topic missing: {topics:?}"
    );
    assert!(
        !topics.iter().any(|t| t.contains("dropme")),
        "filter should have dropped non-matches: {topics:?}"
    );
}
