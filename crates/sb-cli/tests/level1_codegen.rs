//! L1 integration test for `sb message compile --iox2` — the .proto →
//! iceoryx2-flat data definition pipeline across Rust / C++ / Python.

mod common;
use common::{Sandbox, protoc_path};

#[test]
fn iox2_emits_rust_cpp_python_for_forge_std_header() {
    let s = Sandbox::new();
    let Some(protoc) = protoc_path(&s) else {
        eprintln!("protoc not configured — skipping");
        return;
    };
    if !protoc.exists() {
        return;
    }
    // Populate std/* in the sandbox vault.
    let out = s.cmd().args(["message", "list"]).output().unwrap();
    assert!(out.status.success(), "vault populate failed: {out:?}");

    let out_dir = s.home().join("gen");
    let cmd_out = s
        .cmd()
        .args([
            "message",
            "compile",
            "--iox2",
            "--out",
            out_dir.to_str().unwrap(),
            "ros2/std/Header",
        ])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&cmd_out.stdout);
    let stderr = String::from_utf8_lossy(&cmd_out.stderr);
    assert!(
        cmd_out.status.success(),
        "compile --iox2 failed: stdout={stdout} stderr={stderr}"
    );

    let rs = out_dir.join("iox2/std/Header/Header.rs");
    let h = out_dir.join("iox2/std/Header/Header.h");
    let py = out_dir.join("iox2/std/Header/Header.py");
    assert!(rs.exists(), "Rust output missing");
    assert!(h.exists(), "C++ output missing");
    assert!(py.exists(), "Python output missing");

    let rs_src = std::fs::read_to_string(&rs).unwrap();
    let h_src = std::fs::read_to_string(&h).unwrap();
    let py_src = std::fs::read_to_string(&py).unwrap();

    // Header's load-bearing `metadata.msg_freq_desired` survives flattening.
    assert!(
        rs_src.contains("metadata_msg_freq_desired"),
        "Rust missing field"
    );
    assert!(rs_src.contains("f64"), "Rust scalar mapping wrong");
    assert!(
        h_src.contains("metadata_msg_freq_desired"),
        "C++ missing field"
    );
    assert!(h_src.contains("double"), "C++ scalar mapping wrong");
    assert!(
        py_src.contains("metadata_msg_freq_desired"),
        "Python missing field"
    );
    assert!(
        py_src.contains("ctypes.c_double"),
        "Python scalar mapping wrong"
    );

    // Every variable-length field has fixed-size storage.
    assert!(
        rs_src.contains("[u8; 256]"),
        "Rust string cap mapping wrong"
    );
    assert!(
        h_src.contains("char frame_id[256]"),
        "C++ string cap mapping wrong"
    );
    assert!(
        py_src.contains("ctypes.c_char * 256"),
        "Python string cap mapping wrong"
    );

    // Rust: repr(C) + ZeroCopySend derive + the `use` that brings the
    // trait into scope (the derive macro expansion uses the bare ident).
    assert!(rs_src.contains("#[repr(C)]"));
    assert!(rs_src.contains("use iceoryx2::prelude::ZeroCopySend;"));
    assert!(rs_src.contains("ZeroCopySend)]"));

    // C++ namespace is sanitized — must NOT be `namespace std {` (collision).
    assert!(
        h_src.contains("namespace swarmbotix_std"),
        "C++ namespace should be 'swarmbotix_std' (sanitized proto package), got:\n{h_src}"
    );
}

#[test]
fn iox2_respects_custom_caps() {
    let s = Sandbox::new();
    let Some(protoc) = protoc_path(&s) else {
        return;
    };
    if !protoc.exists() {
        return;
    }
    s.cmd().args(["message", "list"]).output().unwrap();

    let out_dir = s.home().join("gen2");
    let cmd_out = s
        .cmd()
        .args([
            "message",
            "compile",
            "--iox2",
            "--out",
            out_dir.to_str().unwrap(),
            "--string-cap",
            "64",
            "ros2/std/Header",
        ])
        .output()
        .unwrap();
    assert!(cmd_out.status.success(), "{cmd_out:?}");
    let src = std::fs::read_to_string(out_dir.join("iox2/std/Header/Header.rs")).unwrap();
    assert!(
        src.contains("[u8; 64]"),
        "expected [u8; 64] from --string-cap, got:\n{src}"
    );
}

