//! Zenoh-backed frame iterator + one-shot raw publisher.
//!
//! `listen_zenoh` opens a short-lived session and a typed subscriber on
//! the exact full-path topic; each sample's payload bytes come back as-is,
//! together with its encoding, classified by [`parse_wire_tag`].
//! Decoding is the caller's job (`sb-listen::decode::decode_frame_dynamic`).
//!
//! `publish_zenoh_raw` is one-and-done: open session → declare publisher →
//! put bytes → close. Used by `sb topic pub <topic> <hex>`; not on a hot
//! path so the per-call setup cost is fine.

use std::time::Duration;

use anyhow::{Context, Result};
use zenoh::Wait;

use sb_discover::{WireTag, parse_wire_tag};

use crate::{Frame, FrameIter, FramePoll};

pub struct ZenohFrames {
    _session: zenoh::Session,
    sub: zenoh::pubsub::Subscriber<zenoh::handlers::FifoChannelHandler<zenoh::sample::Sample>>,
}

/// Open a Zenoh subscriber on `topic_full_name`, the canonical
/// fully-qualified topic, which is the Zenoh key expression itself. The
/// canonical form has no leading `/`; one is tolerated and stripped (as is
/// a trailing one) because Zenoh rejects them.
pub fn listen_zenoh(topic_full_name: &str) -> Result<ZenohFrames> {
    let key = topic_full_name.trim_matches('/').to_string();
    let session = zenoh::open(zenoh::Config::default())
        .wait()
        .map_err(|e| anyhow::anyhow!("zenoh open: {e}"))?;
    let sub = session
        .declare_subscriber(key)
        .wait()
        .map_err(|e| anyhow::anyhow!("zenoh declare_subscriber: {e}"))?;
    Ok(ZenohFrames {
        _session: session,
        sub,
    })
}

impl FrameIter for ZenohFrames {
    fn next_frame(&mut self) -> Result<Option<Frame>> {
        match self.sub.recv() {
            Ok(sample) => Ok(Some(to_frame(&sample))),
            // Disconnected handler — treat as graceful end-of-stream.
            Err(_) => Ok(None),
        }
    }

    fn poll_frame(&mut self, timeout: Duration) -> Result<FramePoll> {
        match self.sub.recv_timeout(timeout) {
            Ok(Some(sample)) => Ok(FramePoll::Frame(to_frame(&sample))),
            Ok(None) => Ok(FramePoll::Timeout),
            // Disconnected handler, as in `next_frame`.
            Err(_) => Ok(FramePoll::Closed),
        }
    }
}

/// A sample as a [`Frame`]. The encoding is classified by the same
/// [`parse_wire_tag`] `sb topic list` uses, so both commands agree on a
/// topic's type; a protobuf tag's vault name also becomes
/// [`Frame::wire_schema`].
fn to_frame(sample: &zenoh::sample::Sample) -> Frame {
    let tag = parse_wire_tag(&sample.encoding().to_string());
    Frame {
        bytes: sample.payload().to_bytes().to_vec(),
        wire_schema: match &tag {
            WireTag::Protobuf(name) => Some(name.clone()),
            _ => None,
        },
        wire_tag: Some(tag),
    }
}

/// Publish `bytes` on `topic_full_name` exactly once. Synchronous: by
/// the time we return, the put has been flushed to the local session
/// AND we've slept long enough for Zenoh's localhost scouting + delivery
/// to a peer on the same machine to actually complete. Without that
/// settle window, closing the session immediately after `put` can drop
/// the sample before any subscriber sees it (observed under the L4
/// round-trip test).
///
/// `topic_full_name` is the canonical topic (no leading `/`); a stray
/// leading or trailing `/` is tolerated and stripped.
pub fn publish_zenoh_raw(topic_full_name: &str, bytes: &[u8]) -> Result<()> {
    let key = topic_full_name.trim_matches('/').to_string();
    let session = zenoh::open(zenoh::Config::default())
        .wait()
        .map_err(|e| anyhow::anyhow!("zenoh open: {e}"))?;
    let publisher = session
        .declare_publisher(key)
        .wait()
        .map_err(|e| anyhow::anyhow!("zenoh declare_publisher: {e}"))?;
    // Wait for the local scouting round-trip — a sub on the same host
    // takes ~200 ms to become visible to this session. Tuned against
    // level4_pub_roundtrip.
    std::thread::sleep(std::time::Duration::from_millis(300));
    publisher
        .put(bytes.to_vec())
        .wait()
        .map_err(|e| anyhow::anyhow!("zenoh put: {e}"))?;
    // Settle: let the put propagate before tearing down the session.
    std::thread::sleep(std::time::Duration::from_millis(100));
    let _ = publisher.undeclare().wait();
    let _ = session.close().wait();
    Ok(())
}

/// Parse a contiguous hex string (no `0x`, no separators) into bytes.
/// Surfaced from this module so the CLI's `sb topic pub` and any test
/// helper share one parser.
pub fn parse_hex(input: &str) -> Result<Vec<u8>> {
    let cleaned: String = input.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    if cleaned.len() % 2 != 0 {
        anyhow::bail!("hex string must have even length, got {}", cleaned.len());
    }
    (0..cleaned.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&cleaned[i..i + 2], 16)
                .with_context(|| format!("invalid hex at byte {}: {:?}", i / 2, &cleaned[i..i + 2]))
        })
        .collect()
}
