//! Self-update for `sb`: learn the newest release, and install it.
//!
//! Releases are published on GitHub as tag `vX.Y.Z` with one zip per
//! platform, `swarmbotix-<version>-<platform>.zip`, plus a `.sha256` beside
//! it. The zip holds one folder of the same name with `bin/sb`, `documents/`,
//! `messages/`, `VERSION` and the package's own installer. `sb update` does
//! nothing an installer cannot: it downloads and verifies the zip, unpacks
//! it in the system temp folder, and runs that installer against `<sb_home>`.
//!
//! Filesystem layout owned by this crate:
//!
//! ```text
//! <sb_home>/                 (~/.swarmbotix by default)
//!   update-check.json        latest release seen, and when (see `cache`)
//!   bin/sb.exe.old           Windows only: the binary `sb update` replaced
//! ```
//!
//! Every function takes the `sb_home` root explicitly, and every network
//! access goes through a [`Fetcher`], so tests point both at tempdirs.
//!
//! Environment:
//!
//! * `SB_RELEASE_BASE`: repository URL to use instead of
//!   `https://github.com/swarmbotix/sbcli`, or `file:///<dir>` for a folder
//!   of release assets plus `latest.txt` (offline tests).
//! * `SB_NO_UPDATE_CHECK`: any value but empty or `0` stops `sb --version`
//!   from looking for a newer release.
//! * `SB_UPDATE_ALLOW_ANY_EXE=1`: lets `sb update` run from a binary other
//!   than `<sb_home>/bin/sb`. For tests only.

pub mod cache;
pub mod check;
pub mod fetch;
pub mod notice;
pub mod update;
pub mod version;

mod util;

pub use check::{CheckOutcome, check};
pub use fetch::{DirFetcher, Fetcher, GithubFetcher, from_env as fetcher_from_env};
pub use notice::version_notice;
pub use semver::Version;
pub use update::{
    UpdateOptions, UpdateOutcome, UpdatePlan, apply, cleanup_stale_binary, plan, update,
};
pub use version::{Status, arch, installed, parse_tag, parse_version};
