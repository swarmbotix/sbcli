//! `sb install`, `sb app update|remove|list|info`.
//!
//! Install order: fetch (clone or locate) the package, load and validate its
//! manifest, check the name is free, run the kind's install steps, check
//! `requires`, and only then record it in the registry. A failure at any
//! step leaves the registry untouched.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use anyhow::{Context, Result, anyhow, bail};

use crate::manifest::{AppKind, AppManifest};
use crate::registry::{AppEntry, Registry, SourceKind, apps_dir, is_owned_clone};
use crate::source::{PullStatus, Source, classify, git_clone, git_head, git_pull, repo_dir_name};
use crate::util::{bash_path_arg, find_bash, rfc3339_utc, unique_suffix};

/// Options for [`install`].
#[derive(Debug, Clone, Copy, Default)]
pub struct InstallOptions {
    /// `Some(true)` forces `docker pull` at install time for `kind: docker`
    /// regardless of the manifest's `docker.prefetch` (`sb install --prefetch`).
    pub prefetch_override: Option<bool>,
}

/// What [`install`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallOutcome {
    pub name: String,
    /// Package root as registered.
    pub path: PathBuf,
    pub source: SourceKind,
    pub kind: AppKind,
    pub version: String,
    /// `HEAD` of the clone (git sources only).
    pub commit: Option<String>,
    /// The app was already registered at this path or URL and was refreshed.
    pub reinstalled: bool,
    /// Commands run for the kind's install step, e.g. `docker pull <image>`.
    pub steps_run: Vec<String>,
    /// Informational lines for the user (deferred pulls, pinned refs).
    pub notes: Vec<String>,
    /// Non-fatal problems, such as a `requires` binary missing from `PATH`.
    pub warnings: Vec<String>,
}

/// What [`update`] did for one app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateOutcome {
    pub name: String,
    pub path: PathBuf,
    pub kind: AppKind,
    /// `None` for apps installed from a local folder (nothing to pull).
    pub pull: Option<PullStatus>,
    pub old_version: String,
    pub new_version: String,
    pub old_commit: Option<String>,
    pub new_commit: Option<String>,
    pub steps_run: Vec<String>,
    pub notes: Vec<String>,
    pub warnings: Vec<String>,
}

/// What [`remove`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveOutcome {
    pub name: String,
    pub path: PathBuf,
    pub source: SourceKind,
    /// True when the folder was a clone under `<sb_home>/apps/` and was
    /// deleted. Folders installed in place are never deleted.
    pub deleted_dir: bool,
}

/// Whether one `requires` binary is on `PATH`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequireStatus {
    pub bin: String,
    pub found: Option<PathBuf>,
}

/// An installed app as seen right now: the registry record plus a live read
/// of its manifest and `requires`.
#[derive(Debug, Clone)]
pub struct AppInfo {
    pub name: String,
    pub record: AppEntry,
    pub path_exists: bool,
    /// The manifest, when it could be read.
    pub manifest: Option<AppManifest>,
    /// Why the manifest could not be read, when it could not.
    pub manifest_error: Option<String>,
    pub requires: Vec<RequireStatus>,
}

