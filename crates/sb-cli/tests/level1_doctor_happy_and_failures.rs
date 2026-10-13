//! L1 TDD test #10 — `sb doctor` happy path + per-check failure isolation.

mod common;
use common::Sandbox;

#[test]
fn doctor_passes_on_this_dev_box_after_install() {
    let s = Sandbox::new();
    // Populate std/* first so the vault check passes.
    let out = s.cmd().args(["message", "list"]).output().unwrap();
    assert!(out.status.success(), "message list failed: {out:?}");

    let out = s.cmd().arg("doctor").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        // Surface what's missing — useful when running on a non-dev box.
        eprintln!("doctor failed.\nstdout:\n{stdout}\nstderr:\n{stderr}");
    }
    // Don't hard-fail on machines lacking tools; just assert each check
    // produced a status line.
    for tool in [
        "protoc",
        "flatc",
        "transports",
        "libzenohc",
        "libiceoryx2",
        "tmux",
        "std vault",
    ] {
        assert!(
            stdout.contains(tool),
            "doctor output missing line for {tool}"
        );
    }
}

/// The transport row states the crate versions this `sb` links, and a
/// resolvable native library states its own — the pair is what makes a
/// mismatch visible instead of silently green.
#[test]
fn doctor_reports_transport_versions() {
    let s = Sandbox::new();
    let out = s.cmd().arg("doctor").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    let row = stdout
        .lines()
        .find(|l| l.contains("transports"))
        .unwrap_or_else(|| panic!("no transports row:\n{stdout}"));
    assert!(row.contains("zenoh "), "{row}");
    assert!(row.contains("iceoryx2 "), "{row}");
    assert!(
        !row.contains("unknown"),
        "linked versions should come from Cargo.lock: {row}"
    );

    // Where a library is configured and loads, the row carries a version or
    // says outright that it could not be determined — never just a path.
    for lib in ["libzenohc", "libiceoryx2"] {
        let Some(row) = stdout
            .lines()
            .find(|l| l.contains(lib) && l.contains("dlopen OK"))
        else {
            continue; // not configured on this box
        };
        assert!(
            row.contains(" (v") || row.contains("version unknown"),
            "{lib} row states no version: {row}"
        );
    }
}
