//! `sb doctor` — environment validation.
//!
//! Reads the merged `sb.config.yml` and checks each declared path / tool.
//! Reports one line per check; **does not** stop at the first failure
//! (per L1 TDD test #10).

use std::path::{Path, PathBuf};
use std::process::Command;

use sb_core::{
    Encoding, Host, Language, ModuleDevConfig, SbCliConfig, TopicRef, Transport, Wire,
    WireSelector, WorkspaceFlow,
};

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

/// Check: `topics`. Reports modules whose module manifest still writes a
/// fully-qualified topic as `topic: /...`.
///
/// sb 0.5.0 reads such an entry as `key:` with the leading `/` dropped,
/// which is the same wire key, so nothing breaks; the warning only says
/// how to store it in the canonical form. `modules` yields
/// `(module name, path)` pairs as the workspace registry records them; the
/// path is the module's manifest or its directory, which is probed for
/// `sb.module.yaml` then the legacy `sb.dev.yml`. A file that cannot be
/// read is skipped silently (other checks own that failure).
///
/// One `[WARN]` per offending module. When none offend and at least one
/// file was scanned, a single `[OK]`; when nothing was scanned, no result.
pub fn check_module_topics<'a>(
    modules: impl IntoIterator<Item = (&'a str, &'a Path)>,
) -> Vec<CheckResult> {
    let mut out = Vec::new();
    let mut scanned = 0usize;
    for (module, path) in modules {
        let manifest = manifest_path(path);
        let Ok(text) = std::fs::read_to_string(&manifest) else {
            continue;
        };
        scanned += 1;
        let legacy = sb_core::legacy_topics_in(&text);
        if legacy.is_empty() {
            continue;
        }
        let (count, pronoun) = if legacy.len() == 1 {
            ("1 legacy `topic: /...` entry".to_owned(), "it")
        } else {
            (
                format!("{} legacy `topic: /...` entries", legacy.len()),
                "them",
            )
        };
        let mut msg = format!(
            "{module}: {count} in {} (sb 0.5.0 reads {pronoun} as `key:`; same wire key)",
            manifest.display()
        );
        for lt in &legacy {
            msg.push_str(&format!(
                "\n{MSG_COL}{} {:?}: topic: {} -> key: {}",
                lt.role, lt.name, lt.topic, lt.key
            ));
        }
        // Any write of the manifest stores every legacy entry as `key:`.
        // `sb sub edit` rewrites the file without changing the entry; a
        // module with only legacy publishers needs `sb pub edit --key`.
        if let Some(sub) = legacy.iter().find(|lt| lt.role == "sub") {
            msg.push_str(&format!(
                "\n{MSG_COL}rewrite: `sb sub edit {} --module {module}` \
                 (publishers: `sb pub edit <name> --key <key>`)",
                sub.key
            ));
        } else if let Some(publ) = legacy.first() {
            msg.push_str(&format!(
                "\n{MSG_COL}rewrite: `sb pub edit {} --key {} --module {module}`",
                publ.name, publ.key
            ));
        }
        out.push(warn("topics", msg));
    }
    if out.is_empty() && scanned > 0 {
        out.push(ok(
            "topics",
            "no legacy `topic: /...` entries in the active workspace",
        ));
    }
    out
}

// ─────────────────────────────────────────────────────────────────────
// Workspace checks (`sb ws check`, and the workspace rows of `sb doctor`)
// ─────────────────────────────────────────────────────────────────────

/// One installed workspace, as [`check_workspace`] inspects it.
#[derive(Debug, Clone, Copy)]
pub struct WorkspaceView<'a> {
    /// Workspace name; the `<ws>` segment of every owned wire key.
    pub name: &'a str,
    /// The installed copy (`<sb_home>/workspaces/<name>/sb.flow.yaml`, or
    /// `flow.yaml` from a pre-0.6 install).
    pub flow: &'a WorkspaceFlow,
    /// Root of the vault `type:` names resolve in (the configured
    /// `messages_root`, the one `sb install` imports into).
    pub vault_root: &'a Path,
    /// The repo flow file when it was modified after the installed copy
    /// was written (`sb_workspace::is_stale`); `None` when current or when
    /// the workspace has no source.
    pub stale_repo_flow: Option<&'a Path>,
    /// The host the workspace is checked for: decides `platforms:` support
    /// and which overlays merge into `requires`.
    pub host: Host,
}

