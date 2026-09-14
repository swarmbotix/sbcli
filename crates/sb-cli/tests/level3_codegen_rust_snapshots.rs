//! L3 TDD #1 + #4 (codegen, Rust slice) — byte-exact snapshots.
//!
//! Re-bless after intentional changes:
//!
//! ```bash
//! SB_BLESS=1 cargo test -p sb-cli --test level3_codegen_rust_snapshots
//! ```

mod common;

use std::path::{Path, PathBuf};

use sb_codegen::{CodegenInput, Kind, render};
use sb_core::{Language, Transport};
use sb_vault::MessageName;

use common::assert_or_bless;

/// Fixed synthetic path baked into the snapshots so the goldens stay
/// machine-independent. The Rust template only does path arithmetic on
/// this; nothing reads from disk at codegen time for the Rust slice.
const FIXTURE_TARGETS: &str = "/sb_test_fixture/message_targets";

fn input<'a>(
    kind: Kind,
    transport: Transport,
    name: &'a str,
    topic: &'a str,
    msg: &'a MessageName,
) -> CodegenInput<'a> {
    CodegenInput {
        module: "pubber",
        workspace: "demo",
        device: "dev01",
        language: Language::Rust,
        kind,
        name,
        topic,
        msg_type: msg,
        transport,
        message_targets: Path::new(FIXTURE_TARGETS),
    }
}

#[test]
fn rust_publisher_zenoh_snapshot() {
    let msg = MessageName::parse("ros2/std/StringStamped").unwrap();
    let r = render(&input(
        Kind::Publisher,
        Transport::Zenoh,
        "hello",
        "hello",
        &msg,
    ))
    .unwrap();
    assert_eq!(r.rel_path, PathBuf::from("publishers/hello.rs"));
    assert_or_bless("rust", "publisher_zenoh_hello.rs", &r.contents);
}

#[test]
fn rust_subscriber_zenoh_snapshot() {
    let msg = MessageName::parse("ros2/std/StringStamped").unwrap();
    let r = render(&input(
        Kind::Subscriber,
        Transport::Zenoh,
        "hello",
        "hello",
        &msg,
    ))
    .unwrap();
    assert_eq!(r.rel_path, PathBuf::from("subscribers/hello.rs"));
    assert_or_bless("rust", "subscriber_zenoh_hello.rs", &r.contents);
}

#[test]
fn rust_publisher_iceoryx_snapshot() {
    let msg = MessageName::parse("ros2/std/ImageStamped").unwrap();
    let r = render(&input(
        Kind::Publisher,
        Transport::Iceoryx2,
        "image_raw",
        "image_raw",
        &msg,
    ))
    .unwrap();
    assert_eq!(r.rel_path, PathBuf::from("publishers/image_raw.rs"));
    assert_or_bless("rust", "publisher_iceoryx_image_raw.rs", &r.contents);
}

#[test]
fn rust_subscriber_iceoryx_snapshot() {
    let msg = MessageName::parse("ros2/std/ImageStamped").unwrap();
    let r = render(&input(
        Kind::Subscriber,
        Transport::Iceoryx2,
        "image_raw",
        "image_raw",
        &msg,
    ))
    .unwrap();
    assert_eq!(r.rel_path, PathBuf::from("subscribers/image_raw.rs"));
    assert_or_bless("rust", "subscriber_iceoryx_image_raw.rs", &r.contents);
}
