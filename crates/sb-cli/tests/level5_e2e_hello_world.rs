//! L5 TDD #9 — full-stack end-to-end smoke test.
//!
//! Delegates to [examples/level5_hello_world/run.sh](../../../examples/level5_hello_world/run.sh)
//! so the example and the test are the same artifact (example breaks ⇒
//! test catches it; test passes ⇒ example is guaranteed to work).
//!
//! Ignored by default because it touches real Zenoh + tmux + cargo and
//! takes ~30 s. Run with:
//!
//! ```bash
//! cargo test -p sb-cli --test level5_e2e_hello_world -- --ignored
//! ```

use std::path::PathBuf;
use std::process::Command;

#[test]
#[ignore]
fn level5_e2e_hello_world() {
    // Make sure the `sb` binary is up to date before the script tries
    // to run it. The script bails out cleanly if not.
    let build = Command::new("cargo")
        .args(["build", "--bin", "sb"])
        .output()
        .expect("spawn cargo build --bin sb");
    assert!(
        build.status.success(),
        "cargo build --bin sb failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root");
    let script = repo_root.join("examples/level5_hello_world/run.sh");
    assert!(script.exists(), "missing {}", script.display());

    let out = Command::new("bash")
        .arg(&script)
        .output()
        .expect("spawn run.sh");
    if !out.status.success() {
        panic!(
            "level5_hello_world/run.sh failed:\nstatus={}\n--- stdout ---\n{}\n--- stderr ---\n{}",
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("PASS:"),
        "expected PASS line, got:\n{stdout}"
    );
}
