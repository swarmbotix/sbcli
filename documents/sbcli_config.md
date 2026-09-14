# `sb config` — Configuration Reference

Official reference for `sb config`: where the config files live, the
layering chain that resolves the merged config every other `sb` verb
reads, and the two subcommands that mutate or open them.

This document is normative for behavior shipped at L1 (`open`,
`set`, `show`, `get`) and L2 (the `sb_home_dir` field). The
`--workspace` flag on `sb config set` is wired up at L5; until then
it errors with a clear message (§4.2).

The canonical spec is `requirements.md` in the sbcli **source
repository** — a maintainer document that is not part of your install;
when it and this page disagree, it wins.

---

## 1. Conceptual model

`sb.config.yml` is a **single schema, three possible locations**.
Every `sb` verb reads the *merged* result; nothing reads only one
layer in isolation.

| Layer | Path | Purpose |
|---|---|---|
| **Env override** | `$SB_CONFIG` env var → arbitrary path | Test isolation, CI overrides, ad-hoc one-off. Highest priority. |
| **Workspace** | `<sb_home>/workspaces/<active>/sb.config.yml` | Per-workspace overrides (e.g. a project that needs a different `device:`). |
| **Global** | `<sb_home>/sb.config.yml` | Per-user defaults. Most edits land here. |
| **Built-in** | hard-coded in `sb_core::config::SbCliConfig::builtin_defaults` | Final fallback (e.g. `transport_on_device: iceoryx2`, `device: dev01`). |

Resolution at each field is **independent**: if `protoc:` is set in
the global layer and `device:` is set in the workspace layer, the
merged config takes `protoc` from global and `device` from workspace.
There's no all-or-nothing inheritance.

---

## 2. Schema (`SbCliConfig`)

Defined in `crates/sb-core/src/config.rs`. `deny_unknown_fields` is
on; typos in keys are rejected at parse time.

```yaml
# Where the per-user swarmbotix tree lives. Default: ~/.swarmbotix
sb_home_dir: ~/swarmbotix_alt

# Host-specific tool paths
protoc:      /opt/protoc/bin/protoc
flatc:       /opt/flatc/bin/flatc
libzenohc:   /opt/zenoh-c/lib/libzenohc.so
libiceoryx2: /opt/iceoryx2/lib/libiceoryx2_ffi_c.so
tmux:        /usr/bin/tmux

# Message styles. Each style is a directory under messages_root holding
# its own message_definitions/ (sources) and message_targets/ (generated).
messages_root: /home/user/.swarmbotix/messages

# One schema, many fixed-size iox2 types (see sbcli_messages.md §6.4).
iox2_variants:
  swarmbotix.images.Image:
    Image480pMono8: { data: 307200 }   # 640 x 480 x 1
    Image1080pRgb8: { data: 6220800 }  # 1920 x 1080 x 3

# Per-field capacity for `repeated <scalar>`.
vec_caps:
  swarmbotix.primitives.Mat33.data: 9

# Defaults
device:                 dev01
transport_on_device:    iceoryx2
transport_cross_device: zenoh

# iox2 codegen capacities (read by `sb message compile --iox2`)
string_array_cap: 10
string_array_caps:
  sensor_msgs.JointState.name: 32
  trajectory_msgs.JointTrajectory.joint_names: 32
```

Every field is **optional** at every layer. Absence at one layer
means "fall through to the next". The merged result populates
unspecified fields from `builtin_defaults()`. Field-by-field:

