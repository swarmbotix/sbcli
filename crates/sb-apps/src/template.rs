//! `sb app init`: write a complete, valid `sb.app.yml` plus script stubs.
//!
//! The generated manifest is commented field by field so a package author
//! never needs the docs. Only the section the chosen `kind` uses is written;
//! the header lists all three kinds so switching later is an edit, not a
//! lookup.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

use crate::manifest::{AppKind, AppManifest, MANIFEST_FILE, validate_app_name};
use crate::util::{canonical, join_relative, set_executable};

/// Column at which trailing `# ...` comments start in the manifest.
const COMMENT_COL: usize = 26;

/// File name of the host install stub written for `kind: host`.
pub const INSTALL_SCRIPT: &str = "install.bash";

/// Host install stub. Exits 0 as written: an untouched stub must not fail
/// `sb install`.
pub const INSTALL_BASH_STUB: &str = r#"#!/usr/bin/env bash
# Host install step of this app package. `sb install` runs it once, and
# `sb app update` runs it again after pulling, both from the package root.
# A non-zero exit aborts the install, so failures surface instead of
# leaving a half-installed app.
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"

# install host dependencies here (pip install, apt, cargo build...)
# python3 -m pip install --user -r requirements.txt
# cargo build --release

exit 0
"#;

/// Entry stub, written only when the entry file does not exist yet.
/// Placeholders: `{{NAME}}`, `{{ENTRY}}`, `{{EXAMPLES}}`.
pub const ENTRY_BASH_STUB: &str = r#"#!/usr/bin/env bash
# Entry point of the `{{NAME}}` app: `sb {{NAME}} [args...]` runs this file.
# sb starts it in the user's current directory (not this folder) and passes
# every argument through unchanged. It also sets:
#   SB_APP_DIR   absolute path of this package (the folder with sb.app.yml)
#   SB_APP_NAME  {{NAME}}
#   SB_HOME      sb's home folder (~/.swarmbotix by default)
set -euo pipefail

# Replace the two TODO lines below with your command, and forward the
# user's arguments with "$@". For example:
{{EXAMPLES}}
echo "TODO: replace with your command (edit {{ENTRY}} in the {{NAME}} package)" >&2
exit 1
"#;

/// Options for [`init_app`]. Mirrors the `sb app init` flags.
#[derive(Debug, Clone, Default)]
pub struct InitAppOptions {
    /// App name; default [`default_name`] of the package folder.
    pub name: Option<String>,
    /// Entry path relative to the package; default [`default_entry`].
    pub entry: Option<String>,
    pub kind: AppKind,
    /// Image for `kind: docker` (required for that kind).
    pub image: Option<String>,
    pub description: Option<String>,
    /// Overwrite an existing `sb.app.yml`. Existing scripts are never
    /// overwritten.
    pub force: bool,
}

/// What [`init_app`] wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitAppOutcome {
    pub name: String,
    pub root_abs: PathBuf,
    pub kind: AppKind,
    /// Entry as written in the manifest (relative).
    pub entry: String,
    pub manifest_written: bool,
    pub entry_stub_written: bool,
    pub install_stub_written: bool,
}

/// Default app name for a package folder: its basename with every character
/// outside `[A-Za-z0-9_]` replaced by `_` (and a leading `_` if it would
/// start with a digit), so `sb-kalibr/` becomes `sb_kalibr`.
pub fn default_name(dir: &Path) -> Result<String> {
    let abs = canonical(dir).with_context(|| format!("resolving {}", dir.display()))?;
    let base = abs
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow!("{} has no usable folder name; pass --name", abs.display()))?;
    let mut name: String = base
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if name.starts_with(|c: char| c.is_ascii_digit()) {
        name.insert(0, '_');
    }
    Ok(name)
}

/// Default entry: `./<name>` when that file exists, else `./run.bash`.
pub fn default_entry(dir: &Path, name: &str) -> String {
    if dir.join(name).is_file() {
        format!("./{name}")
    } else {
        "./run.bash".to_owned()
    }
}

