//! L3 TDD #10 — Rust↔Rust iceoryx2 E2E.
//!
//! Delegates to [`examples/rust_rust_iceoryx/run.sh`](../../../examples/rust_rust_iceoryx/).
//! The example uses a shared `swarmbotix_iox_payload` crate so both
//! ends agree on `Frame`'s type-name (iceoryx2 hashes the crate path
//! into the compatibility check).
//!
//! `#[ignore]`'d because it:
//!   - builds iceoryx2 0.9 from crates.io (first run only),
//!   - requires POSIX shared memory on the host.
//!
//! Run with:
//!
//! ```bash
//! cargo test -p sb-cli --test level3_e2e_rust_iceoryx -- --ignored
//! ```

use std::path::PathBuf;
use std::process::Command;

#[test]
#[ignore = "heavy — needs iceoryx2 runtime + shmem; runs the example script"]
fn rust_publisher_to_rust_subscriber_over_iceoryx2() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir
        .ancestors()
        .nth(2)
        .expect("CARGO_MANIFEST_DIR has a great-grandparent");
    let script = repo_root.join("examples/rust_rust_iceoryx/run.sh");
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
