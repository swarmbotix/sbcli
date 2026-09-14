//! `sb doctor` — environment validation.
//!
//! Reads the merged `sb.config.yml` and checks each declared path / tool.
//! Reports one line per check; **does not** stop at the first failure
//! (per L1 TDD test #10).

use std::path::{Path, PathBuf};
use std::process::Command;

use sb_core::SbCliConfig;

/// The zenoh crate compiled into this binary, from `Cargo.lock` at build time
/// (see `build.rs`). `"unknown"` when the build tree carried no lockfile.
pub const ZENOH_VERSION: &str = env!("SB_ZENOH_VERSION");

/// The iceoryx2 crate compiled into this binary. Same provenance as
/// [`ZENOH_VERSION`].
pub const ICEORYX2_VERSION: &str = env!("SB_ICEORYX2_VERSION");

/// Emitted by `build.rs` when it could not determine a version.
const UNKNOWN: &str = "unknown";

#[derive(Debug, Clone)]
pub struct CheckResult {
    pub name: &'static str,
    pub status: CheckStatus,
}

#[derive(Debug, Clone)]
pub enum CheckStatus {
    Ok(String),
    Fail(String),
    Skipped(String),
    /// Something is wrong but not fatal to `sb` itself — it does not affect
    /// the exit code. Reserved for state that breaks a *host* project rather
    /// than sb (stale iox2 C# under a Unity `Assets/` tree, say): failing
    /// would block every unrelated `sb doctor` until cleanup, and staying
    /// silent is what let the breakage accumulate.
    Warn(String),
}

impl CheckStatus {
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Ok(_))
    }
    pub fn is_fail(&self) -> bool {
        matches!(self, Self::Fail(_))
    }
    pub fn is_warn(&self) -> bool {
        matches!(self, Self::Warn(_))
    }
}

#[derive(Debug, Clone)]
pub struct Report {
    pub checks: Vec<CheckResult>,
}

impl Report {
    pub fn all_ok(&self) -> bool {
        self.checks.iter().all(|c| !c.status.is_fail())
    }

    pub fn render(&self, color: bool) -> String {
        use anstyle::{AnsiColor, Color, Style};
        let green = Style::new()
            .fg_color(Some(Color::Ansi(AnsiColor::Green)))
            .bold();
        let red = Style::new()
            .fg_color(Some(Color::Ansi(AnsiColor::Red)))
            .bold();
        let yellow = Style::new()
            .fg_color(Some(Color::Ansi(AnsiColor::Yellow)))
            .bold();

        let paint = |style: Style, tag: &str| -> String {
            if color {
                format!("{style}{tag}{style:#}")
            } else {
                tag.to_owned()
            }
        };

        let mut s = String::new();
        for c in &self.checks {
            match &c.status {
                CheckStatus::Ok(msg) => s.push_str(&format!(
                    "{}   {:<12} {msg}\n",
                    paint(green, "[OK]"),
                    c.name
                )),
                CheckStatus::Fail(msg) => {
                    s.push_str(&format!("{} {:<12} {msg}\n", paint(red, "[FAIL]"), c.name))
                }
                CheckStatus::Skipped(msg) => {
                    s.push_str(&format!(
                        "{} {:<12} {msg}\n",
                        paint(yellow, "[SKIP]"),
                        c.name
                    ));
                }
                CheckStatus::Warn(msg) => {
                    s.push_str(&format!(
                        "{} {:<12} {msg}\n",
                        paint(yellow, "[WARN]"),
                        c.name
                    ));
                }
            }
        }
        s
    }
}

/// The vault `sb message *` will actually operate on, as resolved by the
/// caller.
///
/// `sb doctor` reports on **this** root, not on cfg's `messages_root`. The
/// two diverge whenever cwd discovery kicks in, and a doctor that describes
/// a different vault than the one the very next command touches is worse
/// than no doctor at all.
#[derive(Debug, Clone, Copy)]
pub struct VaultView<'a> {
    pub root: &'a Path,
    /// Set when cwd is itself a style — only that style is in scope.
    pub pinned_style: Option<&'a str>,
    /// True when the root was discovered from cwd rather than read from config.
    pub local: bool,
}

/// Run every check against cfg's `messages_root`.
///
/// Callers that resolve a vault of their own (the CLI does, via cwd
/// discovery) should use [`run_with_vault`] so the report matches reality.
pub fn run(cfg: &SbCliConfig) -> Report {
    let root = sb_config::resolve_messages_root(cfg).unwrap_or_default();
    run_with_vault(
        cfg,
        &VaultView {
            root: &root,
            pinned_style: None,
            local: false,
        },
    )
}

/// Run every check against an explicitly resolved vault.
pub fn run_with_vault(cfg: &SbCliConfig, vault: &VaultView<'_>) -> Report {
    let checks = vec![
        check_exe("protoc", cfg.protoc.as_deref(), "--version"),
        check_exe("flatc", cfg.flatc.as_deref(), "--version"),
        check_transports(),
        check_lib(
            "libzenohc",
            cfg.libzenohc.as_deref(),
            ZENOH_VERSION,
            VersionPolicy::MinorCompatible,
        ),
        check_lib(
            "libiceoryx2",
            cfg.libiceoryx2.as_deref(),
            ICEORYX2_VERSION,
            VersionPolicy::Exact,
        ),
        check_tmux(cfg.tmux.as_deref()),
        check_message_styles(vault),
        check_unity_targets(vault),
        check_deprecated_keys(cfg),
        check_std_vault(vault),
    ];
    Report { checks }
}

/// Indent that lines a continuation row up under the message column:
/// `[OK]` + 3 spaces + a 12-wide name + 1 space.
const MSG_COL: &str = "                    ";

