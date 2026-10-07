//! The line `sb --version` prints under `sb X.Y.Z`.

use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use semver::Version;

use crate::cache;
use crate::fetch::{self, Fetcher};
use crate::version::{self, Status};

/// Environment variable that turns the check off: any value but empty or `0`.
pub const NO_CHECK_ENV: &str = "SB_NO_UPDATE_CHECK";

/// The longest `sb --version` waits for an answer. ureq's 2 s connect and
/// read timeouts do not cover DNS resolution, which can stall far longer on
/// a machine whose resolver is unreachable (a robot on an isolated network);
/// this bounds the whole lookup.
pub const DEADLINE: Duration = Duration::from_secs(4);

/// True if [`NO_CHECK_ENV`] asks for no update check.
pub fn check_disabled() -> bool {
    std::env::var_os(NO_CHECK_ENV).is_some_and(|v| !v.is_empty() && v != "0")
}

/// The text to print under `sb X.Y.Z` for `sb --version`, if any:
///
/// * `update available: X.Y.Z` and a hint to run `sb update`, when a newer
///   release exists,
/// * `(latest)` when this is the latest release,
/// * `None` when this build is newer than the latest release (a dev build),
///   when [`NO_CHECK_ENV`] is set, or when the latest release cannot be
///   learned for any reason. Failure is silent by design.
///
/// A cached answer younger than [`cache::TTL`] is used as is; otherwise
/// GitHub is asked (2 s connect and read timeouts) and the cache refreshed.
/// Whatever happens, the answer is given up on after [`DEADLINE`]. Prints
/// nothing itself.
pub fn version_notice(home: &Path) -> Option<String> {
    if check_disabled() {
        return None;
    }
    notice_within(
        home,
        version::installed(),
        || fetch::from_env().ok(),
        DEADLINE,
    )
}

/// [`notice_from`] on a helper thread, abandoned after `limit`. The fetcher
/// is built on that thread, so it need not be `Send`. A thread still
/// blocked when `limit` passes is left behind; the process exits without it.
fn notice_within<F>(home: &Path, installed: Version, make: F, limit: Duration) -> Option<String>
where
    F: FnOnce() -> Option<Box<dyn Fetcher>> + Send + 'static,
{
    let home = home.to_path_buf();
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("sb-update-notice".to_owned())
        .spawn(move || {
            let notice = make().and_then(|f| notice_from(&home, &installed, f.as_ref()));
            let _ = tx.send(notice);
        })
        .ok()?;
    rx.recv_timeout(limit).ok().flatten()
}

fn notice_from(home: &Path, installed: &Version, fetcher: &dyn Fetcher) -> Option<String> {
    let cached = cache::read(home)
        .filter(|c| cache::is_fresh(c, cache::TTL))
        .and_then(|c| c.latest_version());
    let latest = cached.or_else(|| {
        let latest = version::parse_tag(&fetcher.latest_tag().ok()?).ok()?;
        // Best effort: without a cache the next call simply asks again.
        let _ = cache::write(home, &latest);
        Some(latest)
    })?;
    notice_text(installed, &latest)
}

