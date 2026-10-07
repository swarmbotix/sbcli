//! `sb.app.yml`: the manifest at the root of every app package.
//!
//! ```yaml
//! name: camcalib            # `sb camcalib ...`
//! version: 0.1.0
//! description: Camera calibration in a box
//! entry: ./camcalib         # relative to this file's folder
//! kind: docker              # docker | host | none
//! docker:
//!   image: swarmbotix/sb_kalibr:latest
//!   prefetch: false
//! requires: [docker]
//! ```
//!
//! [`AppManifest::load`] parses; [`AppManifest::validate`] checks everything
//! that needs the package folder (entry and install script present, name
//! usable). Both report errors that name the offending field and the fix.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use sb_core::validate_identifier;

use crate::builtins::is_builtin;
use crate::util::join_relative;

/// File name of the manifest at a package root.
pub const MANIFEST_FILE: &str = "sb.app.yml";

/// What `sb install` does for a package beyond registering it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppKind {
    /// Pull `docker.image` (at install time when `docker.prefetch` is true).
    Docker,
    /// Run the `host.install` script at install and at every update.
    Host,
    /// Nothing beyond registration: the entry runs as shipped.
    #[default]
    None,
}

impl AppKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::Host => "host",
            Self::None => "none",
        }
    }
}

impl fmt::Display for AppKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The `docker:` section, used when `kind: docker`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DockerSpec {
    /// Image reference, e.g. `swarmbotix/sb_kalibr:latest`.
    pub image: String,
    /// `true`: `sb install` runs `docker pull <image>`. `false`: the app
    /// pulls on its first run (a plain `docker run` does that already).
    #[serde(default)]
    pub prefetch: bool,
}

/// The `host:` section, used when `kind: host`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostSpec {
    /// Script run with `bash` from the package root at `sb install` and at
    /// `sb app update`. Relative to the package folder.
    pub install: String,
}

/// A parsed `sb.app.yml`.
///
/// Unknown keys are rejected so a typo (`entrypoint:`) fails loudly instead
/// of being ignored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppManifest {
    /// Subcommand name: `sb <name> ...`.
    pub name: String,
    /// Package version, recorded in the registry at install time.
    pub version: String,
    /// One line shown by `sb app info`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Executable run with the user's args, from the user's cwd. Relative
    /// to the package folder.
    pub entry: String,
    #[serde(default)]
    pub kind: AppKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docker: Option<DockerSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<HostSpec>,
    /// Binaries that must be on `PATH`. Missing ones are warnings at install
    /// time and in `sb doctor`, never hard errors.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
}