/// Check — `transports`. States which zenoh / iceoryx2 **crate** this `sb`
/// links, as opposed to the native `.so` the two `lib*` checks below load.
///
/// The two are independent and both matter: the crate version decides what
/// `sb pub` / `sb topic list` speak, the native library decides what generated
/// C/C++ consumers speak, and iceoryx2 refuses to open a segment across even a
/// patch difference. Without this row there is nothing to compare the library
/// against, and a mismatch reads as a green report.
///
/// Never fails — it reports a fact about the binary, not a condition of the
/// box.
fn check_transports() -> CheckResult {
    if ZENOH_VERSION == UNKNOWN && ICEORYX2_VERSION == UNKNOWN {
        return ok(
            "transports",
            "linked crate versions unavailable (built without Cargo.lock)".to_owned(),
        );
    }
    ok(
        "transports",
        format!("zenoh {ZENOH_VERSION}, iceoryx2 {ICEORYX2_VERSION} (linked into this sb)"),
    )
}

/// Check — `message styles`. Lists every style under `messages_root` with
/// its message count and whether its bindings are built.
///
/// There is no "active" style to mark. A message name is fully qualified
/// (`<style>/<namespace>/<Leaf>`), so every style here is equally usable
/// from any module at any time — the report says what exists, not what is
/// selected.
///
/// Fails when the root holds no styles at all, which is the one state that
/// makes `sb message`/`sb pub` unusable.
fn check_message_styles(vault: &VaultView<'_>) -> CheckResult {
    let root = vault.root;
    // Styles come from the resolved root, and honor the cwd pin — the same
    // two rules `sb message compile` uses. Reading them from cfg instead
    // would describe a vault the next command is not going to touch.
    let all = sb_vault::Vault::at(root).styles();
    let available: Vec<String> = match vault.pinned_style {
        Some(p) => all.into_iter().filter(|s| s == p).collect(),
        None => all,
    };
    if available.is_empty() {
        return fail(
            "message styles",
            format!("no styles found under {}", root.display()),
        );
    }

    let w = available.iter().map(String::len).max().unwrap_or(0);
    let mut msg = root.display().to_string();
    if vault.local {
        msg.push_str(" (discovered from cwd — overrides `messages_root`)");
    }
    if let Some(p) = vault.pinned_style {
        msg.push_str(&format!(
            "\n{MSG_COL}cwd is the `{p}` style — only it is in scope"
        ));
    }
    for st in &available {
        let dir = root.join(st);
        let msgs = count_ext(&dir.join("message_definitions"), "proto");
        let built = count_any(&dir.join("message_targets")) > 0;
        let state = if msgs == 0 {
            "no definitions".to_owned()
        } else if built {
            "targets built".to_owned()
        } else {
            format!("NOT COMPILED — run `sb message compile --style {st}`")
        };
        msg.push_str(&format!("\n{MSG_COL}{st:<w$}  {msgs:>4} msg  {state}"));
    }
    ok("message styles", msg)
}

/// Check — `unity targets`. Flags generated C# inside a Unity project that
/// predates the C# 9 emitter and therefore will not compile there.
///
/// All three backends are emitted everywhere, including into Unity trees — the
/// generated code is consumed by more than the host project, so suppressing a
/// backend to suit one consumer would deprive the others. What makes that safe
/// is that the emitter targets C# 9 / .NET Standard 2.1.
///
/// Files produced by `sb` **before** that change still carry file-scoped
/// namespaces (C# 10, `error CS8773`) and `[InlineArray]` (.NET 8), and fixing
/// the emitter does not rewrite them — only a recompile does. The same is true
/// of the pre-0.1.39 inline array wrapper, which declared one struct in every
/// message file that used it (`error CS0101`). This check finds all of them by
/// content rather than by directory name, so it stays accurate no matter which
/// backends a style emits.
///
/// Warns rather than fails: the breakage is in the host project, not in `sb`.
fn check_unity_targets(view: &VaultView<'_>) -> CheckResult {
    let vault = sb_vault::Vault::at(view.root);
    let styles = match view.pinned_style {
        Some(p) => vec![p.to_owned()],
        None => vault.styles(),
    };

    let mut offenders: Vec<String> = Vec::new();
    for st in &styles {
        let targets = vault.targets_dir(st);
        if sb_vault::unity_project_root(&targets).is_none() {
            continue;
        }
        let stale = count_stale_csharp(&targets);
        if stale > 0 {
            offenders.push(format!(
                "{st}: {stale} generated .cs file(s) predate the current emitter"
            ));
            offenders.push(format!("  {}", targets.display()));
        }
    }

    if offenders.is_empty() {
        return ok(
            "unity targets",
            "no stale generated C# in a Unity tree".to_owned(),
        );
    }
    let mut msg = String::from("generated C# Unity cannot compile:");
    for line in &offenders {
        msg.push_str(&format!("\n{MSG_COL}{line}"));
    }
    msg.push_str(&format!(
        "\n{MSG_COL}File-scoped namespaces (C# 10) / [InlineArray] (.NET 8); Unity caps at C# 9.\
         \n{MSG_COL}Or an inline `_Array<N>` wrapper, which collides once two messages share\
         \n{MSG_COL}an element type (CS0101). Wrappers now live in iox2/_arrays/.\
         \n{MSG_COL}Fix: re-run `sb message compile` — the emitter handles both now.\
         \n{MSG_COL}These trees are usually committed, so commit the regenerated files too."
    ));
    warn("unity targets", msg)
}

/// Count `.cs` files under `dir` that a current `sb` would not have written.
///
/// Three markers, each a thing the emitter used to produce:
///
/// - `namespace <x>;` — file-scoped, C# 10, `CS8773`
/// - `[InlineArray` — .NET 8 / C# 12
/// - a `_Array<N>` struct declared in a message file — pre-0.1.39 inline
///   wrapper, `CS0101` as soon as a second message shares the element type
///
/// Reading the files is what makes this robust: a directory name says nothing
/// about which `sb` wrote it. The wrapper marker deliberately skips
/// `iox2/_arrays/`, which is where a *current* `sb` declares those structs.
fn count_stale_csharp(dir: &Path) -> usize {
    let mut n = 0;
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut stack: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    while let Some(p) = stack.pop() {
        if p.is_dir() {
            if let Ok(rd) = std::fs::read_dir(&p) {
                stack.extend(rd.flatten().map(|e| e.path()));
            }
            continue;
        }
        if p.extension().and_then(|e| e.to_str()) != Some("cs") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        let file_scoped_ns = text
            .lines()
            .any(|l| l.starts_with("namespace ") && l.trim_end().ends_with(';'));
        let in_arrays_dir = p
            .parent()
            .and_then(|d| d.file_name())
            .and_then(|s| s.to_str())
            == Some("_arrays");
        let inline_wrapper =
            !in_arrays_dir && text.lines().any(|l| declares_array_wrapper(l.trim_start()));
        if file_scoped_ns || text.contains("[InlineArray") || inline_wrapper {
            n += 1;
        }
    }
    n
}

