//! Where releases come from: GitHub, or a local folder standing in for it.
//!
//! The latest release is found through `<repo>/releases/latest`, which GitHub
//! answers with a redirect to `<repo>/releases/tag/vX.Y.Z`. Reading that
//! redirect needs no API token and is not rate limited, unlike the REST API.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use semver::Version;

/// The public sbcli repository, home of every release.
pub const DEFAULT_BASE: &str = "https://github.com/swarmbotix/sbcli";

/// Environment variable that replaces [`DEFAULT_BASE`]: another repository
/// URL (a fork, for manual testing), or `file:///<dir>` for a [`DirFetcher`].
pub const BASE_ENV: &str = "SB_RELEASE_BASE";

/// A source of `sb` releases.
pub trait Fetcher {
    /// Tag of the newest release, e.g. `v0.2.3`.
    fn latest_tag(&self) -> Result<String>;

    /// Where release asset `asset` of `version` is downloaded from. This is
    /// also what `sb update --check` shows the user.
    fn asset_url(&self, version: &Version, asset: &str) -> String;

    /// Save the asset at `url_or_name` (as returned by
    /// [`Fetcher::asset_url`]) to the file `dest`.
    fn download(&self, url_or_name: &str, dest: &Path) -> Result<()>;
}

/// The fetcher `SB_RELEASE_BASE` asks for: a [`DirFetcher`] for a
/// `file://` URL, a [`GithubFetcher`] for anything else, and the public
/// repository when it is unset or empty.
pub fn from_env() -> Result<Box<dyn Fetcher>> {
    let base = std::env::var(BASE_ENV).unwrap_or_default();
    let base = base.trim();
    if base.is_empty() {
        return Ok(Box::new(GithubFetcher::default()));
    }
    if let Some(dir) = dir_from_file_url(base) {
        let fetcher = DirFetcher::open(dir).with_context(|| format!("{BASE_ENV}={base}"))?;
        return Ok(Box::new(fetcher));
    }
    Ok(Box::new(GithubFetcher::new(base)))
}

/// Releases of a GitHub repository, over HTTPS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubFetcher {
    /// Repository URL without a trailing `/`, e.g. [`DEFAULT_BASE`].
    pub base: String,
    /// Connect timeout for [`Fetcher::latest_tag`].
    pub probe_connect: Duration,
    /// Read timeout for [`Fetcher::latest_tag`].
    pub probe_read: Duration,
    /// Connect timeout for [`Fetcher::download`]. Reads have no timeout:
    /// an asset is tens of megabytes and is streamed to disk.
    pub download_connect: Duration,
}

impl GithubFetcher {
    /// A fetcher for the repository at `base`, with short probe timeouts
    /// (2 s connect, 2 s read) so `sb --version` never hangs on a dead
    /// network, and a 30 s connect timeout for downloads.
    pub fn new(base: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_owned(),
            probe_connect: Duration::from_secs(2),
            probe_read: Duration::from_secs(2),
            download_connect: Duration::from_secs(30),
        }
    }
}

impl Default for GithubFetcher {
    fn default() -> Self {
        Self::new(DEFAULT_BASE)
    }
}

impl Fetcher for GithubFetcher {
    fn latest_tag(&self) -> Result<String> {
        let url = format!("{}/releases/latest", self.base);
        let agent = ureq::AgentBuilder::new()
            .redirects(0)
            .timeout_connect(self.probe_connect)
            .timeout_read(self.probe_read)
            .user_agent(&user_agent())
            .build();
        // With redirects off, ureq hands back a 3xx as `Ok`; only >= 400 is `Err`.
        let resp = agent.get(&url).call().map_err(|e| http_error(&url, e))?;
        let status = resp.status();
        let location = resp.header("location").ok_or_else(|| {
            anyhow!("GET {url}: expected a redirect to the latest release, got HTTP {status}")
        })?;
        tag_from_location(location).ok_or_else(|| {
            anyhow!("GET {url}: redirected to {location}, which does not name a release tag")
        })
    }

    fn asset_url(&self, version: &Version, asset: &str) -> String {
        format!("{}/releases/download/v{version}/{asset}", self.base)
    }

    fn download(&self, url_or_name: &str, dest: &Path) -> Result<()> {
        if !url_or_name.contains("://") {
            bail!("cannot download `{url_or_name}`: not a URL");
        }
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(self.download_connect)
            .user_agent(&user_agent())
            .build();
        let resp = agent
            .get(url_or_name)
            .call()
            .map_err(|e| http_error(url_or_name, e))?;
        save_stream(resp.into_reader(), dest).with_context(|| format!("downloading {url_or_name}"))
    }
}

/// Releases laid out flat in a local folder, for tests and offline trials:
/// `<dir>/latest.txt` holds the latest version (`0.2.3` or `v0.2.3`) and
/// every asset sits directly in `<dir>` under its release file name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirFetcher {
    pub dir: PathBuf,
    /// The latest version, as written in `latest.txt`.
    pub latest: String,
}

impl DirFetcher {
    /// Read `<dir>/latest.txt`.
    pub fn open(dir: PathBuf) -> Result<Self> {
        let file = dir.join("latest.txt");
        let latest = fs::read_to_string(&file)
            .with_context(|| format!("reading {}", file.display()))?
            .trim()
            .to_owned();
        Ok(Self { dir, latest })
    }
}

impl Fetcher for DirFetcher {
    fn latest_tag(&self) -> Result<String> {
        let latest = self.latest.trim();
        if latest.is_empty() {
            bail!("{} names no version", self.dir.join("latest.txt").display());
        }
        Ok(if latest.starts_with('v') {
            latest.to_owned()
        } else {
            format!("v{latest}")
        })
    }

