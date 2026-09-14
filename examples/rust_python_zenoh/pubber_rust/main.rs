//! Example pubber driven by `examples/rust_python_zenoh/run.sh`.
//!
//! Reads the publisher class scaffolded by `sb pub add` (lives under
//! `swarmbotix_io/publishers/hello.rs`) and publishes a known payload
//! a handful of times so the Python subscriber has a chance to attach.

#[path = "swarmbotix_io/publishers/hello.rs"]
mod hello;

use zenoh::Wait;

// Zenoh's `Result` is `Result<T, Box<dyn Error + Send + Sync>>`. Match it
// so `?` propagates cleanly.
fn main() -> zenoh::Result<()> {
    let session = zenoh::open(zenoh::Config::default()).wait()?;
    let pubber = hello::HelloPublisher::open(&session)?;
    // Publish for ~5 seconds at 10 Hz so a Python subscriber that
    // hasn't quite finished its scout/declare round-trip still catches
    // at least one sample.
    for _ in 0..50 {
        pubber.publish(b"HELLO_FROM_RUST")?;
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Ok(())
}