/// `public unsafe struct geometry_msgs_Point_Array256 {` → true.
///
/// The `_Array<digits>` suffix is the generated wrapper's shape; a hand-written
/// type would have to copy it exactly to trip this.
fn declares_array_wrapper(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("public unsafe struct ") else {
        return false;
    };
    let name = rest
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .next()
        .unwrap_or_default();
    let Some(idx) = name.rfind("_Array") else {
        return false;
    };
    let tail = &name[idx + "_Array".len()..];
    !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit())
}

/// Check — `config keys`. Reports pre-0.1.35 keys that are still present in
/// the config but no longer do anything.
///
/// They are parsed (so an old file still loads) yet ignored, which is
/// exactly the combination that silently misleads: someone sets
/// `message_style: ros2`, sees no error, and assumes it took effect.
fn check_deprecated_keys(cfg: &SbCliConfig) -> CheckResult {
    let mut dead: Vec<&str> = Vec::new();
    if cfg.message_style.is_some() {
        dead.push("message_style");
    }
    if cfg.message_definitions.is_some() {
        dead.push("message_definitions");
    }
    if cfg.message_targets.is_some() {
        dead.push("message_targets");
    }
    if dead.is_empty() {
        return ok("config keys", "no deprecated keys".to_owned());
    }
    fail(
        "config keys",
        format!(
            "{} set but IGNORED — message names are fully qualified now \
             (<style>/<namespace>/<Leaf>), so there is no active style to \
             select. Delete the line(s) from sb.config.yml.",
            dead.join(", ")
        ),
    )
}

/// Count files with `ext` anywhere under `dir`. 0 if absent.
fn count_ext(dir: &Path, ext: &str) -> usize {
    walk_count(dir, &|p| p.extension().is_some_and(|e| e == ext))
}

/// Count all files under `dir`. 0 if absent.
fn count_any(dir: &Path) -> usize {
    walk_count(dir, &|_| true)
}

fn walk_count(dir: &Path, keep: &dyn Fn(&Path) -> bool) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut n = 0;
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            n += walk_count(&p, keep);
        } else if keep(&p) {
            n += 1;
        }
    }
    n
}

fn check_exe(name: &'static str, path: Option<&Path>, version_flag: &str) -> CheckResult {
    let Some(p) = path else {
        return fail(name, format!("{name} path not set in sb.config.yml"));
    };
    if !p.exists() {
        return fail(name, format!("{name} not found at {}", p.display()));
    }
    let out = Command::new(p).arg(version_flag).output();
    match out {
        Ok(o) if o.status.success() => {
            let line = String::from_utf8_lossy(&o.stdout)
                .lines()
                .next()
                .unwrap_or("")
                .to_owned();
            ok(name, format!("{} ({line})", p.display()))
        }
        Ok(o) => fail(
            name,
            format!(
                "{} {} exited {}: {}",
                p.display(),
                version_flag,
                o.status,
                String::from_utf8_lossy(&o.stderr).trim()
            ),
        ),
        Err(e) => fail(
            name,
            format!("{} {} failed: {e}", p.display(), version_flag),
        ),
    }
}

/// How closely a native library's version must track the crate `sb` links
/// before the difference is worth reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VersionPolicy {
    /// Every digit must match. iceoryx2 writes its full `major.minor.patch`
    /// into each shared-memory segment and `open()` compares for equality, so
    /// 0.9.0 cannot see a service created by 0.9.3 — it gets `VersionMismatch`
    /// and an empty topic list against a live publisher.
    Exact,
    /// `major.minor` must match. Zenoh keeps patch releases wire-compatible,
    /// so flagging those would be noise.
    MinorCompatible,
}

fn check_lib(
    name: &'static str,
    path: Option<&Path>,
    linked: &str,
    policy: VersionPolicy,
) -> CheckResult {
    let Some(p) = path else {
        return fail(name, format!("{name} path not set in sb.config.yml"));
    };
    if !p.exists() {
        return fail(name, format!("{name} not found at {}", p.display()));
    }
    // dlopen attempt — confirms the shared object is loadable.
    // SAFETY: opening a known shared object by absolute path; libloading
    // wraps `dlopen` directly. We immediately drop the handle.
    let result = unsafe { libloading::Library::new(p) };
    if let Err(e) = result {
        return fail(name, format!("dlopen({}) failed: {e}", p.display()));
    }

    let native = native_lib_version(p);
    let shown = match &native {
        Some(v) => format!("v{v}"),
        None => "version unknown".to_owned(),
    };
    let msg = format!("{} ({shown}, dlopen OK)", p.display());

    // Only an *observed* disagreement is worth a word. An undetected library
    // version or an `unknown` linked version means nothing was compared, and
    // saying so on every row would bury the case that matters.
    let Some(native) = native else {
        return ok(name, msg);
    };
    if linked == UNKNOWN || !version_drift(linked, &native, policy) {
        return ok(name, msg);
    }

    // Warn, not fail: the library is what generated C/C++ consumers load, so
    // the breakage lands in a host project rather than in `sb` itself, and
    // flipping the exit code would block every unrelated `sb doctor` run.
    let detail = match policy {
        VersionPolicy::Exact => format!(
            "sb links {c} {linked}, this library is {native}.\
             \n{MSG_COL}Segments are opened on exact version equality, so C/C++ consumers built\
             \n{MSG_COL}against this library will get VersionMismatch and an empty topic list.\
             \n{MSG_COL}Fix: sb config set {name} <path to a {linked} install>",
            c = crate_of(name),
        ),
        VersionPolicy::MinorCompatible => format!(
            "sb links {c} {linked}, this library is {native}.\
             \n{MSG_COL}Patch releases interoperate; a differing major.minor is not guaranteed to.\
             \n{MSG_COL}Fix: sb config set {name} <path to a {linked} install>",
            c = crate_of(name),
        ),
    };
    warn(name, format!("{msg}\n{MSG_COL}{detail}"))
}

