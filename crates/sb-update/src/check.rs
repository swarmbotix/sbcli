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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::DirFetcher;
    use tempfile::TempDir;

    fn dir_fetcher(latest: &str) -> DirFetcher {
        DirFetcher {
            dir: "/releases".into(),
            latest: latest.to_owned(),
        }
    }

    #[test]
    fn reports_status_and_refreshes_the_cache() {
        let home = TempDir::new().unwrap();
        let out = check_from(home.path(), &dir_fetcher("0.2.3"), Version::new(0, 2, 0)).unwrap();
        assert_eq!(out.status, Status::UpdateAvailable);
        assert_eq!(out.latest, Version::new(0, 2, 3));
        assert_eq!(out.platform, version::arch().unwrap());
        assert!(
            out.asset_url
                .ends_with(&format!("/swarmbotix-0.2.3-{}.zip", out.platform)),
            "{}",
            out.asset_url
        );
        assert_eq!(cache::read(home.path()).unwrap().latest, "0.2.3");

        let out = check_from(home.path(), &dir_fetcher("v0.1.41"), Version::new(0, 2, 0)).unwrap();
        assert_eq!(out.status, Status::Ahead);
        assert_eq!(cache::read(home.path()).unwrap().latest, "0.1.41");
    }

    #[test]
    fn bad_tag_is_an_error() {
        let home = TempDir::new().unwrap();
        let err = check_from(home.path(), &dir_fetcher("vNEXT"), Version::new(0, 2, 0))
            .unwrap_err()
            .to_string();
        assert!(err.contains("vNEXT"), "{err}");
    }
}
