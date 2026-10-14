# Contract: the Rust surface for the native-launch wave

Every name here is fixed. An agent that needs something not listed adds it in
its own crate and documents it in its report; it does not rename anything
listed. Line references are to the tree at commit `637f2bb`.

Conventions: all new struct fields are `#[serde(default, skip_serializing_if = ...)]`
so pre-0.6 files parse unchanged and written files stay minimal. Keep
`#[serde(deny_unknown_fields)]` everywhere it is today. Never use
`#[serde(flatten)]` (incompatible with `deny_unknown_fields`) or
`#[serde(untagged)]` (bad error messages); use hand-written `Deserialize`
with `deserialize_any` as `ModuleRef` already does
(`crates/sb-core/src/config.rs:1223`).

---

## 1. sb-core: filenames and host

`crates/sb-core/src/config.rs` (re-exported from `lib.rs`):

```rust
pub const MODULE_FILE: &str = "sb.module.yaml";
pub const LEGACY_MODULE_FILE: &str = "sb.dev.yml";
pub const FLOW_FILE: &str = "sb.flow.yaml";
pub const LEGACY_FLOW_FILE: &str = "flow.yaml";

/// Probe order for a module directory: new name first, then legacy.
pub fn module_file_candidates() -> [&'static str; 2];
/// Probe order for a workspace directory: new name first, then legacy.
pub fn flow_file_candidates() -> [&'static str; 2];
/// True when `path`'s file name is `LEGACY_MODULE_FILE`.
pub fn is_legacy_module_file(path: &std::path::Path) -> bool;
/// True when `path`'s file name is `LEGACY_FLOW_FILE`.
pub fn is_legacy_flow_file(path: &std::path::Path) -> bool;
```

No file IO in sb-core. Probing is done by callers (sb-workspace §5).

New file `crates/sb-core/src/host.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Host { pub os: &'static str, pub arch: &'static str }
impl Host {
    /// `std::env::consts::OS` / `ARCH`: "linux" | "windows" | "macos", "x86_64" | "aarch64" | ...
    pub fn current() -> Host;
    /// "linux", "linux-aarch64", "windows", ... in merge order (general first).
    pub fn platform_keys(&self) -> [String; 2];
    /// ".exe" on windows, "" elsewhere.
    pub fn exe_suffix(&self) -> &'static str;
}
pub const KNOWN_OS: &[&str] = &["linux", "windows", "macos"];
pub const KNOWN_ARCH: &[&str] = &["x86_64", "aarch64"];
```

New shared type, `crates/sb-core/src/config.rs`:

```rust
/// One deprecation hit found while reading a file or planning a launch.
/// Crates collect these; sb-cli prints them (see §9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deprecation { pub item: String, pub hint: String }
```

## 2. sb-core: manifest (`ModuleDevConfig`)

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleDevConfig {
    pub module: String,
    pub language: Language,
    pub root: Option<PathBuf>,            // unchanged
    pub io_dir: PathBuf,                  // unchanged
    pub version: Option<String>,          // new
    pub sb: Option<String>,               // new, free text, not enforced in 0.6.0
    pub description: Option<String>,      // new
    pub scripts: Scripts,                 // new, skip if Scripts::is_empty
    pub env: BTreeMap<String, String>,    // new, skip if empty
    pub requires: Vec<String>,            // new, skip if empty
    pub platforms: Platforms,             // new, skip if Platforms::is_empty
    pub publishers: Vec<PubSpec>,         // unchanged
    pub subscribers: Vec<SubSpec>,        // unchanged
    pub services: Vec<ServiceSpec>,       // unchanged
}

/// One argv, already split.
pub type Argv = Vec<String>;

