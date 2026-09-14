//! L4 TDD #9 — `sb topic list -t 0.2` reports `msg_freq_desired` correctly
//! from a single sample, regardless of how often the publisher actually
//! publishes. This is the design payoff for L1's load-bearing
//! `Header.metadata.msg_freq_desired` field.
//!
//! Approach: encode a synthetic `*Stamped`-shaped protobuf payload by
//! hand (Header at field 1 with the declared rate), publish it once,
//! verify `discover_zenoh` lifts both `schema` and `rate_hz` out of that
//! single frame.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use prost::Message;
use sb_discover::{HeaderProbe, MetadataProbe, StampedProbe, Transport, discover_zenoh};
use zenoh::Wait;

fn build_stamped(msg_type: &str, rate: f64, topic_full: &str) -> Vec<u8> {
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
    probe.encode(&mut buf).expect("encode synthetic stamped");
    buf
}

#[test]
#[ignore = "live zenoh — run with --ignored"]
fn rate_and_schema_lift_from_single_stamped_sample() {
    // The publisher pushes samples at ~20 Hz (50 ms interval) but
    // DECLARES `msg_freq_desired = 10.0` in the Header. The test
    // asserts the reported rate matches the DECLARED value — proving
    // we lift it out of the Header field rather than counting samples.
    // (Sample-counting would have reported ~20 Hz.)
    let key = "test/lvl4/rate/stamped";
    let stop = Arc::new(AtomicBool::new(false));
    let stop_t = Arc::clone(&stop);
    let handle = thread::spawn(move || {
        let session = zenoh::open(zenoh::Config::default()).wait().unwrap();
        let pubr = session.declare_publisher(key).wait().unwrap();
        let payload = build_stamped("ros2/std/StringStamped", 10.0, "/test/lvl4/rate/stamped");
        while !stop_t.load(Ordering::Relaxed) {
            let _ = pubr.put(payload.clone()).wait();
            thread::sleep(Duration::from_millis(50));
        }
        let _ = pubr.undeclare().wait();
        let _ = session.close().wait();
    });

    thread::sleep(Duration::from_millis(200));
    let entries = discover_zenoh(Duration::from_millis(500), None, false).unwrap();
    stop.store(true, Ordering::Relaxed);
    let _ = handle.join();

    let row = entries
        .iter()
        .find(|e| e.topic == key)
        .unwrap_or_else(|| panic!("publisher not seen: {entries:#?}"));
    assert_eq!(row.transport, Transport::Zenoh);
    assert_eq!(row.schema.as_deref(), Some("ros2/std/StringStamped"));
    assert_eq!(
        row.rate_hz,
        Some(10.0),
        "rate should come from Header.msg_freq_desired (10.0), not actual publish rate (~20 Hz)"
    );
}
