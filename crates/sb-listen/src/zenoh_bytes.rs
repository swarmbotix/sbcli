//! Zenoh-backed frame iterator + one-shot raw publisher.
//!
//! `listen_zenoh` opens a short-lived session and a typed subscriber on
//! the exact full-path topic; each sample's payload bytes come back as-is.
//! Decoding is the caller's job (`sb-listen::decode::decode_frame`).
//!
//! `publish_zenoh_raw` is one-and-done: open session → declare publisher →
//! put bytes → close. Used by `sb topic pub <topic> <hex>`; not on a hot
//! path so the per-call setup cost is fine.

use anyhow::{Context, Result};
use zenoh::Wait;

use crate::FrameIter;

pub struct ZenohFrames {
    _session: zenoh::Session,
    sub: zenoh::pubsub::Subscriber<zenoh::handlers::FifoChannelHandler<zenoh::sample::Sample>>,
}

/// Open a Zenoh subscriber. `topic_full_name` may start with `/`; Zenoh
/// key expressions don't carry the leading slash, so we trim it the same
/// way the L3 codegen does (`topic_zenoh` is built without it).
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
    fn next_frame(&mut self) -> Result<Option<Vec<u8>>> {
        match self.sub.recv() {
            Ok(sample) => Ok(Some(sample.payload().to_bytes().to_vec())),
            // Disconnected handler — treat as graceful end-of-stream.
            Err(_) => Ok(None),
        }
    }
}

/// Publish `bytes` on `topic_full_name` exactly once. Synchronous: by
/// the time we return, the put has been flushed to the local session
/// AND we've slept long enough for Zenoh's localhost scouting + delivery
/// to a peer on the same machine to actually complete. Without that
/// settle window, closing the session immediately after `put` can drop
/// the sample before any subscriber sees it (observed under the L4
/// round-trip test).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hex_basic() {
        assert_eq!(parse_hex("deadbeef").unwrap(), vec![0xde, 0xad, 0xbe, 0xef]);
        assert_eq!(
            parse_hex("DE AD BE EF").unwrap(),
            vec![0xde, 0xad, 0xbe, 0xef]
        );
        assert_eq!(parse_hex("").unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn parse_hex_rejects_odd_length() {
        assert!(parse_hex("abc").is_err());
    }

    #[test]
    fn parse_hex_rejects_non_hex_chars() {
        assert!(parse_hex("zz").is_err());
    }
}
