//! L1 TDD test #8 — every shipped `std/*.proto` compiles to Rust.
//!
//! Uses `prost-build` programmatically. If any .proto is malformed for
//! Rust codegen (illegal field name, reserved keyword collision, etc.),
//! prost-build returns Err and this test fails.

mod common;
use common::{Sandbox, protoc_path};

#[test]
fn every_std_proto_compiles_to_rust_via_prost_build() {
    let s = Sandbox::new();
    let Some(protoc) = protoc_path(&s) else {
        eprintln!("protoc not configured on this host — skipping");
        return;
    };
    if !protoc.exists() {
        eprintln!("protoc at {} missing — skipping", protoc.display());
        return;
    }
    // Populate std/* in the sandbox.
    let out = s.cmd().args(["message", "list"]).output().unwrap();
    assert!(out.status.success(), "populate failed: {out:?}");

    let vault = s.vault_dir();
    let out_dir = s.home().join("prost_out");
    std::fs::create_dir_all(&out_dir).unwrap();

    // Discover every std/*.proto.
    let mut sources: Vec<std::path::PathBuf> = std::fs::read_dir(vault.join("std"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("proto"))
        .collect();
    sources.sort();
    assert!(!sources.is_empty(), "no .proto files to compile");

    // Direct prost-build to the configured protoc.
    std::env::set_var("PROTOC", &protoc);
    std::env::set_var("OUT_DIR", &out_dir);

    let mut cfg = prost_build::Config::new();
    cfg.out_dir(&out_dir);
    cfg.compile_protos(&sources, &[vault.as_path()])
        .expect("prost-build must successfully generate Rust for every std/*.proto");

    // Confirm at least one .rs file was produced.
    let produced: Vec<_> = std::fs::read_dir(&out_dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("rs"))
        .collect();
    assert!(!produced.is_empty(), "prost-build produced no .rs output");
}
