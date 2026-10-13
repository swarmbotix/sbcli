//! `sb <name> [args...]`: run an installed app.
//!
//! The entry runs in the user's current directory (never the package
//! folder), with the user's arguments verbatim and inherited stdio. It also
//! receives `SB_HOME`, `SB_APP_DIR` (package root) and `SB_APP_NAME`.
//!
//! On Unix `sb` replaces itself with the entry (`exec`), so the app is the
//! process the shell waits on: Ctrl+C and the exit status belong to the app
//! alone, and the prompt returns only after the app has finished cleaning
//! up. Elsewhere the entry runs as a child and `sb` waits for it.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};

use crate::builtins::BUILTIN_NAMES;
use crate::install::installed_names;
use crate::manifest::AppManifest;
use crate::registry::Registry;
#[cfg(unix)]
use crate::util::is_executable;
use crate::util::{bash_path_arg, edit_distance, find_bash};

/// Run the installed app `name` with `args`.
///
/// On Unix this replaces the `sb` process with the app and never returns
/// `Ok`; an `Err` means the app could not be resolved or started. Elsewhere
/// it waits for the app and returns its exit code, and a child killed by a
/// signal reports exit code 1.
pub fn exec_app(home: &Path, name: &str, args: &[String]) -> Result<i32> {
    let (mut cmd, entry) = app_command(home, name, args)?;
    run_entry(&mut cmd, &entry)
}

/// Resolve the installed app `name` and build the command that starts its
/// entry with `args`, the caller's cwd and the app environment. Also returns
/// the entry path, for error messages.
pub(crate) fn app_command(home: &Path, name: &str, args: &[String]) -> Result<(Command, PathBuf)> {
    let reg = Registry::load(home)?;
    let Some(record) = reg.get(name) else {
        bail!("{}", unknown_command(&reg, name));
    };
    if !record.path.is_dir() {
        bail!(
            "app '{name}' path {} no longer exists; run `sb app remove {name}` then reinstall",
            record.path.display()
        );
    }
    let manifest = AppManifest::load(&record.path).with_context(|| format!("app '{name}'"))?;
    let entry = manifest.entry_path(&record.path)?;
    if !entry.is_file() {
        bail!(
            "app '{name}': entry {} is missing; run `sb app update {name}`, or reinstall it",
            entry.display()
        );
    }
    let cwd = std::env::current_dir().context("reading the current directory")?;
    let mut cmd = entry_command(name, &entry)?;
    cmd.args(args)
        .current_dir(&cwd)
        .env("SB_HOME", home)
        .env("SB_APP_DIR", &record.path)
        .env("SB_APP_NAME", name);
    Ok((cmd, entry))
}

/// Replace this process with `cmd`. Returns only if the exec fails.
#[cfg(unix)]
fn run_entry(cmd: &mut Command, entry: &Path) -> Result<i32> {
    use std::os::unix::process::CommandExt;

    let err = cmd.exec();
    Err(anyhow::Error::new(err).context(format!("cannot run entry {}", entry.display())))
}

/// Run `cmd` as a child, wait for it, and return its exit code.
#[cfg(not(unix))]
fn run_entry(cmd: &mut Command, entry: &Path) -> Result<i32> {
    let status = cmd
        .status()
        .with_context(|| format!("starting {}", entry.display()))?;
    Ok(status.code().unwrap_or(1))
}

/// How to start `entry`: directly when it is executable, through bash when
/// it is a non-executable `.bash` / `.sh` script.
#[cfg(unix)]
fn entry_command(name: &str, entry: &Path) -> Result<Command> {
    if is_executable(entry) {
        return Ok(Command::new(entry));
    }
    let is_shell = entry
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e, "bash" | "sh"));
    if is_shell {
        let bash =
            find_bash().ok_or_else(|| anyhow!("app '{name}' needs bash, which is not on PATH"))?;
        let mut cmd = Command::new(bash);
        cmd.arg(bash_path_arg(entry));
        return Ok(cmd);
    }
    bail!(
        "app '{name}': {} is not executable; run `chmod +x {}` (package authors: commit it with \
         the executable bit set)",
        entry.display(),
        entry.display()
    )
}

/// Windows has no execute bit: every entry runs through bash (Git Bash).
#[cfg(not(unix))]
fn entry_command(name: &str, entry: &Path) -> Result<Command> {
    let bash = find_bash().ok_or_else(|| {
        anyhow!(
            "app '{name}': running sb apps on Windows needs bash (Git for Windows) on PATH; \
             native Windows apps are not supported yet"
        )
    })?;
    let mut cmd = Command::new(bash);
    cmd.arg(bash_path_arg(entry));
    Ok(cmd)
}

/// The error for `sb <name>` when `<name>` is neither a built-in nor an
/// installed app. It also covers typos of built-ins, since clap routes every
/// unknown subcommand here.
fn unknown_command(reg: &Registry, name: &str) -> String {
    let hint = closest(name, reg).map_or_else(String::new, |c| format!(" Did you mean `sb {c}`?"));
    format!(
        "`{name}` is not an sb command or an installed app.{hint} {}. Install an app with \
         `sb install <url|path>`; `sb --help` lists the built-in commands.",
        capitalize(&installed_names(reg))
    )
}

/// The built-in or installed name nearest to `name`, if it is a likely typo.
fn closest<'r>(name: &str, reg: &'r Registry) -> Option<&'r str> {
    let budget = if name.chars().count() <= 3 { 1 } else { 2 };
    BUILTIN_NAMES
        .iter()
        .copied()
        .chain(reg.names())
        .map(|cand| (edit_distance(name, cand), cand))
        .filter(|(d, _)| *d <= budget)
        .min_by_key(|(d, _)| *d)
        .map(|(_, cand)| cand)
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}
