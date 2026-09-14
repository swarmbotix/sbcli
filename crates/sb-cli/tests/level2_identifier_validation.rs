//! L2 TDD test #11 — identifier validation for workspace + module names.

mod common;
use common::WsSandbox;

const REJECT_VERBATIM: &[&str] = &["demo-app", "123demo", "demo app", "a/b", "a-b-c"];
// Basename of `"a/b"` is `"b"` — valid — so init only sees a subset.
const REJECT_AS_BASENAME: &[&str] = &["demo-app", "123demo", "demo app", "a-b-c"];
const ACCEPT: &[&str] = &["demo", "demo_app", "Cam01", "_internal"];

#[test]
fn ws_create_rejects_invalid_names() {
    for &bad in REJECT_VERBATIM {
        let s = WsSandbox::new();
        let out = s.cmd().args(["ws", "create", bad]).output().unwrap();
        assert!(
            !out.status.success(),
            "ws create {bad:?} should fail but succeeded: {out:?}"
        );
    }
}

#[test]
fn ws_create_accepts_valid_names() {
    for &good in ACCEPT {
        let s = WsSandbox::new();
        let out = s.cmd().args(["ws", "create", good]).output().unwrap();
        assert!(
            out.status.success(),
            "ws create {good:?} should succeed: {out:?}"
        );
    }
}

#[test]
fn init_rejects_invalid_module_basenames() {
    for &bad in REJECT_AS_BASENAME {
        let s = WsSandbox::new();
        s.cmd().args(["ws", "create", "demo"]).output().unwrap();
        s.cmd().args(["ws", "set", "demo"]).output().unwrap();
        let project = s.tempdir().join(bad);
        std::fs::create_dir_all(&project).unwrap();
        let out = s
            .cmd()
            .args(["init", "--rust"])
            .arg(&project)
            .output()
            .unwrap();
        assert!(
            !out.status.success(),
            "init on rootpath with basename {bad:?} should fail: {out:?}"
        );
    }
}