fn notice_text(installed: &Version, latest: &Version) -> Option<String> {
    match Status::of(installed, latest) {
        Status::UpdateAvailable => Some(format!("update available: {latest}   run `sb update`")),
        Status::UpToDate => Some("(latest)".to_owned()),
        Status::Ahead => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{Result, bail};
    use std::cell::Cell;
    use std::time::SystemTime;
    use tempfile::TempDir;

    /// Proves a path never reaches the network.
    struct PanicFetcher;

    impl Fetcher for PanicFetcher {
        fn latest_tag(&self) -> Result<String> {
            panic!("the cache should have answered without a fetch")
        }
        fn asset_url(&self, _: &Version, _: &str) -> String {
            panic!("no asset URL expected")
        }
        fn download(&self, _: &str, _: &Path) -> Result<()> {
            panic!("no download expected")
        }
    }

    /// Answers `latest_tag` with a fixed result and counts the calls.
    struct FixedFetcher {
        tag: Option<&'static str>,
        calls: Cell<u32>,
    }

    impl FixedFetcher {
        fn new(tag: Option<&'static str>) -> Self {
            Self {
                tag,
                calls: Cell::new(0),
            }
        }
    }

    impl Fetcher for FixedFetcher {
        fn latest_tag(&self) -> Result<String> {
            self.calls.set(self.calls.get() + 1);
            match self.tag {
                Some(t) => Ok(t.to_owned()),
                None => bail!("network is down"),
            }
        }
        fn asset_url(&self, _: &Version, _: &str) -> String {
            String::new()
        }
        fn download(&self, _: &str, _: &Path) -> Result<()> {
            bail!("no downloads here")
        }
    }

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn env_switch_silences_the_notice() {
        // Set and never unset: no other test in this crate reads it, and the
        // inner function below does not consult it.
        std::env::set_var(NO_CHECK_ENV, "1");
        let home = TempDir::new().unwrap();
        cache::write(home.path(), &v("99.0.0")).unwrap();
        assert_eq!(version_notice(home.path()), None);
    }

    #[test]
    fn fresh_cache_answers_without_a_fetch() {
        let home = TempDir::new().unwrap();
        cache::write(home.path(), &v("0.2.3")).unwrap();
        assert_eq!(
            notice_from(home.path(), &v("0.2.0"), &PanicFetcher).as_deref(),
            Some("update available: 0.2.3   run `sb update`")
        );
        assert_eq!(
            notice_from(home.path(), &v("0.2.3"), &PanicFetcher).as_deref(),
            Some("(latest)")
        );
        assert_eq!(notice_from(home.path(), &v("0.3.0"), &PanicFetcher), None);
    }

    #[test]
    fn stale_or_missing_cache_fetches_and_refreshes() {
        let home = TempDir::new().unwrap();
        let fetcher = FixedFetcher::new(Some("v0.2.5"));
        assert_eq!(
            notice_from(home.path(), &v("0.2.0"), &fetcher).as_deref(),
            Some("update available: 0.2.5   run `sb update`")
        );
        assert_eq!(fetcher.calls.get(), 1);
        let cached = cache::read(home.path()).unwrap();
        assert_eq!(cached.latest, "0.2.5");

        let day_ago = SystemTime::now() - cache::TTL - Duration::from_secs(1);
        cache::write_at(home.path(), &v("0.2.1"), day_ago).unwrap();
        assert_eq!(
            notice_from(home.path(), &v("0.2.5"), &fetcher).as_deref(),
            Some("(latest)")
        );
        assert_eq!(fetcher.calls.get(), 2, "a stale cache must be refreshed");
        assert_eq!(cache::read(home.path()).unwrap().latest, "0.2.5");
    }

    /// Never answers in time, like a resolver that is not there.
    struct HangingFetcher;

    impl Fetcher for HangingFetcher {
        fn latest_tag(&self) -> Result<String> {
            std::thread::sleep(Duration::from_secs(30));
            Ok("v99.0.0".to_owned())
        }
        fn asset_url(&self, _: &Version, _: &str) -> String {
            String::new()
        }
        fn download(&self, _: &str, _: &Path) -> Result<()> {
            bail!("no downloads here")
        }
    }

    #[test]
    fn a_hanging_lookup_is_abandoned() {
        let home = TempDir::new().unwrap();
        let started = std::time::Instant::now();
        let notice = notice_within(
            home.path(),
            v("0.2.0"),
            || Some(Box::new(HangingFetcher) as Box<dyn Fetcher>),
            Duration::from_millis(200),
        );
        assert_eq!(notice, None);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );

        // An answer inside the limit comes through the thread unchanged.
        cache::write(home.path(), &v("0.2.3")).unwrap();
        let notice = notice_within(
            home.path(),
            v("0.2.0"),
            || Some(Box::new(PanicFetcher) as Box<dyn Fetcher>),
            DEADLINE,
        );
        assert_eq!(
            notice.as_deref(),
            Some("update available: 0.2.3   run `sb update`")
        );
    }

    #[test]
    fn failures_are_silent() {
        let home = TempDir::new().unwrap();
        let down = FixedFetcher::new(None);
        assert_eq!(notice_from(home.path(), &v("0.2.0"), &down), None);
        assert_eq!(cache::read(home.path()), None, "no answer, no cache");

        let odd = FixedFetcher::new(Some("nightly"));
        assert_eq!(notice_from(home.path(), &v("0.2.0"), &odd), None);

        // A home that does not exist: the notice still works, uncached.
        let gone = home.path().join("absent");
        let up = FixedFetcher::new(Some("v0.2.0"));
        assert_eq!(
            notice_from(&gone, &v("0.2.0"), &up).as_deref(),
            Some("(latest)")
        );
    }
}
