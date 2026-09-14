//! Compile `std/Header` + `std/StringStamped` from the forge vault.

use std::path::PathBuf;

fn main() {
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = here
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .expect("expected examples/level5_hello_world/subber/ under repo root");
    let messages = repo_root.join("messages");
    let header = messages.join("std/Header.proto");
    let stamped = messages.join("std/StringStamped.proto");
    println!("cargo:rerun-if-changed={}", header.display());
    println!("cargo:rerun-if-changed={}", stamped.display());
    let mut cfg = prost_build::Config::new();
    cfg.compile_protos(
        &[header.to_string_lossy().as_ref(), stamped.to_string_lossy().as_ref()],
        &[messages.to_string_lossy().as_ref()],
    )
    .expect("prost-build compile");
}
