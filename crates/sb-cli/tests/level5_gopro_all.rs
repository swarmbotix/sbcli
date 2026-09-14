//! L5 TDD #8 — `sb gopro --all` covers every module in the workspace.
//!
//! Modules with a broken sb.dev.yml surface as a per-module error
//! without blocking the others.

mod common;
use common::L5Sandbox;

#[test]
fn gopro_all_writes_prd_for_every_module() {
    let mut sb = L5Sandbox::empty();
    let a = sb.adopt("alpha", Some("#!/usr/bin/env bash\nexit 0\n"));
    let b = sb.adopt("beta", Some("#!/usr/bin/env bash\nexit 0\n"));
    let c = sb.adopt("gamma", Some("#!/usr/bin/env bash\nexit 0\n"));

    let out = sb.cmd().args(["gopro", "--all"]).output().unwrap();
    assert!(
        out.status.success(),
        "sb gopro --all failed: {}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    for m in [&a, &b, &c] {
        let prd = m.root.join("sb.prd.yml");
        assert!(
            prd.exists(),
            "missing prd.yml for {}: {}",
            m.name,
            prd.display()
        );
        let body = std::fs::read_to_string(&prd).unwrap();
        assert!(body.contains(&format!("module: {}", m.name)));
        assert!(!body.contains("root:"));
    }

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("3 succeeded"),
        "want '3 succeeded' summary, got: {stdout}"
    );
}

#[test]
fn gopro_all_continues_past_broken_module() {
    let mut sb = L5Sandbox::empty();
    let a = sb.adopt("alpha", Some("#!/usr/bin/env bash\nexit 0\n"));
    let bad = sb.adopt("bad", Some("#!/usr/bin/env bash\nexit 0\n"));
    let c = sb.adopt("gamma", Some("#!/usr/bin/env bash\nexit 0\n"));

    // Corrupt one module's sb.dev.yml — gopro on it should fail but
    // shouldn't stop the others.
    std::fs::write(bad.root.join("sb.dev.yml"), "this: is: not: valid yaml::\n").unwrap();

    let out = sb.cmd().args(["gopro", "--all"]).output().unwrap();
    assert!(
        !out.status.success(),
        "sb gopro --all should fail when any module fails"
    );

    // The two good modules still got their prd.yml.
    for m in [&a, &c] {
        let prd = m.root.join("sb.prd.yml");
        assert!(
            prd.exists(),
            "broken sibling blocked gopro for {}: {}",
            m.name,
            prd.display()
        );
    }

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("bad"),
        "want broken module named in stderr: {stderr}"
    );
}
