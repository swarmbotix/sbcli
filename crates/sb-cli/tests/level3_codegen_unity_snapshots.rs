//! L3 TDD #1 + #4 (codegen, Unity slice) — byte-exact snapshots.
//! Unity has zenoh only (no iceoryx2 across game sandbox).
//! Re-bless with `SB_BLESS=1`.

mod common;

use std::path::{Path, PathBuf};

use sb_codegen::{CodegenInput, Kind, render, template_exists};
use sb_core::{Language, Transport};
use sb_vault::MessageName;

use common::assert_or_bless;

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
        language: Language::Unity,
        kind,
        name,
        topic,
        msg_type: msg,
        transport,
        // Unity is zenoh-only; the codegen ignores message_targets for
        // zenoh templates. Pin a placeholder so the field stays explicit.
        message_targets: Path::new("/sb_test_fixture/message_targets"),
    }
}

#[test]
fn unity_publisher_zenoh_snapshot() {
    let msg = MessageName::parse("ros2/std/StringStamped").unwrap();
    let r = render(&input(
        Kind::Publisher,
        Transport::Zenoh,
        "hello",
        "hello",
        &msg,
    ))
    .unwrap();
    assert_eq!(r.rel_path, PathBuf::from("publishers/hello.cs"));
    assert_or_bless("unity", "publisher_zenoh_hello.cs", &r.contents);
}

#[test]
fn unity_subscriber_zenoh_snapshot() {
    let msg = MessageName::parse("ros2/std/StringStamped").unwrap();
    let r = render(&input(
        Kind::Subscriber,
        Transport::Zenoh,
        "hello",
        "hello",
        &msg,
    ))
    .unwrap();
    assert_eq!(r.rel_path, PathBuf::from("subscribers/hello.cs"));
    assert_or_bless("unity", "subscriber_zenoh_hello.cs", &r.contents);
}

#[test]
fn unity_has_no_iceoryx_template() {
    assert!(!template_exists(
        Language::Unity,
        Transport::Iceoryx2,
        Kind::Publisher
    ));
    assert!(!template_exists(
        Language::Unity,
        Transport::Iceoryx2,
        Kind::Subscriber
    ));
}
