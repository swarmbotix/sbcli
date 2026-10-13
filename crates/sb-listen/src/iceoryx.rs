//! iceoryx2-backed frame iterator + raw publisher.
//!
//! iceoryx2 services are strongly typed: opening one requires the exact
//! payload type it was created with, or the open fails with
//! `IncompatibleTypes`. Every sb-generated publisher is typed (one flat
//! `#[repr(C)]` struct per vault message), and `sb topic listen` has none
//! of those types compiled in. So [`listen_iceoryx`] first reads the live
//! service's payload [`TypeDetail`] from the service registry (the same
//! `Service::list` walk `sb topic list` does) and opens the service with
//! that detail over iceoryx2's type-erased `CustomPayloadMarker` payload,
//! the mechanism the iceoryx2 C / C++ / Python bindings use. Each sample's
//! bytes are then copied out verbatim and decoded later against the vault
//! layout (see [`crate::iox2_decode`]).
//!
//! A listener never creates the service. It has no payload type of its own
//! to give it, and a service it created with a stand-in type (a `[u8]` slab)
//! would make the real typed publisher fail with `IncompatibleTypes` when it
//! starts later. Until the service exists, [`listen_iceoryx`] waits for it.

use std::time::{Duration, Instant};

use anyhow::Result;
use iceoryx2::port::subscriber::Subscriber;
use iceoryx2::prelude::*;
use iceoryx2::service::builder::publish_subscribe::PublishSubscribeOpenError;
use iceoryx2::service::builder::{CustomHeaderMarker, CustomPayloadMarker};
use iceoryx2::service::port_factory::publish_subscribe::PortFactory;
use iceoryx2::service::static_config::message_type_details::{
    MessageTypeDetails, TypeDetail, TypeVariant,
};
use iceoryx2::service::static_config::messaging_pattern::MessagingPattern;

use crate::{Frame, FrameIter, FramePoll};

/// Payload type of the live service, as its creator registered it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Iox2PayloadDetail {
    /// Language-native type name (`StringStamped` for sb-generated types,
    /// which pin it to the vault leaf; otherwise e.g. a Rust
    /// `core::any::type_name` path).
    pub type_name: String,
    /// `size_of` one payload element.
    pub size: usize,
    /// `align_of` one payload element.
    pub alignment: usize,
    /// `true` for a slice payload (`[T]`): a sample holds `n * size` bytes.
    pub is_slice: bool,
}

impl Iox2PayloadDetail {
    fn from_type_detail(detail: &TypeDetail) -> Self {
        Self {
            type_name: detail.type_name().to_string(),
            size: detail.size(),
            alignment: detail.alignment(),
            is_slice: detail.variant() == TypeVariant::Dynamic,
        }
    }

    /// The name to report as the frame's wire schema. A `[u8]` byte slab
    /// (what `sb topic pub` creates) is untyped, so it carries none and the
    /// Header probe gets its turn.
    fn wire_schema(&self) -> Option<String> {
        (self.type_name != "u8" && !self.type_name.is_empty()).then(|| self.type_name.clone())
    }
}

type CustomSubscriber = Subscriber<ipc::Service, [CustomPayloadMarker], CustomHeaderMarker>;
type CustomPortFactory = PortFactory<ipc::Service, [CustomPayloadMarker], CustomHeaderMarker>;

/// Snapshots each iox2 sample into a fresh `Vec<u8>` so it outlives the
/// `Sample` borrow (iox2 reclaims the slot when the sample drops).
///
/// Field order matters here: `sub` borrows from the service port factory
/// which borrows from the node; everything has to outlive everything above
/// it, and fields drop in declaration order. We also intentionally keep
/// `_service` alive — the subscriber port can otherwise lose access to the
/// service config the moment the factory drops (observed under
/// `level4_iox2_smoke`).
pub struct IceoryxFrames {
    sub: CustomSubscriber,
    _service: CustomPortFactory,
    payload: Iox2PayloadDetail,
    poll: Duration,
    _node: Node<ipc::Service>,
}

/// iox2 has no blocking `receive` — mirror the Zenoh blocking shape by
/// polling at 10 ms. Tuned low enough to feel responsive in interactive
/// `sb topic listen`, high enough that idle CPU stays trivial.
const ICEORYX_POLL: Duration = Duration::from_millis(10);

/// How often [`listen_iceoryx`] re-reads the service registry while the
/// service does not exist yet.
const DISCOVERY_POLL: Duration = Duration::from_millis(200);