impl AppManifest {
    /// Parse `<dir>/sb.app.yml`. Does not validate; call [`Self::validate`].
    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join(MANIFEST_FILE);
        if !path.is_file() {
            bail!(
                "no {MANIFEST_FILE} in {}; an app package needs one at its root \
                 (package authors create it with `sb app init`)",
                dir.display()
            );
        }
        let body =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&body).with_context(|| format!("parsing {}", path.display()))
    }

    /// Parse manifest text. Does not validate.
    pub fn parse(body: &str) -> Result<Self> {
        if body.trim().is_empty() {
            bail!("the manifest is empty; regenerate it with `sb app init --force`");
        }
        serde_yaml::from_str(body).map_err(|e| anyhow!("{e}"))
    }

    /// Check the manifest against the package folder `dir`.
    ///
    /// Errors name the field and the fix: the name must be an identifier
    /// that is not an `sb` built-in, `entry` must exist, and the section
    /// that `kind` selects must be present and complete.
    pub fn validate(&self, dir: &Path) -> Result<()> {
        validate_app_name(&self.name).map_err(|e| anyhow!("{MANIFEST_FILE} `name`: {e}"))?;
        if self.version.trim().is_empty() {
            bail!("{MANIFEST_FILE} `version` is empty; set it, e.g. `version: 0.1.0`");
        }
        let entry = self.entry_path(dir)?;
        if !entry.is_file() {
            bail!(
                "{MANIFEST_FILE} `entry: {}`: {} does not exist; create that file or \
                 point `entry` at your launcher script",
                self.entry,
                entry.display()
            );
        }
        match self.kind {
            AppKind::Docker => {
                let image = self.docker.as_ref().map_or("", |d| d.image.trim());
                if image.is_empty() {
                    bail!(
                        "{MANIFEST_FILE} has `kind: docker` but no `docker.image`; add\n  \
                         docker:\n    image: <repo>/<name>:<tag>\n    prefetch: false"
                    );
                }
            }
            AppKind::Host => {
                let script = self.install_script_path(dir)?.ok_or_else(|| {
                    anyhow!(
                        "{MANIFEST_FILE} has `kind: host` but no `host.install`; add\n  \
                         host:\n    install: ./install.bash"
                    )
                })?;
                if !script.is_file() {
                    bail!(
                        "{MANIFEST_FILE} `host.install`: {} does not exist; create it \
                         (`sb app init --kind host` writes a stub) or fix the path",
                        script.display()
                    );
                }
            }
            AppKind::None => {}
        }
        if let Some(bad) = self.requires.iter().find(|r| r.trim().is_empty()) {
            bail!(
                "{MANIFEST_FILE} `requires` has an empty entry {bad:?}; list binary names, e.g. `requires: [docker]`"
            );
        }
        Ok(())
    }

    /// Absolute path of `entry` inside the package folder `dir`.
    pub fn entry_path(&self, dir: &Path) -> Result<PathBuf> {
        join_relative(dir, &self.entry)
            .map_err(|why| anyhow!("{MANIFEST_FILE} `entry: {}` {why}", self.entry))
    }

    /// Absolute path of `host.install`, if the manifest has one.
    pub fn install_script_path(&self, dir: &Path) -> Result<Option<PathBuf>> {
        let Some(host) = &self.host else {
            return Ok(None);
        };
        if host.install.trim().is_empty() {
            return Ok(None);
        }
        join_relative(dir, &host.install)
            .map(Some)
            .map_err(|why| anyhow!("{MANIFEST_FILE} `host.install: {}` {why}", host.install))
    }
}