| Field | Type | Built-in default | Owner / consumer |
|---|---|---|---|
| `sb_home_dir` | `Option<PathBuf>` | `~/.swarmbotix` (via `dirs::home_dir()`) | [sbcli_ws.md](sbcli_ws.md) §2 |
| `protoc` | `Option<PathBuf>` | none → `[SKIP]` in doctor | [sbcli_messages.md](sbcli_messages.md) §4.5, [sbcli_doctor.md](sbcli_doctor.md) §4.1 |
| `flatc` | `Option<PathBuf>` | none | sbcli_messages, sbcli_doctor |
| `libzenohc` | `Option<PathBuf>` | none | sbcli_doctor |
| `libiceoryx2` | `Option<PathBuf>` | none | sbcli_doctor |
| `tmux` | `Option<PathBuf>` | none | sbcli_doctor; L5 `sb up` |
| `messages_root` | `Option<PathBuf>` | `<sb_home>/messages` | sbcli_messages §1 |
| ~~`message_style`~~ | — | **DEAD** — parsed, ignored, stripped on upgrade | §2.1 |
| ~~`message_definitions`~~ | — | **DEAD** — derived per style from the message name | §2.1 |
| ~~`message_targets`~~ | — | **DEAD** — same | §2.1 |
| `device` | `Option<String>` | `"dev01"` | Codegen prefix (`/<device>/<workspace>/<module>/<transport>/<topic>`) |
| `transport_on_device` | `Option<Transport>` (`zenoh` / `iceoryx2`) | `iceoryx2` | [sbcli_pubsub.md](sbcli_pubsub.md) §7 |
| `transport_cross_device` | `Option<Transport>` | `zenoh` | sbcli_pubsub (future routing) |
| `string_array_cap` | `Option<u32>` | `10` | sbcli_messages §5 (iox2 caps) |
| `string_array_caps` | `Option<BTreeMap<String, u32>>` | a small built-in for common ROS2 fields | sbcli_messages §5 |
| `bytes_caps` | `Option<BTreeMap<String, u32>>` | per-field `bytes` capacity, keyed `<pkg>.<Msg>.<field>` | sbcli_messages §5 |
| `vec_caps` | `Option<BTreeMap<String, u32>>` | per-field `repeated <scalar>` element count, same key shape | sbcli_messages §5 |
| `iox2_variants` | `Option<BTreeMap<String, BTreeMap<String, BTreeMap<String, u32>>>>` | `<pkg>.<Msg>` → variant type name → field → capacity; 8 built-in image variants | sbcli_messages §5.1 |

Path fields support `~/` prefix; the resolver tilde-expands them
against `dirs::home_dir()`. Other expansion (`$VAR`, `${VAR}`) is
**not** performed.

### 2.1 Message styles

Messages are grouped into **styles**. A style is one directory under
`messages_root` with two children:

```text
<messages_root>/
  ros2/                        ← shipped: std/ + the ROS2 mirror (138 .proto)
    message_definitions/
    message_targets/
  swarmbotix/                  ← shipped: the main style — header/,
    message_definitions/         primitives/, images/, sensors/
    message_targets/
  <yours>/                     ← styles are an OPEN SET
    message_definitions/
```

`sb_config::resolve_message_definitions` / `resolve_message_targets` are
the single source of truth for these paths — every crate calls them, so
`sb-cli` and `sb-pubsub` cannot disagree about where generated types live.

Using a style — you name it in the message, every time:

```bash
sb message list                                      # every style
sb pub add -m ros2/std/StringStamped chatter --zenoh
sb pub add -m swarmbotix/images/Image480pMono8 cam --iox2
sb message compile --style ros2                      # a FILTER, not a mode
sb doctor                                            # what's on disk
```

