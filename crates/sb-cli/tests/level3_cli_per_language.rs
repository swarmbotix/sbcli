//! L3 — per-language CLI smoke tests.
//!
//! For every language the codegen supports, drive `sb pub add` / `sb sub add`
//! through the CLI and assert:
//!   1. The mutation succeeds.
//!   2. A generated file lands at the right extension under `<io_dir>/`.
//!   3. The file body references something language-specific (a sanity
//!      check beyond the snapshot test — we want the CLI to actually
//!      write the same bytes the in-process render does).
//!
//! Plus the structural-impossibility tests: Flutter and Unity iceoryx2
//! must error with an "impossible" message (not "not yet implemented").

mod common;
use common::L3Sandbox;

// ─────────────────────────────────────────────────────────────────────
// Python — both transports
// ─────────────────────────────────────────────────────────────────────

#[test]
fn python_pub_add_zenoh_writes_py_file() {
    let s = L3Sandbox::python("pubber");
    let out = s
        .cmd_in_module()
        .args([
            "pub",
            "add",
            "-m",
            "ros2/std/StringStamped",
            "hello",
            "--zenoh",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "python --zenoh failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let f = s.io_dir().join("publishers").join("hello.py");
    assert!(f.is_file(), "missing {}", f.display());
    let body = std::fs::read_to_string(&f).unwrap();
    assert!(body.contains("import zenoh"), "body: {body}");
    assert!(body.contains("class HelloPublisher"), "body: {body}");
}

#[test]
fn python_pub_add_iceoryx_writes_py_file() {
    let s = L3Sandbox::python("pubber");
    let out = s
        .cmd_in_module()
        .args([
            "pub",
            "add",
            "-m",
            "ros2/std/ImageStamped",
            "image_raw",
            "--iox2",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "python --iox2 failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let f = s.io_dir().join("publishers").join("image_raw.py");
    assert!(f.is_file(), "missing {}", f.display());
    let body = std::fs::read_to_string(&f).unwrap();
    assert!(body.contains("from iceoryx2"), "body: {body}");
    assert!(body.contains("class ImageRawPublisher"), "body: {body}");
}

// ─────────────────────────────────────────────────────────────────────
// C++
// ─────────────────────────────────────────────────────────────────────

#[test]
fn cpp_pub_add_zenoh_writes_cpp_file() {
    let s = L3Sandbox::cpp("pubber");
    let out = s
        .cmd_in_module()
        .args([
            "pub",
            "add",
            "-m",
            "ros2/std/StringStamped",
            "hello",
            "--zenoh",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "cpp --zenoh failed: {out:?}");
    let f = s.io_dir().join("publishers").join("hello.cpp");
    assert!(f.is_file(), "missing {}", f.display());
    let body = std::fs::read_to_string(&f).unwrap();
    assert!(body.contains("#include <zenoh.hxx>"), "body: {body}");
    assert!(body.contains("class HelloPublisher"), "body: {body}");
}

#[test]
fn cpp_pub_add_iceoryx_writes_cpp_file() {
    let s = L3Sandbox::cpp("pubber");
    let out = s
        .cmd_in_module()
        .args([
            "pub",
            "add",
            "-m",
            "ros2/std/ImageStamped",
            "image_raw",
            "--iox2",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "cpp --iox2 failed: {out:?}");
    let f = s.io_dir().join("publishers").join("image_raw.cpp");
    assert!(f.is_file());
    let body = std::fs::read_to_string(&f).unwrap();
    assert!(body.contains("iox2/iceoryx2.hpp"), "body: {body}");
    // No template — payload type is hardcoded into the generated class
    // (see sb-codegen/templates/cpp/publisher_iceoryx.cpp.j2).
    assert!(
        body.contains("class ImageRawPublisher"),
        "body should declare a non-template ImageRawPublisher class: {body}"
    );
    assert!(
        body.contains("ImageRaw_Payload"),
        "body should alias the proto type as ImageRaw_Payload: {body}"
    );
}

// ─────────────────────────────────────────────────────────────────────
// Flutter
// ─────────────────────────────────────────────────────────────────────

#[test]
fn flutter_pub_add_zenoh_writes_dart_file() {
    let s = L3Sandbox::flutter("pubber");
    let out = s
        .cmd_in_module()
        .args([
            "pub",
            "add",
            "-m",
            "ros2/std/StringStamped",
            "hello",
            "--zenoh",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "flutter --zenoh failed: {out:?}");
    let f = s.io_dir().join("publishers").join("hello.dart");
    assert!(f.is_file(), "missing {}", f.display());
    let body = std::fs::read_to_string(&f).unwrap();
    assert!(body.contains("package:zenoh_flutter"), "body: {body}");
}

#[test]
fn flutter_pub_add_iceoryx_errors_as_impossible() {
    let s = L3Sandbox::flutter("pubber");
    let out = s
        .cmd_in_module()
        .args([
            "pub",
            "add",
            "-m",
            "ros2/std/ImageStamped",
            "image_raw",
            "--iox2",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success(), "flutter --iox2 must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    // Structural-impossibility wording — not "not yet implemented".
    assert!(
        stderr.contains("no iceoryx2 path")
            || stderr.contains("shared memory isn't available")
            || stderr.contains("shared memory"),
        "stderr should explain structural impossibility, got: {stderr}"
    );
    assert!(
        !stderr.contains("not yet implemented"),
        "stderr should NOT claim 'not yet implemented' for Flutter: {stderr}"
    );
}

// ─────────────────────────────────────────────────────────────────────
// Unity
// ─────────────────────────────────────────────────────────────────────

#[test]
fn unity_pub_add_zenoh_writes_cs_file_under_assets() {
    let s = L3Sandbox::unity("pubber");
    let out = s
        .cmd_in_module()
        .args([
            "pub",
            "add",
            "-m",
            "ros2/std/StringStamped",
            "hello",
            "--zenoh",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "unity --zenoh failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // Unity io_dir = Assets/Scripts/swarmbotix_io.
    let f = s
        .module_root
        .join("Assets/Scripts/swarmbotix_io/publishers/hello.cs");
    assert!(f.is_file(), "missing {}", f.display());
    let body = std::fs::read_to_string(&f).unwrap();
    assert!(body.contains("[DllImport"), "body: {body}");
    assert!(body.contains("namespace Swarmbotix.Io"), "body: {body}");
}

#[test]
fn unity_pub_add_iceoryx_errors_as_impossible() {
    let s = L3Sandbox::unity("pubber");
    let out = s
        .cmd_in_module()
        .args([
            "pub",
            "add",
            "-m",
            "ros2/std/ImageStamped",
            "image_raw",
            "--iox2",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success(), "unity --iox2 must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no iceoryx2 path") || stderr.contains("shared memory"),
        "stderr should explain structural impossibility, got: {stderr}"
    );
}
