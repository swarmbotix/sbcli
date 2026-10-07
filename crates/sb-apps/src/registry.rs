//! `<sb_home>/apps.yml`: which apps are installed and where they live.
//!
//! ```yaml
//! apps:
//!   camcalib:
//!     path: /home/el/dev/sb_kalibr      # package root
//!     source: path                      # path | git
//!     url: null                         # git only
//!     git_ref: null                     # git only: the ref the user asked for
//!     commit: null                      # git only: HEAD after clone / pull
//!     version: 0.1.0                    # from the manifest at install time
//!     installed_at: 2026-10-07T12:00:00Z
//! ```
//!
//! Ownership rule: a `path` under `<sb_home>/apps/` is a clone `sb` made and
//! may delete on `sb app remove`; any other path belongs to the user and is
//! never deleted.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::util::unique_suffix;

/// File name of the registry inside `sb_home`.
pub const REGISTRY_FILE: &str = "apps.yml";

/// Directory under `sb_home` that holds the clones `sb` owns.
pub const APPS_DIR: &str = "apps";

const HEADER: &str = "\
# Installed sb apps. Managed by `sb install` and `sb app update|remove`;
# edit with care. `path` under <sb_home>/apps/ is a clone sb owns.
";

/// Where an installed app came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    /// A local folder, registered in place.
    Path,
    /// A git repository cloned under `<sb_home>/apps/`.
    Git,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Path => "path",
            Self::Git => "git",
        }
    }
}

impl fmt::Display for SourceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One installed app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppEntry {
    /// Absolute package root.
    pub path: PathBuf,
    pub source: SourceKind,
    /// Clone URL (git only).
    #[serde(default)]
    pub url: Option<String>,
    /// Branch or tag the user asked for with `<url>@<ref>` (git only).
    #[serde(default)]
    pub git_ref: Option<String>,
    /// `HEAD` after the last clone or pull (git only).
    #[serde(default)]
    pub commit: Option<String>,
    /// Manifest `version` at the last install or update.
    pub version: String,
    /// RFC 3339 UTC time of the last install or update.
    pub installed_at: String,
}

/// The parsed `apps.yml`. Keys are app names, kept sorted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default)]
    apps: BTreeMap<String, AppEntry>,
}

impl Registry {
    /// `<home>/apps.yml`.
    pub fn path(home: &Path) -> PathBuf {
        home.join(REGISTRY_FILE)
    }

    /// Read the registry. A missing or empty file is an empty registry.
    pub fn load(home: &Path) -> Result<Self> {
        let path = Self::path(home);
        if !path.exists() {
            return Ok(Self::default());
        }
        let body =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        if body.trim().is_empty() {
            return Ok(Self::default());
        }
        serde_yaml::from_str(&body).with_context(|| format!("parsing {}", path.display()))
    }

