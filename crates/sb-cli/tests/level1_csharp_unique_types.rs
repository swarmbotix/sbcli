//! L1 regression — the emitted C# for one style must compile as one assembly.
//!
//! Unity (and any .NET project) compiles every `.cs` under the tree at once, so
//! the whole style is a single compilation unit. A type may be declared exactly
//! once in a namespace; anything that declares one twice is `error CS0101`
//! (plus `CS0579` on the duplicated `[StructLayout]`, which Roslyn reports as a
//! follow-on) and the *host project's* build breaks, not ours.
//!
//! Through 0.1.38 the `repeated <Message>` array wrapper was emitted inline in
//! every message that referenced it, so two messages in one package sharing an
//! element type collided. The wrapper now lives in `iox2/_arrays/<Name>.cs`.
//!
//! The uniqueness assertion here is deliberately general rather than a check
//! for that one wrapper: it is the property the host compiler actually
//! enforces, so it holds the line against the next way of breaking it.

mod common;
use common::{Sandbox, protoc_available};

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Two messages in ONE package, each holding a `repeated Elem` — the exact
/// shape reported from `vrobots_msgs` (several `Srv*Msg` with `repeated
/// Vec3Msg`), and the shape 0.1.38 emitted twice.
const ELEM_PROTO: &str = r#"syntax = "proto3";
package sbtest.parts;

message Elem {
  double x = 1;
}
"#;

const FIRST_PROTO: &str = r#"syntax = "proto3";
package sbtest.svc;
import "parts/Elem.proto";

message FirstMsg {
  repeated sbtest.parts.Elem points = 1;
}
"#;

const SECOND_PROTO: &str = r#"syntax = "proto3";
package sbtest.svc;
import "parts/Elem.proto";

message SecondMsg {
  repeated sbtest.parts.Elem points = 1;
}
"#;

/// Every `.cs` under `root`, as (path, contents).
fn csharp_files(root: &Path) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(p) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&p) else {
            continue;
        };
        for e in rd.flatten() {
            let path = e.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|x| x.to_str()) == Some("cs") {
                let text = std::fs::read_to_string(&path).unwrap();
                out.push((path, text));
            }
        }
    }
    out
}

/// `(namespace, type)` → files declaring it. Good enough for generated code:
/// the emitter writes one `namespace X` per file and one declaration per line.
fn declarations(files: &[(PathBuf, String)]) -> BTreeMap<(String, String), Vec<PathBuf>> {
    let mut map: BTreeMap<(String, String), Vec<PathBuf>> = BTreeMap::new();
    for (path, text) in files {
        let ns = text
            .lines()
            .find_map(|l| l.trim().strip_prefix("namespace "))
            .map(|s| s.trim_end_matches(&[' ', '{', ';'][..]).trim().to_owned())
            .unwrap_or_default();
        for line in text.lines() {
            let t = line.trim();
            for prefix in ["public unsafe struct ", "public struct ", "public class "] {
                if let Some(rest) = t.strip_prefix(prefix) {
                    let name = rest
                        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                        .next()
                        .unwrap_or_default();
                    if !name.is_empty() {
                        map.entry((ns.clone(), name.to_owned()))
                            .or_default()
                            .push(path.clone());
                    }
                }
            }
        }
    }
    map
}

/// Install the fixture style and compile it to `--out`, returning that dir.
/// `None` if protoc is not configured on this host.
fn compile_fixture(s: &Sandbox) -> Option<PathBuf> {
    if !protoc_available(s) {
        eprintln!("protoc not configured on this host — skipping");
        return None;
    }
    // Populate the vault so the style tree exists.
    s.cmd().args(["message", "list"]).output().unwrap();

    let defs = s.vault_dir();
    std::fs::create_dir_all(defs.join("parts")).unwrap();
    std::fs::create_dir_all(defs.join("svc")).unwrap();
    std::fs::write(defs.join("parts/Elem.proto"), ELEM_PROTO).unwrap();
    std::fs::write(defs.join("svc/FirstMsg.proto"), FIRST_PROTO).unwrap();
    std::fs::write(defs.join("svc/SecondMsg.proto"), SECOND_PROTO).unwrap();

    let out_dir = s.home().join("gen_cs");
    let cmd_out = s
        .cmd()
        .args([
            "message",
            "compile",
            "--iox2",
            "--out",
            out_dir.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        cmd_out.status.success(),
        "compile failed: stdout={} stderr={}",
        String::from_utf8_lossy(&cmd_out.stdout),
        String::from_utf8_lossy(&cmd_out.stderr)
    );
    Some(out_dir)
}

#[test]
fn no_csharp_type_is_declared_twice_in_one_namespace() {
    let s = Sandbox::new();
    let Some(out_dir) = compile_fixture(&s) else {
        return;
    };

    let files = csharp_files(&out_dir);
    assert!(!files.is_empty(), "no C# emitted at all");

    let dupes: Vec<_> = declarations(&files)
        .into_iter()
        .filter(|(_, paths)| paths.len() > 1)
        .collect();
    assert!(
        dupes.is_empty(),
        "every generated .cs compiles into one assembly, so a duplicate is \
         error CS0101 in the host project. Duplicated:\n{}",
        dupes
            .iter()
            .map(|((ns, ty), paths)| format!(
                "  {ns}.{ty} in {}",
                paths
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn shared_array_wrapper_is_emitted_once_and_referenced_qualified() {
    let s = Sandbox::new();
    let Some(out_dir) = compile_fixture(&s) else {
        return;
    };

    // One file, in the shared dir, for the one (element type, capacity) pair
    // both messages use.
    let wrapper = out_dir.join("iox2/_arrays/sbtest_parts_Elem_Array256.cs");
    assert!(
        wrapper.exists(),
        "expected the shared wrapper at {}",
        wrapper.display()
    );
    let wrapper_src = std::fs::read_to_string(&wrapper).unwrap();
    assert!(wrapper_src.contains("namespace sb_iox2_arrays\n{"));
    assert!(wrapper_src.contains("public global::sbtest_parts.Elem _0;"));

    // Neither consumer redeclares it; both name it through the shared namespace.
    for leaf in ["FirstMsg", "SecondMsg"] {
        let src =
            std::fs::read_to_string(out_dir.join(format!("iox2/svc/{leaf}/{leaf}.cs"))).unwrap();
        assert!(
            !src.contains("struct sbtest_parts_Elem_Array256"),
            "{leaf} must not redeclare the wrapper:\n{src}"
        );
        assert!(
            src.contains("public global::sb_iox2_arrays.sbtest_parts_Elem_Array256 points;"),
            "{leaf} must reference the wrapper fully qualified:\n{src}"
        );
    }
}

/// Compiling one message must still emit the wrappers it needs — the shared
/// file is written per-message, and rewriting it is a no-op because path and
/// contents are a pure function of (element type, capacity).
#[test]
fn single_message_compile_still_emits_its_wrapper() {
    let s = Sandbox::new();
    let Some(_) = compile_fixture(&s) else {
        return;
    };

    let out_dir = s.home().join("gen_one");
    let cmd_out = s
        .cmd()
        .args([
            "message",
            "compile",
            "--iox2",
            "--out",
            out_dir.to_str().unwrap(),
            "ros2/svc/SecondMsg",
        ])
        .output()
        .unwrap();
    assert!(cmd_out.status.success(), "{cmd_out:?}");
    assert!(
        out_dir
            .join("iox2/_arrays/sbtest_parts_Elem_Array256.cs")
            .exists(),
        "a single-message compile must still emit the wrapper it references"
    );
}
