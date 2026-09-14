//! Example pubber driven by `examples/rust_zenoh_stamped/run.sh`.
//!
//! Uses the L3-scaffolded zenoh publisher (lives under
//! `swarmbotix_io/publishers/chatter.rs` after `sb pub add`) and
//! publishes a REAL protobuf-encoded `std/StringStamped` — Header at
//! field 1 with `metadata.msg_type` and `metadata.msg_freq_desired`
//! filled in, payload at field 2 (`data`).
//!
//! Run via [run.sh](../run.sh) (the L4 end-to-end gate); standalone
//! invocations from inside this dir also work once `sb pub add` has
//! scaffolded `swarmbotix_io/publishers/chatter.rs` for you.

#[path = "swarmbotix_io/publishers/chatter.rs"]
mod chatter;

// Prost-generated bindings (see build.rs). Both protos share the
// `swarmbotix.std` package so prost emits them under one Rust module.
pub mod swarmbotix_std {
    include!(concat!(env!("OUT_DIR"), "/swarmbotix.std.rs"));
}

use prost::Message;
use swarmbotix_std::{
    header::Metadata, Header, StringStamped,
};
use zenoh::Wait;

/// Match the topic `run.sh` passes to `sb pub add` so the L4 discovery
/// observes us under the fully-qualified path the user expects.
const TOPIC_FULL: &str = "/dev01/demo/pubber/chatter";
/// What `sb topic list` should report in the rate column for this
/// publisher. Wire publish cadence is independent of this — the design
/// payoff is exactly that.
const DECLARED_RATE_HZ: f64 = 10.0;

fn build_stamped() -> Vec<u8> {
    let msg = StringStamped {
        header: Some(Header {
            timestamp_ns: 1_700_000_000_000_000_000,
            frame_id: "world".to_string(),
            metadata: Some(Metadata {
                topic_full_name: TOPIC_FULL.to_string(),
                msg_type: "std/StringStamped".to_string(),
                msg_freq_desired: DECLARED_RATE_HZ,
            }),
        }),
        data: "HELLO_FROM_STAMPED".to_string(),
    };
    let mut buf = Vec::with_capacity(msg.encoded_len());
    msg.encode(&mut buf).expect("encode StringStamped");
    buf
}

fn main() -> zenoh::Result<()> {
    let session = zenoh::open(zenoh::Config::default()).wait()?;
    let pubber = chatter::ChatterPublisher::open(&session)?;
    let payload = build_stamped();
    // 5 seconds of publishes — gives `sb topic list -t 0.5` plenty of
    // headroom even if it starts after this loop is partway through.
    for _ in 0..50 {
        pubber.publish(&payload)?;
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Ok(())
}
