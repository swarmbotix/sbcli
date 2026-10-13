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