/// A module of the workspace whose manifest parsed and validated.
struct LoadedModule<'a> {
    name: &'a str,
    /// The manifest file (`sb.module.yaml`, or the legacy `sb.dev.yml`).
    manifest: &'a Path,
    dev: ModuleDevConfig,
}

/// One pub/sub entry, flattened across both lists.
struct EntryRef<'a> {
    role: &'static str,
    name: &'a str,
    topic: &'a TopicRef,
    msg_type: Option<&'a str>,
    transport: Transport,
    encoding: Encoding,
}

fn entries_of(dev: &ModuleDevConfig) -> Vec<EntryRef<'_>> {
    let pubs = dev.publishers.iter().map(|p| EntryRef {
        role: "pub",
        name: &p.name,
        topic: &p.topic,
        msg_type: p.msg_type.as_deref(),
        transport: p.transport,
        encoding: p.encoding,
    });
    let subs = dev.subscribers.iter().map(|s| EntryRef {
        role: "sub",
        name: &s.name,
        topic: &s.topic,
        msg_type: s.msg_type.as_deref(),
        transport: s.transport,
        encoding: s.encoding,
    });
    pubs.chain(subs).collect()
}

/// Validate one installed workspace. Rows, in order:
///
/// - `workspace`: the name, module count, and where it was installed from.
/// - `ws modules`: `[FAIL]` per module whose module manifest is missing,
///   does not parse, or fails validation; one `[OK]` otherwise.
/// - `ws types`: `[FAIL]` per proto/fb entry whose `type:` is not a vault
///   message (nor an `iox2_variants` variant); raw entries carry no type.
/// - `ws instances`: `[FAIL]` per instance id listed twice for one module.
/// - `ws encoding`: `[FAIL]` per wire key whose entries, across every
///   module, disagree on `encoding:`.
/// - `ws io files`: `[WARN]` per module with generated files missing under
///   its io dir (`publishers/`, `subscribers/`, `services/<name>.<ext>`).
/// - `ws requires`: one `[FAIL]` naming, per module, the `requires:`
///   executables missing from PATH; modules the host does not support are
///   skipped.
/// - `ws platforms`: `[WARN]` per module whose `platforms:` block does not
///   list this host.
/// - `ws wires`: `[FAIL]` per problem with a `wires:` entry, checked against
///   the module manifests and instance lists.
/// - `deprecated`: one `[WARN]` listing every pre-0.6 file or form the
///   workspace still uses, one per line.
/// - `ws source`: `[WARN]` when the repo flow file is newer than the
///   installed copy; `[SKIP]` for a workspace with no `source:`.
///
/// Rows for modules that failed `ws modules` are not repeated further down.
pub fn check_workspace(cfg: &SbCliConfig, ws: &WorkspaceView<'_>) -> Vec<CheckResult> {
    let manifests: Vec<(String, PathBuf)> = ws
        .flow
        .module_paths()
        .into_iter()
        .map(|(name, path)| {
            let manifest = manifest_path(&path);
            (name, manifest)
        })
        .collect();
    let origin = match &ws.flow.source {
        Some(src) => match &src.url {
            Some(url) => format!("installed from {url}"),
            None => format!("installed from {}", src.repo_flow.display()),
        },
        None => "no source (not installed from a repo flow.yaml)".to_owned(),
    };
    let mut out = vec![ok(
        "workspace",
        format!("{}: {} module(s), {origin}", ws.name, manifests.len()),
    )];

    let mut loaded: Vec<LoadedModule<'_>> = Vec::new();
    let mut module_fails = Vec::new();
    for (name, manifest) in &manifests {
        match load_manifest(manifest) {
            Ok(dev) => loaded.push(LoadedModule {
                name,
                manifest,
                dev,
            }),
            Err(e) => module_fails.push(fail("ws modules", format!("{name}: {e}"))),
        }
    }
    if module_fails.is_empty() {
        out.push(ok(
            "ws modules",
            format!("{} module(s), every module manifest parses", loaded.len()),
        ));
    }
    out.extend(module_fails);

    out.extend(check_ws_types(cfg, ws.vault_root, &loaded));
    out.extend(check_ws_instances(ws.flow));
    out.extend(check_ws_encoding(cfg, ws.name, &loaded));
    out.extend(check_ws_io_files(&loaded));
    out.push(check_ws_requires(&ws.host, &loaded));
    out.extend(check_ws_platforms(&ws.host, &loaded));
    out.extend(check_ws_wires(ws.flow, &loaded));
    out.push(check_ws_deprecated(ws, &loaded));
    out.push(match (&ws.flow.source, ws.stale_repo_flow) {
        (None, _) => skip("ws source", "no repo flow.yaml to compare with"),
        (Some(_), Some(repo)) => warn(
            "ws source",
            format!(
                "{} is newer than the installed copy; run sb install",
                repo.display()
            ),
        ),
        (Some(src), None) => ok(
            "ws source",
            format!("installed copy is current with {}", src.repo_flow.display()),
        ),
    });
    out
}