/// `sb install <arg>`: install an app package from a git URL or a local
/// folder and register it under its manifest `name`.
///
/// * Local folder: registered in place, never copied.
/// * Git URL: cloned to `<home>/apps/<name>/`. Installing a URL that is
///   already installed at the same ref pulls it in place instead.
///
/// Reinstalling the same package is an update; installing a different
/// package under a taken name is an error.
pub fn install(home: &Path, arg: &str, opts: &InstallOptions) -> Result<InstallOutcome> {
    let mut reg = Registry::load(home)?;
    let source = classify(arg)?;
    let (fetched, source_kind, url, git_ref) = match source {
        Source::Path(dir) => {
            let manifest = load_valid(&dir)?;
            let reinstalled = claim_name(&reg, &manifest.name, &dir)?;
            let fetched = Fetched {
                root: dir,
                manifest,
                reinstalled,
                pull: None,
            };
            (fetched, SourceKind::Path, None, None)
        }
        Source::Git { url, git_ref } => {
            let fetched = fetch_git(home, &reg, &url, git_ref.as_deref())?;
            (fetched, SourceKind::Git, Some(url), git_ref)
        }
    };
    let Fetched {
        root,
        manifest,
        reinstalled,
        pull,
    } = fetched;
    let name = manifest.name.clone();

    let mut notes = Vec::new();
    match pull {
        Some(PullStatus::Pulled) => {
            notes.push("already installed from this URL; pulled it in place".to_owned());
        }
        Some(PullStatus::Pinned) => notes.push(format!(
            "already installed from this URL at a pinned ref ({}); nothing to pull",
            git_ref.as_deref().unwrap_or("detached HEAD")
        )),
        None => {}
    }
    let steps = run_kind_steps(
        home,
        &manifest,
        &root,
        Phase::Install(opts.prefetch_override),
    )?;
    notes.extend(steps.notes);
    let warnings = missing_requires(&manifest);
    let commit = match source_kind {
        SourceKind::Git => Some(git_head(&root)?),
        SourceKind::Path => None,
    };

    // A folder that renamed its app leaves its old name pointing here; drop it.
    let stale: Vec<String> = reg
        .iter()
        .filter(|(n, e)| *n != name && same_path(&e.path, &root))
        .map(|(n, _)| n.to_owned())
        .collect();
    for old in stale {
        reg.remove(&old);
        notes.push(format!(
            "unregistered `{old}`: that folder now calls its app `{name}`"
        ));
    }

    reg.insert(
        name.clone(),
        AppEntry {
            path: root.clone(),
            source: source_kind,
            url,
            git_ref,
            commit: commit.clone(),
            version: manifest.version.clone(),
            installed_at: rfc3339_utc(SystemTime::now()),
        },
    );
    reg.save(home)?;

    Ok(InstallOutcome {
        name,
        path: root,
        source: source_kind,
        kind: manifest.kind,
        version: manifest.version,
        commit,
        reinstalled,
        steps_run: steps.run,
        notes,
        warnings,
    })
}

/// `sb app update [name]`: for one app or every app, pull git sources
/// (`git pull --ff-only`), re-run the kind's install steps, and refresh the
/// recorded version and commit. Stops at the first app that fails; apps
/// updated before it stay updated.
pub fn update(home: &Path, name: Option<&str>) -> Result<Vec<UpdateOutcome>> {
    let mut reg = Registry::load(home)?;
    let names: Vec<String> = match name {
        Some(n) => {
            if reg.get(n).is_none() {
                bail!("{}", unknown_app(&reg, n));
            }
            vec![n.to_owned()]
        }
        None => reg.names().into_iter().map(str::to_owned).collect(),
    };
    let mut outcomes = Vec::with_capacity(names.len());
    for n in names {
        let record = reg
            .get(&n)
            .cloned()
            .ok_or_else(|| anyhow!("app `{n}` vanished from the registry during update"))?;
        let (outcome, refreshed) =
            update_one(home, &n, &record).with_context(|| format!("updating app `{n}`"))?;
        reg.insert(n, refreshed);
        reg.save(home)?;
        outcomes.push(outcome);
    }
    Ok(outcomes)
}

fn update_one(home: &Path, name: &str, record: &AppEntry) -> Result<(UpdateOutcome, AppEntry)> {
    if !record.path.is_dir() {
        bail!(
            "app '{name}' path {} no longer exists; run `sb app remove {name}` then reinstall",
            record.path.display()
        );
    }
    let pull = match record.source {
        SourceKind::Git => Some(git_pull(&record.path)?),
        SourceKind::Path => None,
    };
    let manifest = load_valid(&record.path)?;
    if manifest.name != name {
        bail!(
            "the package now calls itself `{}`; run `sb app remove {name}` and install it again",
            manifest.name
        );
    }
    let steps = run_kind_steps(home, &manifest, &record.path, Phase::Update)?;
    let mut notes = Vec::new();
    if pull == Some(PullStatus::Pinned) {
        notes.push(format!(
            "pinned to {}; nothing to pull (reinstall with another `@<ref>` to move it)",
            record.git_ref.as_deref().unwrap_or("a detached HEAD")
        ));
    }
    notes.extend(steps.notes);
    let new_commit = match record.source {
        SourceKind::Git => Some(git_head(&record.path)?),
        SourceKind::Path => None,
    };
    let refreshed = AppEntry {
        commit: new_commit.clone(),
        version: manifest.version.clone(),
        installed_at: rfc3339_utc(SystemTime::now()),
        ..record.clone()
    };
    let outcome = UpdateOutcome {
        name: name.to_owned(),
        path: record.path.clone(),
        kind: manifest.kind,
        pull,
        old_version: record.version.clone(),
        new_version: manifest.version.clone(),
        old_commit: record.commit.clone(),
        new_commit,
        steps_run: steps.run,
        notes,
        warnings: missing_requires(&manifest),
    };
    Ok((outcome, refreshed))
}

