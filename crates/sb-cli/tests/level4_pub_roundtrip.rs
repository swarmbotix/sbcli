//! L4 TDD #8 — `sb topic pub` round-trips exact bytes through Zenoh.
//!
//! Pre-arms a `listen_zenoh` subscriber in a thread, then runs the CLI
//! to publish `deadbeef` on the same topic, and verifies the subscriber
//! sees those four bytes verbatim. One `#[ignore]`'d test per transport —
//! iox2 sibling lives in level4_pub_roundtrip_iceoryx.rs (TODO once iox2
//! `[u8]` slabs are well-exercised on this dev box).

mod common;
use common::Sandbox;

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use sb_listen::{FrameIter, listen_zenoh};

#[test]
#[ignore = "live zenoh — run with --ignored"]
fn sb_topic_pub_round_trips_through_zenoh() {
    let topic_full = "/test/lvl4/roundtrip/zenoh/payload";
    let (tx, rx) = mpsc::channel();
    let sub_handle = thread::spawn(move || {
        let mut sub = listen_zenoh(topic_full).expect("listen_zenoh");
        // Block until first frame.
        let frame = sub.next_frame().expect("next_frame");
        let _ = tx.send(frame);
    });

    // Give the subscriber a moment to declare before publishing.
    thread::sleep(Duration::from_millis(200));

    let sandbox = Sandbox::new();
    let out = sandbox
        .cmd()
        .args(["topic", "pub", topic_full, "deadbeef"])
        .output()
        .expect("sb topic pub");
    assert!(
        out.status.success(),
        "sb topic pub failed: stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let frame = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("subscriber did not receive frame in 5 s");
    let _ = sub_handle.join();

    assert_eq!(frame, Some(vec![0xde, 0xad, 0xbe, 0xef]));
}
