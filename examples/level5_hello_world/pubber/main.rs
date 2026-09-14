//! L5 e2e pubber — launched via `sb up`, uses L3-scaffolded zenoh publisher.

#[path = "swarmbotix_io/publishers/hello.rs"]
mod hello;

pub mod swarmbotix_std {
    include!(concat!(env!("OUT_DIR"), "/swarmbotix.std.rs"));
}

use prost::Message;
use swarmbotix_std::{Header, StringStamped, header::Metadata};
use zenoh::Wait;

const TOPIC_FULL: &str = "/dev01/demo/pubber/hello";
const DECLARED_RATE_HZ: f64 = 10.0;

fn build_stamped() -> Vec<u8> {
    let msg = StringStamped {
        header: Some(Header {
            timestamp_ns: 1_700_000_000_000_000_000,
            frame_id: "world".into(),
            metadata: Some(Metadata {
                topic_full_name: TOPIC_FULL.into(),
                msg_type: "std/StringStamped".into(),
                msg_freq_desired: DECLARED_RATE_HZ,
            }),
        }),
        data: "HELLO_FROM_L5".into(),
    };
    let mut buf = Vec::with_capacity(msg.encoded_len());
    msg.encode(&mut buf).expect("encode StringStamped");
    buf
}

fn main() -> zenoh::Result<()> {
    let session = zenoh::open(zenoh::Config::default()).wait()?;
    let pubber = hello::HelloPublisher::open(&session)?;
    let payload = build_stamped();
    // Publish at ~10 Hz forever — `sb up` runs us under tmux, `sb down`
    // kills the session and us with it.
    loop {
        pubber.publish(&payload)?;
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