/// `sb app remove <name>`: unregister the app, and delete its folder only
/// when it is a clone under `<home>/apps/`. Folders installed in place are
/// left exactly as they were.
pub fn remove(home: &Path, name: &str) -> Result<RemoveOutcome> {
    let mut reg = Registry::load(home)?;
    let Some(record) = reg.remove(name) else {
        bail!("{}", unknown_app(&reg, name));
    };
    reg.save(home)?;
    let owned = is_owned_clone(home, &record.path);
    let deleted_dir = owned && record.path.exists();
    if deleted_dir {
        fs::remove_dir_all(&record.path).with_context(|| {
            format!(
                "unregistered `{name}`, but could not delete {}; delete it by hand",
                record.path.display()
            )
        })?;
    }
    Ok(RemoveOutcome {
        name: name.to_owned(),
        path: record.path,
        source: record.source,
        deleted_dir,
    })
}

/// `sb app list`: every installed app, in name order.
pub fn list(home: &Path) -> Result<Vec<AppInfo>> {
    let reg = Registry::load(home)?;
    Ok(reg.iter().map(|(n, e)| app_info(n, e)).collect())
}

/// `sb app info <name>`: one installed app.
pub fn info(home: &Path, name: &str) -> Result<AppInfo> {
    let reg = Registry::load(home)?;
    reg.get(name)
        .map(|e| app_info(name, e))
        .ok_or_else(|| anyhow!("{}", unknown_app(&reg, name)))
}

fn app_info(name: &str, record: &AppEntry) -> AppInfo {
    let path_exists = record.path.is_dir();
    let (manifest, manifest_error) = if path_exists {
        match AppManifest::load(&record.path) {
            Ok(m) => (Some(m), None),
            Err(e) => (None, Some(format!("{e:#}"))),
        }
    } else {
        (
            None,
            Some(format!("{} does not exist", record.path.display())),
        )
    };
    let requires = manifest
        .as_ref()
        .map(|m| {
            m.requires
                .iter()
                .map(|bin| RequireStatus {
                    bin: bin.clone(),
                    found: which::which(bin).ok(),
                })
                .collect()
        })
        .unwrap_or_default();
    AppInfo {
        name: name.to_owned(),
        record: record.clone(),
        path_exists,
        manifest,
        manifest_error,
        requires,
    }
}

/// "no installed app named ..." with the installed names and the install hint.
pub(crate) fn unknown_app(reg: &Registry, name: &str) -> String {
    format!(
        "no installed app named `{name}`; {}. Install one with `sb install <url|path>`",
        installed_names(reg)
    )
}

/// "installed apps: a, b" or "no apps are installed".
pub(crate) fn installed_names(reg: &Registry) -> String {
    if reg.is_empty() {
        "no apps are installed".to_owned()
    } else {
        format!("installed apps: {}", reg.names().join(", "))
    }
}

// ─────────────────────────────────────────────────────────────────────
// internals
// ─────────────────────────────────────────────────────────────────────

/// A package located on disk with its validated manifest.
struct Fetched {
    root: PathBuf,
    manifest: AppManifest,
    reinstalled: bool,
    pull: Option<PullStatus>,
}

/// Which verb is running the kind's install step.
#[derive(Debug, Clone, Copy)]
enum Phase {
    /// `sb install`, with the `--prefetch` override if given.
    Install(Option<bool>),
    /// `sb app update`: the manifest alone decides.
    Update,
}

/// The install steps' result: commands run, plus notes for the user.
struct Steps {
    run: Vec<String>,
    notes: Vec<String>,
}

