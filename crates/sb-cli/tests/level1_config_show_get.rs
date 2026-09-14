//! L1 — `sb config show` / `sb config get` read-only inspection surface.
//!
//! These verbs let user-side LLMs and scripts ask for the merged config
//! that every other `sb` verb sees, without having to replicate the
//! layering rules. See sbcli_config.md §4.3 / §4.4.

use std::process::Command;

use tempfile::TempDir;

/// Spawn `sb` with `$SB_CONFIG` pinned to a fresh test config AND
/// `$HOME` redirected into the tempdir, so the global layer
/// (`~/.swarmbotix/sb.config.yml` on the real host) doesn't bleed in.
fn hermetic_sb(d: &TempDir, cfg: &std::path::Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_sb"));
    c.env("SB_CONFIG", cfg);
    // `$SB_CONFIG` is the highest layer, not an exclusive one — the global
    // `<sb_home>/sb.config.yml` still merges underneath it. Redirecting
    // SB_HOME into the tempdir points that layer at a file that does not
    // exist, which is what actually makes these tests hermetic. `HOME` alone
    // cannot do it: `dirs::home_dir()` consults `$HOME` only on Unix.
    c.env("SB_HOME", d.path().join(".swarmbotix"));
    c.env("HOME", d.path());
    c
}

/// Write a self-contained `sb.config.yml` and return its path + the
/// tempdir holding it (kept alive for the test).
fn isolated_config(body: &str) -> (TempDir, std::path::PathBuf) {
    let d = TempDir::new().unwrap();
    let p = d.path().join("sb.config.yml");
    std::fs::write(&p, body).unwrap();
    (d, p)
}

/// Like [`isolated_config`], but also pins `sb_home_dir` at a `.swarmbotix`
/// directory inside the tempdir.
///
/// Use this for any test that asserts on `_config_file` / `_documents`.
/// Those resolve through `sb_home_dir`, and `$HOME` alone does **not**
/// isolate them on Windows: `dirs::home_dir()` consults `$HOME` only on
/// Unix and otherwise calls `SHGetKnownFolderPath(FOLDERID_Profile)`,
/// which ignores the environment entirely. Tests that assert `sb_home_dir`
/// is *unset* must keep using [`isolated_config`].
fn isolated_config_pinned_home(body: &str) -> (TempDir, std::path::PathBuf) {
    let d = TempDir::new().unwrap();
    let sb_home = d.path().join(".swarmbotix");
    std::fs::create_dir_all(&sb_home).unwrap();
    let p = d.path().join("sb.config.yml");
    let mut full = body.to_owned();
    if !full.is_empty() && !full.ends_with('\n') {
        full.push('\n');
    }
    full.push_str(&format!("sb_home_dir: {}\n", sb_home.display()));
    std::fs::write(&p, full).unwrap();
    (d, p)
}

/// Compare paths without caring which separator the host uses.
fn slashed(s: &str) -> String {
    s.replace('\\', "/")
}

#[test]
fn show_yaml_contains_resolved_fields() {
    let (d, cfg) = isolated_config(
        "device: dev42\ntransport_on_device: zenoh\nmessage_targets: /opt/targets\n",
    );
    let out = hermetic_sb(&d, &cfg)
        .args(["config", "show"])
        .output()
        .unwrap();
    assert!(out.status.success(), "show failed: {out:?}");
    let s = String::from_utf8(out.stdout).unwrap();
    assert!(s.contains("device: dev42"), "device missing:\n{s}");
    assert!(
        s.contains("transport_on_device: zenoh"),
        "transport missing:\n{s}"
    );
    assert!(
        s.contains("message_targets: /opt/targets"),
        "targets missing:\n{s}"
    );
    // Built-in defaults bleed through (transport_cross_device).
    assert!(
        s.contains("transport_cross_device: zenoh"),
        "built-in cross-device missing:\n{s}"
    );
}

