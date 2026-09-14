//! Bakes the transport crate versions this binary links into the binary.
//!
//! `sb doctor` reports which zenoh / iceoryx2 it was built against, and
//! compares that number with the native `.so` the config points at. Neither
//! number is discoverable at runtime: `env!("CARGO_PKG_VERSION")` only knows
//! about *this* crate, and cargo exposes no environment variable carrying a
//! dependency's resolved version. So it is read here, at build time, and
//! handed through `cargo::rustc-env`.
//!
//! `Cargo.lock` is the source consulted first because it states what actually
//! got compiled in. `versions.json` is the *declared* truth (`/sb-deps` syncs
//! it into `Cargo.toml`'s exact pins); it is read only to catch the case where
//! the two have drifted apart, which is a maintainer error that no compiler
//! check would otherwise catch.
//!
//! Neither file is required. A build from a source tree that lacks them still
//! succeeds, with the version reported as `unknown` — a doctor row that says
//! "unknown" is a smaller problem than a build that refuses to run.

use std::path::{Path, PathBuf};

/// crates.io names whose resolved version is baked in, and the env var each
/// lands in. Keep in step with `versions.json`.
const TRANSPORTS: &[(&str, &str)] = &[
    ("zenoh", "SB_ZENOH_VERSION"),
    ("iceoryx2", "SB_ICEORYX2_VERSION"),
];

const UNKNOWN: &str = "unknown";

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let lock_path = find_up(&manifest, "Cargo.lock");
    let declared_path = find_up(&manifest, "versions.json");

    if let Some(p) = &lock_path {
        println!("cargo::rerun-if-changed={}", p.display());
    }
    if let Some(p) = &declared_path {
        println!("cargo::rerun-if-changed={}", p.display());
    }

    let lock = lock_path.and_then(|p| std::fs::read_to_string(p).ok());
    let declared = declared_path.and_then(|p| std::fs::read_to_string(p).ok());

    for (crate_name, env_var) in TRANSPORTS {
        let resolved = lock
            .as_deref()
            .and_then(|t| locked_version(t, crate_name))
            .unwrap_or_else(|| UNKNOWN.to_owned());

        // Drift is a warning, not an error. The exact pins in Cargo.toml mean
        // cargo itself rejects a lockfile that disagrees with the pin, so the
        // only way to reach here is an edit to versions.json that was never
        // synced — worth saying out loud, not worth breaking the build over.
        if let Some(want) = declared
            .as_deref()
            .and_then(|t| declared_version(t, crate_name))
        {
            if resolved != UNKNOWN && want != resolved {
                println!(
                    "cargo::warning=versions.json pins {crate_name} {want} but Cargo.lock \
                     resolved {resolved} — run `/sb-deps` to re-sync"
                );
            }
        }

        println!("cargo::rustc-env={env_var}={resolved}");
    }
}

/// Nearest ancestor of `from` (inclusive) holding a file called `name`.
fn find_up(from: &Path, name: &str) -> Option<PathBuf> {
    from.ancestors().map(|d| d.join(name)).find(|p| p.is_file())
}

/// The `version` of the top-level `[[package]]` block named `want`.
///
/// Hand-scanned rather than parsed with a toml crate: the shape is two fixed
/// lines emitted by cargo itself, and a build-dependency on a toml parser
/// costs every clean build more than this costs to read.
fn locked_version(lock: &str, want: &str) -> Option<String> {
    let mut in_package = false;
    let mut is_wanted = false;
    for line in lock.lines() {
        let line = line.trim();
        if line == "[[package]]" {
            in_package = true;
            is_wanted = false;
            continue;
        }
        if line.starts_with('[') {
            in_package = false;
            continue;
        }
        if !in_package {
            continue;
        }
        if let Some(v) = line.strip_prefix("name = ") {
            is_wanted = v.trim().trim_matches('"') == want;
        } else if is_wanted {
            if let Some(v) = line.strip_prefix("version = ") {
                return Some(v.trim().trim_matches('"').to_owned());
            }
        }
    }
    None
}

/// The value of `"<want>"` in `versions.json`, which is flat `{"k": "v"}`.
///
/// Scanned by hand for the same reason as the lockfile: one string key, one
/// string value, no nesting to get wrong.
fn declared_version(json: &str, want: &str) -> Option<String> {
    let key = format!("\"{want}\"");
    let rest = json.split_once(&key)?.1;
    let rest = rest.trim_start().strip_prefix(':')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let (v, _) = rest.split_once('"')?;
    Some(v.to_owned())
}
