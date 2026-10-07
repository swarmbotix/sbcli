//! Versions, release tags, and the platform name used in asset file names.

use std::cmp::Ordering;
use std::fmt;

use anyhow::{Context, Result, anyhow, bail};
use semver::Version;

/// Name every release zip starts with: `swarmbotix-<version>-<platform>.zip`.
pub const PRODUCT: &str = "swarmbotix";

/// The version of the running `sb`.
///
/// # Panics
///
/// Never in practice: Cargo only accepts a semver package version, and the
/// whole workspace shares one (`[workspace.package]`).
pub fn installed() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("CARGO_PKG_VERSION is semver")
}

/// A release tag as published on GitHub, `v0.2.1`, to its version. The `v`
/// is required: every release tag is spelled with one, so a tag without it
/// is not a release.
pub fn parse_tag(tag: &str) -> Result<Version> {
    let bare = tag
        .strip_prefix('v')
        .ok_or_else(|| anyhow!("release tag `{tag}` does not start with `v`"))?;
    Version::parse(bare).with_context(|| format!("release tag `{tag}` is not v<X.Y.Z>"))
}

/// A version typed by the user, `0.2.1` or `v0.2.1`.
pub fn parse_version(text: &str) -> Result<Version> {
    let t = text.trim();
    let bare = t.strip_prefix('v').unwrap_or(t);
    Version::parse(bare)
        .with_context(|| format!("`{text}` is not a version; expected X.Y.Z, e.g. 0.2.1"))
}

/// The running version measured against the latest release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Running the latest release.
    UpToDate,
    /// A newer release exists.
    UpdateAvailable,
    /// Running something newer than the latest release (a dev build, or a
    /// release cut but not yet published).
    Ahead,
}

impl Status {
    pub fn of(installed: &Version, latest: &Version) -> Self {
        match installed.cmp(latest) {
            Ordering::Less => Self::UpdateAvailable,
            Ordering::Equal => Self::UpToDate,
            Ordering::Greater => Self::Ahead,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::UpToDate => "up to date",
            Self::UpdateAvailable => "update available",
            Self::Ahead => "ahead",
        }
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The platform part of a release asset name for this build of `sb`.
///
/// Decided at compile time: the binary that is running is the one being
/// replaced, so its own target is the right package.
pub fn arch() -> Result<&'static str> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Ok("linux-x86_64")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Ok("linux-aarch64")
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Ok("windows-x86_64")
    } else {
        bail!(
            "no release asset for this platform ({}-{}); releases exist for \
             linux-x86_64, linux-aarch64 and windows-x86_64",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    }
}

/// `swarmbotix-<version>-<platform>`: the zip's stem and its one top folder.
pub fn package_stem(version: &Version, platform: &str) -> String {
    format!("{PRODUCT}-{version}-{platform}")
}

/// `swarmbotix-<version>-<platform>.zip`.
pub fn asset_name(version: &Version, platform: &str) -> String {
    format!("{}.zip", package_stem(version, platform))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn tags_need_the_v() {
        assert_eq!(parse_tag("v0.2.1").unwrap(), v("0.2.1"));
        assert_eq!(parse_tag("v1.0.0-rc.1").unwrap(), v("1.0.0-rc.1"));
        let err = parse_tag("0.2.1").unwrap_err().to_string();
        assert!(err.contains("does not start with `v`"), "{err}");
        assert!(parse_tag("v0.2").is_err());
        assert!(parse_tag("vlatest").is_err());
        assert!(parse_tag("v").is_err());
    }

    #[test]
    fn user_versions_take_an_optional_v() {
        assert_eq!(parse_version("0.2.1").unwrap(), v("0.2.1"));
        assert_eq!(parse_version(" v0.2.1 ").unwrap(), v("0.2.1"));
        let err = format!("{:#}", parse_version("latest").unwrap_err());
        assert!(err.contains("expected X.Y.Z"), "{err}");
    }

    #[test]
    fn status_orders_by_semver() {
        assert_eq!(
            Status::of(&v("0.2.0"), &v("0.2.3")),
            Status::UpdateAvailable
        );
        assert_eq!(Status::of(&v("0.2.3"), &v("0.2.3")), Status::UpToDate);
        assert_eq!(Status::of(&v("0.2.0"), &v("0.1.41")), Status::Ahead);
        // Numeric, not lexical: 0.1.10 is newer than 0.1.9.
        assert_eq!(
            Status::of(&v("0.1.9"), &v("0.1.10")),
            Status::UpdateAvailable
        );
    }

    #[test]
    fn installed_is_the_package_version() {
        assert_eq!(installed().to_string(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn asset_names() {
        assert_eq!(
            asset_name(&v("0.2.3"), "linux-x86_64"),
            "swarmbotix-0.2.3-linux-x86_64.zip"
        );
        assert_eq!(
            package_stem(&v("0.2.3"), "windows-x86_64"),
            "swarmbotix-0.2.3-windows-x86_64"
        );
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn arch_of_this_build() {
        assert_eq!(arch().unwrap(), "linux-x86_64");
    }

    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    #[test]
    fn arch_of_this_build() {
        assert_eq!(arch().unwrap(), "linux-aarch64");
    }

    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    #[test]
    fn arch_of_this_build() {
        assert_eq!(arch().unwrap(), "windows-x86_64");
    }
}