/// Read, parse and validate one module manifest; the error is the row text.
fn load_manifest(manifest: &Path) -> Result<ModuleDevConfig, String> {
    if !manifest.is_file() {
        return Err(format!(
            "no module manifest at {} (run `sb init` in the module)",
            manifest.display()
        ));
    }
    let text = std::fs::read_to_string(manifest)
        .map_err(|e| format!("cannot read {}: {e}", manifest.display()))?;
    if text.trim().is_empty() {
        return Err(format!("{} is empty", manifest.display()));
    }
    let dev: ModuleDevConfig = serde_yaml::from_str(&text)
        .map_err(|e| format!("{} does not parse: {e}", manifest.display()))?;
    // Parsing alone checks neither the `platforms:` keys nor the
    // one-command rule for `scripts.start`; launch refuses such a file, so
    // the check says so first.
    dev.validate()
        .map_err(|e| format!("{} is invalid: {e}", manifest.display()))?;
    Ok(dev)
}

/// The manifest file a flow or registry path names. A directory is probed
/// for [`sb_core::module_file_candidates`] in order (new name first) and
/// falls back to the new name when neither exists; a file path is used as
/// is, so a 0.5 install that recorded `.../sb.dev.yml` keeps resolving.
fn manifest_path(path: &Path) -> PathBuf {
    if !path.is_dir() {
        return path.to_path_buf();
    }
    sb_core::module_file_candidates()
        .into_iter()
        .map(|name| path.join(name))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| path.join(sb_core::MODULE_FILE))
}

