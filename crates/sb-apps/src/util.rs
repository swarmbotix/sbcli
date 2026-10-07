//! Small helpers shared by the modules of this crate.

use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;

/// Join `rel` onto `dir`, accepting only a plain relative path.
///
/// Manifest paths (`entry`, `host.install`) are relative to the package
/// folder by contract, so absolute paths and `..` are rejected rather than
/// silently resolved somewhere outside the package. A leading `./` is
/// dropped so displayed paths stay clean. The `Err` text completes the
/// sentence "`<field>` ..." in the caller's message.
pub(crate) fn join_relative(dir: &Path, rel: &str) -> std::result::Result<PathBuf, &'static str> {
    let mut out = dir.to_path_buf();
    let mut named = false;
    for comp in Path::new(rel).components() {
        match comp {
            Component::CurDir => {}
            Component::Normal(seg) => {
                out.push(seg);
                named = true;
            }
            Component::ParentDir => {
                return Err("must stay inside the package folder (no `..`)");
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(
                    "must be relative to the package folder (e.g. `./run.bash`), not absolute",
                );
            }
        }
    }
    if named {
        Ok(out)
    } else {
        Err("must name a file (e.g. `./run.bash`)")
    }
}

/// Expand a leading `~` (alone, or followed by `/`) to the home directory.
pub(crate) fn expand_user(arg: &str) -> PathBuf {
    if arg == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    sb_config::expand_tilde(Path::new(arg))
}

/// Mark `path` executable (`0o755`). No-op on Windows, which has no mode bits.
#[cfg(unix)]
pub(crate) fn set_executable(path: &Path) -> Result<()> {
    use anyhow::Context;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)
        .with_context(|| format!("stat {}", path.display()))?
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).with_context(|| format!("chmod 0755 {}", path.display()))
}

// Kept fallible so both platforms share one `?` at every call site.
#[allow(clippy::unnecessary_wraps)]
#[cfg(not(unix))]
pub(crate) fn set_executable(_path: &Path) -> Result<()> {
    Ok(())
}

/// True if `path` carries any execute bit. Always false on Windows.
#[cfg(unix)]
pub(crate) fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// `p` without the `\\?\` verbatim prefix that `canonicalize` adds on
/// Windows (`\\?\C:\x` becomes `C:\x`; UNC forms are kept). Git Bash and
/// git cannot open verbatim paths, and they read badly in the registry.
/// No-op on Unix.
pub(crate) fn plain_path(p: &Path) -> PathBuf {
    match p.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(rest) if !rest.starts_with(r"UNC\") => PathBuf::from(rest),
        _ => p.to_path_buf(),
    }
}

/// `canonicalize`, then [`plain_path`].
pub(crate) fn canonical(p: &Path) -> std::io::Result<PathBuf> {
    p.canonicalize().map(|c| plain_path(&c))
}

/// Locate a bash that can run a script by its native path.
///
/// On Windows a bare `bash` often resolves to `System32\bash.exe`, the WSL
/// launcher, which cannot open `C:\...` paths. Prefer any other bash on
/// `PATH`, then the usual Git for Windows install locations.
#[cfg(windows)]
pub(crate) fn find_bash() -> Option<PathBuf> {
    let is_wsl_shim = |p: &Path| {
        p.to_string_lossy()
            .to_ascii_lowercase()
            .contains(r"\windows\system32\")
    };
    if let Ok(found) = which::which_all("bash") {
        for p in found {
            if !is_wsl_shim(&p) {
                return Some(p);
            }
        }
    }
    for base in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
        let Some(root) = std::env::var_os(base) else {
            continue;
        };
        for tail in [r"Git\bin\bash.exe", r"Git\usr\bin\bash.exe"] {
            let cand = PathBuf::from(&root).join(tail);
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

#[cfg(not(windows))]
pub(crate) fn find_bash() -> Option<PathBuf> {
    which::which("bash").ok()
}

/// A script path as bash expects it: forward slashes, even on Windows,
/// where a raw `C:\...` argument reaches bash with every `\` read as an
/// escape. No-op on Unix.
pub(crate) fn bash_path_arg(path: &Path) -> String {
    let s = plain_path(path).display().to_string();
    if cfg!(windows) {
        s.replace('\\', "/")
    } else {
        s
    }
}

/// A suffix unique enough for a scratch directory name: pid plus the
/// current time in nanoseconds. No randomness crate needed.
pub(crate) fn unique_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    format!("{}-{nanos}", std::process::id())
}

/// `t` as an RFC 3339 UTC timestamp with second precision,
/// e.g. `2026-10-07T12:00:00Z`.
pub(crate) fn rfc3339_utc(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Days since 1970-01-01 to a proleptic Gregorian `(year, month, day)`.
///
/// Howard Hinnant's `civil_from_days`, exact for every date after the epoch.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Levenshtein distance, for "did you mean" hints on short command names.
pub(crate) fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.chars().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let subst = prev[j] + usize::from(ca != *cb);
            cur[j + 1] = subst.min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn at(secs: u64) -> String {
        rfc3339_utc(UNIX_EPOCH + Duration::from_secs(secs))
    }

    #[test]
    fn rfc3339_known_instants() {
        assert_eq!(at(0), "1970-01-01T00:00:00Z");
        assert_eq!(at(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(at(1_000_000_000), "2001-09-09T01:46:40Z");
        assert_eq!(at(1_791_374_400), "2026-10-07T12:00:00Z");
    }

    #[test]
    fn join_relative_rules() {
        let dir = Path::new("/pkg");
        assert_eq!(
            join_relative(dir, "./run.bash"),
            Ok(PathBuf::from("/pkg/run.bash"))
        );
        assert_eq!(
            join_relative(dir, "bin/app"),
            Ok(PathBuf::from("/pkg/bin/app"))
        );
        assert!(join_relative(dir, "../escape").is_err());
        assert!(join_relative(dir, "/etc/passwd").is_err());
        assert!(join_relative(dir, "./").is_err());
        assert!(join_relative(dir, "").is_err());
    }

    #[test]
    fn plain_path_strips_only_drive_verbatim_prefixes() {
        assert_eq!(
            plain_path(Path::new(r"\\?\C:\pkg")),
            PathBuf::from(r"C:\pkg")
        );
        let unc = Path::new(r"\\?\UNC\server\share");
        assert_eq!(plain_path(unc), unc.to_path_buf());
        assert_eq!(plain_path(Path::new("/home/x")), PathBuf::from("/home/x"));
    }

    #[test]
    fn edit_distance_basics() {
        assert_eq!(edit_distance("doctor", "doctor"), 0);
        assert_eq!(edit_distance("doctr", "doctor"), 1);
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(edit_distance("kitten", "sitting"), 3);
    }
}
