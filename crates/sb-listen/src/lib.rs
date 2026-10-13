//! L4 — frame iterators + raw one-shot publishers for `sb topic listen/pub`.
//!
//! Two transports, one shape per direction:
//!
//! - **Listen**: [`FrameIter::next_frame`] blocks until the next sample and
//!   returns it as a [`Frame`]: the payload bytes plus whatever schema the
//!   transport itself carried (Zenoh's encoding schema, iceoryx2's payload
//!   type name). `Ok(None)` = session closed; `Err(_)` = transport error.
//!   The caller (`sb topic listen`) then decodes via
//!   [`decode_frame_dynamic`] or, with `--raw`, hex-dumps.
//! - **Publish**: [`publish_zenoh_raw`] / [`publish_iceoryx_raw`] put exactly
//!   one sample on the wire and return. Iox2's path requires a known payload
//!   layout; at L4 we restrict it to fixed-width byte slabs (the user
//!   passed us hex bytes) sized to match the receiver's `[u8; N]` payload.

pub mod decode;
pub mod iceoryx;
pub mod image_fields;
pub mod iox2_decode;
pub mod pool;
pub mod view;
pub mod zenoh_bytes;

pub use decode::{
    DecodeContext, DecodedFrame, FbLookup, FramePayload, Iox2Lookup, decode_frame,
    decode_frame_dynamic, render_decoded_json_line,
};
pub use iceoryx::{IceoryxFrames, Iox2PayloadDetail, listen_iceoryx, publish_iceoryx_raw};
pub use image_fields::{ImageFields, extract_compressed, extract_image};
pub use iox2_decode::{
    Iox2DecodeError, Iox2DecodeOptions, Iox2Field, Iox2Packing, Iox2Scalar, Iox2Struct, Iox2Type,
    decode_iox2_struct,
};
pub use pool::{CompiledStyle, build_pool, compile_styles, pool_from_styles};
pub use sb_discover::{WireTag, parse_wire_tag};
pub use zenoh_bytes::{ZenohFrames, listen_zenoh, publish_zenoh_raw};

use std::time::Duration;

use anyhow::Result;

/// One received sample, as the transport delivered it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Payload bytes, copied out of the transport's buffer.
    pub bytes: Vec<u8>,
    /// Protobuf schema the transport carried alongside the payload, if any.
    ///
    /// - Zenoh: the vault name (`<style>/<ns>/<Leaf>`) of a
    ///   [`WireTag::Protobuf`] encoding; `None` for every other tag.
    /// - iceoryx2: the live service's payload type name, which is
    ///   language-native (`StringStamped`, `my_crate::StringStamped`, …),
    ///   not a vault name. `None` for plain `[u8]` byte-slab services.
    pub wire_schema: Option<String>,
    /// The sample's Zenoh encoding, classified ([`parse_wire_tag`]).
    /// `None` on iceoryx2, which has no per-sample encoding.
    pub wire_tag: Option<WireTag>,
}

impl From<Vec<u8>> for Frame {
    /// A frame with no transport-level schema.
    fn from(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            wire_schema: None,
            wire_tag: None,
        }
    }
}

/// Outcome of [`FrameIter::poll_frame`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FramePoll {
    Frame(Frame),
    /// Nothing arrived within the timeout.
    Timeout,
    /// The session closed; no more frames will come.
    Closed,
}

/// Common shape every `sb topic listen` consumer iterates over. Returning
/// `Ok(None)` signals the underlying session closed cleanly; errors are
/// surfaced verbatim so the CLI can render a clean exit message.
pub trait FrameIter {
    /// Block until the next frame.
    fn next_frame(&mut self) -> Result<Option<Frame>>;

    /// Wait at most `timeout` for the next frame, for a caller with other
    /// work between samples (the `--show` window has to keep handling its
    /// events).
    fn poll_frame(&mut self, timeout: Duration) -> Result<FramePoll>;
}
