//! Small helpers shared by the modules of this crate.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since the Unix epoch; 0 for a clock set before 1970.
pub(crate) fn unix_secs(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// `t` as an RFC 3339 UTC timestamp with second precision,
/// e.g. `2026-10-07T12:00:00Z`.
pub(crate) fn rfc3339_utc(t: SystemTime) -> String {
    let secs = unix_secs(t);
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

/// `p` without the `\\?\` verbatim prefix that `canonicalize` adds on
/// Windows (`\\?\C:\x` becomes `C:\x`; UNC forms are kept), so paths in
/// messages read the way the user typed them. No-op on Unix.
pub(crate) fn plain_path(p: &Path) -> PathBuf {
    match p.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(rest) if !rest.starts_with(r"UNC\") => PathBuf::from(rest),
        _ => p.to_path_buf(),
    }
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
        assert_eq!(at(1_791_374_400), "2026-10-07T12:00:00Z");
    }

    #[test]
    fn plain_path_strips_only_drive_verbatim_prefixes() {
        assert_eq!(plain_path(Path::new(r"\\?\C:\sb")), PathBuf::from(r"C:\sb"));
        let unc = Path::new(r"\\?\UNC\server\share");
        assert_eq!(plain_path(unc), unc.to_path_buf());
        assert_eq!(plain_path(Path::new("/home/x")), PathBuf::from("/home/x"));
    }
}
