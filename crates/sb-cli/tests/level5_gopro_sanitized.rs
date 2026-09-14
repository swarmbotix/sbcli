//! L5 TDD #6 — `sb gopro` output passes sanitization.
//!
//! `sb.prd.yml` must contain no `/home/`, `/Users/`, `C:\`, no `root:` /
//! `language:` / `io_dir:` keys. We run gopro on a module whose `root`
//! is a `/tmp/...` tempdir path (which would leak any of those if the
//! transform forgot to drop the field), then grep the output.

mod common;
use common::L5Sandbox;

#[test]
fn gopro_output_strips_host_specific_fields() {
    let mut sb = L5Sandbox::empty();
    let m = sb.adopt("alpha", Some("#!/usr/bin/env bash\nexit 0\n"));

    // Run gopro inside the module's cwd so the default-cwd resolution
    // picks it up.
    let out = sb
        .cmd()
        .current_dir(&m.root)
        .args(["gopro"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "sb gopro failed: {}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    let prd = m.root.join("sb.prd.yml");
    assert!(prd.exists(), "sb.prd.yml not written at {}", prd.display());
    let body = std::fs::read_to_string(&prd).unwrap();

    // Hard sanitization invariants from level5.html TDD #6.
    assert!(
        !body.contains("root:"),
        "leaked root: key in prd.yml:\n{body}"
    );
    assert!(!body.contains("language:"), "leaked language: key:\n{body}");
    assert!(!body.contains("io_dir:"), "leaked io_dir: key:\n{body}");
    assert!(!body.contains("/home/"), "leaked /home/ path:\n{body}");
    assert!(!body.contains("/Users/"), "leaked /Users/ path:\n{body}");

    // What MUST remain.
    assert!(
        body.contains("module: alpha"),
        "missing module field:\n{body}"
    );
}