    fn asset_url(&self, _version: &Version, asset: &str) -> String {
        let dir = self.dir.display().to_string().replace('\\', "/");
        let slash = if dir.starts_with('/') { "" } else { "/" };
        format!("file://{slash}{}/{asset}", dir.trim_end_matches('/'))
    }

    fn download(&self, url_or_name: &str, dest: &Path) -> Result<()> {
        let name = url_or_name
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(url_or_name);
        let src = self.dir.join(name);
        fs::copy(&src, dest)
            .with_context(|| format!("copying {} to {}", src.display(), dest.display()))?;
        Ok(())
    }
}

fn user_agent() -> String {
    format!("sb/{}", env!("CARGO_PKG_VERSION"))
}

fn http_error(url: &str, err: ureq::Error) -> anyhow::Error {
    match err {
        ureq::Error::Status(code, _) => anyhow!("GET {url}: HTTP {code}"),
        // ureq already starts the message with the URL when it knows it.
        ureq::Error::Transport(t) if t.url().is_some() => anyhow!("GET {t}"),
        ureq::Error::Transport(t) => anyhow!("GET {url}: {t}"),
    }
}

/// Copy `reader` into a new file at `dest`, removing the partial file if
/// the copy fails midway.
fn save_stream(mut reader: impl Read, dest: &Path) -> Result<()> {
    let mut file = File::create(dest).with_context(|| format!("creating {}", dest.display()))?;
    if let Err(e) = io::copy(&mut reader, &mut file) {
        drop(file);
        let _ = fs::remove_file(dest);
        return Err(e).with_context(|| format!("writing {}", dest.display()));
    }
    Ok(())
}

/// The release tag at the end of a `.../releases/tag/v0.2.3` redirect.
fn tag_from_location(location: &str) -> Option<String> {
    let path = location.split(['?', '#']).next()?;
    let tag = path.trim_end_matches('/').rsplit('/').next()?;
    (tag.len() > 1 && tag.starts_with('v')).then(|| tag.to_owned())
}

/// The folder a `file://` URL names: `file:///tmp/rel` is `/tmp/rel` and
/// `file:///C:/rel` is `C:/rel`. The path is taken literally, without
/// percent-decoding. `None` for any other scheme.
fn dir_from_file_url(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let bytes = rest.as_bytes();
    let has_drive = bytes.len() >= 3 && bytes[0] == b'/' && bytes[2] == b':';
    Some(PathBuf::from(if has_drive { &rest[1..] } else { rest }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn tag_is_the_last_segment_of_the_redirect() {
        let loc = "https://github.com/swarmbotix/sbcli/releases/tag/v0.1.41";
        assert_eq!(tag_from_location(loc).as_deref(), Some("v0.1.41"));
        assert_eq!(
            tag_from_location("/swarmbotix/sbcli/releases/tag/v0.2.0/?x=1#y").as_deref(),
            Some("v0.2.0")
        );
        // GitHub sends a repo with no releases to `.../releases`.
        assert_eq!(
            tag_from_location("https://github.com/swarmbotix/sbcli/releases"),
            None
        );
        assert_eq!(
            tag_from_location("https://github.com/x/y/releases/tag/v"),
            None
        );
    }

    #[test]
    fn file_urls_name_a_folder() {
        assert_eq!(
            dir_from_file_url("file:///tmp/rel"),
            Some(PathBuf::from("/tmp/rel"))
        );
        assert_eq!(
            dir_from_file_url("file://localhost/tmp/rel"),
            Some(PathBuf::from("/tmp/rel"))
        );
        assert_eq!(
            dir_from_file_url("file:///C:/rel"),
            Some(PathBuf::from("C:/rel"))
        );
        assert_eq!(dir_from_file_url("https://github.com/x/y"), None);
    }

    #[test]
    fn github_urls() {
        let f = GithubFetcher::new("https://github.com/swarmbotix/sbcli/");
        assert_eq!(f.base, DEFAULT_BASE);
        let v = Version::new(0, 2, 3);
        assert_eq!(
            f.asset_url(&v, "swarmbotix-0.2.3-linux-x86_64.zip"),
            "https://github.com/swarmbotix/sbcli/releases/download/v0.2.3/swarmbotix-0.2.3-linux-x86_64.zip"
        );
        assert!(
            f.download("swarmbotix-0.2.3-linux-x86_64.zip", Path::new("x"))
                .is_err()
        );
    }

    #[test]
    fn dir_fetcher_serves_assets_by_file_name() {
        let tmp = TempDir::new().unwrap();
        let rel = tmp.path().join("rel");
        fs::create_dir(&rel).unwrap();
        fs::write(rel.join("latest.txt"), "0.2.3\n").unwrap();
        fs::write(rel.join("a.zip"), b"zip bytes").unwrap();

        let f = DirFetcher::open(rel.clone()).unwrap();
        assert_eq!(f.latest_tag().unwrap(), "v0.2.3");
        let url = f.asset_url(&Version::new(0, 2, 3), "a.zip");
        assert!(url.starts_with("file:///"), "{url}");
        assert!(url.ends_with("/rel/a.zip"), "{url}");

        let dest = tmp.path().join("got.zip");
        f.download(&url, &dest).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"zip bytes");
        assert!(f.download("missing.zip", &dest).is_err());

        fs::write(rel.join("latest.txt"), "v0.2.4").unwrap();
        assert_eq!(
            DirFetcher::open(rel).unwrap().latest_tag().unwrap(),
            "v0.2.4"
        );
        assert!(DirFetcher::open(tmp.path().join("nowhere")).is_err());
    }
}