/// Render a commented `sb.app.yml`. Values are YAML-quoted as needed.
pub fn render_manifest(
    name: &str,
    entry: &str,
    kind: AppKind,
    image: Option<&str>,
    description: Option<&str>,
) -> String {
    let mut out = String::from(
        "# sb.app.yml: swarmbotix app package manifest. Written by `sb app init`.\n\
         # `kind` picks what `sb install` does besides registering the app:\n\
         #   none    nothing; `entry` runs as shipped\n\
         #   docker  `docker pull` docker.image (at install if prefetch: true, else on first run)\n\
         #   host    run host.install from the package root, at install and at `sb app update`\n",
    );
    out.push_str(&commented(
        &format!("name: {}", scalar(name)),
        &format!(
            "subcommand: `sb {name} ...`. Letters, digits, underscore; must not be an sb builtin."
        ),
    ));
    out.push_str("version: 0.1.0\n");
    match description.map(|d| d.replace(['\n', '\r'], " ")) {
        Some(d) if !d.trim().is_empty() => {
            out.push_str("description: ");
            out.push_str(&scalar(d.trim()));
            out.push('\n');
        }
        _ => out.push_str("# description: one line shown by `sb app info`\n"),
    }
    out.push_str(&commented(
        &format!("entry: {}", scalar(entry)),
        "run with the user's args, from the user's cwd. Relative to this file's folder.",
    ));
    out.push_str(&commented(
        &format!("kind: {}", kind.as_str()),
        "docker | host | none",
    ));
    let requires = match kind {
        AppKind::Docker => {
            out.push_str(&commented("docker:", "only for kind: docker"));
            out.push_str("  image: ");
            out.push_str(&scalar(image.unwrap_or_default()));
            out.push('\n');
            out.push_str(&commented(
                "  prefetch: false",
                "true = pull at `sb install`; false = app pulls on first run",
            ));
            "[docker]"
        }
        AppKind::Host => {
            out.push_str(&commented("host:", "only for kind: host"));
            out.push_str(&commented(
                &format!("  install: ./{INSTALL_SCRIPT}"),
                "run once at `sb install` and again at `sb app update`, cwd = package root",
            ));
            "[]"
        }
        AppKind::None => "[]",
    };
    out.push_str(&commented(
        &format!("requires: {requires}"),
        "binaries that must be on PATH; `sb doctor` checks them",
    ));
    out
}

/// The entry stub for `name`, with examples suited to `kind`.
pub fn render_entry_stub(name: &str, entry: &str, kind: AppKind, image: Option<&str>) -> String {
    let examples = match (kind, image) {
        (AppKind::Docker, Some(img)) => {
            format!("#   exec docker run --rm -it -v \"$PWD:$PWD\" -w \"$PWD\" {img} \"$@\"\n")
        }
        _ => "#   exec python3 \"$SB_APP_DIR/main.py\" \"$@\"\n\
              #   exec \"$SB_APP_DIR/build/app\" \"$@\"\n"
            .to_owned(),
    };
    ENTRY_BASH_STUB
        .replace("{{EXAMPLES}}\n", &examples)
        .replace("{{NAME}}", name)
        .replace("{{ENTRY}}", entry)
}