/// The crate name behind a `lib*` check name, for message text.
fn crate_of(check: &str) -> &'static str {
    if check == "libzenohc" {
        "zenoh"
    } else {
        "iceoryx2"
    }
}

/// True when `native` differs from `linked` by more than `policy` allows.
///
/// Unparseable input is never drift: a library whose version could only be
/// guessed at should not produce a warning that reads like a measurement.
fn version_drift(linked: &str, native: &str, policy: VersionPolicy) -> bool {
    let (Some(l), Some(n)) = (semver_triple(linked), semver_triple(native)) else {
        return false;
    };
    match policy {
        VersionPolicy::Exact => l != n,
        VersionPolicy::MinorCompatible => (l.0, l.1) != (n.0, n.1),
    }
}

/// `"v0.9.3"` / `"0.9.3"` → `(0, 9, 3)`. Anything else → `None`.
fn semver_triple(v: &str) -> Option<(u64, u64, u64)> {
    // Pre-release / build metadata is dropped: `1.9.0-rc.1` compares as 1.9.0.
    let core = v
        .trim()
        .trim_start_matches('v')
        .split(['-', '+'])
        .next()
        .unwrap_or_default();
    let mut it = core.split('.');
    let a = it.next()?.parse().ok()?;
    let b = it.next()?.parse().ok()?;
    let c = it.next()?.parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some((a, b, c))
}

/// The version of the native library at `lib`, if the install says so anywhere.
///
/// Neither `libzenohc` nor `libiceoryx2_ffi_c` exports a version symbol, so
/// there is nothing to call: `nm -D` on both lists only API entry points. What
/// the installs *do* carry is metadata beside the binary and a version in the
/// path, and the probes below read those in descending order of authority:
///
/// 1. `<libdir>/pkgconfig/*.pc` → the `Version:` field (zenoh-c ships this).
/// 2. `<libdir>/cmake/*/…ConfigVersion.cmake` → `set(PACKAGE_VERSION "…")`
///    (both ship this; iceoryx2 ships one per component).
/// 3. A `vX.Y.Z` / `X.Y.Z` component of the path — which is why the path is
///    canonicalized first: the configured path usually runs through a
///    `current` symlink that hides the version it points at.
/// 4. A `libfoo.so.X.Y.Z` filename suffix.
///
/// All four are file-name and text probes, so they behave the same against a
/// Windows `zenohc.dll` install laid out the same way; where they all miss,
/// the caller prints "version unknown" rather than guessing.
fn native_lib_version(lib: &Path) -> Option<String> {
    let real = std::fs::canonicalize(lib).unwrap_or_else(|_| lib.to_path_buf());
    let stem = real
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_owned();
    let libdir = real.parent()?;
    version_from_pkgconfig(libdir, &stem)
        .or_else(|| version_from_cmake(libdir, &stem))
        .or_else(|| version_from_path(&real))
        .or_else(|| version_from_filename(&stem))
}

/// `Version:` out of the `.pc` files next to the library.
fn version_from_pkgconfig(libdir: &Path, lib_file: &str) -> Option<String> {
    let mut found: Vec<(String, String)> = Vec::new();
    for p in files_in(&libdir.join("pkgconfig")) {
        if p.extension().and_then(|e| e.to_str()) != Some("pc") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        let version = text.lines().find_map(|l| {
            let v = l.strip_prefix("Version:")?.trim();
            semver_triple(v).map(|_| v.to_owned())
        });
        if let (Some(v), Some(name)) = (version, p.file_stem().and_then(|s| s.to_str())) {
            found.push((name.to_owned(), v));
        }
    }
    pick_by_affinity(found, lib_file)
}

/// `set(PACKAGE_VERSION "X.Y.Z")` out of the CMake package files.
fn version_from_cmake(libdir: &Path, lib_file: &str) -> Option<String> {
    let mut found: Vec<(String, String)> = Vec::new();
    for pkg in dirs_in(&libdir.join("cmake")) {
        let Some(pkg_name) = pkg.file_name().and_then(|s| s.to_str()).map(str::to_owned) else {
            continue;
        };
        for f in files_in(&pkg) {
            if !f
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|n| n.ends_with("ConfigVersion.cmake"))
            {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&f) else {
                continue;
            };
            if let Some(v) = cmake_package_version(&text) {
                found.push((pkg_name.clone(), v));
                break;
            }
        }
    }
    pick_by_affinity(found, lib_file)
}

/// The first `set(PACKAGE_VERSION "X.Y.Z")` in a CMake version file.
fn cmake_package_version(text: &str) -> Option<String> {
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("set(PACKAGE_VERSION") else {
            continue;
        };
        let Some((_, after)) = rest.split_once('"') else {
            continue;
        };
        let Some((v, _)) = after.split_once('"') else {
            continue;
        };
        if semver_triple(v).is_some() {
            return Some(v.to_owned());
        }
    }
    None
}

/// The deepest `vX.Y.Z` / `X.Y.Z` directory component of a canonical path.
///
/// Deepest first because installs nest the version under the product
/// (`/opt/iceoryx2/v0.9.3/lib/…`), and a shallower match would more likely be
/// something else entirely.
fn version_from_path(real: &Path) -> Option<String> {
    real.ancestors()
        .skip(1)
        .filter_map(|d| d.file_name().and_then(|s| s.to_str()))
        .find(|c| semver_triple(c).is_some())
        .map(|c| c.trim_start_matches('v').to_owned())
}

/// `libfoo.so.1.9.0` → `1.9.0`.
fn version_from_filename(lib_file: &str) -> Option<String> {
    let (_, tail) = lib_file.split_once(".so.")?;
    semver_triple(tail).map(|_| tail.to_owned())
}

