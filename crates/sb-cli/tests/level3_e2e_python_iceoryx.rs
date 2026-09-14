//! L3 — Python↔Python iceoryx2 E2E (extra coverage beyond the
//! level3.html checklist). Drives
//! [`examples/python_python_iceoryx/run.sh`](../../../examples/python_python_iceoryx/).
//!
//! `#[ignore]`'d because it requires `iceoryx2` in conda's base env and
//! a host that allows POSIX shared memory.
//!
//! Run with:
//!
//! ```bash
//! cargo test -p sb-cli --test level3_e2e_python_iceoryx -- --ignored
//! ```

use std::path::PathBuf;
use std::process::Command;

#[test]
#[ignore = "heavy — needs python iceoryx2 + shmem; runs the example script"]
fn python_publisher_to_python_subscriber_over_iceoryx2() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir.ancestors().nth(2).expect("repo root");
    let script = repo_root.join("examples/python_python_iceoryx/run.sh");
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
