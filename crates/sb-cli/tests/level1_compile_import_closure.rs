//! `sb message compile <name>` on a message that imports another proto must
//! emit bindings for the whole import closure, on every backend.
//!
//! Regression for sb 0.2.1: the single-message path handed each backend only
//! the named file. flatc then failed on the translated `.fbs` (`unable to
//! load include file`), and iox2 / proto silently dropped the dependency
//! bindings the named message's output refers to.
//!
//! Needs real protoc and flatc; skips when either is not configured.

mod common;
use common::{SANDBOX_STYLE, Sandbox, protoc_available};

/// True if the merged config exposes a working `flatc` path.
fn flatc_available(sandbox: &Sandbox) -> bool {
    let s = std::fs::read_to_string(&sandbox.config_path).unwrap_or_default();
    s.lines().any(|l| {
        let t = l.trim_start();
        t.strip_prefix("flatc:")
            .and_then(|rest| rest.split_whitespace().next())
            .is_some_and(|p| !p.is_empty() && std::path::Path::new(p).exists())
    })
}

#[test]
fn single_message_compile_emits_import_closure_on_every_backend() {
    let s = Sandbox::new();
    if !protoc_available(&s) || !flatc_available(&s) {
        eprintln!("protoc or flatc not configured on this host — skipping");
        return;
    }

    // Two protos in one package; the second imports the first, and the
    // first declares extra top-level types the second embeds.
    let pkg = s.vault_dir().join("custom");
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(
        pkg.join("MarkerDetections.proto"),
        "syntax = \"proto3\";\n\
         package swarmbotix.custom;\n\
         message Vec3 { double x = 1; double y = 2; double z = 3; }\n\
         message Quat { double x = 1; double y = 2; double z = 3; double w = 4; }\n\
         message MarkerDetections { repeated Vec3 positions = 1; repeated Quat orientations = 2; }\n",
    )
    .unwrap();
    std::fs::write(
        pkg.join("CubePoses.proto"),
        "syntax = \"proto3\";\n\
         package swarmbotix.custom;\n\
         import \"custom/MarkerDetections.proto\";\n\
         message CubePoses { Vec3 position = 1; Quat orientation = 2; }\n",
    )
    .unwrap();

    // No backend flag: every available backend runs, which is also the path
    // `sb message new` / `sb message edit` take.
    let name = format!("{SANDBOX_STYLE}/custom/CubePoses");
    let out = s
        .cmd()
        .args(["message", "compile", &name])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "compile failed: stdout={stdout} stderr={stderr}"
    );

    let t = s.targets_dir();
    for rel in [
        // fb: the imported proto had to be translated and generated too.
        "fb/custom/MarkerDetections/MarkerDetections_generated.rs",
        // iox2: `[Vec3; N]` in the named struct needs Vec3's own bindings.
        "iox2/custom/Vec3/Vec3.rs",
        // proto: CubePoses.pb.h includes custom/MarkerDetections.pb.h.
        "proto/custom/MarkerDetections/MarkerDetections.pb.h",
    ] {
        assert!(
            t.join(rel).is_file(),
            "missing dependency output {rel} under {}",
            t.display()
        );
    }
}