fn check_ws_types(
    cfg: &SbCliConfig,
    vault_root: &Path,
    modules: &[LoadedModule<'_>],
) -> Vec<CheckResult> {
    let vault = sb_vault::Vault::at(vault_root);
    let mut out = Vec::new();
    let mut typed = 0usize;
    for m in modules {
        for e in entries_of(&m.dev) {
            let Some(t) = e.msg_type else { continue };
            if e.encoding == Encoding::Raw {
                continue;
            }
            typed += 1;
            let problem = match sb_vault::MessageName::parse(t) {
                Err(err) => Some(format!("type {t} is not a vault name: {err}")),
                Ok(name) if vault.path_of(&name).is_file() || is_iox2_variant(cfg, &name) => None,
                Ok(_) => Some(format!(
                    "type {t} is not in the vault at {}",
                    vault_root.display()
                )),
            };
            if let Some(p) = problem {
                out.push(fail(
                    "ws types",
                    format!("{} {} {}: {p}", m.name, e.role, e.name),
                ));
            }
        }
    }
    if out.is_empty() {
        out.push(ok(
            "ws types",
            if typed == 1 {
                "1 typed entry resolves in the vault".to_owned()
            } else {
                format!("{typed} typed entries resolve in the vault")
            },
        ));
    }
    out
}

/// Whether `name` is a declared iceoryx2 variant (`iox2_variants:`), which
/// has no `.proto` of its own. Same rule `sb pub add` applies.
fn is_iox2_variant(cfg: &SbCliConfig, name: &sb_vault::MessageName) -> bool {
    cfg.iox2_variants.as_ref().is_some_and(|variants| {
        variants.iter().any(|(base_fqn, vs)| {
            base_fqn
                .rsplit_once('.')
                .and_then(|(pkg, _)| pkg.rsplit('.').next())
                .is_some_and(|ns| ns == name.namespace && vs.contains_key(&name.leaf))
        })
    })
}

fn check_ws_instances(flow: &WorkspaceFlow) -> Vec<CheckResult> {
    let mut out = Vec::new();
    let mut total = 0usize;
    // `instances_for` is the inline list under the module entry, else the
    // legacy top-level one, which is what launch fans out.
    for module in flow.modules.keys() {
        let mut seen = std::collections::BTreeSet::new();
        for inst in flow.instances_for(module) {
            total += 1;
            if !seen.insert(inst.id.as_str()) {
                out.push(fail(
                    "ws instances",
                    format!(
                        "{module}: instance id {:?} is declared more than once",
                        inst.id
                    ),
                ));
            }
        }
    }
    if out.is_empty() {
        out.push(ok(
            "ws instances",
            format!("{total} instance(s), ids unique per module"),
        ));
    }
    out
}

fn check_ws_encoding(
    cfg: &SbCliConfig,
    workspace: &str,
    modules: &[LoadedModule<'_>],
) -> Vec<CheckResult> {
    let device = cfg.device.as_deref().unwrap_or("dev01");
    // (wire key, transport) -> every (who, encoding) that names it. The same
    // key on the other transport is a different channel.
    let mut by_key: std::collections::BTreeMap<(String, &str), Vec<(String, Encoding)>> =
        std::collections::BTreeMap::new();
    for m in modules {
        for e in entries_of(&m.dev) {
            let key = e
                .topic
                .full_key(device, workspace, &m.dev.module, e.transport);
            by_key
                .entry((key, e.transport.as_str()))
                .or_default()
                .push((format!("{} {} {}", m.name, e.role, e.name), e.encoding));
        }
    }
    let mut out = Vec::new();
    for ((key, transport), users) in &by_key {
        let first = users[0].1;
        if users.iter().all(|(_, enc)| *enc == first) {
            continue;
        }
        let who: Vec<String> = users
            .iter()
            .map(|(who, enc)| format!("{who} = {enc}"))
            .collect();
        out.push(fail(
            "ws encoding",
            format!(
                "{key} ({transport}): encodings disagree ({})",
                who.join(", ")
            ),
        ));
    }
    if out.is_empty() {
        out.push(ok(
            "ws encoding",
            format!("{} wire key(s), encodings agree", by_key.len()),
        ));
    }
    out
}

/// File extension of a generated io file; mirrors `sb_codegen::render`.
fn io_ext(lang: Language) -> &'static str {
    match lang {
        Language::Rust => "rs",
        Language::Python => "py",
        Language::Cpp => "cpp",
        Language::Flutter => "dart",
        Language::Unity => "cs",
    }
}

/// Every generated io file the module's entries call for, relative to its
/// io dir. Entries with no template (iceoryx2 on Flutter / Unity, services
/// outside Rust / Python) generate nothing and are left out.
fn expected_io_files(dev: &ModuleDevConfig) -> Vec<PathBuf> {
    let ext = io_ext(dev.language);
    let zenoh_only = matches!(dev.language, Language::Flutter | Language::Unity);
    let mut files = Vec::new();
    for (dir, list) in [
        (
            "publishers",
            entries_of(dev)
                .into_iter()
                .filter(|e| e.role == "pub")
                .collect::<Vec<_>>(),
        ),
        (
            "subscribers",
            entries_of(dev)
                .into_iter()
                .filter(|e| e.role == "sub")
                .collect::<Vec<_>>(),
        ),
    ] {
        for e in list {
            if zenoh_only && e.transport == Transport::Iceoryx2 {
                continue;
            }
            files.push(PathBuf::from(dir).join(format!("{}.{ext}", e.name)));
        }
    }
    if matches!(dev.language, Language::Rust | Language::Python) {
        for s in &dev.services {
            files.push(PathBuf::from("services").join(format!("{}.{ext}", s.name)));
        }
    }
    files
}

fn check_ws_io_files(modules: &[LoadedModule<'_>]) -> Vec<CheckResult> {
    let mut out = Vec::new();
    let mut expected = 0usize;
    for m in modules {
        let io = m.dev.module_root(m.manifest).join(&m.dev.io_dir);
        let files = expected_io_files(&m.dev);
        expected += files.len();
        let missing: Vec<String> = files
            .iter()
            .filter(|f| !io.join(f).is_file())
            .map(|f| f.to_string_lossy().replace('\\', "/"))
            .collect();
        if !missing.is_empty() {
            out.push(warn(
                "ws io files",
                format!(
                    "{}: {} generated file(s) missing under {} ({}); run `sb io update {}`",
                    m.name,
                    missing.len(),
                    io.display(),
                    missing.join(", "),
                    m.name
                ),
            ));
        }
    }
    if out.is_empty() {
        out.push(ok(
            "ws io files",
            format!("{expected} generated file(s) present"),
        ));
    }
    out
}

/// `<os>-<arch>` of `host`, as `ws platforms` names it.
fn host_label(host: &Host) -> String {
    format!("{}-{}", host.os, host.arch)
}

/// `ws requires`: every executable in a module's merged `requires:` must
/// be on PATH. Modules the host does not support are skipped; `ws
/// platforms` reports them. One row: `[FAIL]` naming each module's missing
/// tools, joined by `; `, or `[OK]`.
fn check_ws_requires(host: &Host, modules: &[LoadedModule<'_>]) -> CheckResult {
    let mut problems: Vec<String> = Vec::new();
    for m in modules {
        if !m.dev.supports_host(host) {
            continue;
        }
        let missing: Vec<String> = merged_requires(&m.dev, host)
            .into_iter()
            .filter(|bin| which::which(bin).is_err())
            .collect();
        if !missing.is_empty() {
            problems.push(format!("{}: missing {}", m.name, missing.join(", ")));
        }
    }
    if problems.is_empty() {
        ok("ws requires", "all required tools present")
    } else {
        fail("ws requires", problems.join("; "))
    }
}

/// A module's `requires:` after the platform merge for `host`, taken from
/// [`ModuleDevConfig::resolve`]. When `resolve` refuses (a unity module has
/// no start command) the same merge, top level then each overlay, appended
/// and deduplicated, is done here so those `requires` are still checked.
fn merged_requires(dev: &ModuleDevConfig, host: &Host) -> Vec<String> {
    if let Ok(resolved) = dev.resolve(host) {
        return resolved.requires;
    }
    let overlays = dev.platforms.overlays_for(host);
    let lists = std::iter::once(&dev.requires).chain(overlays.iter().map(|o| &o.requires));
    let mut out: Vec<String> = Vec::new();
    for req in lists.flatten() {
        if !out.contains(req) {
            out.push(req.clone());
        }
    }
    out
}

/// `ws platforms`: `[WARN]` per module whose `platforms:` block does not
/// list this host (`sb up` skips it, `sb run` refuses it). A module with no
/// `platforms:` block supports every host.
fn check_ws_platforms(host: &Host, modules: &[LoadedModule<'_>]) -> Vec<CheckResult> {
    let here = host_label(host);
    let mut out = Vec::new();
    for m in modules {
        if m.dev.supports_host(host) {
            continue;
        }
        let declares: Vec<&str> = m.dev.platforms.0.keys().map(String::as_str).collect();
        out.push(warn(
            "ws platforms",
            format!(
                "{}: not supported on {here} (declares: {})",
                m.name,
                declares.join(", ")
            ),
        ));
    }
    if out.is_empty() {
        out.push(ok("ws platforms", "all modules support this host"));
    }
    out
}

/// `ws wires`: `[FAIL]` per problem, as `<lhs>: <reason>`, for every
/// `wires:` entry. Grammar errors come from [`WorkspaceFlow::wires`];
/// existence, instance, transport and type checks run against the loaded
/// manifests and the flow's instance lists. A wire with an end in a module
/// whose manifest did not load is skipped (`ws modules` already failed it).
fn check_ws_wires(flow: &WorkspaceFlow, modules: &[LoadedModule<'_>]) -> Vec<CheckResult> {
    let wires = match flow.wires() {
        Ok(wires) => wires,
        Err(e) => return vec![fail("ws wires", e.to_string())],
    };
    if wires.is_empty() {
        return vec![ok("ws wires", "no wires")];
    }
    let mut out = Vec::new();
    let mut resolved = 0usize;
    for wire in &wires {
        let Some(problems) = wire_problems(flow, modules, wire) else {
            continue;
        };
        if problems.is_empty() {
            resolved += 1;
            continue;
        }
        let lhs = format!("{}.{}", wire.sub.module, wire.sub.entry);
        out.extend(
            problems
                .into_iter()
                .map(|reason| fail("ws wires", format!("{lhs}: {reason}"))),
        );
    }
    if out.is_empty() {
        out.push(ok("ws wires", format!("{resolved} wire(s) resolve")));
    }
    out
}

/// Every reason `wire` does not resolve; empty when it does. `None` when an
/// end names a module that is in the flow but whose manifest failed to
/// load, so it is not misreported as unknown.
///
/// A missing module or entry stops the check at that point. Past it, the
/// selector, transport and type checks are independent and all reported.
fn wire_problems(
    flow: &WorkspaceFlow,
    modules: &[LoadedModule<'_>],
    wire: &Wire,
) -> Option<Vec<String>> {
    let (sub_end, pub_end) = (&wire.sub, &wire.pub_);

    let Some(sub_module) = find_loaded(modules, &sub_end.module) else {
        return (!flow.modules.contains_key(&sub_end.module))
            .then(|| vec!["unknown subscriber module".to_owned()]);
    };
    let Some(sub) = sub_module
        .dev
        .subscribers
        .iter()
        .find(|s| s.name == sub_end.entry)
    else {
        return Some(vec![format!(
            "no subscriber {} in {}",
            sub_end.entry, sub_end.module
        )]);
    };
    let Some(pub_module) = find_loaded(modules, &pub_end.module) else {
        return (!flow.modules.contains_key(&pub_end.module))
            .then(|| vec!["unknown publisher module".to_owned()]);
    };
    let Some(publ) = pub_module
        .dev
        .publishers
        .iter()
        .find(|p| p.name == pub_end.entry)
    else {
        return Some(vec![format!(
            "no publisher {} in {}",
            pub_end.entry, pub_end.module
        )]);
    };

    let mut problems = Vec::new();
    let pub_ids = flow.instances_for(&pub_end.module);
    match &pub_end.selector {
        WireSelector::None => {}
        WireSelector::All => {
            if sub.transport == Transport::Iceoryx2 || publ.transport == Transport::Iceoryx2 {
                problems.push("[*] on iceoryx2 is not supported".to_owned());
            }
        }
        WireSelector::SameId => {
            let sub_ids = flow.instances_for(&sub_end.module);
            if sub_ids.is_empty() {
                problems.push(format!("{{id}} but {} has no instances", sub_end.module));
            }
            if pub_ids.is_empty() {
                problems.push(format!("{{id}} but {} has no instances", pub_end.module));
            } else {
                // Each subscriber instance reads the publisher instance with
                // its own id, so every one of them needs a counterpart.
                for inst in sub_ids {
                    if !pub_ids.iter().any(|p| p.id == inst.id) {
                        problems.push(format!("no instance {} in {}", inst.id, pub_end.module));
                    }
                }
            }
        }
        WireSelector::Id(id) => {
            if !pub_ids.iter().any(|p| p.id == *id) {
                problems.push(format!("no instance {id} in {}", pub_end.module));
            }
        }
    }
    if sub.transport != publ.transport {
        problems.push(format!(
            "transport mismatch ({} vs {})",
            sub.transport.as_str(),
            publ.transport.as_str()
        ));
    }
    if sub.msg_type != publ.msg_type {
        // `type:` is absent only on raw entries.
        problems.push(format!(
            "type mismatch ({} vs {})",
            sub.msg_type.as_deref().unwrap_or("raw"),
            publ.msg_type.as_deref().unwrap_or("raw")
        ));
    }
    Some(problems)
}

/// The loaded module the flow calls `name`, if its manifest loaded.
fn find_loaded<'m, 'a>(
    modules: &'m [LoadedModule<'a>],
    name: &str,
) -> Option<&'m LoadedModule<'a>> {
    modules.iter().find(|m| m.name == name)
}

/// Suffix of every `deprecated` line: where the migration steps live.
const MIGRATING: &str = "-> see sbcli_module.md §Migrating";

/// `deprecated`: one `[WARN]` listing every pre-0.6 file or form the
/// workspace still relies on, one per line, each pointing at the migration
/// guide. All of them still work in 0.6 and are removed in 0.7.0. Only
/// modules whose manifest loaded are inspected.
fn check_ws_deprecated(ws: &WorkspaceView<'_>, modules: &[LoadedModule<'_>]) -> CheckResult {
    let mut items: Vec<String> = Vec::new();
    for m in modules {
        if sb_core::is_legacy_module_file(m.manifest) {
            items.push(format!("sb.dev.yml at {}", m.manifest.display()));
        }
    }
    let repo_flow = ws.flow.source.as_ref().map(|src| src.repo_flow.as_path());
    if let Some(repo) = repo_flow {
        if sb_core::is_legacy_flow_file(repo) && repo.is_file() {
            items.push(format!("flow.yaml at {}", repo.display()));
        }
    }
    for script in ["runscript.bash", "stopscript.bash"] {
        for m in modules {
            let path = m.dev.module_root(m.manifest).join(script);
            if path.is_file() {
                items.push(format!("{script} at {}", path.display()));
            }
        }
    }
    if ws.flow.has_legacy_instances() {
        let flow = match repo_flow {
            Some(repo) => repo.display().to_string(),
            None => format!("the installed flow of workspace {}", ws.name),
        };
        items.push(format!("top-level instances: in {flow}"));
    }
    if items.is_empty() {
        return ok("deprecated", "nothing deprecated in use");
    }
    let lines: Vec<String> = items
        .into_iter()
        .map(|item| format!("{item} {MIGRATING}"))
        .collect();
    warn("deprecated", lines.join(&format!("\n{MSG_COL}")))
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
    fn linked_versions_are_baked_in_from_the_lockfile() {
        // A build inside this workspace always has Cargo.lock, so `unknown`
        // here means build.rs stopped finding it.
        assert_ne!(ZENOH_VERSION, UNKNOWN);
        assert_ne!(ICEORYX2_VERSION, UNKNOWN);
        assert!(semver_triple(ZENOH_VERSION).is_some());
        assert!(semver_triple(ICEORYX2_VERSION).is_some());
    }
}