Style names must be plain directory names — `/`, `\`, `:`, `.` and `..`
are rejected so a style can never resolve outside `messages_root`.

**`swarmbotix` is the default** — both the built-in fallback and what a
fresh install seeds into `sb.config.yml`.

The one exception is an **upgrade from the pre-0.1.31 flat layout**: that
vault migrates wholesale into `ros2`, so the installer seeds `ros2` for
those boxes. Activating `swarmbotix` there would hide every message the
user already had. Switching afterwards is one line.

Cross-style imports are not supported — `protoc` runs with a single
style's `message_definitions/` as its include root. The shipped split is
verified clean: nothing in the ROS2 namespaces imports `std/`, and `std/`
imports only `std/`.

---

## 3. Layering — `SbCliConfig::merge(env, ws, global)`

Algorithm:

1. Start with `SbCliConfig::default()` (all `None`).
2. For each layer in order [env, workspace, global, built-in], call
   `fill_from(layer)`: for each field, if the current accumulator is
   `None`, copy the layer's value. Non-`None` fields in higher
   layers are **never** overwritten.

Loading happens via `sb_config::load()` which returns a `(merged,
ConfigSources)` pair. `ConfigSources` records which file populated
each layer; it's used internally for diagnostics. There's no CLI
surface for it yet (planned).

`$SB_CONFIG` is the **only** env var sb reads for config. There's no
`SB_PROTOC=...`-style per-field override. Set the file path, write
the field, point `$SB_CONFIG` at it.

### 3.1 Workspace layer activation

The workspace layer reads from
`<sb_home>/workspaces/<active>/sb.config.yml`. "Active" is whatever
`<sb_home>/active` names ([sbcli_ws.md](sbcli_ws.md) §3). If no
workspace is active, the workspace layer is skipped entirely (no
file read attempted).

The active workspace's `sb_home_dir:` field is **ignored** for
self-resolution — if it weren't, you'd have a chicken-and-egg
(resolving `<sb_home>` requires reading the workspace config which
lives under `<sb_home>`). The resolver computes `<sb_home>` from the
env + global layers only.

### 3.2 Built-in defaults

```rust
SbCliConfig {
    device: Some("dev01".into()),
    transport_on_device: Some(Transport::Iceoryx2),
    transport_cross_device: Some(Transport::Zenoh),
    string_array_cap: Some(10),
    string_array_caps: Some(<small ROS2 map>),
    // everything else None
}
```

The built-ins guarantee a minimal-working config even when no file
exists anywhere. The user's first `sb message list` against an empty
host:

- Uses `~/.swarmbotix/messages/ros2/message_definitions` as the vault path.
- Installs the forge-bundled `std/*` there.
- Falls back to `device: dev01`, `transport_on_device: iceoryx2` for
  every codegen call.

---

## 4. Subcommands

### 4.1 `sb config` (alias: `sb config open`)

```bash
sb config
sb config open      # equivalent
```

**Effects (in order)**:

1. Resolve the global config path:
   `dirs::home_dir().unwrap().join(".swarmbotix/sb.config.yml")`.
   This is the **global** layer's path — `open` is always editing
   the global file. The workspace-scoped variant (planned) will be a
   future `sb config open --workspace`.
2. If the file doesn't exist, create the parent directory and write
   a placeholder:
   ```yaml
   # sb.config.yml — see requirements.md
   ```
3. Read `$EDITOR`. If unset, default to `vi`.
4. Spawn `$EDITOR <path>` and wait.
5. Exit 0 if the editor exits cleanly. Surface a non-zero editor
   exit as `<editor> exited with <status>`.

`open` does **not** validate the YAML after the editor closes. A
broken edit becomes a load-time error on the next sb command.
Workarounds:

- Use a YAML-aware editor (vim with `set ft=yaml`, VS Code with the
  YAML extension, etc.) so syntax errors are obvious.
- Run `sb doctor` immediately after to sanity-check.

### 4.2 `sb config set <key> <value> [--workspace]`

```bash
sb config set protoc /opt/protoc/bin/protoc
sb config set transport_on_device zenoh
sb config set sb_home_dir /home/user/swarm_alt
```

**Effects (in order)**:

1. If `--workspace` is set, error:
   ```
   --workspace requires an active workspace (L5+); not yet implemented
   ```
   The flag exists on the CLI surface so scripts written today won't
   break when L5 wires it up; the current behavior is the explicit
   error.
2. Resolve the global config path (same as `open`'s step 1).
3. Read the existing file as YAML (or treat as empty if missing).
4. Apply the `<key> <value>` pair via `sb_config::set_key`:
   - The key string must match one of the schema fields (§2).
     Typos like `transport_ondevice` produce an error from
     `SbCliConfig`'s `deny_unknown_fields` once the file is read
     back.
   - The value is parsed according to the field's type — string
     fields take the raw string, path fields take the raw string
     (no tilde-expansion at set-time), `Transport` parses
     `zenoh` / `iceoryx2`, numeric caps parse u32.
5. Write the merged document back, preserving the rest of the
   file's content.

Output:

```
set protoc in /home/el/.swarmbotix/sb.config.yml
```

Limitations of `sb config set` (current implementation):

- Doesn't support nested fields (e.g. `string_array_caps.sensor_msgs.JointState.name`).
  For those, use `sb config open` and hand-edit the map.
- Doesn't validate path existence — `sb config set protoc /nope` is
  accepted; `sb doctor` then reports `[FAIL] protoc /nope`. By
  design (configs are sometimes set on one host for a different
  host).
- Doesn't surface the layer being written. It's **always** the
  global layer until `--workspace` is wired.

### 4.3 `sb config show [--json] [--sources]`

```bash
sb config show
sb config show --json
sb config show --sources
sb config show --sources --json
```

Prints the **merged** config that every other `sb` verb sees —
`$SB_CONFIG` → workspace → global → built-in defaults, in that
priority order. Every mode also leads with two location hints so the
caller doesn't have to know the layering rules to find them:

- **`config file:`** — path to the global `sb.config.yml`
  (`~/.swarmbotix/sb.config.yml` by default). This is the file
  `sb config open` and `sb config set` write to. Surfaced even when
  the file doesn't exist yet, so the user knows where the first
  `set` will land.
- **`documents:`** — path to the bundled reference docs
  (`<sb_home>/documents`, where `<sb_home>` honors the merged
  `sb_home_dir`). This is where `sbcli_config.md`, `sbcli_pubsub.md`,
  etc. live on the installed host.

Output modes:

| Flags | Output |
|---|---|
| _(none)_ | YAML on stdout, fields in the canonical order. Built-in defaults are inlined (so e.g. `transport_on_device: iceoryx2` shows up even when no file mentions it). The two location hints lead as `# config file: ...` and `# documents: ...` comments — still valid YAML. |
| `--json` | Same content, pretty-printed JSON. Designed for downstream tools — `jq '.message_targets'`, an LLM in a runscript wrapper, etc. Adds top-level `_config_file` and `_documents` string keys alongside the schema fields. |
| `--sources` | A three-column text table: `key  value  source`. The source is one of `env` / `workspace` / `global` / `builtin` / `unset`. The two location hints print as plain `config file: ...` / `documents: ...` lines above the table. |
| `--sources --json` | A JSON object keyed by field name; each value is `{"value": ..., "source": "..."}` — `value` is `null` when the field is unset. Also includes top-level `_config_file` and `_documents` string keys. |

```bash
$ sb config show --sources
config file: /home/el/.swarmbotix/sb.config.yml
documents:   /home/el/.swarmbotix/documents

key                     value                           source
---------------------------------------------------------------
sb_home_dir             (unset)                         unset
protoc                  /opt/protoc/bin/protoc          global
flatc                   /opt/flatc/bin/flatc            global
libzenohc               /opt/zenoh-c/lib/libzenohc.so   global
libiceoryx2             (unset)                         unset
tmux                    /usr/bin/tmux                   global
messages_root           ~/.swarmbotix/messages          global
message_definitions     (unset)                         unset
message_targets         /opt/build/targets              env
device                  dev01                           builtin
transport_on_device     iceoryx2                        builtin
transport_cross_device  zenoh                           builtin
string_array_cap        10                              builtin
string_array_caps       5 entries                       builtin
```

Example of the default YAML output (the two header comments come
first so a user pasting the output into a chat can see where things
live without re-running anything):

```bash
$ sb config show
# config file: /home/el/.swarmbotix/sb.config.yml
# documents:   /home/el/.swarmbotix/documents
protoc: /opt/protoc/bin/protoc
flatc: /opt/flatc/bin/flatc
device: dev01
transport_on_device: iceoryx2
...
```

The `documents:` path tracks `sb_home_dir` — if a user moves the
swarmbotix tree (§6.3) the documents path moves with it.

`string_array_caps` (a nested map) is rendered as `N entries` in the
table; use `sb config show` (full YAML/JSON) to see its contents.

`show` is read-only — it never writes or creates files, never opens
an editor. It's a snapshot of `sb_config::load_layers` over the
current `$SB_CONFIG` / workspace / global state.

### 4.4 `sb config get <key> [--json]`

```bash
sb config get device
sb config get message_targets
sb config get protoc --json
```

Print a single resolved field. Without `--json`, the bare value is
written to stdout (no quoting, no trailing decoration — safe for
`$(sb config get …)` substitution). With `--json`, output is one
JSON object `{"key": "...", "value": "...", "source": "..."}`; the
`value` is `null` when the field is unset.

Exit codes:

| Outcome | Exit code |
|---|---|
| Field set, value printed | 0 |
| Field is a known key but unset in every layer (incl. built-ins) | 1 — stderr says `<key> is unset (source: unset)` |
| Field name is not in the schema | 1 — stderr lists every known key |

The unset-exits-nonzero behavior mirrors `git config --get`, so
scripts can do `if val=$(sb config get protoc 2>/dev/null); then …`
without parsing stderr.

`get` is read-only — same as `show`, never writes anything.

### 4.5 Designed for downstream LLMs and build glue

The `show` + `get` pair exists so a *user-side* LLM or runscript
doesn't have to re-implement sb's layering rules to know where the
message bindings live or which transport will be picked. The
prescribed pattern for a downstream tool that needs sb's paths:

1. Call `sb config show --json` once, cache it for the session.
2. Read whatever fields it needs (`message_targets`,
   `message_definitions`, `device`, `transport_on_device`, …).
3. Wire those into the toolchain.

For example: a CMake glue script that needs `protoc`-generated C++
headers picks them up with `$(sb config get message_targets)/proto`
on its `-I` path. A Python launcher needs the iox2 bindings dir
similarly.

#### 4.5.1 Python import resolution — prefer `.pth`, otherwise ask

The L3 codegen emits Python files under
`<module_root>/<io_dir>/{publishers,subscribers}/<name>.py` and the
message bindings under `<message_targets>/{iox2,proto,fb}/<ns>/<Leaf>/`.
None of these are on `sys.path` by default. The user-side LLM has
to set things up; for that decision, **prefer `.pth` files** unless
the user explicitly wants something else.

A `.pth` file in the user's site-packages (or the active venv's
site-packages) is the lowest-friction option:

- Python imports it automatically every interpreter start; no
  per-script `sys.path.insert(...)` boilerplate.
- It's just two lines in a text file; the user can read and delete it.
- It survives editor reloads, IDE Python integrations, Jupyter
  kernels, `pytest`, and CLI scripts uniformly.
- One `.pth` covers both the per-module `swarmbotix_io/` and the
  central `<message_targets>/{iox2,proto}/` trees.

Recommended LLM script (assuming sb is on PATH and Python is the
current interpreter):

```python
# Resolve sb paths once.
import json, subprocess, sys, site, pathlib
cfg = json.loads(subprocess.check_output(["sb", "config", "show", "--json"]))
targets = pathlib.Path(cfg["message_targets"])

# Pick the right site-packages: prefer the active venv, fall back to user.
site_dir = pathlib.Path(site.getsitepackages()[0] if hasattr(sys, "real_prefix")
                        or sys.base_prefix != sys.prefix
                        else site.getusersitepackages())
site_dir.mkdir(parents=True, exist_ok=True)

# Write the .pth — each line is a directory to add to sys.path.
pth = site_dir / "swarmbotix.pth"
pth.write_text("\n".join([
    str(targets / "iox2"),    # for `from <ns>.<Leaf>.<Leaf> import <Leaf>` (iox2 ctypes)
    str(targets / "proto"),   # for `from <ns>.<Leaf>.<Leaf>_pb2 import <Leaf>` (protobuf)
    # Each adopted module's swarmbotix_io/ also needs an entry, e.g.:
    # str(pathlib.Path("/abs/path/to/camera_app/swarmbotix_io")),
]) + "\n")
```

(The per-module `swarmbotix_io/` paths come from each adopted
module's `sb.dev.yml` — iterate over the active workspace's
`flow.yaml` to gather them.)

If `.pth` isn't viable — common reasons:

- User is on a managed Python install where site-packages is
  read-only (`/usr/lib/python3/dist-packages` under apt, the system
  framework on macOS).
- User uses `uv` / `poetry` / `pipenv` and prefers project-scoped
  setup.
- User has a build system (Bazel, Pants, Buck) that prefers
  explicit `PYTHONPATH` injection at run time.

Then **ask the user which they prefer** rather than silently
picking one:

1. `.pth` in user-site (`~/.local/lib/python<ver>/site-packages/swarmbotix.pth`)
   — survives venv changes for the same interpreter, requires no
   editing of the project.
2. `.pth` inside the project's active venv (`<venv>/lib/python<ver>/site-packages/swarmbotix.pth`)
   — scoped to the venv, gone when the venv is recreated.
3. `PYTHONPATH=` prefix in the module's `runscript.bash` — keeps
   the choice in the repo, requires nothing outside the project.
4. `sys.path.insert(...)` shim at the top of `main.py` — what the
   shipped examples in the source repo's `examples/` do; least magic, most
   visible.

The user's preference depends on their toolchain, not on sb. The
downstream LLM should present the four options, pick the one the
user names, and write the chosen artifact. Default to option 1 if
the user expresses no preference and the user-site is writable.

#### 4.5.2 Python IDE / static analyzer paths — orthogonal to the runtime

`.pth`, `PYTHONPATH`, and `sys.path.insert(...)` all affect the
**interpreter** at run time. They do **not** automatically tell the
IDE's static analyzer where the generated modules live, so the
editor still flags `from std.Header.Header_pb2 import Header` with a
yellow "unresolved import" squiggle even though the code runs fine.
Downstream LLMs configuring a swarmbotix workspace should treat
**runtime path** and **analyzer path** as two distinct surfaces and
write both.

The analyzer-side path is keyed on the editor's language server, not
on Python itself. Each language server has its own knob; values are
the **same directory list** the runtime uses (`<message_targets>/iox2`,
`<message_targets>/proto`, each module's `swarmbotix_io/`).

| Editor / LS | File the LLM writes | Key |
|---|---|---|
| VS Code (Pylance / `ms-python.python`) | `<project>/.vscode/settings.json` | `python.analysis.extraPaths` — JSON array of absolute paths |
| Pyright (standalone, also coc-pyright / nvim-lspconfig) | `<project>/pyrightconfig.json` | `extraPaths` — JSON array, same shape as Pylance |
| Pyright via `pyproject.toml` | `<project>/pyproject.toml` `[tool.pyright]` | `extraPaths = [...]` (TOML array) |
| JetBrains PyCharm | `<project>/.idea/<name>.iml` | `<sourceFolder url="file://...">` entries (often done via UI "Mark Directory as → Sources Root"; `.pth` is also honored when the interpreter is set correctly) |
| Neovim + `pylsp` (python-lsp-server) | `<project>/.python-lsp.json` or workspace settings | `pylsp.plugins.jedi.extra_paths` |
| Sublime Text + LSP-pyright | Sublime project file | `settings.LSP-pyright.settings.python.analysis.extraPaths` |
| mypy (separate tool, not an LS) | `<project>/mypy.ini` or `[tool.mypy]` in `pyproject.toml` | `mypy_path` (colon-separated string) |
| Ruff (no type-checking yet, but reports `F401`) | `<project>/pyproject.toml` `[tool.ruff]` | `src = [...]` |
| Jupyter / IPython | none — kernels use the interpreter's `sys.path`, so `.pth` + `PYTHONPATH` already work |

Worked example for VS Code — the LLM resolves the paths via
`sb config show --json`, then writes `.vscode/settings.json`:

```python
import json, pathlib, subprocess
cfg = json.loads(subprocess.check_output(["sb", "config", "show", "--json"]))
targets = pathlib.Path(cfg["message_targets"])

vscode_dir = pathlib.Path(".vscode")
vscode_dir.mkdir(exist_ok=True)
settings_path = vscode_dir / "settings.json"

# Merge with any existing settings rather than clobbering.
settings = json.loads(settings_path.read_text()) if settings_path.exists() else {}
extra = settings.setdefault("python.analysis.extraPaths", [])
for p in [str(targets / "iox2"), str(targets / "proto")]:
    if p not in extra:
        extra.append(p)
# Each adopted module's swarmbotix_io/ also belongs here — iterate
# the active workspace's flow.yaml for those.

settings_path.write_text(json.dumps(settings, indent=2) + "\n")
```

Same content, different file, for Pyright outside VS Code:

```json
// pyrightconfig.json
{
  "extraPaths": [
    "/abs/path/to/message_targets/iox2",
    "/abs/path/to/message_targets/proto",
    "/abs/path/to/camera_app/swarmbotix_io"
  ]
}
```

Rules of thumb for the downstream LLM:

1. **Always do both**: write the runtime path-setup (§4.5.1) *and* the
   analyzer-path config. Skipping the analyzer leaves the user with
   working code that looks broken in the editor — a worse UX than
   visibly-broken code.
2. **Detect the editor from the project tree, not the user's OS**:
   `.vscode/` → VS Code; `.idea/` → JetBrains; `pyrightconfig.json`
   or `[tool.pyright]` already present → Pyright; otherwise ask.
3. **Use absolute paths**. Relative paths are interpreted from the
   project root by some language servers and from the LS's cwd by
   others; absolute removes the ambiguity.
4. **Merge, don't clobber**. The user may already have unrelated
   `extraPaths` entries; append rather than overwriting the array.
5. **Mention the change in the chat reply**. The user will likely
   need to reload the window (`Developer: Reload Window` in VS Code,
   `:LspRestart` in Neovim) before the analyzer rescans.

For C++, the analogous mechanism is `target_include_directories` in
`CMakeLists.txt` — same shape of decision, but CMake doesn't have a
`.pth` equivalent, so the user must wire the include paths
themselves. The matching IDE-analyzer surface for C++ is
`compile_commands.json` (clangd, VS Code C/C++ extension, CLion);
the CMake build itself emits it via `-DCMAKE_EXPORT_COMPILE_COMMANDS=ON`,
so once the build paths are right the analyzer follows
automatically. `sb config get message_targets` gives the LLM the
value to plug into the CMake step.

### 4.6 Inspecting raw layers

If you want to see the *unmerged* layer files directly:

```bash
cat ~/.swarmbotix/sb.config.yml             # global layer
cat "$SB_CONFIG"                            # env override layer (if set)
cat ~/.swarmbotix/workspaces/$(cat ~/.swarmbotix/active)/sb.config.yml   # workspace layer
```

`sb config show` is normally enough — `--sources` already attributes
each field to a layer.

---

## 5. The `$SB_CONFIG` env override

```bash
export SB_CONFIG=/tmp/scratch.yml
cat > /tmp/scratch.yml <<EOF
sb_home_dir: /tmp/sb_home
transport_on_device: zenoh
EOF

sb ws create demo    # uses /tmp/sb_home as the root
sb pub add ...       # uses zenoh as the default transport
```

The env override is the highest layer in the merge chain (§3). It's
how the L2/L3 integration tests pin each test to a hermetic tempdir
(see `crates/sb-cli/tests/common/l2_sandbox.rs`).

`$SB_CONFIG`'s **file** can be incomplete — missing fields fall
through to lower layers like any other. Pin only what the test or
workflow needs to override.

`sb config set` does **not** write to `$SB_CONFIG`'s path even when
that var is set. The env override is read-only from `sb`'s
perspective; mutate it manually if needed.

---

## 6. Examples

### 6.1 First-run setup on a new host

```bash
# Bring up the message vault and discover what's missing.
sb message list                # installs std/* bundle to ~/.swarmbotix/messages/ros2/message_definitions
sb doctor                      # five [SKIP]s for every tool field

# Wire up the tool paths.
sb config set protoc      /opt/protoc/bin/protoc
sb config set flatc       /opt/flatc/bin/flatc
sb config set libzenohc   /opt/zenoh-c/lib/libzenohc.so
sb config set libiceoryx2 /opt/iceoryx2/lib/libiceoryx2_ffi_c.so
sb config set tmux        /usr/bin/tmux

sb doctor                      # everything [OK]
```

### 6.2 Per-host device identifier

Each robot in a fleet gets its own `device:` so topics namespace
cleanly:

```bash
# On dev01
sb config set device dev01

# On nav-robot-5
sb config set device nav_robot_5
```

`sb pub add` then prefixes `/nav_robot_5/<ws>/<module>/<topic>` for
publishers and the cross-device subscribers can address that
namespace directly.

### 6.3 Move the swarmbotix tree

```bash
# Move from ~/.swarmbotix to /opt/swarm
mv ~/.swarmbotix /opt/swarm
sb config set sb_home_dir /opt/swarm

# Verify
sb ws list             # reads /opt/swarm/workspaces
sb doctor              # std vault check uses /opt/swarm/...
```

### 6.4 Test-isolated config via `$SB_CONFIG`

```bash
SB_HOME_DIR=$(mktemp -d)
SB_CONFIG=$SB_HOME_DIR/config.yml
cat > "$SB_CONFIG" <<EOF
sb_home_dir: $SB_HOME_DIR
transport_on_device: zenoh
EOF

SB_CONFIG=$SB_CONFIG sb ws create demo
SB_CONFIG=$SB_CONFIG sb ws set demo
SB_CONFIG=$SB_CONFIG sb pub add -m ros2/std/StringStamped hello   # uses zenoh as the default

rm -rf "$SB_HOME_DIR"
```

This is the pattern every integration test uses. Notice that the
user's real `~/.swarmbotix/` is untouched throughout.

### 6.5 Flip the default transport per-workspace (post-L5)

```bash
# (future, once `sb config --workspace` lands)
sb ws set nav
sb config set --workspace transport_on_device zenoh
# now `sb pub add` from within the `nav` workspace defaults to zenoh
sb ws set perception
# perception still uses iceoryx2 (the global default)
```

Until L5 ships this, hand-write
`<sb_home>/workspaces/nav/sb.config.yml`:

```yaml
transport_on_device: zenoh
```

The workspace layer is read by the merge function today; the only
thing missing is the convenience writer.

---

## 7. Where each verb actually reads from

A reference table to clarify which config keys influence which sb
behavior — useful when debugging "why did sb pick X transport here?"
or "where does this filename come from?".

| Verb | Reads (merged config) |
|---|---|
| `sb doctor` | `protoc`, `flatc`, `libzenohc`, `libiceoryx2`, `tmux`, `messages_root`, `sb_home_dir` |
| `sb message list` / `new` / `rm` | `sb_home_dir`, `messages_root` (latter only for `rm` cleanup) |
| `sb message edit` | `messages_root`, `protoc` / `flatc` (post-edit auto-compile) |
| `sb message compile` | `messages_root`, `protoc`, `flatc`, `string_array_cap`, `string_array_caps`, all cap CLI flags |
| `sb ws *` | `sb_home_dir` (resolves `<sb_home>`) |
| `sb init` | `sb_home_dir` (workspace lookup) |
| `sb pub/sub add/edit/rm` | `sb_home_dir`, `messages_root` (vault lookup), `device`, `transport_on_device`, `transport_cross_device` |
| `sb list` / `sb pub list` / `sb sub list` | `sb_home_dir`, `device` (only used by topic-prefix derivation in subcommands that re-emit files) |
| `sb service init` | `sb_home_dir`, `messages_root`, `device` — the router's namespace is `<device>/<workspace>/<module>/zenoh/service/**`, resolved through the same chain as `sb pub/sub` |
| `sb topic list` / `pub` | `sb_home_dir`, `device` — needed only to expand a **bare** topic against cwd's module + the active workspace. A leading-`/` topic short-circuits before the config is even loaded |
| `sb topic listen` | same as above |
| `sb topic prune` | nothing from `sb.config.yml` — it walks iceoryx2's own global node registry |
| `sb up` / `run` / `stop` / `down` / `attach` | `sb_home_dir` (to find the active workspace's `flow.yaml`) and `tmux` (falls back to `tmux` on `PATH` when unset). Module paths come from `flow.yaml`, never from config |
| `sb gopro` | `sb_home_dir`, `messages_root`, `device` (module resolution, same chain as `sb pub/sub`) |
| `sb config open` / `set` | only the path resolver (writes to global) |
| `sb config show` / `get` | every field — full merge over env / workspace / global / built-in |

`sb_home_dir` is read essentially everywhere — it's the root of
every other path.

---

## 8. Error catalogue

| Code path | Message |
|---|---|
| `set --workspace` (L1) | `--workspace requires an active workspace (L5+); not yet implemented` |
| `open` editor fails | `<editor> exited with <status>` |
| `open` editor spawn | `failed to spawn <editor>: <io error>` |
| Parse failure (any verb that reads config) | `parsing <path>` (chained from `serde_yaml`) |
| Unknown field | `unknown field "<key>", expected one of ...` (from `deny_unknown_fields`) |
| Bad `Transport` value | `unknown transport: "<v>"` |
| `get` of an unset field | `<key> is unset (source: unset)` — exit code 1 |
| `get` / `show` with unknown key | `unknown key "<key>". Known: <comma-separated list>` |

---

## 9. Edge cases and gotchas

### 9.1 The placeholder file has no fields

When `sb config open` creates the global file on first invocation,
the body is just a comment. That's intentional — empty + comment
parses cleanly to `SbCliConfig::default()`, and the merge chain
fills in every field from built-ins. You don't need to populate
anything to use sb; you only edit the file when you want to
override something.

### 9.2 YAML floats vs integers in cap fields

`string_array_cap: 10` is u32. `string_array_cap: 10.0` parses as
the YAML float `10.0` and fails the u32 deserialize. The error
message points at the offending line via `serde_yaml`.

### 9.3 Tilde expansion is one-shot

`sb.config.yml`:

```yaml
sb_home_dir: ~/swarm
```

resolves to `/home/user/swarm`. But:

```yaml
sb_home_dir: ~/swarm/sub
```

also works — tilde is only special at the **start** of the string.
`/home/user/swarm/~/sub` (tilde in the middle) is taken literally.

### 9.4 `set` is line-aware via `serde_yaml::to_string`

`sb config set <k> <v>` reads + parses + rewrites the entire YAML.
Comments in the file are **lost** on the rewrite. If you want to
keep hand-annotated comments, use `sb config open` for any change
you'd otherwise make with `set`.

### 9.5 `protoc:` vs `protoc-gen-*`

The `protoc:` field is the **path to the protoc binary itself**, not
a directory. Common mistake:

```yaml
protoc: /opt/protoc                     # ❌ dir, doctor fails
protoc: /opt/protoc/bin/protoc          # ✓ binary path
```

Doctor's failure message includes the full path it tried, so the
mistake surfaces quickly.

### 9.6 Layering across multiple `$SB_CONFIG` paths

There's no chain — `$SB_CONFIG` is a single path, not a colon-
separated list. To compose multiple overrides, write one file that
contains the desired union.

### 9.7 Writing to `$SB_CONFIG` directly

`sb config set` always targets the global path
(`~/.swarmbotix/sb.config.yml`), even when `$SB_CONFIG` is set. If
you intend to mutate the env-pointed file, edit it directly:

```bash
echo "transport_on_device: zenoh" >> "$SB_CONFIG"
```

`sb config set` is for the persistent global config. `$SB_CONFIG` is
for short-lived overrides; you typically don't `set` against it.

### 9.8 First-run race

Two parallel `sb message list` invocations on a fresh box might both
try to install the `std/*` bundle. The second writer wins because
`Vault::ensure_installed` writes per-file with no locking. Final
state is consistent (both see the same bundle bytes), but the
intermediate stdout may show "installed N std file(s)" from each.
Single-user interactive flows don't hit this; CI matrices that share
`$HOME` could.