    /// Write the registry, creating `home` if needed. The write goes to a
    /// sibling temp file first and is renamed into place, so a crash never
    /// leaves a half-written registry.
    pub fn save(&self, home: &Path) -> Result<()> {
        fs::create_dir_all(home).with_context(|| format!("creating {}", home.display()))?;
        let path = Self::path(home);
        let body = format!(
            "{HEADER}{}",
            serde_yaml::to_string(self).context("serializing apps.yml")?
        );
        let tmp = home.join(format!(".{REGISTRY_FILE}.{}", unique_suffix()));
        fs::write(&tmp, body).with_context(|| format!("writing {}", tmp.display()))?;
        if let Err(e) = fs::rename(&tmp, &path) {
            let _ = fs::remove_file(&tmp);
            return Err(e).with_context(|| format!("replacing {}", path.display()));
        }
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&AppEntry> {
        self.apps.get(name)
    }

    /// Insert or replace `name`, returning the previous entry.
    pub fn insert(&mut self, name: impl Into<String>, entry: AppEntry) -> Option<AppEntry> {
        self.apps.insert(name.into(), entry)
    }

    pub fn remove(&mut self, name: &str) -> Option<AppEntry> {
        self.apps.remove(name)
    }

    /// Entries in name order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &AppEntry)> {
        self.apps.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Installed app names, sorted.
    pub fn names(&self) -> Vec<&str> {
        self.apps.keys().map(String::as_str).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.apps.is_empty()
    }

    pub fn len(&self) -> usize {
        self.apps.len()
    }

    /// The git-installed entry cloned from `url`, if any.
    pub fn find_git_url(&self, url: &str) -> Option<(&str, &AppEntry)> {
        self.iter()
            .find(|(_, e)| e.source == SourceKind::Git && e.url.as_deref() == Some(url))
    }
}

/// `<home>/apps/`, the folder holding the clones `sb` owns.
pub fn apps_dir(home: &Path) -> PathBuf {
    home.join(APPS_DIR)
}

/// True if `path` is strictly inside `<home>/apps/`, i.e. a clone `sb` made
/// and may delete. Paths are compared both as given and canonicalized, so a
/// symlinked `sb_home` still matches.
pub fn is_owned_clone(home: &Path, path: &Path) -> bool {
    let apps = apps_dir(home);
    let inside = |root: &Path, p: &Path| p.starts_with(root) && p != root;
    if inside(&apps, path) {
        return true;
    }
    match (apps.canonicalize(), path.canonicalize()) {
        (Ok(a), Ok(p)) => inside(&a, &p),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn entry(path: &Path, source: SourceKind) -> AppEntry {
        AppEntry {
            path: path.to_path_buf(),
            source,
            url: None,
            git_ref: None,
            commit: None,
            version: "0.1.0".into(),
            installed_at: "2026-10-07T12:00:00Z".into(),
        }
    }

    #[test]
    fn missing_or_empty_file_is_empty_registry() {
        let home = TempDir::new().unwrap();
        assert!(Registry::load(home.path()).unwrap().is_empty());
        fs::write(Registry::path(home.path()), "\n").unwrap();
        assert!(Registry::load(home.path()).unwrap().is_empty());
    }

    #[test]
    fn save_load_remove_round_trip() {
        let home = TempDir::new().unwrap();
        let mut reg = Registry::default();
        reg.insert("zeta", entry(Path::new("/opt/zeta"), SourceKind::Path));
        let mut git = entry(Path::new("/h/apps/alpha"), SourceKind::Git);
        git.url = Some("https://example.com/alpha.git".into());
        git.git_ref = Some("v1".into());
        git.commit = Some("abc123".into());
        reg.insert("alpha", git.clone());
        reg.save(home.path()).unwrap();

        let body = fs::read_to_string(Registry::path(home.path())).unwrap();
        assert!(body.starts_with("# Installed sb apps."), "{body}");
        assert!(
            body.contains("source: git") && body.contains("url: null"),
            "{body}"
        );

        let mut back = Registry::load(home.path()).unwrap();
        assert_eq!(back, reg);
        assert_eq!(back.names(), vec!["alpha", "zeta"]);
        assert_eq!(back.get("alpha"), Some(&git));
        assert_eq!(
            back.find_git_url("https://example.com/alpha.git")
                .map(|(n, _)| n),
            Some("alpha")
        );

        assert!(back.remove("zeta").is_some());
        assert!(back.remove("zeta").is_none());
        back.save(home.path()).unwrap();
        let again = Registry::load(home.path()).unwrap();
        assert_eq!(again.len(), 1);
        assert!(again.get("zeta").is_none());
        let leftovers: Vec<_> = fs::read_dir(home.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with(".apps.yml."))
            .collect();
        assert!(leftovers.is_empty(), "temp file left behind");
    }

    #[test]
    fn ownership_rule() {
        let home = TempDir::new().unwrap();
        let owned = apps_dir(home.path()).join("camcalib");
        fs::create_dir_all(&owned).unwrap();
        assert!(is_owned_clone(home.path(), &owned));
        assert!(!is_owned_clone(home.path(), &apps_dir(home.path())));
        assert!(!is_owned_clone(home.path(), home.path()));
        assert!(!is_owned_clone(
            home.path(),
            Path::new("/home/el/dev/sb_kalibr")
        ));
    }
}
