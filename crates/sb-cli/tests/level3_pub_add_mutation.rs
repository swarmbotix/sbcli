//! L3 TDD #1 (partial) + #7 — `sb io add pub` mutates `sb.module.yaml`
//! and drops the language file under `<io_dir>/publishers/`.
//!
//! Golden file matching is split into [level3_codegen_rust.rs](./level3_codegen_rust.rs)
//! per language. This file proves the mutation+emit pipeline end-to-end.

mod common;
use common::L3Sandbox;

#[test]
fn pub_add_appends_to_dev_yml_and_emits_file() {
    let s = L3Sandbox::rust("pubber");

    let out = s
        .cmd_in_module()
        .args([
            "io",
            "add",
            "pub",
            "-m",
            "ros2/std/StringStamped",
            "hello",
            "--zenoh",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "io add pub failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    // 1. The module manifest has the publisher, written in the map form
    //    (keyed by name, no `name:` line).
    let dev_body = std::fs::read_to_string(s.dev_yml()).unwrap();
    assert!(
        dev_body.contains("publishers:\n  hello:\n"),
        "dev_yml: {dev_body}"
    );
    assert!(!dev_body.contains("name: hello"), "dev_yml: {dev_body}");
    assert!(
        dev_body.contains("type: ros2/std/StringStamped"),
        "dev_yml: {dev_body}"
    );
    assert!(dev_body.contains("transport: zenoh"), "dev_yml: {dev_body}");

    // 2. Generated file exists.
    let pub_file = s.io_dir().join("publishers").join("hello.rs");
    assert!(
        pub_file.is_file(),
        "missing generated file: {}",
        pub_file.display()
    );

    // 3. File references the topic and message type.
    let body = std::fs::read_to_string(&pub_file).unwrap();
    assert!(body.contains("zenoh"), "body: {body}");
    assert!(body.contains("HelloPublisher"), "body: {body}");
    assert!(body.contains("StringStamped"), "body: {body}");
    // Zenoh key = the canonical fully-qualified topic (no leading `/`),
    // including the transport segment.
    assert!(
        body.contains("dev01/demo/pubber/zenoh/hello"),
        "expected qualified topic in body:\n{body}"
    );
}

/// 0.5.0: a `/` in the positional is a wire key, not a bare topic. It is
/// rejected (pointing at `--key`) and nothing is written.
#[test]
fn pub_add_rejects_slashed_topic_positional() {
    let s = L3Sandbox::rust("pubber");
    let dev_before = std::fs::read_to_string(s.dev_yml()).unwrap();

    let out = s
        .cmd_in_module()
        .args([
            "io",
            "add",
            "pub",
            "-m",
            "ros2/std/StringStamped",
            "/dev01/demo/pubber/zenoh/hello",
            "--zenoh",
        ])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(1),
        "slashed positional must exit 1: stdout={} stderr={stderr}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        stderr.contains("may not contain '/'"),
        "error should explain the rule: {stderr}"
    );
    assert!(
        stderr.contains("--key"),
        "error should point at --key: {stderr}"
    );

    let dev_after = std::fs::read_to_string(s.dev_yml()).unwrap();
    assert_eq!(
        dev_before, dev_after,
        "rejected add must not touch the module manifest"
    );
    assert!(
        !s.io_dir().join("publishers").join("hello.rs").exists(),
        "rejected add must not emit a file"
    );
}

/// A 0.4.x manifest spelled foreign topics as `topic: /<wire key>`.
/// 0.5.0 reads that as `key:` with the identical wire key, `sb doctor`
/// flags it, and the next write rewrites it to `key:`. The regenerated
/// file must match what a fresh `--key` add produces, byte for byte.
#[test]
fn legacy_slashed_topic_keeps_wire_key_and_is_rewritten_on_edit() {
    const WIRE: &str = "dev01/demo/pubber/zenoh/hello";

    let s = L3Sandbox::rust("subber");
    write_legacy_subscriber_yml(&s, "/dev01/demo/pubber/zenoh/hello");

    // 1. Doctor names the module still carrying the legacy spelling.
    let doctor = run_doctor(&s);
    assert!(
        doctor
            .lines()
            .any(|l| l.starts_with("[WARN] topics") && l.contains("subber")),
        "doctor should warn about the legacy entry in subber:\n{doctor}"
    );

    // 2. Regenerate the subscriber by name (`sb io edit` with no flags);
    //    the write rewrites the legacy spelling.
    let out = s
        .cmd_in_module()
        .args(["io", "edit", "hello"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "io edit failed: stdout={stdout} stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("rewrote 1 legacy topic entry"),
        "io edit should report the rewrite: {stdout}"
    );

    // 3. The wire key is unchanged and used verbatim.
    let sub_file = s.io_dir().join("subscribers").join("hello.rs");
    let legacy_body = std::fs::read_to_string(&sub_file)
        .unwrap_or_else(|e| panic!("missing {}: {e}", sub_file.display()));
    assert!(
        legacy_body.contains(&format!(r#"pub const TOPIC: &str = "{WIRE}";"#)),
        "generated file should pin the original wire key:\n{legacy_body}"
    );
    assert!(
        !legacy_body.contains("DEFAULT_MODULE"),
        "a key entry must not assemble segments:\n{legacy_body}"
    );

    // 4. The write rewrote the entry to `key:`.
    let dev = std::fs::read_to_string(s.dev_yml()).unwrap();
    assert!(dev.contains(&format!("key: {WIRE}")), "dev_yml: {dev}");
    assert!(!dev.contains("topic: /"), "legacy spelling survived: {dev}");

    // 5. Doctor is clean once the entry is rewritten.
    let doctor = run_doctor(&s);
    assert!(
        doctor.lines().any(|l| l.starts_with("[OK]   topics")),
        "doctor should report topics OK after the rewrite:\n{doctor}"
    );

    // 6. A fresh `--key` add emits the identical file.
    let fresh = L3Sandbox::rust("subber");
    let out = fresh
        .cmd_in_module()
        .args([
            "io",
            "add",
            "sub",
            "-m",
            "ros2/std/StringStamped",
            "--key",
            WIRE,
            "--zenoh",
            "--force",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "fresh io add sub --key failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let fresh_body =
        std::fs::read_to_string(fresh.io_dir().join("subscribers").join("hello.rs")).unwrap();
    assert_eq!(
        legacy_body, fresh_body,
        "legacy-upgraded and fresh --key subscribers must be byte-identical"
    );
}

/// Overwrite the manifest with the shape a 0.4.x user file had: one Zenoh
/// subscriber whose foreign topic is spelled with a leading `/`. `sb init`
/// writes no `root:` line (the yaml's directory is the module root), so the
/// legacy body carries none either.
fn write_legacy_subscriber_yml(s: &L3Sandbox, legacy_topic: &str) {
    let init_body = std::fs::read_to_string(s.dev_yml()).unwrap();
    assert!(
        !init_body.lines().any(|l| l.starts_with("root:")),
        "sb init wrote a root: line:\n{init_body}"
    );
    let body = format!(
        "module: {module}\n\
         language: rust\n\
         io_dir: swarmbotix_io\n\
         subscribers:\n\
         - name: hello\n  \
           topic: {legacy_topic}\n  \
           type: ros2/std/StringStamped\n  \
           transport: zenoh\n",
        module = s.module,
    );
    std::fs::write(s.dev_yml(), body).unwrap();
}

/// `sb doctor` stdout from the module root. The exit code is ignored: the
/// tool checks depend on the host, and only the `topics` row matters here.
fn run_doctor(s: &L3Sandbox) -> String {
    let out = s.cmd_in_module().arg("doctor").output().unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

// ── sb io ─────────────────────────────────────────────────────────────

/// Run `sb <args>` in the module dir; returns (success, stdout, stderr).
fn sb_io(s: &L3Sandbox, args: &[&str]) -> (bool, String, String) {
    let out = s.cmd_in_module().args(args).output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn io_add_pub_raw_mime_writes_raw_entry_and_untyped_file() {
    let s = L3Sandbox::rust("pubber");
    let (ok, stdout, stderr) = sb_io(
        &s,
        &[
            "io",
            "add",
            "pub",
            "status",
            "--zenoh",
            "--raw",
            "text/json",
        ],
    );
    assert!(ok, "io add pub --raw failed: {stdout}{stderr}");

    let dev = std::fs::read_to_string(s.dev_yml()).unwrap();
    assert!(dev.contains("encoding: raw"), "{dev}");
    assert!(dev.contains("mime: text/json"), "{dev}");
    assert!(!dev.contains("type:"), "a raw entry has no type:\n{dev}");

    let body = std::fs::read_to_string(s.io_dir().join("publishers").join("status.rs")).unwrap();
    assert!(
        body.contains("pub const WIRE_TAG: &str = \"text/json\";"),
        "{body}"
    );
    assert!(
        !body.contains("prost::Message"),
        "a raw publisher has no typed publish:\n{body}"
    );

    let (ok, list, _) = sb_io(&s, &["io", "list"]);
    assert!(ok);
    let row = list.lines().find(|l| l.contains("status")).unwrap();
    assert!(row.contains("raw") && row.contains("zenoh"), "{list}");
}

#[test]
fn io_add_service_then_rm() {
    let s = L3Sandbox::rust("pubber");
    let (ok, stdout, stderr) = sb_io(&s, &["io", "add", "service", "nav"]);
    assert!(ok, "io add service failed: {stdout}{stderr}");
    let file = s.io_dir().join("services").join("nav.rs");
    assert!(file.is_file(), "missing {}", file.display());
    let dev = std::fs::read_to_string(s.dev_yml()).unwrap();
    let parsed: sb_core::ModuleDevConfig = serde_yaml::from_str(&dev).unwrap();
    assert!(parsed.services.iter().any(|svc| svc.name == "nav"), "{dev}");
    assert!(
        !dev.contains("name: nav"),
        "services are written as a list of names: {dev}"
    );

    let (ok, _, stderr) = sb_io(&s, &["io", "edit", "nav", "--raw"]);
    assert!(!ok);
    assert!(
        stderr.contains("services have no editable fields; edit the file"),
        "{stderr}"
    );

    let (ok, stdout, stderr) = sb_io(&s, &["io", "rm", "nav"]);
    assert!(ok, "io rm failed: {stdout}{stderr}");
    assert!(stdout.contains("removed service"), "{stdout}");
    assert!(!file.exists());
    let dev = std::fs::read_to_string(s.dev_yml()).unwrap();
    assert!(!dev.contains("services:"), "{dev}");
}

#[test]
fn io_update_second_run_leaves_everything_unchanged() {
    let s = L3Sandbox::rust("pubber");
    for args in [
        [
            "io",
            "add",
            "pub",
            "hello",
            "-m",
            "ros2/std/StringStamped",
            "--zenoh",
        ],
        [
            "io",
            "add",
            "sub",
            "cmd",
            "-m",
            "ros2/std/StringStamped",
            "--zenoh",
        ],
    ] {
        let (ok, stdout, stderr) = sb_io(&s, &args);
        assert!(ok, "{args:?}: {stdout}{stderr}");
    }
    // A deleted file is rewritten; everything else is left alone.
    std::fs::remove_file(s.io_dir().join("subscribers").join("cmd.rs")).unwrap();
    let (ok, stdout, stderr) = sb_io(&s, &["io", "update"]);
    assert!(ok, "io update failed: {stdout}{stderr}");
    assert_eq!(stdout, "pubber: 1 written, 1 unchanged, 0 skipped\n");

    let (ok, stdout, stderr) = sb_io(&s, &["io", "update"]);
    assert!(ok, "second io update failed: {stdout}{stderr}");
    assert_eq!(stdout, "pubber: 0 written, 2 unchanged, 0 skipped\n");

    // --all walks the active workspace from anywhere.
    let out = s
        .cmd_in_tempdir()
        .args(["io", "update", "--all"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "pubber: 0 written, 2 unchanged, 0 skipped\n"
    );
}

#[test]
fn io_edit_changes_a_subscriber_key() {
    let s = L3Sandbox::rust("pubber");
    let (ok, stdout, stderr) = sb_io(
        &s,
        &[
            "io",
            "add",
            "sub",
            "hello",
            "-m",
            "ros2/std/StringStamped",
            "--zenoh",
        ],
    );
    assert!(ok, "{stdout}{stderr}");
    let (ok, stdout, stderr) = sb_io(
        &s,
        &[
            "io",
            "edit",
            "hello",
            "--key",
            "dev01/demo/other/zenoh/hello",
        ],
    );
    assert!(ok, "io edit failed: {stdout}{stderr}");
    let dev = std::fs::read_to_string(s.dev_yml()).unwrap();
    assert!(dev.contains("key: dev01/demo/other/zenoh/hello"), "{dev}");
    assert!(!dev.contains("topic: hello"), "{dev}");
    let body = std::fs::read_to_string(s.io_dir().join("subscribers").join("hello.rs")).unwrap();
    assert!(body.contains("dev01/demo/other/zenoh/hello"), "{body}");
}