/// Check that `name` can be an app name: a valid identifier
/// (`[A-Za-z_][A-Za-z0-9_]*`) that is not an `sb` built-in.
///
/// The `Err` text is a complete sentence suitable for a prompt or error.
pub fn validate_app_name(name: &str) -> std::result::Result<(), String> {
    validate_identifier(name).map_err(|e| {
        format!("invalid app name {name:?}: {e}; use letters, digits, and `_` (e.g. `cam_calib`)")
    })?;
    if is_builtin(name) {
        return Err(format!(
            "{name:?} is a built-in sb command, so `sb {name}` could never reach the app; \
             pick another name"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const DOCKER_YML: &str = "\
name: camcalib
version: 0.1.0
description: Camera calibration in a box
entry: ./camcalib
kind: docker
docker:
  image: swarmbotix/sb_kalibr:latest
  prefetch: true
requires: [docker]
";

    fn pkg_with(files: &[&str]) -> TempDir {
        let tmp = TempDir::new().unwrap();
        for f in files {
            let p = tmp.path().join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, "#!/usr/bin/env bash\n").unwrap();
        }
        tmp
    }

    fn err_of(m: &AppManifest, dir: &Path) -> String {
        format!("{:#}", m.validate(dir).unwrap_err())
    }

    #[test]
    fn docker_manifest_round_trips() {
        let m = AppManifest::parse(DOCKER_YML).unwrap();
        assert_eq!(m.name, "camcalib");
        assert_eq!(m.kind, AppKind::Docker);
        assert_eq!(
            m.docker,
            Some(DockerSpec {
                image: "swarmbotix/sb_kalibr:latest".into(),
                prefetch: true
            })
        );
        assert_eq!(m.requires, vec!["docker".to_string()]);
        let again = AppManifest::parse(&serde_yaml::to_string(&m).unwrap()).unwrap();
        assert_eq!(again, m);

        let pkg = pkg_with(&["camcalib"]);
        fs::write(pkg.path().join(MANIFEST_FILE), DOCKER_YML).unwrap();
        let loaded = AppManifest::load(pkg.path()).unwrap();
        assert_eq!(loaded, m);
        loaded.validate(pkg.path()).unwrap();
    }

    #[test]
    fn defaults_apply_for_optional_fields() {
        let m = AppManifest::parse("name: tool\nversion: 1.0.0\nentry: run.bash\n").unwrap();
        assert_eq!(m.kind, AppKind::None);
        assert!(m.description.is_none() && m.docker.is_none() && m.host.is_none());
        assert!(m.requires.is_empty());
    }

    #[test]
    fn numeric_looking_versions_parse_as_strings() {
        let m = AppManifest::parse("name: t\nversion: 1.0\nentry: run.bash\n").unwrap();
        assert_eq!(m.version, "1.0");
        let m = AppManifest::parse("name: t\nversion: 2\nentry: run.bash\n").unwrap();
        assert_eq!(m.version, "2");
    }

    #[test]
    fn unknown_field_is_rejected() {
        let e = AppManifest::parse("name: t\nversion: 1\nentry: r\nentrypoint: x\n").unwrap_err();
        assert!(format!("{e:#}").contains("entrypoint"), "{e:#}");
    }

    #[test]
    fn missing_manifest_points_at_app_init() {
        let tmp = TempDir::new().unwrap();
        let e = format!("{:#}", AppManifest::load(tmp.path()).unwrap_err());
        assert!(e.contains("sb app init"), "{e}");
    }

    #[test]
    fn invalid_name_is_rejected() {
        let pkg = pkg_with(&["run.bash"]);
        let mut m = AppManifest::parse("name: t\nversion: 1\nentry: run.bash\n").unwrap();
        m.name = "cam-calib".into();
        let e = err_of(&m, pkg.path());
        assert!(
            e.contains("`name`") && e.contains("invalid app name"),
            "{e}"
        );
    }

    #[test]
    fn builtin_name_is_rejected() {
        let pkg = pkg_with(&["run.bash"]);
        let m = AppManifest::parse("name: doctor\nversion: 1\nentry: run.bash\n").unwrap();
        let e = err_of(&m, pkg.path());
        assert!(e.contains("built-in"), "{e}");
    }

    #[test]
    fn missing_entry_is_rejected() {
        let pkg = pkg_with(&[]);
        let m = AppManifest::parse("name: t\nversion: 1\nentry: ./nope.bash\n").unwrap();
        let e = err_of(&m, pkg.path());
        assert!(
            e.contains("`entry: ./nope.bash`") && e.contains("does not exist"),
            "{e}"
        );
    }

    #[test]
    fn escaping_entry_is_rejected() {
        let pkg = pkg_with(&[]);
        let m = AppManifest::parse("name: t\nversion: 1\nentry: ../outside\n").unwrap();
        let e = err_of(&m, pkg.path());
        assert!(e.contains("inside the package folder"), "{e}");
    }

    #[test]
    fn empty_version_is_rejected() {
        let pkg = pkg_with(&["run.bash"]);
        let m = AppManifest::parse("name: t\nversion: ''\nentry: run.bash\n").unwrap();
        assert!(err_of(&m, pkg.path()).contains("`version`"));
    }

    #[test]
    fn docker_kind_needs_image() {
        let pkg = pkg_with(&["run.bash"]);
        let m = AppManifest::parse("name: t\nversion: 1\nentry: run.bash\nkind: docker\n").unwrap();
        let e = err_of(&m, pkg.path());
        assert!(e.contains("docker.image"), "{e}");
    }

    #[test]
    fn host_kind_needs_install_section_and_file() {
        let pkg = pkg_with(&["run.bash"]);
        let m = AppManifest::parse("name: t\nversion: 1\nentry: run.bash\nkind: host\n").unwrap();
        assert!(err_of(&m, pkg.path()).contains("host.install"));

        let m = AppManifest::parse(
            "name: t\nversion: 1\nentry: run.bash\nkind: host\nhost:\n  install: ./install.bash\n",
        )
        .unwrap();
        let e = err_of(&m, pkg.path());
        assert!(
            e.contains("`host.install`") && e.contains("does not exist"),
            "{e}"
        );

        fs::write(pkg.path().join("install.bash"), "exit 0\n").unwrap();
        m.validate(pkg.path()).unwrap();
    }

    #[test]
    fn empty_requires_entry_is_rejected() {
        let pkg = pkg_with(&["run.bash"]);
        let m =
            AppManifest::parse("name: t\nversion: 1\nentry: run.bash\nrequires: ['']\n").unwrap();
        assert!(err_of(&m, pkg.path()).contains("`requires`"));
    }
}
