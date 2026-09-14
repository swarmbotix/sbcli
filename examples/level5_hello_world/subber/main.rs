//! L5 e2e subber — launched via `sb up`, uses L3-scaffolded zenoh subscriber.
//!
//! Decodes each `StringStamped` frame and prints `RX_HELLO data=...`.
//! The shell test harness greps for that prefix as proof the launch +
//! cross-module wiring is alive.

#[path = "swarmbotix_io/subscribers/hello.rs"]
mod hello;

pub mod swarmbotix_std {
    include!(concat!(env!("OUT_DIR"), "/swarmbotix.std.rs"));
}

use prost::Message;
use std::io::Write;
use swarmbotix_std::StringStamped;
use zenoh::Wait;

fn main() -> zenoh::Result<()> {
    let session = zenoh::open(zenoh::Config::default()).wait()?;
    let sub = hello::HelloSubscriber::open(&session)?;
    loop {
        if let Some(bytes) = sub.try_recv_latest()? {
            match StringStamped::decode(bytes.as_slice()) {
                Ok(msg) => {
                    println!("RX_HELLO data={}", msg.data);
                    std::io::stdout().flush().ok();
                }
                Err(e) => eprintln!("decode error: {e}"),
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}
