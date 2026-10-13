//! `sb update --check`: compare the running version with the latest release.

use std::path::Path;

use anyhow::{Context, Result};
use semver::Version;

use crate::cache;
use crate::fetch::Fetcher;
use crate::version::{self, Status};

/// What [`check`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckOutcome {
    /// The running version.
    pub installed: Version,
    /// The latest published release.
    pub latest: Version,
    /// This build's platform, as in asset names (`linux-x86_64`, ...).
    pub platform: &'static str,
    /// Download URL of the latest release's zip for [`CheckOutcome::platform`].
    pub asset_url: String,
    pub status: Status,
}

/// Ask `fetcher` for the latest release and compare it with the running
/// version. Always goes to the network (no cache read), and refreshes the
/// cache `sb --version` reads.
pub fn check(home: &Path, fetcher: &dyn Fetcher) -> Result<CheckOutcome> {
    check_from(home, fetcher, version::installed())
}

fn check_from(home: &Path, fetcher: &dyn Fetcher, installed: Version) -> Result<CheckOutcome> {
    let platform = version::arch()?;
    let tag = fetcher
        .latest_tag()
        .context("looking up the latest sb release")?;
    let latest = version::parse_tag(&tag)?;
    // Best effort: the answer stands whether or not it could be cached.
    let _ = cache::write(home, &latest);
    let asset_url = fetcher.asset_url(&latest, &version::asset_name(&latest, platform));
    let status = Status::of(&installed, &latest);
    Ok(CheckOutcome {
        installed,
        latest,
        platform,
        asset_url,
        status,
    })
}
