//! L5 TDD #2 — `sb up` fails *before* creating the session if any
//! module lacks a `runscript.bash`. No orphan tmux state.

mod common;
use common::{L5Sandbox, has_session};

#[test]
fn up_blames_module_with_no_runscript_and_makes_no_session() {
    let mut sb = L5Sandbox::empty();
    sb.adopt("alpha", Some("#!/usr/bin/env bash\nsleep 30\n"));
    sb.adopt("bad", None); // no runscript on disk
    sb.adopt("gamma", Some("#!/usr/bin/env bash\nsleep 30\n"));

    let session = format!("sb-{}", sb.workspace);
    assert!(!has_session(&session));

    let out = sb.cmd().arg("up").output().unwrap();
    assert!(
        !out.status.success(),
        "sb up should fail when a module has no runscript"
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("bad"),
        "want the bad module named in the error: {stderr}"
    );
    assert!(
        stderr.contains("runscript"),
        "want 'runscript' in the error: {stderr}"
    );

    assert!(
        !has_session(&session),
        "no tmux session must be created when up bails — got orphan {session}"
    );
}
