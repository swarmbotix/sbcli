//! L4 — frame iterators + raw one-shot publishers for `sb topic listen/pub`.
//!
//! Two transports, one shape per direction:
//!
//! - **Listen**: [`FrameIter::next_frame`] blocks until the next sample,
//!   returns its raw payload bytes (Zenoh) or a snapshot of the typed
//!   iox2 payload bytes. `Ok(None)` = session closed; `Err(_)` = transport
//!   error. The caller (`sb topic listen`) then either decodes via
//!   sb-discover's Header probe or — with `--raw` — hex-dumps.
//! - **Publish**: [`publish_zenoh`] / [`publish_iceoryx`] put exactly one
//!   sample on the wire and return. Iox2's path requires a known payload
//!   layout; at L4 we restrict it to fixed-width byte slabs (the user
//!   passed us hex bytes) sized to match the receiver's `[u8; N]` payload.

pub mod decode;
pub mod iceoryx;
pub mod zenoh_bytes;

pub use decode::{
    DecodedFrame, FramePayload, decode_frame, decode_frame_dynamic, render_decoded_json_line,
};
pub use iceoryx::{IceoryxFrames, listen_iceoryx, publish_iceoryx_raw};
pub use zenoh_bytes::{ZenohFrames, listen_zenoh, publish_zenoh_raw};

use anyhow::Result;

/// Common shape every `sb topic listen` consumer iterates over. Returning
/// `Ok(None)` signals the underlying session closed cleanly; errors are
/// surfaced verbatim so the CLI can render a clean exit message.
pub trait FrameIter {
    fn next_frame(&mut self) -> Result<Option<Vec<u8>>>;
}
