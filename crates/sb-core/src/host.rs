//! The machine `sb` is running on, as the manifest's `platforms:` block
//! names it.
//!
//! A platform key is `<os>` or `<os>-<arch>`. The general key merges
//! first and the specific one after it, so `linux-aarch64` can refine
//! what `linux` already says. Pure data: [`Host::current`] reads the
//! compile-time target constants, nothing else.

use std::fmt;

/// Operating system names a `platforms:` key may use. These are the
/// values of `std::env::consts::OS` for the hosts sb supports.
pub const KNOWN_OS: &[&str] = &["linux", "windows", "macos"];

/// Architecture names a `platforms:` `<os>-<arch>` key may use. These are
/// values of `std::env::consts::ARCH`.
pub const KNOWN_ARCH: &[&str] = &["x86_64", "aarch64"];

/// One host: operating system plus CPU architecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Host {
    /// `"linux"`, `"windows"`, `"macos"`, ... (`std::env::consts::OS`).
    pub os: &'static str,
    /// `"x86_64"`, `"aarch64"`, ... (`std::env::consts::ARCH`).
    pub arch: &'static str,
}

impl Host {
    /// The host this binary was built for, which is the host it runs on.
    pub fn current() -> Host {
        Host {
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
        }
    }

    /// The two `platforms:` keys that apply to this host, in merge order
    /// (general first): `["linux", "linux-x86_64"]`.
    pub fn platform_keys(&self) -> [String; 2] {
        [self.os.to_owned(), format!("{}-{}", self.os, self.arch)]
    }

    /// `".exe"` on Windows, `""` elsewhere. The value of `{exe}`.
    pub fn exe_suffix(&self) -> &'static str {
        if self.os == "windows" { ".exe" } else { "" }
    }
}

/// `<os>-<arch>`, e.g. `linux-x86_64`. The form error and doctor messages
/// use when they name a host.
impl fmt::Display for Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.os, self.arch)
    }
}

/// Whether `key` is a valid `platforms:` key: a known OS, optionally
/// followed by `-` and a known architecture.
pub(crate) fn is_valid_platform_key(key: &str) -> bool {
    match key.split_once('-') {
        None => KNOWN_OS.contains(&key),
        Some((os, arch)) => KNOWN_OS.contains(&os) && KNOWN_ARCH.contains(&arch),
    }
}
