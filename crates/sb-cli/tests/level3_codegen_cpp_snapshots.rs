//! L3 TDD #1 + #4 (codegen, C++ slice) — byte-exact snapshots.
//! Re-bless with `SB_BLESS=1`.

mod common;

use std::path::{Path, PathBuf};

use sb_codegen::{CodegenInput, Kind, render};
use sb_core::{Language, Transport};
use sb_vault::MessageName;

use common::{assert_or_bless, assert_or_bless_with};

/// Fixture iox2 target tree under `tests/fixtures/`. The C++ template
/// reads the `.h` to resolve the proto-derived namespace, so this path
/// must actually exist on disk during the test. The committed file
/// `tests/fixtures/iox2_targets/iox2/std/ImageStamped/ImageStamped.h`
/// declares `namespace swarmbotix_std { ... }`, mirroring what
/// sb-iox2-typegen would emit for `std/ImageStamped`.
fn fixture_targets() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/iox2_targets")
}

fn input<'a>(
    kind: Kind,
    transport: Transport,
    name: &'a str,
    topic: &'a str,
    msg: &'a MessageName,
    targets: &'a Path,
) -> CodegenInput<'a> {
    CodegenInput {
        module: "pubber",
        workspace: "demo",
        device: "dev01",
        language: Language::Cpp,
        kind,
        name,
        topic,
        msg_type: msg,
        transport,
        message_targets: targets,
    }
}

#[test]
fn cpp_publisher_zenoh_snapshot() {
    let msg = MessageName::parse("ros2/std/StringStamped").unwrap();
    let targets = fixture_targets();
    let r = render(&input(
        Kind::Publisher,
        Transport::Zenoh,
        "hello",
        "hello",
        &msg,
        &targets,
    ))
    .unwrap();
    assert_eq!(r.rel_path, PathBuf::from("publishers/hello.cpp"));
    assert_or_bless("cpp", "publisher_zenoh_hello.cpp", &r.contents);
}

#[test]
fn cpp_subscriber_zenoh_snapshot() {
    let msg = MessageName::parse("ros2/std/StringStamped").unwrap();
    let targets = fixture_targets();
    let r = render(&input(
        Kind::Subscriber,
        Transport::Zenoh,
        "hello",
        "hello",
        &msg,
        &targets,
    ))
    .unwrap();
    assert_eq!(r.rel_path, PathBuf::from("subscribers/hello.cpp"));
    assert_or_bless("cpp", "subscriber_zenoh_hello.cpp", &r.contents);
}

#[test]
fn cpp_publisher_iceoryx_snapshot() {
    let msg = MessageName::parse("ros2/std/ImageStamped").unwrap();
    let targets = fixture_targets();
    let targets_str = targets.to_string_lossy().into_owned();
    let r = render(&input(
        Kind::Publisher,
        Transport::Iceoryx2,
        "image_raw",
        "image_raw",
        &msg,
        &targets,
    ))
    .unwrap();
    assert_eq!(r.rel_path, PathBuf::from("publishers/image_raw.cpp"));
    assert_or_bless_with(
        "cpp",
        "publisher_iceoryx_image_raw.cpp",
        &r.contents,
        &[(targets_str.as_str(), "<FIXTURE_TARGETS>")],
    );
}

#[test]
fn cpp_subscriber_iceoryx_snapshot() {
    let msg = MessageName::parse("ros2/std/ImageStamped").unwrap();
    let targets = fixture_targets();
    let targets_str = targets.to_string_lossy().into_owned();
    let r = render(&input(
        Kind::Subscriber,
        Transport::Iceoryx2,
        "image_raw",
        "image_raw",
        &msg,
        &targets,
    ))
    .unwrap();
    assert_eq!(r.rel_path, PathBuf::from("subscribers/image_raw.cpp"));
    assert_or_bless_with(
        "cpp",
        "subscriber_iceoryx_image_raw.cpp",
        &r.contents,
        &[(targets_str.as_str(), "<FIXTURE_TARGETS>")],
    );
}
