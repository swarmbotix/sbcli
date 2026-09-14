//! L4 TDD #2 — live iceoryx2 discovery.
//!
//! Create an iox2 publish/subscribe service in-process, then verify
//! [`sb_discover::discover_iceoryx`] surfaces it with the schema column
//! populated from `Service::list`'s type-name registry. No subscribe, no
//! sleep window — `Service::list` is name-only and cheap.
//!
//! `#[ignore]`'d because iceoryx2 requires shared-memory access that
//! CI environments often lock down. Run locally with:
//!   `cargo test -p sb-cli --test level4_discover_iceoryx -- --ignored`

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use iceoryx2::prelude::*;
use sb_discover::{Transport, discover_iceoryx};

/// Spawn a `[u8]`-slab iox2 service that holds itself open until the
/// returned `stop` flag flips. Returns once the service is registered
/// (so the discovery call right after is guaranteed to see it).
fn spawn_u8_slab_service(name: &'static str) -> (Arc<AtomicBool>, thread::JoinHandle<()>) {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_t = Arc::clone(&stop);
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
    let handle = thread::spawn(move || {
        let svc_name = ServiceName::new(name).expect("valid iox2 service name");
        let node = NodeBuilder::new()
            .create::<ipc::Service>()
            .expect("iox2 node");
        let service = node
            .service_builder(&svc_name)
            .publish_subscribe::<[u8]>()
            .open_or_create()
            .expect("iox2 service open_or_create");
        // Holding the publisher open is what makes the service visible
        // to `Service::list`. Without an active port the entry can
        // be reclaimed.
        let _publisher = service
            .publisher_builder()
            .initial_max_slice_len(8)
            .create()
            .expect("iox2 publisher");
        ready_tx.send(()).ok();
        while !stop_t.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(20));
        }
    });
    ready_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("iox2 service ready");
    (stop, handle)
}

#[test]
#[ignore = "live iceoryx2 — run with --ignored"]
fn discover_iceoryx_finds_live_u8_service() {
    let name = "test_lvl4_discover_iox_basic";
    let (stop, handle) = spawn_u8_slab_service(name);

    let entries = discover_iceoryx(None, false).expect("discover_iceoryx");
    stop.store(true, Ordering::Relaxed);
    let _ = handle.join();

    let row = entries
        .iter()
        .find(|e| e.topic == name)
        .unwrap_or_else(|| panic!("service not found, saw: {entries:#?}"));
    assert_eq!(row.transport, Transport::Iceoryx2);
    // `[u8]` payload — iox2 stores the rust type name; we don't pin the
    // exact text (varies across iox2 versions), just that *something* came
    // back, proving `static_details` → `type_name()` was reached.
    assert!(
        row.schema.is_some(),
        "schema should populate from type_name(): {row:?}"
    );
    assert!(row.rate_hz.is_none(), "L4 iox2 discovery never sets rate");
    // PID column comes from `dynamic_details.nodes[*].node_id().pid()` —
    // since the publisher lives in this test process, the surfaced PID
    // must equal `std::process::id()` and the entry must be alive.
    assert_eq!(
        row.publisher_pid,
        Some(std::process::id()),
        "owning PID should be this test process: {row:?}"
    );
    assert!(
        !row.is_dead,
        "live publisher must not be flagged dead: {row:?}"
    );
}

#[test]
#[ignore = "live iceoryx2 — run with --ignored"]
fn discover_iceoryx_keyword_filter_drops_non_matches() {
    let keep = "test_lvl4_iox_filter_keepme";
    let drop = "test_lvl4_iox_filter_dropme";
    let (s1, h1) = spawn_u8_slab_service(keep);
    let (s2, h2) = spawn_u8_slab_service(drop);

    let entries = discover_iceoryx(Some("keepme"), false).expect("discover_iceoryx");
    s1.store(true, Ordering::Relaxed);
    s2.store(true, Ordering::Relaxed);
    let _ = h1.join();
    let _ = h2.join();

    let topics: Vec<&str> = entries.iter().map(|e| e.topic.as_str()).collect();
    assert!(topics.contains(&keep), "filter kept too little: {topics:?}");
    assert!(
        !topics.iter().any(|t| t.contains("dropme")),
        "filter should drop non-matches: {topics:?}"
    );
}