/// A script slot: one or more commands run in order. On disk: a string
/// (one command, split per §2.1) or a list of strings (one command each).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Script { pub steps: Vec<Argv> }
impl Script {
    pub fn parse_line(line: &str) -> Result<Argv, ScriptError>;   // §2.1
    pub fn single(&self) -> Option<&Argv>;                          // Some iff steps.len() == 1
}
// Serialize: one step with no token containing whitespace or quotes -> string
// joined by " "; otherwise a list, each step joined by " " with tokens that
// contain whitespace double-quoted.
// Deserialize: visit_str -> one step; visit_seq of strings -> one step each.

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scripts {
    pub build: Option<Script>,
    pub start: Option<Script>,
    pub stop:  Option<Script>,
    pub test:  Option<Script>,
}
impl Scripts {
    pub fn is_empty(&self) -> bool;         // all None
    pub fn merge_over(&mut self, over: &Scripts); // per-key: Some in `over` replaces
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Overlay {
    pub scripts: Scripts,
    pub env: BTreeMap<String, String>,
    pub requires: Vec<String>,
}

/// Keys are "<os>" or "<os>-<arch>" from KNOWN_OS / KNOWN_ARCH; validated.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Platforms(pub BTreeMap<String, Overlay>);
impl Platforms {
    pub fn is_empty(&self) -> bool;
    /// Absent block = every host supported. Present = host.os key or host.os-arch key exists.
    pub fn supports(&self, host: &Host) -> bool;
    /// Overlays for this host in merge order: "<os>" then "<os>-<arch>".
    pub fn overlays_for(&self, host: &Host) -> Vec<&Overlay>;
    pub fn validate(&self) -> Result<(), ManifestError>;
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("platforms: unknown key {0:?} (expected <os> or <os>-<arch>, os in linux|windows|macos, arch in x86_64|aarch64)")]
    BadPlatformKey(String),
    #[error("scripts.start must be exactly one command")]
    StartNotSingle,
    #[error("language {0} has no start command (unity modules launch from the editor)")]
    NoStart(Language),
    #[error("module {0:?} is not supported on {1}")]
    Unsupported(String, String),
}

#[derive(Debug, thiserror::Error)]
pub enum ScriptError {
    #[error("unterminated quote in command: {0}")]
    UnterminatedQuote(String),
    #[error("empty command")]
    Empty,
}
```

`ModuleDevConfig::validate()` (today at `config.rs:423`) additionally calls
`platforms.validate()` and rejects `scripts.start` (top level or any overlay)
with more than one step.

### 2.1 String-to-argv split

Whitespace separates tokens. `"..."` and `'...'` group a token and may appear
inside a token (`a"b c"d` is one token `ab cd`). No backslash escapes. Quotes
are removed. Empty result is `ScriptError::Empty`.

### 2.2 Resolution for a host

```rust
/// Fully merged launch facts for one module on one host, before placeholder expansion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub build: Vec<Argv>,
    pub start: Argv,
    pub stop:  Vec<Argv>,
    pub test:  Vec<Argv>,
    pub env:   BTreeMap<String, String>,
    pub requires: Vec<String>,
    /// Relative path of the built binary for `{bin}`, OS-aware, may be empty (python, flutter).
    pub bin: String,
}
impl ModuleDevConfig {
    pub fn supports_host(&self, host: &Host) -> bool;      // platforms.supports
    /// language defaults <- top level <- overlays_for(host). Errors: Unsupported, NoStart, StartNotSingle.
    pub fn resolve(&self, host: &Host) -> Result<Resolved, ManifestError>;
}
/// The defaults table (§3) for one language on one host.
pub fn language_defaults(lang: Language, host: &Host) -> Resolved;
```

Merge semantics: `scripts` per key, `env` per key (later wins), `requires`
append then dedupe preserving first occurrence, `bin` from language defaults
only.

## 3. Language defaults table

Raw, before expansion. `{exe}` is `.exe` on Windows, empty elsewhere.

| language | build | start | bin | test |
|---|---|---|---|---|
| rust | `cargo build --release` | `{bin} {args}` | `target/release/{module}{exe}` | `cargo test` |
| cpp | `cmake -S . -B build -DCMAKE_BUILD_TYPE=Release` then `cmake --build build --config Release` | `{bin} {args}` | linux/macos `build/{module}`; windows `build/Release/{module}.exe` | none |
| python | none | `{python} main.py {args}` | empty | none |
| flutter | none | `flutter run -d {os}` | empty | none |
| unity | none | none (resolve returns `NoStart`) | empty | none |

`{python}`: resolved by the caller (sb-launch) as `.venv/bin/python` if that
file exists under the module root (`.venv/Scripts/python.exe` on Windows),
else `python3` on linux/macos, `python` on windows. sb-core exposes:

```rust
pub fn python_venv_relpath(host: &Host) -> &'static str;   // ".venv/bin/python" | ".venv/Scripts/python.exe"
pub fn python_fallback(host: &Host) -> &'static str;       // "python3" | "python"
```

## 4. sb-core: placeholder and env expansion

New file `crates/sb-core/src/expand.rs`:

```rust
pub struct ExpandCtx<'a> {
    pub module: &'a str,
    pub root: &'a str,        // absolute module root, forward slashes
    pub bin: &'a str,
    pub exe: &'a str,
    pub id: Option<&'a str>,
    pub args: &'a [String],
    pub ws: &'a str,
    pub os: &'a str,
    pub python: &'a str,
}
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ExpandError {
    #[error("unknown placeholder {{{0}}}")] Unknown(String),
    #[error("{{id}} used but this module has no instances")] NoInstance,
    #[error("{{args}} must be a whole token")] ArgsNotWhole,
    #[error("unterminated placeholder in {0:?}")] Unterminated(String),
}
/// Expand one argv. `{args}` as a whole token splices ctx.args. Single pass, left to right.
pub fn expand_argv(argv: &[String], ctx: &ExpandCtx, env: &dyn Fn(&str) -> Option<String>) -> Result<Argv, ExpandError>;
/// Expand one string (env values, bin path). `{args}` is an error here.
pub fn expand_str(s: &str, ctx: &ExpandCtx, env: &dyn Fn(&str) -> Option<String>) -> Result<String, ExpandError>;
```

Grammar, single pass: `${NAME}` -> env or empty; `${NAME:-default}` -> env or
`default` (default may itself contain `{x}` placeholders, expanded in the
same pass); `{name}` -> ctx field; `{{` -> `{`; `}}` -> `}`. Because the scan
is single-pass and left to right, `${LIBCLANG_PATH:-/usr/lib}` is consumed as
one env reference and never re-read as a `{...}` placeholder, and an env value
containing `{root}` is not expanded again.

## 5. sb-core: flow (`WorkspaceFlow`)

```rust
pub struct ModuleRef {
    pub path: PathBuf,
    pub up: bool,
    pub instances: Vec<ModuleInstance>,     // new
}
// Short form `name: path` is written only when up && instances.is_empty().
// ModuleRefMap (private) and the public ModuleRefRaw::Full gain `instances`.

