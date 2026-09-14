//! L3 TDD #1 + #4 (codegen, Flutter slice) — byte-exact snapshots.
//! Flutter has zenoh only (no iceoryx2 on mobile sandbox).
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
        language: Language::Flutter,
        kind,
        name,
        topic,
        msg_type: msg,
        transport,
        // Flutter is zenoh-only; the codegen ignores message_targets for
        // zenoh templates. Pin a placeholder so the field stays explicit.
        message_targets: Path::new("/sb_test_fixture/message_targets"),
    }
}

#[test]
fn flutter_publisher_zenoh_snapshot() {
    let msg = MessageName::parse("ros2/std/StringStamped").unwrap();
    let r = render(&input(
        Kind::Publisher,
        Transport::Zenoh,
        "hello",
        "hello",
        &msg,
    ))
    .unwrap();
    assert_eq!(r.rel_path, PathBuf::from("publishers/hello.dart"));
    assert_or_bless("flutter", "publisher_zenoh_hello.dart", &r.contents);
}

#[test]
fn flutter_subscriber_zenoh_snapshot() {
    let msg = MessageName::parse("ros2/std/StringStamped").unwrap();
    let r = render(&input(
        Kind::Subscriber,
        Transport::Zenoh,
        "hello",
        "hello",
        &msg,
    ))
    .unwrap();
    assert_eq!(r.rel_path, PathBuf::from("subscribers/hello.dart"));
    assert_or_bless("flutter", "subscriber_zenoh_hello.dart", &r.contents);
}

#[test]
fn flutter_has_no_iceoryx_template() {
    // Structural — flutter has no iox2 path. The CLI must surface this
    // as an "impossible" error, not "not yet implemented".
    assert!(!template_exists(
        Language::Flutter,
        Transport::Iceoryx2,
        Kind::Publisher
    ));
    assert!(!template_exists(
        Language::Flutter,
        Transport::Iceoryx2,
        Kind::Subscriber
    ));
}
