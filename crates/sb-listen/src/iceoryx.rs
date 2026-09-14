//! iceoryx2-backed frame iterator + raw publisher.
//!
//! L4 scope cut: `sb topic listen --transport iceoryx2 <topic>` only
//! works when the service's typed payload is a fixed-width `[u8; N]` slab
//! we can declare without knowing the user's payload struct ahead of time.
//! Concretely we open the service generically as a `u8`-slice subscriber
//! and copy the bytes out. Services published with a typed struct (the
//! common L3 path) will fail to open due to iceoryx2's `IncompatibleTypes`
//! check — that's the honest L4 behaviour, surfaced as an error message.
//!
//! Once L5 lands typed iox2 introspection (reading `static_details`'s
//! type name back to a generated struct), this restriction lifts.

use std::time::Duration;

use anyhow::Result;
use iceoryx2::prelude::*;

use crate::FrameIter;

/// Generic byte-slab subscriber. Each iox2 sample is copied into a fresh
/// `Vec<u8>` so it outlives the `Sample` borrow (iox2 reclaims the slot
/// when the sample drops).
///
/// Field order matters here: `sub` borrows from the service port factory
/// which borrows from the node; everything has to outlive everything
/// above it. We also intentionally keep `_service` alive — the
/// subscriber port can otherwise lose access to the service config
/// the moment the factory drops (observed under `level4_iox2_smoke`).
pub struct IceoryxFrames {
    sub: iceoryx2::port::subscriber::Subscriber<ipc::Service, [u8], ()>,
    _service:
        iceoryx2::service::port_factory::publish_subscribe::PortFactory<ipc::Service, [u8], ()>,
    _node: Node<ipc::Service>,
    poll: Duration,
}

/// iox2 has no blocking `receive` — mirror the Zenoh blocking shape by
/// polling at 10 ms. Tuned low enough to feel responsive in interactive
/// `sb topic listen`, high enough that idle CPU stays trivial.
const ICEORYX_POLL: Duration = Duration::from_millis(10);

/// Open a generic iox2 subscriber for `[u8]` slabs. Fails if the service
/// was created with a typed struct payload.
pub fn listen_iceoryx(topic_full_name: &str) -> Result<IceoryxFrames> {
    let trimmed = topic_full_name.trim_matches('/');
    let svc = ServiceName::new(trimmed)
        .map_err(|e| anyhow::anyhow!("iox2 service name {trimmed:?}: {e}"))?;
    let node = NodeBuilder::new()
        .create::<ipc::Service>()
        .map_err(|e| anyhow::anyhow!("iox2 node create: {e}"))?;
    // `open_or_create` so a subscriber can race ahead of any publisher
    // (matches Zenoh's declare_subscriber semantics). Fails-open with an
    // actionable error when the service already exists with a different
    // (typed) payload — `IncompatibleTypes`, which we surface verbatim.
    let service = node
        .service_builder(&svc)
        .publish_subscribe::<[u8]>()
        .open_or_create()
        .map_err(|e| {
            anyhow::anyhow!(
                "iox2 service {trimmed:?} open_or_create as `[u8]` failed: {e}. \
                 If the service already exists with a typed payload, this \
                 hits IncompatibleTypes — wait for L5's typed introspection \
                 or use `--transport zenoh`."
            )
        })?;
    let sub = service
        .subscriber_builder()
        .create()
        .map_err(|e| anyhow::anyhow!("iox2 subscriber create: {e}"))?;
    Ok(IceoryxFrames {
        sub,
        _service: service,
        _node: node,
        poll: ICEORYX_POLL,
    })
}

impl FrameIter for IceoryxFrames {
    fn next_frame(&mut self) -> Result<Option<Vec<u8>>> {
        loop {
            match self
                .sub
                .receive()
                .map_err(|e| anyhow::anyhow!("iox2 receive: {e}"))?
            {
                Some(sample) => {
                    let slice: &[u8] = &sample;
                    return Ok(Some(slice.to_vec()));
                }
                None => std::thread::sleep(self.poll),
            }
        }
    }
}

/// Publish a single `[u8]` slab on `topic_full_name`. Same typed-vs-untyped
/// caveat as [`listen_iceoryx`]: the service must be `[u8]` or fail open.
///
/// The post-send sleep is a hack against a subtle iox2 ordering: when
/// `publish_iceoryx_raw` is called from the CLI (one-shot subprocess) and
/// the subscriber is a long-running thread, dropping the publisher port
/// immediately after `send()` can race with the subscriber's next poll
/// such that the sample becomes invisible. A short settle window before
/// the publisher port drops is enough for any local-process subscriber
/// to pick the sample up. Tuned against `level4_pub_roundtrip_iceoryx`.
pub fn publish_iceoryx_raw(topic_full_name: &str, bytes: &[u8]) -> Result<()> {
    let trimmed = topic_full_name.trim_matches('/');
    let svc = ServiceName::new(trimmed)
        .map_err(|e| anyhow::anyhow!("iox2 service name {trimmed:?}: {e}"))?;
    let node = NodeBuilder::new()
        .create::<ipc::Service>()
        .map_err(|e| anyhow::anyhow!("iox2 node create: {e}"))?;
    let service = node
        .service_builder(&svc)
        .publish_subscribe::<[u8]>()
        .open_or_create()
        .map_err(|e| {
            anyhow::anyhow!("iox2 service {trimmed:?} open_or_create as `[u8]` failed: {e}")
        })?;
    let publisher = service
        .publisher_builder()
        .initial_max_slice_len(bytes.len().max(1))
        .create()
        .map_err(|e| anyhow::anyhow!("iox2 publisher create: {e}"))?;
    let sample = publisher
        .loan_slice_uninit(bytes.len())
        .map_err(|e| anyhow::anyhow!("iox2 loan_slice_uninit: {e}"))?;
    let sample = sample.write_from_slice(bytes);
    sample
        .send()
        .map_err(|e| anyhow::anyhow!("iox2 send: {e}"))?;
    // Settle window before publisher drops — see doc comment.
    std::thread::sleep(std::time::Duration::from_millis(200));
    Ok(())
}
