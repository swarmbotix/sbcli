//! Where `sb install <arg>` fetches a package from, and the git plumbing.
//!
//! `<arg>` is a git source when it starts with `http://`, `https://`,
//! `git@`, `ssh://`, `git://` or `file://`, or when it ends in `.git`
//! (which also covers a bare repository on the local disk). Anything else is
//! a local folder. A git source may pin a branch or tag with a trailing
//! `@<ref>`: `https://github.com/org/repo.git@v1.2`.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};

use crate::util::{canonical, expand_user};

const GIT_PREFIXES: &[&str] = &["http://", "https://", "git@", "ssh://", "git://", "file://"];

/// A classified `sb install` argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// Clone `url`, at `git_ref` when given (`<url>@<ref>`).
    Git {
        url: String,
        git_ref: Option<String>,
    },
    /// An existing local folder, canonicalized.
    Path(PathBuf),
}

/// What a pull did to a git-installed app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullStatus {
    /// `git pull --ff-only` ran (it may have found nothing new).
    Pulled,
    /// `HEAD` is detached at a pinned tag or commit; nothing to pull.
    Pinned,
}

/// Classify an `sb install` argument as a git source or a local folder.
///
/// A local folder is tilde-expanded and canonicalized and must exist. A
/// local git source (a path ending in `.git`) is canonicalized when it
/// exists, so the URL recorded in the registry does not depend on the cwd
/// of the install.
pub fn classify(arg: &str) -> Result<Source> {
    let arg = arg.trim();
    if arg.is_empty() {
        bail!("no source given; pass a git URL or the path of a package folder");
    }
    let has_git_prefix = GIT_PREFIXES.iter().any(|p| arg.starts_with(p));
    let (url, git_ref) = split_ref(arg);
    if has_git_prefix || has_git_suffix(url) {
        if git_ref == Some("") {
            bail!(
                "`{arg}` ends with `@` but names no ref; use `<url>@<branch-or-tag>` or drop the `@`"
            );
        }
        return Ok(Source::Git {
            url: normalize_local_url(url),
            git_ref: git_ref.map(str::to_owned),
        });
    }

    let path = expand_user(arg);
    if !path.exists() {
        bail!(
            "{arg}: no such folder. Pass the folder that holds sb.app.yml, or a git URL \
             (https://..., ssh://..., git@host:org/repo, or a path ending in .git)"
        );
    }
    let abs = canonical(&path).with_context(|| format!("resolving {}", path.display()))?;
    if !abs.is_dir() {
        bail!(
            "{} is a file; pass the package folder that holds sb.app.yml",
            abs.display()
        );
    }
    Ok(Source::Path(abs))
}

/// True if `s` (ignoring trailing `/`) ends in `.git`, in any case.
fn has_git_suffix(s: &str) -> bool {
    let s = s.trim_end_matches('/');
    s.len()
        .checked_sub(4)
        .and_then(|i| s.get(i..))
        .is_some_and(|tail| tail.eq_ignore_ascii_case(".git"))
}

/// Split `<url>@<ref>` on the last `@`.
///
/// The text after the `@` is a ref only if it contains neither `/` nor `:`
/// (so `https://user@host/repo` and `ssh://git@host/repo` keep their user
/// part). A leading `git@` is skipped first, since in `git@host:org/repo` it
/// is the ssh user, not a ref separator.
fn split_ref(arg: &str) -> (&str, Option<&str>) {
    let skip = if arg.starts_with("git@") { 4 } else { 0 };
    let Some(at) = arg[skip..].rfind('@').map(|i| i + skip) else {
        return (arg, None);
    };
    let (url, rest) = (&arg[..at], &arg[at + 1..]);
    if url.is_empty() || rest.contains(['/', ':']) {
        return (arg, None);
    }
    (url, Some(rest))
}

/// Make a local-disk git URL absolute; leave remote URLs untouched.
fn normalize_local_url(url: &str) -> String {
    if GIT_PREFIXES.iter().any(|p| url.starts_with(p)) {
        return url.to_owned();
    }
    match canonical(&expand_user(url)) {
        Ok(abs) => abs.display().to_string(),
        Err(_) => url.to_owned(),
    }
}

/// Folder-name stem for a clone of `url`: the last path segment without
/// `.git`, reduced to `[A-Za-z0-9_.-]`.
pub fn repo_dir_name(url: &str) -> String {
    let trimmed = url.trim_end_matches(['/', '\\']);
    let last = trimmed.rsplit(['/', '\\', ':']).next().unwrap_or(trimmed);
    let stem = last.strip_suffix(".git").unwrap_or(last);
    let clean: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if clean.is_empty() {
        "repo".into()
    } else {
        clean
    }
}

fn git() -> Result<Command> {
    let bin = which::which("git").map_err(|_| {
        anyhow!("`git` is not on PATH; install git to install or update apps from a URL")
    })?;
    Ok(Command::new(bin))
}

/// `git clone --depth 1 [--branch <ref>] <url> <dest>`, with git's stderr
/// folded into the error on failure.
pub(crate) fn git_clone(url: &str, git_ref: Option<&str>, dest: &Path) -> Result<()> {
    let mut cmd = git()?;
    cmd.args(["clone", "--depth", "1"]);
    if let Some(r) = git_ref {
        cmd.args(["--branch", r]);
    }
    cmd.arg(url).arg(dest);
    let out = cmd.output().context("running git clone")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let hint = git_ref.map_or(String::new(), |r| {
            format!("\n(`@{r}` must name a branch or tag of that repository)")
        });
        bail!(
            "git clone {url} failed ({}):\n{}{hint}",
            out.status,
            stderr.trim_end()
        );
    }
    Ok(())
}

/// Fast-forward the clone at `dir`. A detached `HEAD` (a pinned tag) is
/// left alone and reported as [`PullStatus::Pinned`].
pub(crate) fn git_pull(dir: &Path) -> Result<PullStatus> {
    let on_branch = git()?
        .args(["symbolic-ref", "-q", "HEAD"])
        .current_dir(dir)
        .output()
        .context("running git symbolic-ref")?
        .status
        .success();
    if !on_branch {
        return Ok(PullStatus::Pinned);
    }
    let out = git()?
        .args(["pull", "--ff-only"])
        .current_dir(dir)
        .output()
        .context("running git pull")?;
    if !out.status.success() {
        bail!(
            "git pull --ff-only in {} failed ({}):\n{}\nIf that clone has local edits, \
             remove the app (`sb app remove <name>`) and install it again.",
            dir.display(),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim_end()
        );
    }
    Ok(PullStatus::Pulled)
}

/// `git rev-parse HEAD` in `dir`.
pub(crate) fn git_head(dir: &Path) -> Result<String> {
    let out = git()?
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()
        .context("running git rev-parse")?;
    if !out.status.success() {
        bail!(
            "git rev-parse HEAD in {} failed: {}",
            dir.display(),
            String::from_utf8_lossy(&out.stderr).trim_end()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}
