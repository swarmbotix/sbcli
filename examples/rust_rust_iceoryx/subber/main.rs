#[path = "swarmbotix_io/subscribers/frame.rs"]
mod frame_sub;

use iceoryx2::prelude::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let node = NodeBuilder::new().create::<ipc::Service>()?;
    let subscriber = frame_sub::FrameSubscriber::open(&node)?;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while std::time::Instant::now() < deadline {
        if let Some(msg) = subscriber.try_recv_latest()? {
            println!("FRAME ts_ns={}", msg.header_timestamp_ns);
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    Err("timeout waiting for sample".into())
}
