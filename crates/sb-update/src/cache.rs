//! `<sb_home>/update-check.json`: the last answer to "what is the latest
//! release?", so `sb --version` asks GitHub at most once a day.
//!
//! ```json
//! { "latest": "0.2.3", "checked_at": "2026-10-07T12:00:00Z", "checked_at_unix": 1791374400 }
//! ```
//!
//! `checked_at_unix` is what freshness is computed from; `checked_at` is the
//! same instant for a human reading the file. The cache is best effort: a
//! missing, unreadable or malformed file reads as no cache, and the writers
//! never create `<sb_home>` (a development build should not leave one behind).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::util::{rfc3339_utc, unix_secs};

/// File name under `<sb_home>`.
pub const FILE: &str = "update-check.json";

/// How long a cached answer is trusted.
pub const TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// One cached answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cached {
    /// Latest release seen, `X.Y.Z` without the tag's `v`.
    pub latest: String,
    /// When it was seen, RFC 3339 UTC. Informational only.
    pub checked_at: String,
    /// When it was seen, in seconds since the Unix epoch.
    pub checked_at_unix: u64,
}

impl Cached {
    /// [`Cached::latest`] as a version; `None` if it does not parse.
    pub fn latest_version(&self) -> Option<Version> {
        Version::parse(&self.latest).ok()
    }
}

/// `<home>/update-check.json`.
pub fn path(home: &Path) -> PathBuf {
    home.join(FILE)
}

/// The cached answer, if there is a readable one.
pub fn read(home: &Path) -> Option<Cached> {
    let text = fs::read_to_string(path(home)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Record `latest` as seen now. Fails if `home` does not exist.
pub fn write(home: &Path, latest: &Version) -> Result<()> {
    write_at(home, latest, SystemTime::now())
}

pub(crate) fn write_at(home: &Path, latest: &Version, now: SystemTime) -> Result<()> {
    let cached = Cached {
        latest: latest.to_string(),
        checked_at: rfc3339_utc(now),
        checked_at_unix: unix_secs(now),
    };
    let mut body = serde_json::to_string_pretty(&cached).context("serializing the update cache")?;
    body.push('\n');
    // Write then rename, so a concurrent reader sees the old file or the new
    // one, never half of one.
    let dest = path(home);
    let tmp = home.join(format!("{FILE}.tmp-{}", std::process::id()));
    fs::write(&tmp, body).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, &dest).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        anyhow::Error::new(e).context(format!("replacing {}", dest.display()))
    })
}

/// True if `cached` is younger than `ttl`. A timestamp in the future (the
/// clock was set back since) is not trusted.
pub fn is_fresh(cached: &Cached, ttl: Duration) -> bool {
    is_fresh_at(cached, ttl, SystemTime::now())
}

pub(crate) fn is_fresh_at(cached: &Cached, ttl: Duration, now: SystemTime) -> bool {
    unix_secs(now)
        .checked_sub(cached.checked_at_unix)
        .is_some_and(|age| age < ttl.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;
    use tempfile::TempDir;

    #[test]
    fn round_trip() {
        let home = TempDir::new().unwrap();
        assert_eq!(read(home.path()), None);
        let now = UNIX_EPOCH + Duration::from_secs(1_791_374_400);
        write_at(home.path(), &Version::new(0, 2, 3), now).unwrap();
        let c = read(home.path()).unwrap();
        assert_eq!(
            c,
            Cached {
                latest: "0.2.3".to_owned(),
                checked_at: "2026-10-07T12:00:00Z".to_owned(),
                checked_at_unix: 1_791_374_400,
            }
        );
        assert_eq!(c.latest_version(), Some(Version::new(0, 2, 3)));
        // Nothing but the cache file is left behind.
        let names: Vec<_> = fs::read_dir(home.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, [FILE]);
    }

    #[test]
    fn ttl() {
        let c = Cached {
            latest: "0.2.3".to_owned(),
            checked_at: String::new(),
            checked_at_unix: 1_000_000,
        };
        let at = |secs| UNIX_EPOCH + Duration::from_secs(secs);
        assert!(is_fresh_at(&c, TTL, at(1_000_000)));
        assert!(is_fresh_at(&c, TTL, at(1_000_000 + TTL.as_secs() - 1)));
        assert!(!is_fresh_at(&c, TTL, at(1_000_000 + TTL.as_secs())));
        assert!(!is_fresh_at(&c, TTL, at(999_999)), "future stamp");
        // Written now, read now: fresh.
        let home = TempDir::new().unwrap();
        write(home.path(), &Version::new(0, 2, 3)).unwrap();
        assert!(is_fresh(&read(home.path()).unwrap(), TTL));
    }

    #[test]
    fn junk_reads_as_no_cache() {
        let home = TempDir::new().unwrap();
        fs::write(path(home.path()), "{ not json").unwrap();
        assert_eq!(read(home.path()), None);
        fs::write(
            path(home.path()),
            r#"{"latest":"soon","checked_at":"","checked_at_unix":1}"#,
        )
        .unwrap();
        assert_eq!(read(home.path()).unwrap().latest_version(), None);
    }

    #[test]
    fn write_never_creates_home() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("absent");
        assert!(write(&home, &Version::new(0, 2, 3)).is_err());
        assert!(!home.exists());
    }
}
