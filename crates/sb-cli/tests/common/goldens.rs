//! Golden snapshot helper shared by every per-language codegen test.
//!
//! Re-bless with `SB_BLESS=1` (writes the golden to disk instead of
//! asserting against it).
//!
//! Layout on disk: each language gets a subdir under `tests/level3_goldens/`.
//! Callers pick the subdir (e.g. `"rust"`, `"python"`, `"cpp"`, `"flutter"`,
//! `"unity"`) and a leaf filename.

#![allow(dead_code)]

use std::path::PathBuf;

/// Directory holding goldens for a given language. Lives under the
/// crate's `tests/` so paths stay stable regardless of where `cargo test`
/// is invoked from.
pub fn goldens_dir(subdir: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/level3_goldens")
        .join(subdir)
}

/// Compare `actual` against the on-disk golden, or write it if missing /
/// if `SB_BLESS=1`.
pub fn assert_or_bless(subdir: &str, golden_name: &str, actual: &str) {
    assert_or_bless_with(subdir, golden_name, actual, &[]);
}

/// Like [`assert_or_bless`] but applies a list of `(needle, placeholder)`
/// substitutions before compare/write. Used by the L3 iceoryx2 snapshots
/// so machine-specific absolute paths (e.g. the codegen-baked
/// `<CARGO_MANIFEST_DIR>/tests/fixtures/iox2_targets/...` include path)
/// don't leak into committed goldens.
pub fn assert_or_bless_with(
    subdir: &str,
    golden_name: &str,
    actual: &str,
    substitutions: &[(&str, &str)],
) {
    let path = goldens_dir(subdir).join(golden_name);
    let mut normalized = actual.to_owned();
    for (needle, placeholder) in substitutions {
        normalized = normalized.replace(needle, placeholder);
    }
    let actual = normalized.as_str();
    let bless = std::env::var_os("SB_BLESS").is_some();
    if bless || !path.exists() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        if !bless && std::env::var_os("CI").is_none() {
            eprintln!(
                "wrote new golden {} ({} bytes) — review and commit",
                path.display(),
                actual.len()
            );
            return;
        }
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("could not read golden {}: {e}", path.display()));
    if expected != actual {
        let cand = path.with_extension("actual");
        std::fs::write(&cand, actual).unwrap();
        panic!(
            "golden mismatch for {}\n  expected {} bytes, got {}\n  diff candidate written to {}\n  set SB_BLESS=1 to update",
            path.display(),
            expected.len(),
            actual.len(),
            cand.display()
        );
    }
}