/// Subscribe to the iox2 publish-subscribe service `topic_full_name`,
/// opened with the payload type it was created with, whatever that type is.
///
/// Blocks until the service exists, re-reading the registry every 200 ms
/// and saying so once on stderr. A service that disappears between being
/// listed and being opened is waited for again.
pub fn listen_iceoryx(topic_full_name: &str) -> Result<IceoryxFrames> {
    let trimmed = topic_full_name.trim_matches('/');
    let svc = ServiceName::new(trimmed)
        .map_err(|e| anyhow::anyhow!("iox2 service name {trimmed:?}: {e}"))?;
    // Created only once a service exists, then kept across retries.
    let mut node: Option<Node<ipc::Service>> = None;
    let mut announced = false;
    loop {
        if let Some(details) = live_message_type_details(trimmed)? {
            let live_node = match node.take() {
                Some(n) => n,
                None => NodeBuilder::new()
                    .create::<ipc::Service>()
                    .map_err(|e| anyhow::anyhow!("iox2 node create: {e}"))?,
            };
            let payload = Iox2PayloadDetail::from_type_detail(&details.payload);
            match open_typed(&live_node, &svc, &details) {
                Ok(service) => {
                    let sub = service
                        .subscriber_builder()
                        .create()
                        .map_err(|e| anyhow::anyhow!("iox2 subscriber create: {e}"))?;
                    return Ok(IceoryxFrames {
                        sub,
                        _service: service,
                        payload,
                        poll: ICEORYX_POLL,
                        _node: live_node,
                    });
                }
                Err(e) if service_not_ready(&e) => {}
                Err(e) => anyhow::bail!(
                    "iox2 service {trimmed:?} (payload type {:?}, {} byte(s)) could not be \
                     opened: {e}",
                    payload.type_name,
                    payload.size,
                ),
            }
            node = Some(live_node);
        }
        if !announced {
            eprintln!("waiting for iox2 service {trimmed}...");
            announced = true;
        }
        std::thread::sleep(DISCOVERY_POLL);
    }
}

impl IceoryxFrames {
    /// The live service's payload type.
    pub fn payload_detail(&self) -> &Iox2PayloadDetail {
        &self.payload
    }

    /// One non-blocking receive.
    fn try_receive(&self) -> Result<Option<Frame>> {
        // SAFETY: the port was opened with the service's own payload type
        // detail (see `open_typed`), so each sample header's element count
        // is in units of that detail's `size`, which is what
        // `receive_custom_payload` requires.
        let sample = unsafe { self.sub.receive_custom_payload() }
            .map_err(|e| anyhow::anyhow!("iox2 receive: {e}"))?;
        Ok(sample.map(|sample| {
            let payload: &[CustomPayloadMarker] = sample.payload();
            // SAFETY: `CustomPayloadMarker` is a `#[repr(C)]` newtype over one
            // `u8` (size 1, align 1), so the slice is `payload.len()`
            // contiguous bytes. They live in the sample's shared-memory chunk,
            // which stays mapped while `sample` is alive, and are copied out
            // before it drops. The chunk is mapped from a shared-memory
            // object, so every byte is initialized memory (the iceoryx2
            // bindings read custom payloads the same way).
            let raw =
                unsafe { std::slice::from_raw_parts(payload.as_ptr().cast::<u8>(), payload.len()) };
            Frame {
                bytes: raw.to_vec(),
                wire_schema: self.payload.wire_schema(),
            }
        }))
    }
}

/// Message type details of the publish-subscribe service named `name`, read
/// from the iox2 service registry. `None` when no such service exists.
fn live_message_type_details(name: &str) -> Result<Option<MessageTypeDetails>> {
    let mut found = None;
    ipc::Service::list(Config::global_config(), |svc| {
        if svc.static_details.name().as_str() != name {
            return CallbackProgression::Continue;
        }
        match svc.static_details.messaging_pattern() {
            MessagingPattern::PublishSubscribe(c) => {
                found = Some(*c.message_type_details());
                CallbackProgression::Stop
            }
            // Same name, other pattern (event, request-response): not ours.
            _ => CallbackProgression::Continue,
        }
    })
    .map_err(|e| anyhow::anyhow!("iceoryx2 Service::list failed: {e}"))?;
    Ok(found)
}

/// Open an existing service with its registered payload and user-header
/// types.
fn open_typed(
    node: &Node<ipc::Service>,
    svc: &ServiceName,
    details: &MessageTypeDetails,
) -> Result<CustomPortFactory, PublishSubscribeOpenError> {
    let builder = node
        .service_builder(svc)
        .publish_subscribe::<[CustomPayloadMarker]>()
        .user_header::<CustomHeaderMarker>();
    // SAFETY: both overrides are the service's own registered type details,
    // copied verbatim from its static config a moment ago. The open below
    // re-checks them against that config (iceoryx2 `is_compatible_to`), so
    // the layout this port assumes is exactly the one the publisher writes.
    // The payload is only ever read back as raw bytes, never as a typed
    // value, so no Rust type has to match it.
    let builder = unsafe {
        builder
            .__internal_set_payload_type_details(&details.payload)
            .__internal_set_user_header_type_details(&details.user_header)
    };
    builder.open()
}

/// Open failures that mean "the service is going away or not finished
/// being created", i.e. wait and look again.
fn service_not_ready(e: &PublishSubscribeOpenError) -> bool {
    matches!(
        e,
        PublishSubscribeOpenError::DoesNotExist
            | PublishSubscribeOpenError::IsMarkedForDestruction
            | PublishSubscribeOpenError::ServiceInCorruptedState
            | PublishSubscribeOpenError::HangsInCreation
    )
}

impl FrameIter for IceoryxFrames {
    fn next_frame(&mut self) -> Result<Option<Frame>> {
        loop {
            if let Some(frame) = self.try_receive()? {
                return Ok(Some(frame));
            }
            std::thread::sleep(self.poll);
        }
    }

    fn poll_frame(&mut self, timeout: Duration) -> Result<FramePoll> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(frame) = self.try_receive()? {
                return Ok(FramePoll::Frame(frame));
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(FramePoll::Timeout);
            }
            std::thread::sleep(self.poll.min(deadline - now));
        }
    }
}

/// Publish a single `[u8]` slab on `topic_full_name`. The service must be a
/// `[u8]` slab or not exist yet; a typed service fails to open
/// (`IncompatibleTypes`).
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
