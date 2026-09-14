//! L4 TDD #8 (iox2 half) — `sb topic pub --transport iceoryx2` round-trips
//! bytes through an iox2 `[u8]`-slab service.
//!
//! Pre-arms an iox2 subscriber in a thread, runs the CLI to publish
//! `deadbeef` on the same service, and verifies the subscriber sees the
//! exact 4 bytes. `#[ignore]`'d for shmem-restricted environments.

mod common;
use common::sb_bin;

use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use sb_listen::{FrameIter, listen_iceoryx};

#[test]
#[ignore = "live iceoryx2 — run with --ignored"]
fn sb_topic_pub_round_trips_through_iceoryx2() {
    // Fully-qualified topic — iox2 service names allow `/`-prefixed
    // paths, but the CLI trims the leading `/` before opening, so both
    // forms target the same service. Use the fully-qualified form to
    // mirror the user-facing call.
    let topic_full = "/test/lvl4/roundtrip/iox2/payload";
    let (tx, rx) = mpsc::channel();
    let sub_handle = thread::spawn(move || {
        // Open the subscriber first so the service exists when the
        // publisher attaches. iox2's `open_or_create` then matches the
        // type the subscriber declared (`[u8]`).
        let mut sub = match listen_iceoryx(topic_full) {
            Ok(s) => s,
            Err(e) => {
                let _ = tx.send(Err(format!("listen_iceoryx: {e}")));
                return;
            }
        };
        match sub.next_frame() {
            Ok(frame) => {
                let _ = tx.send(Ok(frame));
            }
            Err(e) => {
                let _ = tx.send(Err(format!("next_frame: {e}")));
            }
        }
    });

    // Give the subscriber time to declare the service before pub
    // attaches. iox2 attachment is cheap but does need the service
    // entry visible first.
    thread::sleep(Duration::from_millis(300));

    // NOTE: use the real HOME / iox2 system root so the child publisher
    // shares the same shared-memory namespace as the in-process
    // subscriber thread. The standard `Sandbox::cmd()` helper overrides
    // HOME, which makes iox2 use a different per-user shm directory and
    // the publisher's samples become invisible to the subscriber.
    let out = Command::new(sb_bin())
        .args([
            "topic",
            "pub",
            "--transport",
            "iceoryx2",
            topic_full,
            "deadbeef",
        ])
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
        .expect("subscriber did not receive frame in 5 s")
        .expect("subscriber errored");
    let _ = sub_handle.join();

    assert_eq!(frame, Some(vec![0xde, 0xad, 0xbe, 0xef]));
}