fn load_valid(dir: &Path) -> Result<AppManifest> {
    let m = AppManifest::load(dir)?;
    m.validate(dir)?;
    Ok(m)
}

/// Check `name` is free for the package at `root`. `Ok(true)` when it is
/// already registered to this very folder (a reinstall).
fn claim_name(reg: &Registry, name: &str, root: &Path) -> Result<bool> {
    match reg.get(name) {
        None => Ok(false),
        Some(e) if same_path(&e.path, root) => Ok(true),
        Some(e) => bail!(
            "an app named `{name}` is already installed from {}; run `sb app remove {name}` first",
            origin(e)
        ),
    }
}

fn origin(e: &AppEntry) -> String {
    match (&e.source, &e.url) {
        (SourceKind::Git, Some(url)) => format!("{url} (cloned to {})", e.path.display()),
        _ => e.path.display().to_string(),
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// Removes a scratch directory on drop unless [`ScratchDir::keep`] ran.
struct ScratchDir {
    path: PathBuf,
    armed: bool,
}

impl ScratchDir {
    fn keep(mut self) {
        self.armed = false;
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

/// Fetch a git package into `<home>/apps/<name>/`.
fn fetch_git(home: &Path, reg: &Registry, url: &str, git_ref: Option<&str>) -> Result<Fetched> {
    // Already installed from this URL at this ref: pull in place, no reclone.
    if let Some((name, e)) = reg.find_git_url(url) {
        if e.git_ref.as_deref() == git_ref && e.path.is_dir() {
            let pull = git_pull(&e.path)?;
            let manifest = load_valid(&e.path)?;
            if manifest.name != name {
                bail!(
                    "the package at {url} now calls itself `{}` (installed as `{name}`); \
                     run `sb app remove {name}` and install it again",
                    manifest.name
                );
            }
            return Ok(Fetched {
                root: e.path.clone(),
                manifest,
                reinstalled: true,
                pull: Some(pull),
            });
        }
    }

    let apps = apps_dir(home);
    fs::create_dir_all(&apps).with_context(|| format!("creating {}", apps.display()))?;
    let scratch = ScratchDir {
        path: apps.join(format!(".tmp-{}-{}", repo_dir_name(url), unique_suffix())),
        armed: true,
    };
    git_clone(url, git_ref, &scratch.path)?;
    let manifest = load_valid(&scratch.path)
        .with_context(|| format!("{url} is not a valid sb app package"))?;
    let name = manifest.name.clone();
    let dest = apps.join(&name);

    let reinstalled = match reg.get(&name) {
        None => false,
        // Same package at another ref, or its clone vanished: replace the clone.
        Some(e) if e.source == SourceKind::Git && e.url.as_deref() == Some(url) => {
            if e.path.exists() && is_owned_clone(home, &e.path) {
                fs::remove_dir_all(&e.path)
                    .with_context(|| format!("removing the old clone {}", e.path.display()))?;
            }
            true
        }
        Some(e) => bail!(
            "an app named `{name}` is already installed from {}; run `sb app remove {name}` first",
            origin(e)
        ),
    };
    if dest.exists() {
        if let Some((other, _)) = reg.iter().find(|(_, e)| same_path(&e.path, &dest)) {
            bail!(
                "{} is registered to app `{other}`; run `sb app remove {other}` first",
                dest.display()
            );
        }
        // Unregistered leftover of an interrupted install. sb owns apps/.
        fs::remove_dir_all(&dest)
            .with_context(|| format!("removing leftover {}", dest.display()))?;
    }
    fs::rename(&scratch.path, &dest)
        .with_context(|| format!("moving {} to {}", scratch.path.display(), dest.display()))?;
    scratch.keep();
    Ok(Fetched {
        root: dest,
        manifest,
        reinstalled,
        pull: None,
    })
}

/// Run what `kind` asks for at install or update time.
fn run_kind_steps(home: &Path, manifest: &AppManifest, root: &Path, phase: Phase) -> Result<Steps> {
    let mut steps = Steps {
        run: Vec::new(),
        notes: Vec::new(),
    };
    match manifest.kind {
        AppKind::None => {}
        AppKind::Docker => {
            let spec = manifest
                .docker
                .as_ref()
                .ok_or_else(|| anyhow!("sb.app.yml has `kind: docker` but no `docker:` section"))?;
            let image = spec.image.trim();
            let prefetch_override = match phase {
                Phase::Install(o) => o,
                Phase::Update => None,
            };
            if prefetch_override.unwrap_or(spec.prefetch) {
                let docker = which::which("docker").map_err(|_| {
                    anyhow!(
                        "installing `{}` needs `docker pull {image}`, but `docker` is not on PATH; \
                         install Docker, then rerun",
                        manifest.name
                    )
                })?;
                let status = Command::new(docker)
                    .args(["pull", image])
                    .status()
                    .context("running docker pull")?;
                if !status.success() {
                    bail!(
                        "docker pull {image} failed ({status}); check the image name and your \
                         registry login, then rerun"
                    );
                }
                steps.run.push(format!("docker pull {image}"));
            } else {
                let hint = match phase {
                    Phase::Install(_) => "; pass --prefetch to pull it now",
                    Phase::Update => "",
                };
                steps.notes.push(format!(
                    "image {image} is pulled on first run (prefetch: false){hint}"
                ));
            }
        }
        AppKind::Host => {
            let script = manifest.install_script_path(root)?.ok_or_else(|| {
                anyhow!("sb.app.yml has `kind: host` but no `host.install` script")
            })?;
            let bash = find_bash().ok_or_else(|| {
                anyhow!("`host.install` scripts run with bash, but bash is not on PATH")
            })?;
            let status = Command::new(bash)
                .arg(bash_path_arg(&script))
                .current_dir(root)
                .env("SB_HOME", home)
                .env("SB_APP_DIR", root)
                .env("SB_APP_NAME", &manifest.name)
                .status()
                .with_context(|| format!("running {}", script.display()))?;
            if !status.success() {
                bail!(
                    "install script {} failed ({status}); fix it, then rerun `sb install` \
                     (or `sb app update {}` if already installed)",
                    script.display(),
                    manifest.name
                );
            }
            let rel = manifest.host.as_ref().map_or("", |h| h.install.as_str());
            steps.run.push(format!("bash {rel}"));
        }
    }
    Ok(steps)
}

/// One warning per `requires` binary missing from `PATH`.
fn missing_requires(manifest: &AppManifest) -> Vec<String> {
    manifest
        .requires
        .iter()
        .filter(|bin| which::which(bin.as_str()).is_err())
        .map(|bin| {
            format!(
                "`{bin}` is not on PATH (listed in `requires`); `sb {}` may fail until it is installed",
                manifest.name
            )
        })
        .collect()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::exec::{app_command, exec_app};
    use crate::manifest::MANIFEST_FILE;
    use std::process::Command;
    use tempfile::TempDir;

    /// Entry that records its args and cwd next to itself, prints them, and
    /// exits with `$2` when called as `fail <code>`.
    const ECHO_ENTRY: &str = r#"#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" > "$SB_APP_DIR/last_args"
pwd > "$SB_APP_DIR/last_cwd"
echo "args: $*"
echo "cwd: $PWD"
if [[ "${1:-}" == "fail" ]]; then exit "$2"; fi
"#;

    fn have(bin: &str) -> bool {
        which::which(bin).is_ok()
    }

    /// Run an installed app as a child and return its exit code. A
    /// successful `exec_app` would replace the test process with the app, so
    /// in-process tests only call it for its error paths.
    fn run_app(home: &Path, name: &str, args: &[String]) -> i32 {
        let (mut cmd, _) = app_command(home, name, args).unwrap();
        cmd.status().unwrap().code().unwrap()
    }

    fn write_exec(path: &Path, body: &str) {
        fs::write(path, body).unwrap();
        crate::util::set_executable(path).unwrap();
    }

    /// A package folder `dir` with an echo entry and the given manifest tail.
    fn make_pkg(dir: &Path, name: &str, extra: &str) {
        fs::create_dir_all(dir).unwrap();
        write_exec(&dir.join("run.bash"), ECHO_ENTRY);
        fs::write(
            dir.join(MANIFEST_FILE),
            format!("name: {name}\nversion: 0.1.0\nentry: ./run.bash\n{extra}"),
        )
        .unwrap();
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

    #[test]
    fn path_install_then_exec_then_remove_keeps_folder() {
        if !have("bash") {
            return;
        }
        let home = TempDir::new().unwrap();
        let src = TempDir::new().unwrap();
        let pkg = src.path().join("echo_pkg");
        make_pkg(&pkg, "echoer", "kind: none\n");

        let out = install(
            home.path(),
            &pkg.display().to_string(),
            &InstallOptions::default(),
        )
        .unwrap();
        assert_eq!(out.name, "echoer");
        assert_eq!(out.source, SourceKind::Path);
        assert_eq!(out.kind, AppKind::None);
        assert_eq!(out.path, pkg.canonicalize().unwrap());
        assert!(!out.reinstalled && out.steps_run.is_empty() && out.commit.is_none());

        let reg = Registry::load(home.path()).unwrap();
        let rec = reg.get("echoer").unwrap();
        assert_eq!(rec.source, SourceKind::Path);
        assert_eq!(rec.version, "0.1.0");
        assert!(rec.installed_at.ends_with('Z'));

        let args = vec!["a".to_string(), "b c".to_string()];
        assert_eq!(run_app(home.path(), "echoer", &args), 0);
        assert_eq!(
            fs::read_to_string(pkg.join("last_args")).unwrap(),
            "a b c\n"
        );
        let cwd = std::env::current_dir().unwrap().canonicalize().unwrap();
        let ran_in = PathBuf::from(fs::read_to_string(pkg.join("last_cwd")).unwrap().trim());
        assert_eq!(
            ran_in.canonicalize().unwrap(),
            cwd,
            "app must run in the caller's cwd"
        );

        let fail = vec!["fail".to_string(), "7".to_string()];
        assert_eq!(run_app(home.path(), "echoer", &fail), 7);

        let again = install(
            home.path(),
            &pkg.display().to_string(),
            &InstallOptions::default(),
        )
        .unwrap();
        assert!(again.reinstalled);

        let listed = list(home.path()).unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].path_exists && listed[0].manifest.is_some());

        let rm = remove(home.path(), "echoer").unwrap();
        assert!(!rm.deleted_dir);
        assert!(
            pkg.join(MANIFEST_FILE).is_file(),
            "in-place folder must survive remove"
        );
        assert!(Registry::load(home.path()).unwrap().is_empty());
        let e = exec_app(home.path(), "echoer", &[]).unwrap_err();
        assert!(format!("{e:#}").contains("sb install"), "{e:#}");
    }

    #[test]
    fn remove_deletes_folder_under_home_apps() {
        let home = TempDir::new().unwrap();
        let pkg = apps_dir(home.path()).join("owned_pkg");
        make_pkg(&pkg, "owned", "");
        install(
            home.path(),
            &pkg.display().to_string(),
            &InstallOptions::default(),
        )
        .unwrap();
        let rm = remove(home.path(), "owned").unwrap();
        assert!(rm.deleted_dir);
        assert!(!pkg.exists());
    }

    #[test]
    fn name_taken_by_another_folder_is_an_error() {
        let home = TempDir::new().unwrap();
        let src = TempDir::new().unwrap();
        make_pkg(&src.path().join("one"), "twin", "");
        make_pkg(&src.path().join("two"), "twin", "");
        install(
            home.path(),
            &src.path().join("one").display().to_string(),
            &InstallOptions::default(),
        )
        .unwrap();
        let e = install(
            home.path(),
            &src.path().join("two").display().to_string(),
            &InstallOptions::default(),
        )
        .unwrap_err();
        assert!(format!("{e:#}").contains("sb app remove twin"), "{e:#}");
    }

    #[test]
    fn missing_path_and_unknown_names_are_reported() {
        let home = TempDir::new().unwrap();
        let src = TempDir::new().unwrap();
        let pkg = src.path().join("gone");
        make_pkg(&pkg, "gone", "");
        install(
            home.path(),
            &pkg.display().to_string(),
            &InstallOptions::default(),
        )
        .unwrap();
        fs::remove_dir_all(&pkg).unwrap();
        let e = format!("{:#}", exec_app(home.path(), "gone", &[]).unwrap_err());
        assert!(
            e.contains("no longer exists") && e.contains("sb app remove gone"),
            "{e}"
        );
        let e = format!("{:#}", update(home.path(), Some("gone")).unwrap_err());
        assert!(e.contains("no longer exists"), "{e}");
        let info = info(home.path(), "gone").unwrap();
        assert!(!info.path_exists && info.manifest_error.is_some());

        assert!(format!("{:#}", info_err(home.path(), "nope")).contains("installed apps: gone"));
        assert!(format!("{:#}", remove(home.path(), "nope").unwrap_err()).contains("sb install"));
    }

    fn info_err(home: &Path, name: &str) -> anyhow::Error {
        info(home, name).unwrap_err()
    }

    #[test]
    fn host_kind_runs_install_script_from_package_root() {
        if !have("bash") {
            return;
        }
        let home = TempDir::new().unwrap();
        let src = TempDir::new().unwrap();
        let pkg = src.path().join("hosty");
        make_pkg(
            &pkg,
            "hosty",
            "kind: host\nhost:\n  install: ./install.bash\n",
        );
        write_exec(
            &pkg.join("install.bash"),
            "#!/usr/bin/env bash\npwd > installed_from\necho \"$SB_APP_NAME\" >> installed_from\n",
        );
        let out = install(
            home.path(),
            &pkg.display().to_string(),
            &InstallOptions::default(),
        )
        .unwrap();
        assert_eq!(out.steps_run, vec!["bash ./install.bash".to_string()]);
        let marker = fs::read_to_string(pkg.join("installed_from")).unwrap();
        let mut lines = marker.lines();
        assert_eq!(
            PathBuf::from(lines.next().unwrap()).canonicalize().unwrap(),
            pkg.canonicalize().unwrap()
        );
        assert_eq!(lines.next(), Some("hosty"));

        // A failing install script fails the install and registers nothing new.
        let bad = src.path().join("badhost");
        make_pkg(
            &bad,
            "badhost",
            "kind: host\nhost:\n  install: ./install.bash\n",
        );
        write_exec(&bad.join("install.bash"), "#!/usr/bin/env bash\nexit 3\n");
        let e = install(
            home.path(),
            &bad.display().to_string(),
            &InstallOptions::default(),
        )
        .unwrap_err();
        assert!(format!("{e:#}").contains("install script"), "{e:#}");
        assert!(
            Registry::load(home.path())
                .unwrap()
                .get("badhost")
                .is_none()
        );
    }

    #[test]
    fn docker_without_prefetch_defers_pull_and_missing_requires_warn() {
        let home = TempDir::new().unwrap();
        let src = TempDir::new().unwrap();
        let pkg = src.path().join("dock");
        make_pkg(
            &pkg,
            "dock",
            "kind: docker\ndocker:\n  image: example/none:0\n  prefetch: false\nrequires: [sb_surely_missing_bin_42]\n",
        );
        let out = install(
            home.path(),
            &pkg.display().to_string(),
            &InstallOptions::default(),
        )
        .unwrap();
        assert!(out.steps_run.is_empty());
        assert!(
            out.notes.iter().any(|n| n.contains("first run")),
            "{:?}",
            out.notes
        );
        assert_eq!(out.warnings.len(), 1);
        assert!(out.warnings[0].contains("sb_surely_missing_bin_42"));
        assert!(Registry::load(home.path()).unwrap().get("dock").is_some());
        let info = info(home.path(), "dock").unwrap();
        assert_eq!(info.requires.len(), 1);
        assert!(info.requires[0].found.is_none());
    }

    /// A local bare repo `<tmp>/pkg.git` fed from a work tree `<tmp>/work`.
    fn bare_repo_fixture(tmp: &Path, name: &str) -> (PathBuf, PathBuf) {
        let work = tmp.join("work");
        make_pkg(&work, name, "");
        git(&work, &["init", "-q", "-b", "main"]);
        git(&work, &["add", "."]);
        git(&work, &["commit", "-q", "-m", "v0.1.0"]);
        git(&work, &["tag", "v1"]);
        let bare = tmp.join("pkg.git");
        git(tmp, &["clone", "-q", "--bare", "work", "pkg.git"]);
        (work, bare)
    }

    #[test]
    fn git_install_update_reinstall_pin_and_remove() {
        if !have("git") || !have("bash") {
            return;
        }
        let home = TempDir::new().unwrap();
        let src = TempDir::new().unwrap();
        let (work, bare) = bare_repo_fixture(src.path(), "gitapp");
        let url = bare.display().to_string();

        let out = install(home.path(), &url, &InstallOptions::default()).unwrap();
        assert_eq!(out.source, SourceKind::Git);
        let clone = apps_dir(home.path()).join("gitapp");
        assert_eq!(out.path, clone);
        assert!(clone.join(MANIFEST_FILE).is_file());
        let first = out.commit.clone().unwrap();
        assert_eq!(first.len(), 40, "{first}");
        let rec = Registry::load(home.path())
            .unwrap()
            .get("gitapp")
            .cloned()
            .unwrap();
        assert_eq!(
            rec.url.as_deref(),
            Some(bare.canonicalize().unwrap().to_str().unwrap())
        );
        assert_eq!(rec.git_ref, None);
        let leftovers: Vec<_> = fs::read_dir(apps_dir(home.path()))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "scratch clone left behind");
        assert_eq!(run_app(home.path(), "gitapp", &["x".into()]), 0);

        // Upstream bumps the version; update pulls it.
        let manifest = fs::read_to_string(work.join(MANIFEST_FILE)).unwrap();
        fs::write(work.join(MANIFEST_FILE), manifest.replace("0.1.0", "0.2.0")).unwrap();
        git(&work, &["commit", "-q", "-am", "v0.2.0"]);
        git(&work, &["push", "-q", &url, "main"]);
        let ups = update(home.path(), Some("gitapp")).unwrap();
        assert_eq!(ups[0].pull, Some(PullStatus::Pulled));
        assert_eq!(
            (ups[0].old_version.as_str(), ups[0].new_version.as_str()),
            ("0.1.0", "0.2.0")
        );
        assert_ne!(ups[0].new_commit.as_deref(), Some(first.as_str()));
        assert_eq!(
            Registry::load(home.path())
                .unwrap()
                .get("gitapp")
                .unwrap()
                .version,
            "0.2.0"
        );

        // Same URL again: pulled in place, not recloned.
        let again = install(home.path(), &url, &InstallOptions::default()).unwrap();
        assert!(again.reinstalled);
        assert!(
            again.notes.iter().any(|n| n.contains("pulled it in place")),
            "{:?}",
            again.notes
        );

        // Pin to the v1 tag: the clone is replaced, and update has nothing to pull.
        let pinned = install(
            home.path(),
            &format!("{url}@v1"),
            &InstallOptions::default(),
        )
        .unwrap();
        assert!(pinned.reinstalled);
        assert_eq!(pinned.version, "0.1.0");
        assert_eq!(pinned.commit.as_deref(), Some(first.as_str()));
        let rec = Registry::load(home.path())
            .unwrap()
            .get("gitapp")
            .cloned()
            .unwrap();
        assert_eq!(rec.git_ref.as_deref(), Some("v1"));
        let ups = update(home.path(), None).unwrap();
        assert_eq!(ups[0].pull, Some(PullStatus::Pinned));

        let rm = remove(home.path(), "gitapp").unwrap();
        assert!(rm.deleted_dir);
        assert!(!clone.exists());
    }

    #[test]
    fn git_install_of_a_non_package_cleans_up() {
        if !have("git") {
            return;
        }
        let home = TempDir::new().unwrap();
        let src = TempDir::new().unwrap();
        let work = src.path().join("work");
        fs::create_dir_all(&work).unwrap();
        fs::write(work.join("README"), "not a package\n").unwrap();
        git(&work, &["init", "-q", "-b", "main"]);
        git(&work, &["add", "."]);
        git(&work, &["commit", "-q", "-m", "init"]);
        git(src.path(), &["clone", "-q", "--bare", "work", "plain.git"]);
        let e = install(
            home.path(),
            &src.path().join("plain.git").display().to_string(),
            &InstallOptions::default(),
        )
        .unwrap_err();
        assert!(format!("{e:#}").contains("sb.app.yml"), "{e:#}");
        let left = fs::read_dir(apps_dir(home.path())).unwrap().count();
        assert_eq!(left, 0, "failed clone must be cleaned up");
        assert!(Registry::load(home.path()).unwrap().is_empty());
    }
}