#[test]
fn show_json_is_parseable_and_uses_field_names() {
    let (d, cfg) = isolated_config_pinned_home("device: bot01\n");
    let out = hermetic_sb(&d, &cfg)
        .args(["config", "show", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "show --json failed: {out:?}");
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("show --json must be parseable JSON");
    assert_eq!(v["device"], "bot01");
    assert_eq!(v["transport_on_device"], "iceoryx2"); // built-in
    // The global config file path is surfaced so downstream tools can
    // discover where `sb config open` / `set` write.
    let cf = v["_config_file"]
        .as_str()
        .expect("_config_file must be a string");
    assert!(
        slashed(cf).ends_with(".swarmbotix/sb.config.yml"),
        "unexpected _config_file: {cf}"
    );
    // Reference docs path (<sb_home>/documents) is surfaced too.
    let dx = v["_documents"]
        .as_str()
        .expect("_documents must be a string");
    assert!(
        slashed(dx).ends_with("/documents"),
        "unexpected _documents: {dx}"
    );
}

#[test]
fn show_yaml_includes_config_file_header() {
    let (d, cfg) = isolated_config_pinned_home("device: dev42\n");
    let out = hermetic_sb(&d, &cfg)
        .args(["config", "show"])
        .output()
        .unwrap();
    assert!(out.status.success(), "show failed: {out:?}");
    let s = String::from_utf8(out.stdout).unwrap();
    assert!(
        s.lines()
            .next()
            .unwrap_or("")
            .starts_with("# config file: "),
        "expected leading `# config file: <path>` header, got:\n{s}"
    );
    let sl = slashed(&s);
    assert!(
        sl.contains(".swarmbotix/sb.config.yml"),
        "header should point at global sb.config.yml:\n{s}"
    );
    assert!(
        sl.lines()
            .any(|l| l.starts_with("# documents:") && l.contains("/documents")),
        "expected `# documents: <path>` header pointing at <sb_home>/documents:\n{s}"
    );
}

#[test]
fn show_sources_attributes_each_field_to_its_layer() {
    let (d, cfg) = isolated_config("device: from-env\nmessages_root: /env/messages\n");
    let out = hermetic_sb(&d, &cfg)
        .args(["config", "show", "--sources", "--json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "show --sources --json failed: {out:?}"
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["device"]["value"], "from-env");
    assert_eq!(v["device"]["source"], "env");
    assert_eq!(v["messages_root"]["source"], "env");
    // No global file in this hermetic env → transport defaults from builtins.
    assert_eq!(v["transport_on_device"]["source"], "builtin");
    // sb_home_dir has no built-in → unset.
    assert_eq!(v["sb_home_dir"]["source"], "unset");
    assert!(v["sb_home_dir"]["value"].is_null());
}

#[test]
fn get_scalar_field_prints_bare_value() {
    let (d, cfg) = isolated_config("device: hello-bot\n");
    let out = hermetic_sb(&d, &cfg)
        .args(["config", "get", "device"])
        .output()
        .unwrap();
    assert!(out.status.success(), "get failed: {out:?}");
    let s = String::from_utf8(out.stdout).unwrap();
    assert_eq!(s.trim(), "hello-bot");
}

#[test]
fn get_json_includes_key_value_source() {
    let (d, cfg) = isolated_config("device: hello-bot\n");
    let out = hermetic_sb(&d, &cfg)
        .args(["config", "get", "device", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "get --json failed: {out:?}");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["key"], "device");
    assert_eq!(v["value"], "hello-bot");
    assert_eq!(v["source"], "env");
}

#[test]
fn get_unset_field_exits_nonzero_with_actionable_message() {
    // sb_home_dir has no built-in default, and we don't set it here.
    let (d, cfg) = isolated_config("device: x\n");
    let out = hermetic_sb(&d, &cfg)
        .args(["config", "get", "sb_home_dir"])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "get of unset field should exit non-zero: {out:?}"
    );
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("sb_home_dir") && stderr.contains("unset"),
        "stderr should mention unset state:\n{stderr}"
    );
}

#[test]
fn get_unknown_key_lists_known_keys() {
    let (d, cfg) = isolated_config("device: x\n");
    let out = hermetic_sb(&d, &cfg)
        .args(["config", "get", "not_a_real_key"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "bogus key should error: {out:?}");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("not_a_real_key") && stderr.contains("messages_root"),
        "should list known keys in the error:\n{stderr}"
    );
}