/// Reduce several `(package name, version)` hits to one.
///
/// A unanimous set needs no arbitration — that is the iceoryx2 case, where
/// four CMake components all state the same release. Otherwise prefer the
/// package whose name shares the longest prefix with the library file
/// (`iceoryx2-c` over `iceoryx2-bb-cxx` for `libiceoryx2_ffi_c.so`), breaking
/// remaining ties on name so the answer is stable across filesystem order.
fn pick_by_affinity(mut found: Vec<(String, String)>, lib_file: &str) -> Option<String> {
    if found.is_empty() {
        return None;
    }
    if found.iter().all(|(_, v)| *v == found[0].1) {
        return Some(found.remove(0).1);
    }
    let target = normalize_name(lib_file);
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
        .into_iter()
        .max_by_key(|(name, _)| common_prefix_len(&normalize_name(name), &target))
        .map(|(_, v)| v)
}

/// `libiceoryx2_ffi_c.so` / `iceoryx2-c` → `iceoryx2ffic` / `iceoryx2c`.
fn normalize_name(s: &str) -> String {
    let s = s.split('.').next().unwrap_or(s);
    let s = s.strip_prefix("lib").unwrap_or(s);
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn common_prefix_len(a: &str, b: &str) -> usize {
    a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count()
}

fn files_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    rd.flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect()
}

fn dirs_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    rd.flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect()
}

fn check_tmux(path: Option<&Path>) -> CheckResult {
    // tmux uses `-V` (uppercase), not `--version` — quirky.
    if let Some(p) = path {
        return check_exe("tmux", Some(p), "-V");
    }
    match which::which("tmux") {
        Ok(p) => check_exe("tmux", Some(&p), "-V"),
        Err(_) => fail("tmux", "tmux not on PATH and not set in sb.config.yml"),
    }
}

/// Check — `std vault`. Walks `<messages_root>/ros2/message_definitions/std/`
/// and confirms every file the forge-bundled `STD_BUNDLE` ships is present.
///
/// Always the **ros2** style: that is where `std/` lives and where the
/// embedded bundle installs. It is not affected by anything the user is
/// currently working on, because nothing is "currently selected" any more.
fn check_std_vault(view: &VaultView<'_>) -> CheckResult {
    let vault = sb_vault::Vault::at(view.root);
    let std_dir = vault.std_dir();
    // A standalone message repo legitimately has no `ros2` style — the forge
    // bundle is installed into the CONFIGURED vault only. Demanding std/ here
    // would fail every such repo for doing nothing wrong.
    if view.local && !std_dir.is_dir() {
        return skip(
            "std vault",
            format!(
                "{} has no ros2 style — std bundle lives in the configured vault, not a discovered one",
                view.root.display()
            ),
        );
    }
    if !std_dir.is_dir() {
        return fail(
            "std vault",
            format!(
                "{} not found (run `sb message list` to install the bundle)",
                std_dir.display()
            ),
        );
    }
    let missing = vault.missing_std_files();
    if missing.is_empty() {
        ok("std vault", format!("{} populated", std_dir.display()))
    } else {
        fail(
            "std vault",
            format!(
                "{} missing {} expected file(s) (run `sb message list` to install)",
                std_dir.display(),
                missing.len()
            ),
        )
    }
}

