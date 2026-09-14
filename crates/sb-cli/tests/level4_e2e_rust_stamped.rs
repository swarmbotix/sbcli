//! L4 TDD #3 — true end-to-end: an L3-scaffolded Rust Zenoh publisher
//! emitting real protobuf `std/StringStamped` → `sb topic list` shows
//! the topic with schema + rate populated from the Header.
//!
//! Delegates to [`examples/rust_zenoh_stamped/run.sh`](../../../examples/rust_zenoh_stamped/)
//! so the example and the test can never drift apart. The script is
//! the source of truth.
//!
//! `#[ignore]`'d because it:
//!   - depends on `protoc` (the dev box's `/opt/protobuffer/...`),
//!   - compiles zenoh 1.9 the first time (cached after),
//!   - needs a working Zenoh runtime on loopback.
//!
//! Run with:
//! ```bash
//! cargo test -p sb-cli --test level4_e2e_rust_stamped -- --ignored
//! ```

use std::path::PathBuf;
use std::process::Command;

#[test]
#[ignore = "heavy — needs protoc + zenoh; runs the example script"]
fn l3_scaffolded_stamped_publisher_is_discovered_with_schema_and_rate() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir
        .ancestors()
        .nth(2)
        .expect("CARGO_MANIFEST_DIR has a great-grandparent");
    let script = repo_root.join("examples/rust_zenoh_stamped/run.sh");
    assert!(
        script.is_file(),
        "example script not found: {}",
        script.display()
    );

    let out = Command::new("bash")
        .arg(&script)
        .output()
        .expect("spawn example run.sh");
    if !out.status.success() {
        panic!(
            "example failed:\n--- stdout ---\n{}\n--- stderr ---\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("PASS:"),
        "example did not print PASS:\n{stdout}"
    );
}
