//! L3 TDD #1 + #4 (codegen, Python slice) — byte-exact snapshots.
//!
//! Re-bless with `SB_BLESS=1`.

mod common;

use std::path::{Path, PathBuf};

use sb_codegen::{CodegenInput, Kind, render};
use sb_core::{Language, Transport};
use sb_vault::MessageName;

use common::assert_or_bless;

/// Synthetic absolute path baked into the snapshots so they stay
/// machine-independent. Python templates only do path arithmetic on
/// this — no disk read at codegen time.
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
        language: Language::Python,
        kind,
        name,
        topic,
        msg_type: msg,
        transport,
        message_targets: Path::new(FIXTURE_TARGETS),
    }
}

#[test]
fn python_publisher_zenoh_snapshot() {
    let msg = MessageName::parse("ros2/std/StringStamped").unwrap();
    let r = render(&input(
        Kind::Publisher,
        Transport::Zenoh,
        "hello",
        "hello",
        &msg,
    ))
    .unwrap();
    assert_eq!(r.rel_path, PathBuf::from("publishers/hello.py"));
    assert_or_bless("python", "publisher_zenoh_hello.py", &r.contents);
}

#[test]
fn python_subscriber_zenoh_snapshot() {
    let msg = MessageName::parse("ros2/std/StringStamped").unwrap();
    let r = render(&input(
        Kind::Subscriber,
        Transport::Zenoh,
        "hello",
        "hello",
        &msg,
    ))
    .unwrap();
    assert_eq!(r.rel_path, PathBuf::from("subscribers/hello.py"));
    assert_or_bless("python", "subscriber_zenoh_hello.py", &r.contents);
}

#[test]
fn python_publisher_iceoryx_snapshot() {
    let msg = MessageName::parse("ros2/std/ImageStamped").unwrap();
    let r = render(&input(
        Kind::Publisher,
        Transport::Iceoryx2,
        "image_raw",
        "image_raw",
        &msg,
    ))
    .unwrap();
    assert_eq!(r.rel_path, PathBuf::from("publishers/image_raw.py"));
    assert_or_bless("python", "publisher_iceoryx_image_raw.py", &r.contents);
}

#[test]
fn python_subscriber_iceoryx_snapshot() {
    let msg = MessageName::parse("ros2/std/ImageStamped").unwrap();
    let r = render(&input(
        Kind::Subscriber,
        Transport::Iceoryx2,
        "image_raw",
        "image_raw",
        &msg,
    ))
    .unwrap();
    assert_eq!(r.rel_path, PathBuf::from("subscribers/image_raw.py"));
    assert_or_bless("python", "subscriber_iceoryx_image_raw.py", &r.contents);
}
