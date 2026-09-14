//! L1 TDD test #9 — vault lookup API via FileDescriptorSet.
//!
//! `compile_all` writes a FileDescriptorSet; we decode it and confirm
//! `std/Header` is present with the load-bearing
//! `metadata.msg_freq_desired` field.

mod common;
use common::{Sandbox, protoc_path};

use prost::Message;
use prost_types::FileDescriptorSet;

#[test]
fn header_has_msg_freq_desired_field_in_descriptor_set() {
    let s = Sandbox::new();
    let Some(protoc) = protoc_path(&s) else {
        eprintln!("protoc not configured on this host — skipping");
        return;
    };
    if !protoc.exists() {
        eprintln!("protoc at {} missing — skipping", protoc.display());
        return;
    }

    // Populate std/* and compile.
    s.cmd().args(["message", "list"]).output().unwrap();
    let out = s.cmd().args(["message", "compile"]).output().unwrap();
    assert!(out.status.success(), "compile failed: {out:?}");

    let descriptor = s.vault_dir().join(".cache").join("descriptor.bin");
    let bytes = std::fs::read(&descriptor).unwrap();
    let fds = FileDescriptorSet::decode(&*bytes).expect("decode FileDescriptorSet");

    // Find std/Header.proto.
    let header_file = fds
        .file
        .iter()
        .find(|f| f.name() == "std/Header.proto")
        .expect("std/Header.proto in descriptor set");

    let header_msg = header_file
        .message_type
        .iter()
        .find(|m| m.name() == "Header")
        .expect("Header message");

    // Header has a `metadata` field referencing Metadata.
    let metadata_field = header_msg
        .field
        .iter()
        .find(|f| f.name() == "metadata")
        .expect("Header.metadata field");
    assert_eq!(
        metadata_field.type_name(),
        ".swarmbotix.std.Header.Metadata"
    );

    // Header.Metadata has msg_freq_desired (double).
    let metadata_msg = header_msg
        .nested_type
        .iter()
        .find(|m| m.name() == "Metadata")
        .expect("Header.Metadata nested type");
    let freq_field = metadata_msg
        .field
        .iter()
        .find(|f| f.name() == "msg_freq_desired")
        .expect("metadata.msg_freq_desired");
    use prost_types::field_descriptor_proto::Type;
    assert_eq!(freq_field.r#type(), Type::Double);
}

#[test]
fn all_bundled_std_messages_present_in_descriptor_set() {
    let s = Sandbox::new();
    let Some(protoc) = protoc_path(&s) else {
        eprintln!("protoc not configured on this host — skipping");
        return;
    };
    if !protoc.exists() {
        return;
    }
    s.cmd().args(["message", "list"]).output().unwrap();
    let out = s.cmd().args(["message", "compile"]).output().unwrap();
    assert!(out.status.success());

    let descriptor = s.vault_dir().join(".cache").join("descriptor.bin");
    let fds = FileDescriptorSet::decode(&*std::fs::read(&descriptor).unwrap()).unwrap();
    let names: Vec<&str> = fds.file.iter().map(|f| f.name()).collect();
    for expected in [
        "std/Header.proto",
        "std/String.proto",
        "std/StringStamped.proto",
        "std/Vector3.proto",
        "std/Twist.proto",
        "std/TwistStamped.proto",
        "std/Image.proto",
        "std/ImageStamped.proto",
    ] {
        assert!(
            names.contains(&expected),
            "missing {expected} in descriptor set, got {names:?}"
        );
    }
}
