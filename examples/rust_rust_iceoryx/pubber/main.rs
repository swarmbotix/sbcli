#[path = "swarmbotix_io/publishers/frame.rs"]
mod frame_pub;

// The codegen re-exports the iox2-flat type for us — no shared payload
// crate, no `<T>` generic. Both pubber and subber agree on
// `swarmbotix_std::ImageStamped` because both pull it from the same
// `<message_targets>/iox2/std/ImageStamped/ImageStamped.rs`.
use frame_pub::ImageStamped;

use iceoryx2::prelude::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let node = NodeBuilder::new().create::<ipc::Service>()?;
    let publisher = frame_pub::FramePublisher::open(&node)?;
    for seq in 0..40u32 {
        let mut msg = ImageStamped::default();
        msg.header_timestamp_ns = 1_700_000_000_000_000_000 + u64::from(seq);
        publisher.publish(msg)?;
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    Ok(())
}