pub struct ModuleInstance {
    pub id: String,
    pub label: Option<String>,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,      // new
    pub docker: Option<DockerSpec>,
}
// Deserialize: visit_str / visit_u64 / visit_i64 -> { id }, visit_map -> strict map form.
// Serialize: bare string when only `id` is set, map otherwise.

pub struct WorkspaceFlow {
    pub name: Option<String>,
    pub modules: BTreeMap<String, ModuleRef>,
    pub messages: Vec<MessageSource>,
    /// Legacy top-level form. Read, never written by 0.6 code paths that rebuild a flow.
    pub instances: BTreeMap<String, Vec<ModuleInstance>>,
    pub wires: BTreeMap<String, String>,    // new, skip if empty
    pub source: Option<FlowSource>,
}
impl WorkspaceFlow {
    /// Inline list if non-empty, else the legacy top-level list, else empty.
    pub fn instances_for(&self, module: &str) -> &[ModuleInstance];
    /// True when the legacy top-level map has any entry (for the deprecation warning).
    pub fn has_legacy_instances(&self) -> bool;
    /// Parsed wires. Errors are grammar only; existence checks need manifests (sb-doctor).
    pub fn wires(&self) -> Result<Vec<Wire>, FlowValidationError>;
}
// validate() additionally: a module with both inline and top-level instances
// -> BothInstanceForms(module); a top-level instances key naming no module ->
// InstancesForUnknownModule(key); each wire parses -> BadWire(lhs, reason).

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wire { pub sub: WireEnd, pub pub_: WireSource }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireEnd { pub module: String, pub entry: String }
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireSelector { None, SameId, All, Id(String) }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireSource { pub module: String, pub selector: WireSelector, pub entry: String }
/// lhs "<module>.<entry>", rhs "<module>[<sel>].<entry>" | "<module>.<entry>".
/// Split on the first '.'; module must pass validate_identifier; selector is
/// "{id}" | "*" | an id passing validate_instance_id.
pub fn parse_wire(lhs: &str, rhs: &str) -> Result<Wire, FlowValidationError>;
```

`crates/sb-core/src/topic.rs`:

```rust
/// "<module>" or "<module>-<id>": the one place the instance suffix is defined.
pub fn instance_module(module: &str, id: Option<&str>) -> String;
```

Optional `topic:`: in `RawSpec::topic_ref` (`config.rs:795-811`) the
`(None, None)` arm returns `TopicRef::owned(&self.name)` after checking every
char of `name` with `topic::is_valid_segment_char`. Serialization is
unchanged: `topic:` is always written back. `NeitherTopicNorKey` stays as a
variant (unreachable). Update the `expecting` text at `config.rs:931`.

## 6. sb-workspace

`crates/sb-workspace/src/lib.rs`:

```rust
pub use sb_core::{MODULE_FILE, LEGACY_MODULE_FILE, FLOW_FILE, LEGACY_FLOW_FILE};
/// First existing candidate in `dir`, else None.
pub fn find_module_file(dir: &Path) -> Option<PathBuf>;
pub fn find_flow_file(dir: &Path) -> Option<PathBuf>;
/// Where a new manifest / flow goes: always the new name.
pub fn new_module_file(dir: &Path) -> PathBuf;
pub fn new_flow_file(dir: &Path) -> PathBuf;
```

`resolve_module_yaml` (`lib.rs:484`): a directory entry probes
`module_file_candidates()`; an explicit file path is used as is. Registry
entries recorded by 0.5 installs (absolute `.../sb.dev.yml`) keep working
because they are explicit files.

`SbHome::flow_path(ws)` (`lib.rs:89`): installed copy is written as
`sb.flow.yaml`; `read_flow` probes `flow_file_candidates()` in the workspace
dir. `find_flow_upwards` (`lib.rs:269`) probes both names per directory, new
first.

```rust
pub struct InitOptions { pub root, pub language, pub force, pub docker, pub runscript: bool }  // runscript new, default false
pub struct InitOutcome {
    pub module, pub root_abs, pub language,
    pub manifest_path: PathBuf,       // replaces dev_yml_written's implied path
    pub manifest_written: bool,       // renamed from dev_yml_written
    pub legacy_manifest: bool,        // an existing sb.dev.yml was adopted, nothing written
    pub runscript_written: bool,
    pub io_dir_created, pub flow_appended, pub workspace, pub repo_flow,
    pub deprecations: Vec<sb_core::Deprecation>,
}
pub struct InstallReport { ..., pub deprecations: Vec<sb_core::Deprecation> }   // legacy flow name, legacy manifest names, top-level instances
```

`init_module`: `--docker` now means "preflight the docker instances and
write the manifest"; no bash template. Docker preflight runs before any file
is written. The runscript stub is written only when `opts.runscript` (new
CLI flag `sb init --runscript`), using the existing stub template kept in
`runscript.rs` (docker template and `render_docker` deleted).

`install_flow`: the rebuilt `ModuleRef` carries `instances`; `wires` is copied;
the legacy top-level `instances` map is copied as is (not normalized), and a
`Deprecation` is pushed when present.

`crates/sb-pubsub/src/lib.rs`:

```rust
pub enum ServiceRender { Skip, IfMissing, Always }
pub fn update_all(handle: &ModuleHandle, ctx: &GenCtx, services: ServiceRender) -> Result<UpdateReport>;  // was `include_services: bool`
```

`sb install` passes `IfMissing`; `sb io update --services` passes `Always`;
`sb io update` without the flag passes `Skip`. `resolve_module` (`lib.rs:77`)
probes `module_file_candidates()` for the two directory cases.

`crates/sb-gopro/src/lib.rs`: the prd file is written next to the manifest
as today; messages name "the module manifest" instead of `sb.dev.yml`.

## 7. sb-launch

```rust
pub const ENV_WORKSPACE: &str = "SB_WORKSPACE";
pub const ENV_MODULE: &str = "SB_MODULE";
pub const ENV_INSTANCE_ID: &str = "SB_INSTANCE_ID";
pub const ENV_MODULE_ROOT: &str = "SB_MODULE_ROOT";