#[test]
fn iox2_reports_missing_message() {
    let s = Sandbox::new();
    if protoc_path(&s).is_none_or(|p| !p.exists()) {
        return;
    }
    s.cmd().args(["message", "list"]).output().unwrap();

    let out = s
        .cmd()
        .args([
            "message",
            "compile",
            "--iox2",
            "--out",
            s.home().join("gen3").to_str().unwrap(),
            "ros2/std/DefinitelyNotThere",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("DefinitelyNotThere"),
        "stderr should name the missing message: {stderr}"
    );
}

#[test]
fn no_format_flag_runs_every_available_format() {
    // `sb message compile --out X NAME` with no format flag should run
    // iox2 (always) + proto (if protoc set) + fb (if flatc set).
    // On this dev box all three tools are configured, so all three emit.
    let s = Sandbox::new();
    if protoc_path(&s).is_none_or(|p| !p.exists()) {
        return;
    }
    s.cmd().args(["message", "list"]).output().unwrap();
    let out_dir = s.home().join("gen_all");
    let cmd_out = s
        .cmd()
        .args([
            "message",
            "compile",
            "--out",
            out_dir.to_str().unwrap(),
            "ros2/std/Header",
        ])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&cmd_out.stdout);
    assert!(cmd_out.status.success(), "{cmd_out:?}");

    // iox2 outputs.
    assert!(
        out_dir.join("iox2/std/Header/Header.rs").exists(),
        "iox2 Rust missing"
    );
    assert!(
        out_dir.join("iox2/std/Header/Header.h").exists(),
        "iox2 C++ missing"
    );
    assert!(
        out_dir.join("iox2/std/Header/Header.py").exists(),
        "iox2 Python missing"
    );
    assert!(stdout.contains("iox2:"));

    // proto outputs (if protoc was configured — it should be on this box).
    if stdout.contains("proto:") {
        assert!(
            out_dir.join("proto/std/Header/Header.pb.h").exists(),
            "protoc C++ header missing"
        );
        assert!(
            out_dir.join("proto/std/Header/Header.pb.cc").exists(),
            "protoc C++ source missing"
        );
        assert!(
            out_dir.join("proto/std/Header/Header_pb2.py").exists(),
            "protoc Python missing"
        );
    }

    // fb outputs (if flatc was configured). Layout is
    // `fb/<pkg>/<Leaf>/`, matching iox2/ and proto/ — see the
    // `<root>/{iox2,proto,fb}/<pkg>/<Leaf>/` contract in main.rs.
    if stdout.contains("fb:") {
        assert!(
            out_dir.join("fb/std/Header/Header_generated.rs").exists(),
            "flatc Rust missing"
        );
        assert!(
            out_dir.join("fb/std/Header/Header_generated.h").exists(),
            "flatc C++ missing"
        );
    }
}

#[test]
fn no_out_flag_defaults_to_message_targets_root() {
    // `sb message compile --iox2 std/Header` without --out lands the
    // generated files under the active style's `message_targets` root,
    // which is a peer of that style's `message_definitions` — not a subtree
    // of the defs vault. Source `.proto`s and generated bindings therefore
    // never share a directory.
    let s = Sandbox::new();
    if protoc_path(&s).is_none_or(|p| !p.exists()) {
        return;
    }
    s.cmd().args(["message", "list"]).output().unwrap();

    let cmd_out = s
        .cmd()
        .args(["message", "compile", "--iox2", "ros2/std/Header"])
        .output()
        .unwrap();
    assert!(cmd_out.status.success(), "{cmd_out:?}");

    let vault = s.vault_dir();
    let targets = s.targets_dir();
    // Source .proto still there.
    assert!(vault.join("std").join("Header.proto").exists());
    // Generated iox2 files land under `<message_targets>/iox2/std/Header/`.
    let iox2 = targets.join("iox2").join("std").join("Header");
    assert!(
        iox2.join("Header.rs").exists(),
        "Header.rs should be at message_targets/iox2/std/Header/"
    );
    assert!(iox2.join("Header.h").exists());
    assert!(iox2.join("Header.py").exists());
}

#[test]
fn explicit_fb_errors_when_flatc_missing() {
    // protoc is required for the FileDescriptorSet IR step (so we keep
    // it set), but flatc is intentionally absent. `--fb` must fail with
    // an actionable error mentioning flatc.
    let s = Sandbox::new();
    let Some(protoc) = protoc_path(&s) else {
        return;
    };
    if !protoc.exists() {
        return;
    }
    // flatc is pointed at a path that does not exist rather than left unset.
    // Omitting the key does NOT make flatc unavailable: `$SB_CONFIG` is the
    // highest layer, not an exclusive one, so an unset key falls through to
    // the global `~/.swarmbotix/sb.config.yml` — which on any real install
    // has flatc filled in. `flatc: null` does not help either; null is
    // defined to fall through to the next layer. A bogus path is the only
    // way the env layer can actually deny the tool.
    //
    // write_config (not a bare overwrite) so sb_home_dir and the vault paths
    // stay pinned at the sandbox.
    s.write_config(&format!(
        "protoc: {}\nflatc: {}\n",
        protoc.display(),
        s.home().join("no_such_flatc").display(),
    ));
    s.cmd().args(["message", "list"]).output().unwrap();

    let cmd_out = s
        .cmd()
        .args([
            "message",
            "compile",
            "--out",
            s.home().join("gen_x").to_str().unwrap(),
            "--fb",
            "ros2/std/Header",
        ])
        .output()
        .unwrap();
    assert!(
        !cmd_out.status.success(),
        "--fb should fail when flatc isn't configured"
    );
    let stderr = String::from_utf8_lossy(&cmd_out.stderr);
    assert!(
        stderr.contains("flatc"),
        "stderr should name flatc: {stderr}"
    );
}
