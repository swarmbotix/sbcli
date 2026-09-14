//! L3 TDD #5 — generated Rust files compile inside a real Rust crate.
//!
//! Marked `#[ignore]` because the fixture pulls `zenoh = "1.9"` + the
//! iceoryx2 deps from crates.io and compiles them. First run downloads
//! ~100 MB and takes minutes; subsequent runs hit the registry cache.
//!
//! Run with:
//!
//! ```bash
//! cargo test -p sb-cli --test level3_rust_cargo_check -- --ignored
//! ```
//!
//! Catches API drift in the pinned transport versions — if zenoh changes
//! `declare_publisher` or iceoryx2 reshuffles `loan_uninit`, the
//! generated templates stop compiling and this test catches it.

mod common;
use common::L3Sandbox;

/// Write a minimal `Cargo.toml` to the module root so `cargo check` has
/// something to chew on. The module dir is just an empty user project
/// to L2's `sb init`; we provide the deps here because sb intentionally
/// doesn't touch user build files.
fn seed_cargo_crate(s: &L3Sandbox, deps: &str) {
    let cargo_toml = format!(
        r#"[package]
name = "{module}"
version = "0.0.1"
edition = "2021"

[lib]
path = "lib.rs"

[dependencies]
{deps}
"#,
        module = s.module,
        deps = deps,
    );
    std::fs::write(s.module_root.join("Cargo.toml"), cargo_toml).unwrap();

    // lib.rs re-exports the generated module so cargo actually compiles
    // the file under swarmbotix_io/. Without a `mod swarmbotix_io;`
    // line cargo would skip it.
    let lib_rs = r#"#![allow(dead_code, unused_imports)]
#[path = "swarmbotix_io/mod.rs"]
mod swarmbotix_io_root;
"#;
    std::fs::write(s.module_root.join("lib.rs"), lib_rs).unwrap();

    // Stitch the directory together so the generator output is reachable
    // through the module tree.
    let io_root = s.io_dir();
    std::fs::create_dir_all(&io_root).unwrap();
    std::fs::write(
        io_root.join("mod.rs"),
        "pub mod publishers;\npub mod subscribers;\n",
    )
    .unwrap();
    std::fs::create_dir_all(io_root.join("publishers")).unwrap();
    std::fs::create_dir_all(io_root.join("subscribers")).unwrap();
}

/// `mod.rs` enumerating every `*.rs` file in `dir` (one `pub mod <stem>;`
/// per file). Rerun after every `sb pub/sub add` so cargo picks up the
/// new files.
fn refresh_mod_rs(dir: &std::path::Path) {
    let mut names: Vec<String> = Vec::new();
    if let Ok(read) = std::fs::read_dir(dir) {
        for e in read.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("rs") {
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    if stem != "mod" {
                        names.push(stem.to_string());
                    }
                }
            }
        }
    }
    names.sort();
    let body: String = names.iter().map(|n| format!("pub mod {n};\n")).collect();
    std::fs::write(dir.join("mod.rs"), body).unwrap();
}

fn cargo_check(module_root: &std::path::Path) -> std::process::Output {
    std::process::Command::new("cargo")
        .args(["check", "--quiet"])
        .current_dir(module_root)
        .env_remove("RUSTC_WRAPPER")
        .output()
        .expect("spawn cargo check")
}

#[test]
#[ignore = "heavy — downloads zenoh + iceoryx2 dependencies"]
fn generated_zenoh_publisher_compiles() {
    let s = L3Sandbox::rust("pubber_zenoh");
    seed_cargo_crate(&s, "zenoh = \"1.9\"\n");

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
    assert!(out.status.success(), "sb pub add failed: {out:?}");

    refresh_mod_rs(&s.io_dir().join("publishers"));
    refresh_mod_rs(&s.io_dir().join("subscribers"));

    let check = cargo_check(&s.module_root);
    assert!(
        check.status.success(),
        "cargo check failed for generated Zenoh publisher:\n--- stdout ---\n{}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr)
    );
}

#[test]
#[ignore = "heavy — downloads iceoryx2 dependencies"]
fn generated_iceoryx_publisher_compiles() {
    let s = L3Sandbox::rust("pubber_iox");
    seed_cargo_crate(&s, "iceoryx2 = \"0.9\"\n");

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
    assert!(out.status.success(), "sb pub add failed: {out:?}");

    refresh_mod_rs(&s.io_dir().join("publishers"));
    refresh_mod_rs(&s.io_dir().join("subscribers"));

    let check = cargo_check(&s.module_root);
    assert!(
        check.status.success(),
        "cargo check failed for generated iceoryx2 publisher:\n--- stdout ---\n{}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr)
    );
}