/// `sb app init`: turn `dir` into an app package.
///
/// Writes `sb.app.yml`, an entry stub when the entry file does not exist
/// yet, and `install.bash` for `kind: host` when missing. Refuses to replace
/// an existing `sb.app.yml` unless `opts.force`; never replaces an existing
/// script. The rendered manifest is validated before it is written and
/// re-loaded afterwards, so this never leaves an invalid manifest behind.
pub fn init_app(dir: &Path, opts: &InitAppOptions) -> Result<InitAppOutcome> {
    if !dir.is_dir() {
        bail!("{} is not an existing folder", dir.display());
    }
    let root = canonical(dir).with_context(|| format!("resolving {}", dir.display()))?;
    let manifest_path = root.join(MANIFEST_FILE);
    if manifest_path.exists() && !opts.force {
        bail!(
            "{} already exists; pass --force to overwrite it (scripts are never overwritten)",
            manifest_path.display()
        );
    }

    let name = match &opts.name {
        Some(n) => n.trim().to_owned(),
        None => default_name(&root)?,
    };
    validate_app_name(&name).map_err(|e| {
        let hint = if opts.name.is_none() {
            " (derived from the folder name; pass --name <name>)"
        } else {
            ""
        };
        anyhow!("{e}{hint}")
    })?;
    let entry = opts
        .entry
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map_or_else(|| default_entry(&root, &name), str::to_owned);
    let entry_abs =
        join_relative(&root, &entry).map_err(|why| anyhow!("--entry {entry:?} {why}"))?;
    if entry_abs.is_dir() {
        bail!("--entry {entry:?} is a folder; point it at the file to run");
    }
    let image = opts
        .image
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if opts.kind == AppKind::Docker && image.is_none() {
        bail!("--kind docker needs --image <repo>/<name>:<tag>, the image the app runs");
    }

    let mut outcome = InitAppOutcome {
        name: name.clone(),
        root_abs: root.clone(),
        kind: opts.kind,
        entry: entry.clone(),
        manifest_written: false,
        entry_stub_written: false,
        install_stub_written: false,
    };

    if !entry_abs.exists() {
        if let Some(parent) = entry_abs.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        write_script(
            &entry_abs,
            &render_entry_stub(&name, &entry, opts.kind, image),
        )?;
        outcome.entry_stub_written = true;
    }
    if opts.kind == AppKind::Host {
        let install = root.join(INSTALL_SCRIPT);
        if !install.exists() {
            write_script(&install, INSTALL_BASH_STUB)?;
            outcome.install_stub_written = true;
        }
    }

    let body = render_manifest(&name, &entry, opts.kind, image, opts.description.as_deref());
    AppManifest::parse(&body)
        .context("internal error: the rendered sb.app.yml does not parse")?
        .validate(&root)?;
    fs::write(&manifest_path, &body)
        .with_context(|| format!("writing {}", manifest_path.display()))?;
    AppManifest::load(&root)?.validate(&root)?;
    outcome.manifest_written = true;
    Ok(outcome)
}

fn write_script(path: &Path, body: &str) -> Result<()> {
    fs::write(path, body).with_context(|| format!("writing {}", path.display()))?;
    set_executable(path)
}

/// `key: value` padded to [`COMMENT_COL`], then `# comment`.
fn commented(line: &str, comment: &str) -> String {
    format!("{line:<COMMENT_COL$} # {comment}\n")
}