fn ok(name: &'static str, msg: impl Into<String>) -> CheckResult {
    CheckResult {
        name,
        status: CheckStatus::Ok(msg.into()),
    }
}
fn fail(name: &'static str, msg: impl Into<String>) -> CheckResult {
    CheckResult {
        name,
        status: CheckStatus::Fail(msg.into()),
    }
}
fn warn(name: &'static str, msg: impl Into<String>) -> CheckResult {
    CheckResult {
        name,
        status: CheckStatus::Warn(msg.into()),
    }
}
fn skip(name: &'static str, msg: impl Into<String>) -> CheckResult {
    CheckResult {
        name,
        status: CheckStatus::Skipped(msg.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A config pinned at a path that does not exist, so `run` cannot reach
    /// the developer's real `~/.swarmbotix`. `run` derives every path from
    /// the config now, so an unpinned `default()` would silently test the
    /// dev box instead of the fixture.
    fn isolated_cfg() -> SbCliConfig {
        SbCliConfig {
            sb_home_dir: Some(PathBuf::from("/nonexistent/sb_home")),
            ..Default::default()
        }
    }

    // ── native library version discovery ──────────────────────────────

    /// An install tree `<dir>/<prefix_dirs>/lib/<file>`, with the library
    /// file created so `canonicalize` resolves.
    fn lib_at(dir: &Path, prefix_dirs: &str, file: &str) -> PathBuf {
        let libdir = dir.join(prefix_dirs).join("lib");
        std::fs::create_dir_all(&libdir).unwrap();
        let lib = libdir.join(file);
        std::fs::write(&lib, b"not a real elf").unwrap();
        lib
    }

    fn write_pc(libdir: &Path, name: &str, version: &str) {
        let d = libdir.join("pkgconfig");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join(format!("{name}.pc")),
            format!("Name: {name}\nVersion: {version}\nLibs: -l{name}\n"),
        )
        .unwrap();
    }

    fn write_cmake(libdir: &Path, pkg: &str, version: &str) {
        let d = libdir.join("cmake").join(pkg);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join(format!("{pkg}ConfigVersion.cmake")),
            format!("set(PACKAGE_VERSION \"{version}\")\n"),
        )
        .unwrap();
    }

    #[test]
    fn pkgconfig_version_is_preferred() {
        let d = tempfile::tempdir().unwrap();
        let lib = lib_at(d.path(), "zenoh-c/1.9.0", "libzenohc.so");
        let libdir = lib.parent().unwrap();
        write_pc(libdir, "zenohc", "1.9.0");
        // Deliberately disagrees, to prove which probe answered.
        write_cmake(libdir, "zenohc", "9.9.9");
        assert_eq!(native_lib_version(&lib).as_deref(), Some("1.9.0"));
    }

    #[test]
    fn cmake_version_is_used_when_no_pkgconfig() {
        let d = tempfile::tempdir().unwrap();
        let lib = lib_at(d.path(), "iceoryx2/install", "libiceoryx2_ffi_c.so");
        let libdir = lib.parent().unwrap();
        // The real install ships one per component, all agreeing.
        for pkg in ["iceoryx2-c", "iceoryx2-cxx", "iceoryx2-bb-cxx"] {
            write_cmake(libdir, pkg, "0.9.3");
        }
        assert_eq!(native_lib_version(&lib).as_deref(), Some("0.9.3"));
    }

    #[test]
    fn disagreeing_cmake_packages_resolve_by_name_affinity() {
        let d = tempfile::tempdir().unwrap();
        let lib = lib_at(d.path(), "iceoryx2/install", "libiceoryx2_ffi_c.so");
        let libdir = lib.parent().unwrap();
        write_cmake(libdir, "iceoryx2-c", "0.9.3");
        write_cmake(libdir, "unrelated-thing", "3.0.0");
        assert_eq!(native_lib_version(&lib).as_deref(), Some("0.9.3"));
    }

    #[test]
    fn version_comes_from_the_path_when_metadata_is_absent() {
        let d = tempfile::tempdir().unwrap();
        let lib = lib_at(d.path(), "iceoryx2/v0.9.3", "libiceoryx2_ffi_c.so");
        assert_eq!(native_lib_version(&lib).as_deref(), Some("0.9.3"));
    }

    #[test]
    fn a_current_symlink_does_not_hide_the_version() {
        // The configured path is `<d>/iceoryx2/current/lib/…`; only the
        // canonical path carries `v0.9.3`.
        let d = tempfile::tempdir().unwrap();
        let lib = lib_at(d.path(), "iceoryx2/v0.9.3", "libiceoryx2_ffi_c.so");
        let link = d.path().join("iceoryx2").join("current");
        #[cfg(unix)]
        std::os::unix::fs::symlink(d.path().join("iceoryx2").join("v0.9.3"), &link).unwrap();
        #[cfg(not(unix))]
        std::os::windows::fs::symlink_dir(d.path().join("iceoryx2").join("v0.9.3"), &link).unwrap();
        let through_link = link.join("lib").join("libiceoryx2_ffi_c.so");
        assert_eq!(
            native_lib_version(&through_link).as_deref(),
            native_lib_version(&lib).as_deref()
        );
        assert_eq!(native_lib_version(&through_link).as_deref(), Some("0.9.3"));
    }

    #[test]
    fn soname_suffix_is_the_last_resort() {
        let d = tempfile::tempdir().unwrap();
        let lib = lib_at(d.path(), "somewhere", "libzenohc.so.1.9.0");
        assert_eq!(native_lib_version(&lib).as_deref(), Some("1.9.0"));
    }

    #[test]
    fn an_install_that_states_nothing_yields_no_version() {
        let d = tempfile::tempdir().unwrap();
        let lib = lib_at(d.path(), "somewhere", "libzenohc.so");
        assert_eq!(native_lib_version(&lib), None);
    }

    // ── drift policy ──────────────────────────────────────────────────

    #[test]
    fn iceoryx2_patch_difference_is_drift() {
        assert!(version_drift("0.9.3", "0.9.0", VersionPolicy::Exact));
        assert!(!version_drift("0.9.3", "0.9.3", VersionPolicy::Exact));
        // The library is free to say `v0.9.3` — the segment header does not.
        assert!(!version_drift("0.9.3", "v0.9.3", VersionPolicy::Exact));
    }

    #[test]
    fn zenoh_tolerates_patch_but_not_minor() {
        assert!(!version_drift(
            "1.9.0",
            "1.9.2",
            VersionPolicy::MinorCompatible
        ));
        assert!(version_drift(
            "1.9.0",
            "1.8.4",
            VersionPolicy::MinorCompatible
        ));
    }

    #[test]
    fn an_unreadable_version_is_never_reported_as_drift() {
        assert!(!version_drift("0.9.3", "nightly", VersionPolicy::Exact));
        assert!(!version_drift(UNKNOWN, "0.9.3", VersionPolicy::Exact));
    }

    #[test]
    fn transports_row_names_both_crates() {
        let msg = msg_of(check_transports());
        assert!(msg.contains(ZENOH_VERSION), "{msg}");
        assert!(msg.contains(ICEORYX2_VERSION), "{msg}");
        assert!(check_transports().status.is_ok());
    }

    #[test]
    fn linked_versions_are_baked_in_from_the_lockfile() {
        // A build inside this workspace always has Cargo.lock, so `unknown`
        // here means build.rs stopped finding it.
        assert_ne!(ZENOH_VERSION, UNKNOWN);
        assert_ne!(ICEORYX2_VERSION, UNKNOWN);
        assert!(semver_triple(ZENOH_VERSION).is_some());
        assert!(semver_triple(ICEORYX2_VERSION).is_some());
    }

    #[test]
    fn missing_protoc_path_produces_fail() {
        let cfg = isolated_cfg();
        let report = run(&cfg);
        let protoc = report.checks.iter().find(|c| c.name == "protoc").unwrap();
        assert!(matches!(protoc.status, CheckStatus::Fail(_)));
    }

    #[test]
    fn each_missing_dep_reported_separately() {
        let cfg = isolated_cfg();
        let report = run(&cfg);
        let failures: Vec<&str> = report
            .checks
            .iter()
            .filter(|c| c.status.is_fail())
            .map(|c| c.name)
            .collect();
        // Each check produces its own line — not aggregated.
        assert!(failures.contains(&"protoc"));
        assert!(failures.contains(&"flatc"));
        assert!(failures.contains(&"libzenohc"));
        assert!(failures.contains(&"libiceoryx2"));
        assert!(failures.contains(&"std vault"));
    }

    #[test]
    fn render_marks_each_status() {
        let cfg = isolated_cfg();
        let report = run(&cfg);
        let out = report.render(false);
        assert!(out.contains("[FAIL]"));
        assert!(out.contains("protoc"));
    }

    #[test]
    fn render_colored_wraps_tag_with_ansi() {
        let cfg = isolated_cfg();
        let report = run(&cfg);
        let out = report.render(true);
        // anstyle's red+bold reset sequence wraps the [FAIL] tag.
        assert!(out.contains("\x1b["));
        assert!(out.contains("[FAIL]"));
    }

    // ── message style listing ─────────────────────────────────────────

    /// Build a messages root with the given styles; each tuple is
    /// (name, has_defs, has_targets).
    fn styles_at(dir: &Path, styles: &[(&str, bool, bool)]) {
        for (name, defs, targets) in styles {
            let s = dir.join("messages").join(name);
            if *defs {
                let d = s.join("message_definitions").join("ns");
                std::fs::create_dir_all(&d).unwrap();
                std::fs::write(d.join("A.proto"), "syntax=\"proto3\";").unwrap();
            }
            if *targets {
                let t = s.join("message_targets").join("iox2");
                std::fs::create_dir_all(&t).unwrap();
                std::fs::write(t.join("A.rs"), "// generated").unwrap();
            }
        }
    }

    fn cfg_at(dir: &Path) -> SbCliConfig {
        SbCliConfig {
            sb_home_dir: Some(dir.to_path_buf()),
            messages_root: Some(dir.join("messages")),
            ..Default::default()
        }
    }

    /// The configured (non-discovered) vault under `<dir>/messages` — what
    /// `sb doctor` sees with no cwd discovery in play.
    fn view_at(root: &Path) -> VaultView<'_> {
        VaultView {
            root,
            pinned_style: None,
            local: false,
        }
    }

    fn msg_of(r: CheckResult) -> String {
        match r.status {
            CheckStatus::Ok(m)
            | CheckStatus::Fail(m)
            | CheckStatus::Skipped(m)
            | CheckStatus::Warn(m) => m,
        }
    }

    #[test]
    fn every_style_is_listed_with_no_active_marker() {
        let d = tempfile::tempdir().unwrap();
        styles_at(
            d.path(),
            &[("ros2", true, true), ("swarmbotix", true, true)],
        );
        let msg = msg_of(check_message_styles(&view_at(&d.path().join("messages"))));
        assert!(msg.contains("ros2"), "{msg}");
        assert!(msg.contains("swarmbotix"), "{msg}");
        // Nothing is "selected" — a message name carries its own style, so
        // marking one would imply a mode that no longer exists.
        assert!(!msg.contains('*'), "no active marker expected: {msg}");
    }

    #[test]
    fn a_style_with_definitions_but_no_targets_is_called_out() {
        let d = tempfile::tempdir().unwrap();
        styles_at(
            d.path(),
            &[("ros2", true, true), ("swarmbotix", true, false)],
        );
        let msg = msg_of(check_message_styles(&view_at(&d.path().join("messages"))));
        assert!(msg.contains("NOT COMPILED"), "{msg}");
        assert!(
            msg.contains("sb message compile --style swarmbotix"),
            "must name the fix: {msg}"
        );
    }

    /// `sb doctor` must describe the vault the NEXT command will touch. When
    /// cwd discovery wins, that is the discovered root — reporting cfg's
    /// `messages_root` instead would name a tree nothing is about to use.
    #[test]
    fn discovered_root_is_reported_and_labeled() {
        let d = tempfile::tempdir().unwrap();
        styles_at(d.path(), &[("vrobots_msgs", true, true)]);
        let root = d.path().join("messages");
        let view = VaultView {
            root: &root,
            pinned_style: None,
            local: true,
        };
        let msg = msg_of(check_message_styles(&view));
        assert!(
            msg.contains("discovered from cwd"),
            "must say where it came from: {msg}"
        );
        assert!(msg.contains("vrobots_msgs"), "{msg}");
    }

    /// Standing inside a style pins it: doctor lists that style only, matching
    /// what a bare `sb message compile` would build.
    #[test]
    fn pinned_style_narrows_the_listing_to_that_style() {
        let d = tempfile::tempdir().unwrap();
        styles_at(
            d.path(),
            &[("ros2", true, true), ("vrobots_msgs", true, true)],
        );
        let root = d.path().join("messages");
        let view = VaultView {
            root: &root,
            pinned_style: Some("vrobots_msgs"),
            local: true,
        };
        let msg = msg_of(check_message_styles(&view));
        assert!(msg.contains("only it is in scope"), "{msg}");
        assert!(msg.contains("vrobots_msgs"), "{msg}");
        assert!(
            !msg.contains("ros2"),
            "pinned style must exclude siblings: {msg}"
        );
    }

    /// A standalone message repo has no `ros2` style and should not be failed
    /// for it — the forge bundle only ever installs into the configured vault.
    #[test]
    fn std_vault_is_skipped_not_failed_for_a_discovered_root() {
        let d = tempfile::tempdir().unwrap();
        styles_at(d.path(), &[("vrobots_msgs", true, true)]);
        let root = d.path().join("messages");
        let local = VaultView {
            root: &root,
            pinned_style: Some("vrobots_msgs"),
            local: true,
        };
        let r = check_std_vault(&local);
        assert!(
            !r.status.is_fail(),
            "discovered root must not fail std vault"
        );
        assert!(
            matches!(r.status, CheckStatus::Skipped(_)),
            "expected a skip"
        );

        // The configured vault still fails loudly — that one is meant to have std/.
        let configured = VaultView {
            root: &root,
            pinned_style: None,
            local: false,
        };
        assert!(check_std_vault(&configured).status.is_fail());
    }

    #[test]
    fn no_styles_at_all_is_a_failure() {
        let d = tempfile::tempdir().unwrap();
        let r = check_message_styles(&view_at(&d.path().join("messages")));
        assert!(r.status.is_fail());
    }

    // ── unity targets ─────────────────────────────────────────────────

    /// Build `<dir>/virtual_robots/Assets/Scripts` as a vault root, with one
    /// style holding a single generated `.cs` at `iox2/<sub>/<file>`.
    fn unity_vault_with(dir: &Path, style: &str, sub: &str, file: &str, body: &str) -> PathBuf {
        let unity = dir.join("virtual_robots");
        std::fs::create_dir_all(unity.join("ProjectSettings")).unwrap();
        let scripts = unity.join("Assets").join("Scripts");
        let s = scripts.join(style);
        std::fs::create_dir_all(s.join("message_definitions").join("ns")).unwrap();
        let t = s.join("message_targets").join("iox2").join(sub);
        std::fs::create_dir_all(&t).unwrap();
        std::fs::write(t.join(file), body).unwrap();
        scripts
    }

    /// As above, with generated C# that is either pre-C#9 (`stale`) or current.
    fn unity_vault(dir: &Path, style: &str, stale: bool) -> PathBuf {
        let body = if stale {
            "namespace swarmbotix_states;
[InlineArray(4)]
public struct A { }
"
        } else {
            "namespace swarmbotix_states
{
    public struct A { }
}
"
        };
        unity_vault_with(dir, style, "states", "A.cs", body)
    }

    /// A message file that declares its own array wrapper — the pre-0.1.39
    /// shape. Legal on its own; `CS0101` the moment a second message in the
    /// package shares the element type, which is how it was reported.
    const INLINE_WRAPPER: &str = "namespace swarmbotix_services
{
    public unsafe struct SrvOMRWallMsg { }

    [StructLayout(LayoutKind.Sequential, Pack = 1)]
    public unsafe struct swarmbotix_primitives_Vec3Msg_Array256 {
        public const int Length = 256;
    }
}
";

    /// Files written by an older `sb` still carry file-scoped namespaces and
    /// [InlineArray]; fixing the emitter does not rewrite them, only a
    /// recompile does. Detection is by content, not by directory name.
    #[test]
    fn pre_csharp9_generated_code_in_a_unity_tree_is_warned_about() {
        let d = tempfile::tempdir().unwrap();
        let root = unity_vault(d.path(), "vrobots_msgs", true);
        let view = VaultView {
            root: &root,
            pinned_style: Some("vrobots_msgs"),
            local: true,
        };
        let r = check_unity_targets(&view);
        assert!(r.status.is_warn(), "expected a warning, got {:?}", r.status);
        let m = msg_of(r);
        assert!(m.contains("predate the current emitter"), "{m}");
        assert!(m.contains("sb message compile"), "must name the fix: {m}");
    }

    /// A tree compiled before 0.1.39 still has the wrapper inline in every
    /// message that uses it. Recompiling is the only thing that moves it, so
    /// doctor has to say so — the host build is what breaks, not `sb`.
    #[test]
    fn inline_array_wrapper_in_a_unity_tree_is_warned_about() {
        let d = tempfile::tempdir().unwrap();
        let root = unity_vault_with(
            d.path(),
            "vrobots_msgs",
            "services",
            "SrvOMRWallMsg.cs",
            INLINE_WRAPPER,
        );
        let view = VaultView {
            root: &root,
            pinned_style: Some("vrobots_msgs"),
            local: true,
        };
        let r = check_unity_targets(&view);
        assert!(r.status.is_warn(), "expected a warning, got {:?}", r.status);
        let m = msg_of(r);
        assert!(m.contains("_arrays"), "must point at the new home: {m}");
    }

    /// The same declaration under `iox2/_arrays/` is exactly what a current
    /// `sb` writes. Flagging it would make the warning permanent.
    #[test]
    fn wrapper_in_the_arrays_dir_is_not_flagged() {
        let d = tempfile::tempdir().unwrap();
        let root = unity_vault_with(
            d.path(),
            "vrobots_msgs",
            "_arrays",
            "swarmbotix_primitives_Vec3Msg_Array256.cs",
            "namespace sb_iox2_arrays
{
    [StructLayout(LayoutKind.Sequential, Pack = 1)]
    public unsafe struct swarmbotix_primitives_Vec3Msg_Array256 {
        public const int Length = 256;
    }
}
",
        );
        let view = VaultView {
            root: &root,
            pinned_style: Some("vrobots_msgs"),
            local: true,
        };
        assert!(check_unity_targets(&view).status.is_ok());
    }

    /// The whole point of the emitter fix: iox2 C# is now allowed to live in a
    /// Unity tree, so current output must NOT be flagged.
    #[test]
    fn current_csharp_in_a_unity_tree_is_clean() {
        let d = tempfile::tempdir().unwrap();
        let root = unity_vault(d.path(), "vrobots_msgs", false);
        let view = VaultView {
            root: &root,
            pinned_style: Some("vrobots_msgs"),
            local: true,
        };
        assert!(check_unity_targets(&view).status.is_ok());
    }

    /// A warning must not fail the exit code — the breakage is in the host
    /// project, and failing would block every unrelated `sb doctor`.
    #[test]
    fn a_warning_does_not_fail_the_report() {
        let d = tempfile::tempdir().unwrap();
        let root = unity_vault(d.path(), "vrobots_msgs", true);
        let view = VaultView {
            root: &root,
            pinned_style: Some("vrobots_msgs"),
            local: true,
        };
        let report = Report {
            checks: vec![check_unity_targets(&view)],
        };
        assert!(report.all_ok(), "warn must not be a failure");
        assert!(report.render(false).contains("[WARN]"));
    }

    /// The same output outside a Unity project is nobody's problem.
    #[test]
    fn stale_csharp_outside_a_unity_project_is_not_flagged() {
        let d = tempfile::tempdir().unwrap();
        styles_at(d.path(), &[("swarmbotix", true, true)]);
        let root = d.path().join("messages");
        let view = VaultView {
            root: &root,
            pinned_style: None,
            local: false,
        };
        assert!(check_unity_targets(&view).status.is_ok());
    }

    #[test]
    fn deprecated_keys_are_reported_as_ignored() {
        let d = tempfile::tempdir().unwrap();
        let mut cfg = cfg_at(d.path());
        // Parsed but dead. Silence here would let someone believe a stale
        // `message_style:` still took effect.
        cfg.message_style = Some("ros2".into());
        let r = check_deprecated_keys(&cfg);
        assert!(r.status.is_fail());
        let m = msg_of(r);
        assert!(m.contains("message_style"), "{m}");
        assert!(m.contains("IGNORED"), "{m}");

        assert!(check_deprecated_keys(&cfg_at(d.path())).status.is_ok());
    }
}