pub fn session_name(workspace: &str) -> String;           // unchanged: "sb-<ws>"

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneCmd {
    Legacy { script: PathBuf, args: Vec<String> },        // today's `cd && ./runscript.bash args`
    Native { argv: Argv, env: BTreeMap<String, String> }, // argv already expanded; env already merged incl. SB_* vars
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pane {
    pub module: String,            // manifest `module:`
    pub window: String,            // instance_module(module, instance)
    pub instance: Option<String>,
    pub cwd: PathBuf,              // module root
    pub cmd: PaneCmd,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildStep { pub module: String, pub cwd: PathBuf, pub steps: Vec<Argv>, pub env: BTreeMap<String, String> }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    pub workspace: String,
    pub session: String,
    pub builds: Vec<BuildStep>,    // one per native module, in pane order, deduped by module
    pub panes: Vec<Pane>,
    pub skipped: Vec<(String, String)>,          // (module, reason) e.g. unsupported platform
    pub deprecations: Vec<sb_core::Deprecation>, // legacy runscript, legacy manifest name
}

pub fn plan_workspace(home: &SbHome, workspace: &str, host: &Host) -> Result<LaunchPlan>;
/// target: "<module>" (every instance) or "<module>:<id>" (one). `up: false` ignored here.
pub fn plan_run(home: &SbHome, workspace: &str, target: &str, args: &[String], host: &Host) -> Result<LaunchPlan>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopPlan {
    pub session: String,
    pub module: String,
    pub legacy: Option<Pane>,          // Some = run stopscript.bash in the module window (old path)
    pub stop: Option<BuildStep>,       // Some = run scripts.stop inline after killing windows
    pub deprecations: Vec<sb_core::Deprecation>,
}
pub fn plan_stop(home: &SbHome, workspace: &str, module: &str, args: &[String], host: &Host) -> Result<StopPlan>;

/// Run every BuildStep inline (inherited stdio), in order, fail fast. Never touches tmux.
pub fn run_builds(plan: &LaunchPlan) -> Result<()>;

pub struct UpOutcome { pub session: String, pub windows: Vec<String> }
pub fn up(plan: &LaunchPlan, tmux: &Path) -> Result<UpOutcome>;   // errors if session exists (unchanged rule)

pub struct RunOutcome { pub session: String, pub windows: Vec<String>, pub replaced: Vec<String> }
/// Creates the session on demand; respawns windows that exist, creates the rest; then kills
/// windows named <module> / <module>-* that are not in the plan (stale instances).
pub fn run(plan: &LaunchPlan, tmux: &Path) -> Result<RunOutcome>;

pub struct StopOutcome { pub session: String, pub killed: Vec<String>, pub ran_stop: bool, pub legacy: bool }
/// Native: kill every window named <module> or <module>-<anything> by window id, then run `stop`
/// inline. If that empties the session, let tmux end it. Legacy: today's respawn-with-stopscript.
pub fn stop(plan: &StopPlan, tmux: &Path) -> Result<StopOutcome>;

/// Exactly one pane in the plan, Native only. Builds first. Unix: exec; Windows: spawn and wait.
pub fn run_foreground(plan: &LaunchPlan) -> Result<i32>;

pub fn resolve_tmux(cfg_tmux: Option<&Path>) -> Result<PathBuf>;   // + on windows: which("tmux") then which("psmux")
pub fn has_session(tmux: &Path, session: &str) -> bool;            // exact: "-t", "=<session>"
pub fn down(workspace: &str, tmux: &Path) -> Result<DownOutcome>;  // unchanged shape, exact target
pub fn attach_info(workspace: &str, tmux: &Path) -> Result<AttachInfo>;  // unchanged
```

Mode rule in `plan_*`, per module: `scripts` declared anywhere (top level or
an overlay) or no `runscript.bash` at the root -> Native. `runscript.bash`
present and no `scripts` -> Legacy, push
`Deprecation{item: "runscript.bash", hint: "declare scripts: in sb.module.yaml; see sbcli_module.md §Migrating"}`.
Both present -> Native, push a Deprecation saying the script is ignored.

Native pane env, in this order (later wins): `Resolved.env` expanded with
`expand_str`, instance `env`, then `SB_WORKSPACE`, `SB_MODULE`,
`SB_MODULE_ROOT`, and `SB_INSTANCE_ID` when `instance` is Some.

Native pane argv: `expand_argv(Resolved.start)` with `ctx.args` = instance
`args` followed by passthrough args, then, when `instance` is Some, the two
tokens `--id <id>` appended **after** expansion (so a start command without
`{args}` still gets its id).

tmux invocation for a Native pane: `new-window` / `new-session` /
`respawn-window` with `-c <cwd>`, one `-e KEY=VAL` per env entry, `-n
<window>`, then the argv tokens as separate trailing arguments. tmux 3.0+
executes two or more trailing arguments directly, without a shell. tmux
treats exactly one trailing argument as a shell command line, so a
single-token argv is passed as one `shell_quote`d string instead. Legacy
panes keep today's `shell_command()` string. Set `remain-on-exit on` on
native windows so a crashed process leaves its output readable; `sb stop`
and `sb down` still close them. The launch agent confirms both behaviours
against `man tmux` on the box and records the psmux status in its report.

Targets: sessions as `=<session>`, windows by `#{window_id}` obtained from
`list-windows -t =<session> -F '#{window_id}\t#{window_name}'`.

## 8. sb-doctor

`check_workspace` (`crates/sb-doctor/src/lib.rs:562`) gains, after
`check_ws_io_files`, in this order:

| row name | level | message |
|---|---|---|
| `ws requires` | FAIL | `<module>: missing <bin>[, <bin>]` per module, joined by `; `; OK text `all required tools present` |
| `ws platforms` | WARN | `<module>: not supported on <os>-<arch> (declares: linux, windows)`; OK text `all modules support this host` |
| `ws wires` | FAIL | `<lhs>: <reason>` where reason is one of `unknown subscriber module`, `no subscriber <entry> in <module>`, `unknown publisher module`, `no publisher <entry> in <module>`, `[*] on iceoryx2 is not supported`, `{id} but <module> has no instances`, `transport mismatch (<a> vs <b>)`, `type mismatch (<a> vs <b>)`; OK text `<n> wire(s) resolve` or `no wires` |
| `deprecated` | WARN | one item per line: `sb.dev.yml at <path>`, `flow.yaml at <path>`, `runscript.bash at <path>`, `stopscript.bash at <path>`, `top-level instances: in <flow>`; each line ends `-> see sbcli_module.md §Migrating`; OK text `nothing deprecated in use` |

`requires` are resolved with `which::which` after
`ModuleDevConfig::resolve(host)`; `ws requires` is skipped (not FAIL) for a
module the host does not support. `WorkspaceView` gains `host: Host`.

The `ws modules` and other existing rows replace the literal `sb.dev.yml` in
messages with "module manifest".

## 9. sb-cli: deprecation helper

New file `crates/sb-cli/src/deprecation.rs`:

```rust
pub const SINCE: &str = "0.6.0";
pub const UNTIL: &str = "0.7.0";
pub mod items {
    pub const VERB_PUB: &str = "`sb pub`";
    pub const VERB_SUB: &str = "`sb sub`";
    pub const VERB_LIST: &str = "`sb list`";
    pub const VERB_SERVICE: &str = "`sb service`";
}
/// stderr: "warning: {item} is deprecated since {SINCE} and will be removed in {UNTIL}; {hint}"
/// Printed once per `item` per process (OnceLock<Mutex<HashSet<String>>>).
pub fn warn_deprecated(item: &str, hint: &str);
/// Convenience for the Vec<Deprecation> carried by outcomes and plans.
pub fn warn_all(items: &[sb_core::Deprecation]);
```

Hints for the four verbs: `use sb io list` / `use sb io add pub, sb io edit, sb io rm` /
`use sb io add sub, sb io edit, sb io rm` / `use sb io add service`.
Help strings on the four `Cmd` variants become
`Deprecated since 0.5.0, removed in 0.7.0: use ...`.

## 10. sb-cli: handlers and output strings

`RunArgs` gains `#[arg(long)] pub fg: bool`. `module` positional accepts
`<module>` or `<module>:<id>`. No `--id` flag (an L5 test passes `--id` as
passthrough). `StopArgs` unchanged.

Exact stdout lines (docs quote them):

```
sb up:
  launched {n} window(s) in tmux session "{session}"
    - {window}            (one per pane)
  skipped {module}: {reason}      (one per skipped, before the launched block)
  attach with: tmux attach -t {session}

sb run (tmux):
  launched {window} in tmux session "{session}"       (or "restarted {window} ...")
  one line per window; " (args: a b)" suffix when passthrough args given

sb run --fg:
  no banner; child's stdio inherited; sb exits with the child's code

sb stop (native):
  stopped {module}: closed {k} window(s) [{w1}, {w2}]
  ran scripts.stop for {module}         (only when it ran)
sb stop (legacy): unchanged text

build (before each BuildStep, stdout):
  building {module}: {argv joined by spaces}

sb init:
  adopted module {m} (language: {l}, root: {abs})
    wrote sb.module.yaml                 (or "  adopted existing sb.dev.yml (deprecated)")
    wrote runscript.bash (executable)    (only with --runscript)
    created IO directory
  registered in {repo flow}  |  (already registered in {repo flow})
    installed workspace {ws} (active)

sb install (workspace): existing lines; "no sb.dev.yml at" becomes "no module manifest at";
  "-- io: X skipped (no sb.dev.yml)" becomes "(no module manifest)";
  new line when a service router was rendered: "  rendered services/{name} for {module}"
```

Deprecation warnings go to stderr through §9 and never change stdout.

## 11. Not in 0.6.0

- Wire resolution in codegen (`GenCtx`, templates, `CodegenInput`): phase 3.
- Any change to `sb.config.yml`, `sb.app.yml`, or their extensions.
- `sb.prd.yml` schema.
- Removal of anything deprecated.