/// `s` as a YAML scalar, quoted only when YAML needs it.
fn scalar(s: &str) -> String {
    serde_yaml::to_string(s).map_or_else(|_| format!("{s:?}"), |y| y.trim_end().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn stage(dir: &Path, files: &[&str]) {
        for f in files {
            fs::write(dir.join(f), "#!/usr/bin/env bash\nexit 0\n").unwrap();
        }
    }

    #[test]
    fn renders_each_kind_as_a_valid_manifest() {
        let tmp = TempDir::new().unwrap();
        stage(tmp.path(), &["run.bash", INSTALL_SCRIPT]);
        for kind in [AppKind::None, AppKind::Docker, AppKind::Host] {
            let body = render_manifest(
                "camcalib",
                "./run.bash",
                kind,
                Some("swarmbotix/sb_kalibr:latest"),
                Some("Camera calibration in a box"),
            );
            assert!(body.starts_with("# sb.app.yml"), "{body}");
            for k in ["none", "docker", "host"] {
                assert!(
                    body.contains(&format!("#   {k} ")),
                    "header lists {k}:\n{body}"
                );
            }
            assert_eq!(
                body.contains("\ndocker:"),
                kind == AppKind::Docker,
                "{body}"
            );
            assert_eq!(body.contains("\nhost:"), kind == AppKind::Host, "{body}");

            let m = AppManifest::parse(&body).unwrap();
            m.validate(tmp.path()).unwrap();
            assert_eq!(m.kind, kind);
            assert_eq!(m.name, "camcalib");
            assert_eq!(m.version, "0.1.0");
            assert_eq!(
                m.description.as_deref(),
                Some("Camera calibration in a box")
            );
            match kind {
                AppKind::Docker => {
                    let d = m.docker.unwrap();
                    assert_eq!(d.image, "swarmbotix/sb_kalibr:latest");
                    assert!(!d.prefetch);
                    assert_eq!(m.requires, vec!["docker".to_string()]);
                }
                AppKind::Host => {
                    assert_eq!(m.host.unwrap().install, "./install.bash");
                    assert!(m.requires.is_empty());
                }
                AppKind::None => assert!(m.docker.is_none() && m.host.is_none()),
            }
        }
    }

    #[test]
    fn yaml_special_characters_survive() {
        let body = render_manifest(
            "t",
            "./run.bash",
            AppKind::None,
            None,
            Some("calib: fast # really\nsecond line"),
        );
        let m = AppManifest::parse(&body).unwrap();
        assert_eq!(
            m.description.as_deref(),
            Some("calib: fast # really second line")
        );
        let body = render_manifest("t", "./run.bash", AppKind::None, None, None);
        assert!(AppManifest::parse(&body).unwrap().description.is_none());
    }

    #[test]
    fn default_name_sanitizes_folder_names() {
        let tmp = TempDir::new().unwrap();
        for (folder, expect) in [
            ("sb-kalibr", "sb_kalibr"),
            ("9lives", "_9lives"),
            ("ok_1", "ok_1"),
        ] {
            let d = tmp.path().join(folder);
            fs::create_dir_all(&d).unwrap();
            assert_eq!(default_name(&d).unwrap(), expect);
        }
    }

    #[cfg(unix)]
    fn mode(p: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn init_none_writes_manifest_and_entry_stub() {
        let tmp = TempDir::new().unwrap();
        let pkg = tmp.path().join("my-pkg");
        fs::create_dir_all(&pkg).unwrap();
        let out = init_app(&pkg, &InitAppOptions::default()).unwrap();
        assert_eq!(out.name, "my_pkg");
        assert_eq!(out.entry, "./run.bash");
        assert!(out.manifest_written && out.entry_stub_written && !out.install_stub_written);
        assert!(!pkg.join(INSTALL_SCRIPT).exists());
        let stub = fs::read_to_string(pkg.join("run.bash")).unwrap();
        assert!(stub.starts_with("#!/usr/bin/env bash\n"), "{stub}");
        assert!(stub.contains("TODO: replace with your command") && stub.contains("\"$@\""));
        assert!(stub.contains("exit 1"));
        #[cfg(unix)]
        assert_eq!(mode(&pkg.join("run.bash")), 0o755);
        let m = AppManifest::load(&pkg).unwrap();
        assert_eq!((m.name.as_str(), m.kind), ("my_pkg", AppKind::None));
    }

    #[test]
    fn init_host_writes_install_stub() {
        let tmp = TempDir::new().unwrap();
        let opts = InitAppOptions {
            name: Some("hosty".into()),
            kind: AppKind::Host,
            ..InitAppOptions::default()
        };
        let out = init_app(tmp.path(), &opts).unwrap();
        assert!(out.install_stub_written && out.entry_stub_written);
        let install = tmp.path().join(INSTALL_SCRIPT);
        let body = fs::read_to_string(&install).unwrap();
        assert!(body.contains("set -euo pipefail") && body.trim_end().ends_with("exit 0"));
        assert!(body.contains("install host dependencies here"));
        #[cfg(unix)]
        assert_eq!(mode(&install), 0o755);
        let m = AppManifest::load(tmp.path()).unwrap();
        assert_eq!(m.host.unwrap().install, "./install.bash");
    }

    #[test]
    fn init_docker_needs_image_and_uses_it() {
        let tmp = TempDir::new().unwrap();
        let mut opts = InitAppOptions {
            name: Some("dock".into()),
            kind: AppKind::Docker,
            ..InitAppOptions::default()
        };
        let e = init_app(tmp.path(), &opts).unwrap_err();
        assert!(format!("{e}").contains("--image"), "{e}");
        assert!(!tmp.path().join(MANIFEST_FILE).exists());

        opts.image = Some("org/img:1".into());
        init_app(tmp.path(), &opts).unwrap();
        let m = AppManifest::load(tmp.path()).unwrap();
        assert_eq!(m.docker.unwrap().image, "org/img:1");
        let stub = fs::read_to_string(tmp.path().join("run.bash")).unwrap();
        assert!(stub.contains("org/img:1"), "{stub}");
    }

    #[test]
    fn existing_entry_named_after_app_is_the_default_and_kept() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("camcalib"), "#!/bin/sh\necho mine\n").unwrap();
        let opts = InitAppOptions {
            name: Some("camcalib".into()),
            ..InitAppOptions::default()
        };
        let out = init_app(tmp.path(), &opts).unwrap();
        assert_eq!(out.entry, "./camcalib");
        assert!(!out.entry_stub_written);
        assert_eq!(
            fs::read_to_string(tmp.path().join("camcalib")).unwrap(),
            "#!/bin/sh\necho mine\n"
        );
    }

    #[test]
    fn refuses_overwrite_without_force_and_keeps_scripts_with_force() {
        let tmp = TempDir::new().unwrap();
        let opts = InitAppOptions {
            name: Some("again".into()),
            ..InitAppOptions::default()
        };
        init_app(tmp.path(), &opts).unwrap();
        fs::write(tmp.path().join("run.bash"), "#!/bin/sh\necho edited\n").unwrap();
        let e = init_app(tmp.path(), &opts).unwrap_err();
        assert!(format!("{e}").contains("--force"), "{e}");

        let forced = InitAppOptions {
            force: true,
            description: Some("now described".into()),
            ..opts
        };
        let out = init_app(tmp.path(), &forced).unwrap();
        assert!(out.manifest_written && !out.entry_stub_written);
        assert_eq!(
            fs::read_to_string(tmp.path().join("run.bash")).unwrap(),
            "#!/bin/sh\necho edited\n"
        );
        let m = AppManifest::load(tmp.path()).unwrap();
        assert_eq!(m.description.as_deref(), Some("now described"));
    }

    #[test]
    fn builtin_or_invalid_name_writes_nothing() {
        let tmp = TempDir::new().unwrap();
        for bad in ["run", "bad-name"] {
            let opts = InitAppOptions {
                name: Some(bad.into()),
                ..InitAppOptions::default()
            };
            assert!(
                init_app(tmp.path(), &opts).is_err(),
                "{bad} must be rejected"
            );
        }
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 0);
    }

    #[test]
    fn entry_in_subfolder_is_created() {
        let tmp = TempDir::new().unwrap();
        let opts = InitAppOptions {
            name: Some("deep".into()),
            entry: Some("bin/deep.sh".into()),
            ..InitAppOptions::default()
        };
        init_app(tmp.path(), &opts).unwrap();
        assert!(tmp.path().join("bin").join("deep.sh").is_file());
        let e = init_app(
            tmp.path(),
            &InitAppOptions {
                entry: Some("../out.sh".into()),
                force: true,
                ..opts
            },
        )
        .unwrap_err();
        assert!(format!("{e}").contains("--entry"), "{e}");
    }

    #[cfg(unix)]
    #[test]
    fn stubs_behave_when_run() {
        if which::which("bash").is_err() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let opts = InitAppOptions {
            name: Some("stubby".into()),
            kind: AppKind::Host,
            ..InitAppOptions::default()
        };
        init_app(tmp.path(), &opts).unwrap();
        let entry = std::process::Command::new(tmp.path().join("run.bash"))
            .output()
            .unwrap();
        assert_eq!(entry.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&entry.stderr).contains("TODO: replace with your command"));
        let install = std::process::Command::new(tmp.path().join(INSTALL_SCRIPT))
            .output()
            .unwrap();
        assert!(install.status.success(), "{install:?}");
    }
}
