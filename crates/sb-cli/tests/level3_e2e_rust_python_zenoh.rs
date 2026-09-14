//! L3 TDD #9 — Rust↔Python Zenoh E2E.
//!
//! Delegates to [`examples/rust_python_zenoh/run.sh`](../../../examples/rust_python_zenoh/)
//! so the example and the test can never drift apart. The example is
//! the source of truth; this test just runs it.
//!
//! `#[ignore]`'d because it:
//!   - builds zenoh 1.9 from crates.io (first run only — cached after),
//!   - requires `eclipse-zenoh` in conda's base env,
//!   - requires a working Zenoh runtime on loopback.
//!
//! Run with:
//!
//! ```bash
//! cargo test -p sb-cli --test level3_e2e_rust_python_zenoh -- --ignored
//! ```

use std::path::PathBuf;
use std::process::Command;

#[test]
#[ignore = "heavy — needs zenoh runtime + python eclipse-zenoh; runs the example script"]
fn rust_publisher_to_python_subscriber_over_zenoh() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir
        .ancestors()
        .nth(2)
        .expect("CARGO_MANIFEST_DIR has a great-grandparent");
    let script = repo_root.join("examples/rust_python_zenoh/run.sh");
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
