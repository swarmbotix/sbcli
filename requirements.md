# Swarmbotix CLI (`sb`) — Requirements

> Built in Rust. A tooling CLI — not a runtime library.

---

## Problem

### ROS2 dictates too much
- Build system (colcon, ament)
- Python environment — conflicts with system packages (e.g. OpenCV)
- Message protocol — ROS2-only IDL, Flutter/mobile apps cannot parse
- Transport for the entire system — DDS or Zenoh, no mixing
- Result: bridge layers everywhere, adding latency in real-time robot operation

### dora-rs dictates too much
- Dataflow graph — apps must conform to declared structure
- Message protocol — Apache Arrow only
- No `topic list` / `topic listen` from another terminal — blind during runtime
- Shared memory only, no off-board transport to mobile

### The real gap
Neither tool gives you:
- `ros2 topic list` / `ros2 topic echo` ergonomics without the ROS2 tax
- A mobile-native (Flutter/Android) path without bridge layers
- Freedom to use your own build system, transport, and message protocol

---

## Goal

A **unified IO pattern** for apps written in Rust, Python, C++, Unity (C#), and Flutter.

- Does **not** dictate transport, message protocol, or build system
- Does **not** add runtime dependencies to your app
- Provides minimum assumptions only: Zenoh and Iceoryx2 must be available via the system or language package manager
- Ships with standard message definitions in Protobuf IDL as the default — easy to convert to other formats

### Minimum assumptions per language

| Language | Required |
|---|---|
| Rust | `zenoh`, `iceoryx2` in `Cargo.toml` (cargo manages) |
| Python | `pip install eclipse-zenoh iceoryx2` in current env |
| C++ | `libzenohc.so`, `libiceoryx2_ffi_c.so` at system level |
| Flutter | `flutter pub add zenoh_flutter` (Zenoh only — no shmem on mobile) |
| Unity | `zenoh.dll` in `Assets/Plugins/` (Zenoh only) |

Optional:
- Path to `protoc` executable
- Path to `flatc` executable
- Path to other message protocol compilers

---

## What `sb` Is

`sb` is a **Rust CLI** (single static binary, installable via `cargo install` or prebuilt release) that:

1. Enforces topic naming rules across apps
2. Creates and manages the `swarmbotix_io/` IO folder structure per app
3. Generates pub/sub files using raw Zenoh / Iceoryx2 APIs (no wrapper)
4. Manages the workspace registry (`sb.workspace.yaml` plus flow files `*.flow.yaml`, `sb 0.8.0+`; one `sb.flow.yaml` through 0.7; `flow.yaml` before 0.6.0, not read since 0.7.0)
5. Generates per-app config files (`sb.module.yaml`, `sb.prd.yml`; `sb.dev.yml` before 0.6.0, not read since 0.7.0)
6. Provides live topic introspection via raw Zenoh (`sb topic list/listen`)
7. Builds and launches modules via tmux/psmux (`sb up [FLOW]`), natively from `sb.module.yaml` (`sb 0.6.0+`), the nodes of one flow (`sb 0.8.0+`)

`sb` does **not**:
- Wrap Zenoh or Iceoryx2 APIs
- Add runtime dependencies to your app
- Touch your app's build system
- Dictate your app's internal folder structure beyond the IO folder

---

## Transport

### Zenoh — off-board (cross-device, mobile-native)
- Default for cross-device, cross-process communication
- Flutter and Unity use Zenoh exclusively (no shared memory across mobile/game sandbox)
- Key expression = topic name

### Iceoryx2 — on-board (same machine, zero-copy)
- Default for intra-machine communication
- Zero-copy shared memory — no serialization overhead
- Best for images, point clouds, high-frequency data
- Service name = topic name

**Rule:** For same-device communication, use Iceoryx2. No reason to use UDP/TCP on-device.

---

## Topic Naming Convention

All topics follow:

```
<device>/<workspace>/<module>/<transport>/<topic>
```

Example: `dev01/perception/camera/iox2/image_raw` (or `.../zenoh/image_raw`)

- `device` — identifies the machine
- `workspace` — the active swarmbotix workspace
- `module` — the app/module name
- `transport` — literal `iox2` or `zenoh`, picked from the `--iox2` / `--zenoh` flag at `sb io add` time
- `topic` — the pub/sub channel name

The transport segment lets the same bare name (e.g. `image_raw`) coexist on both transports for the same module without collision, and makes the transport visible directly in the key. `sb` enforces this naming automatically. Users type the full path only to address a topic they do not own, with `--key`.

`sb 0.5.0+`: the canonical form has no leading `/` and is byte-identical to the wire key on both transports. Zenoh forbids a leading `/` in key expressions, and iceoryx2 service names use the same form. Every publisher / subscriber entry in `sb.module.yaml` / `sb.prd.yml` carries at most one of the following; `sb 0.6.0+` reads an entry with neither as `topic: <name>` (each character of `name` checked with `topic::is_valid_segment_char`), and `topic:` is always written back, so files written by 0.6 still parse on 0.5 when they use no other new key:

- `topic: <bare>`: owned; codegen prefixes it with `<device>/<workspace>/<module>/<transport>/`. May not contain `/`. Generated code has `is_owned = true`.
- `key: <wire key>`: verbatim, used as is. Generated code has `is_owned = false`.

A pre-0.5.0 `topic: /x/y` is read as `key: x/y` (the identical wire key); `sb doctor` warns about it (`topics` check) and the next `sb io` write (or `sb io update`) rewrites it. A pre-0.5.0 `topic: a/b` (inner `/`, no leading one) is refused at load; the equivalent entry is `key: <device>/<ws>/<module>/<transport>/a/b`.

---

## Configuration & State Layout

`sb` reads and writes state in three real files on disk. No environment variables, no implicit search paths.

### Global state — `~/.swarmbotix/`

```
~/.swarmbotix/
  sb.config.yml              ← global config (host-specific paths, defaults)
  active                 ← one-line file: name of active workspace
  messages/              ← message styles; one subdir per style
    ros2/                ← shipped style: std/ + the ROS2 mirror
      message_definitions/
        std/             ← bundled by the forge — Header, String, Twist, Image, ...
        std_msgs/ geometry_msgs/ sensor_msgs/ ...
      message_targets/   ← generated bindings (iox2/, proto/, fb/); disposable
    swarmbotix/          ← shipped style: the main one
      message_definitions/
        header/ primitives/ images/ sensors/
      message_targets/
    <yourstyle>/         ← styles are an open set
      message_definitions/
  workspaces/            ← all workspaces live here
    <workspace_name>/    ← one directory per installed workspace
      sb.config.yml      ← optional per-workspace override
      sb.workspace.yaml  ← installed copy of the repo sb.workspace.yaml: absolute paths + source: (sb 0.8.0+); never hand-edited
      flows/             ← installed copies of every repo *.flow.yaml (sb.flow.yaml = the default flow) (sb 0.8.0+)
      active_flow        ← one line: the flow the last `sb up` launched (sb 0.8.0+)
  repos/<name>/          ← default clone location for `sb install <git-url>` of a workspace (sb 0.5.0+)
  apps.yml               ← installed app packages: name → path, source, commit, version
  update-check.json      ← cache for the `sb --version` update notice: latest, checked_at
  apps/<name>/           ← clones made by `sb install <git-url>`; sb owns and deletes these
```

### Repository state: `<repo>/` (sb 0.5.0+)

```
ws_ips/                  ← a workspace repository, cloned anywhere
  sb.workspace.yaml      ← the inventory: modules + messages; committed (sb 0.8.0+)
  sb.flow.yaml           ← the default flow: nodes, instances, instance settings, wires; committed (sb 0.8.0+; through 0.7 it held the inventory too; `flow.yaml` before 0.6.0, not read since 0.7.0)
  calib.flow.yaml        ← any other flow, launched with `sb up calib` (sb 0.8.0+)
  messages/custom/       ← optional .proto sources, imported by `sb install` via `messages:`
  modules/<module>/      ← one module root per module (see below)
```

### Per-app state: `<module_root>/`

```
camera_app/
  sb.module.yaml         ← module manifest: interface, build, launch (sb 0.6.0+; `sb.dev.yml` before, not read since 0.7.0); no host path, committed
  sb.prd.yml             ← production interface (ships with binary)
  swarmbotix_io/         ← generated IO files (default io_dir)
```

`sb 0.7.0+` reads the module manifest as `sb.module.yaml` only and the workspace flow as `sb.flow.yaml` only (`sb_core::module_file_candidates` / `flow_file_candidates`, one name each). `sb 0.8.0+` reads the workspace as `sb.workspace.yaml` (`find_workspace_file`, `find_workspace_upwards`) and every `*.flow.yaml` beside it (`find_flow_files`, sorted, default first) (§Workspace). Where only the legacy name exists, a caller that needs the file fails with `module manifest sb.module.yaml not found at <dir>; sb.dev.yml is no longer read since 0.7.0, rename it (see sbcli_module.md §Migrating)` or `sb.flow.yaml not found at <dir>; flow.yaml is no longer read since 0.7.0, rename it (see sbcli_module.md §Migrating)` (§Deprecations). 0.6.0 read the legacy names as a fallback, with a deprecation warning.

### `sb.config.yml` schema

```yaml
# Host-specific tool paths (protoc / flatc: written by `sb tools install`, sb 0.9.0+)
protoc:      /opt/protobuf/protoc-35.0-linux-x86_64/bin/protoc
flatc:       /opt/flatbuffers/flatc-25.12.19/flatc
libzenohc:   /usr/local/lib/libzenohc.so
libiceoryx2: /usr/local/lib/libiceoryx2_ffi_c.so

# Message styles. Each style under messages_root owns its own
# message_definitions/ (sources) and message_targets/ (generated).
# There is no "active" style — a message is named in full,
# <style>/<namespace>/<Leaf>, wherever it appears.
messages_root: ~/.swarmbotix/messages     # default <sb_home>/messages

# Defaults
device:                  dev01
transport_on_device:     iceoryx2
transport_cross_device:  zenoh
```

**Layering (highest priority first):**
1. `$SB_CONFIG` — env var pointing at an explicit file (used by tests, CI, ad-hoc overrides)
2. `<workspace>/sb.config.yml` — per-workspace overrides
3. `~/.swarmbotix/sb.config.yml` — global defaults
4. Built-in defaults

`sb doctor` reports every tool path: an unset one is `[SKIP]` (only the commands that use the tool need it), a set one that does not exist on disk is `[FAIL]`. Nothing is silently swallowed (§Doctor).

### Forge → user install

This repository (the forge) bundles the canonical `std/` message set under `messages/ros2/message_definitions/`. On install (`cargo install sb-cli` or release tarball), `sb` copies the bundle into `~/.swarmbotix/messages/ros2/message_definitions/std/` if not already present. The forge is the source of truth; user machines hold a copy.

### Pinned third-party versions: `versions.json`

`versions.json` at the repository root is the single source of truth for every third-party version `sb` is built and tested against. It is flat `"name": "X.Y.Z"`:

```json
{ "zenoh": "1.9.0", "iceoryx2": "0.9.3", "protoc": "35.0", "flatc": "25.12.19" }
```

`zenoh` / `iceoryx2` are linked crates (exact `=` pins in `[workspace.dependencies]`). `protoc` / `flatc` (sb 0.9.0+) are not linked: they are the versions `sb tools install` downloads (`sb_update::tools::PROTOC_VERSION` / `FLATC_VERSION`, with the asset table and SHA-256 hashes in `crates/sb-update/src/tools.rs`) and the versions `sb doctor` compares the configured executables against (`sb-doctor`'s build script bakes them in as `SB_PROTOC_VERSION` / `SB_FLATC_VERSION`, `unknown` when absent). A unit test in `tools.rs` fails when the consts and `versions.json` disagree. Bumping a pin means editing `versions.json`, then the consts, URLs and hashes together (`/sb-deps`). The installed tools live outside `~/.swarmbotix/`, under the prefix (§Host Tools).

---

## Workspace

Workspaces isolate projects. Analogous to Python virtual environments. Each installed workspace is a real directory at `~/.swarmbotix/workspaces/<name>/`. Shipped docs: `documents/sbcli_ws.md` (commands), `documents/sbcli_workspace.md` (the inventory file), `documents/sbcli_flow.md` (flow files).

`sb 0.5.0+`: a workspace is defined by files at the root of a repository, committed with the code; `sb install` derives an **installed copy** under `~/.swarmbotix/workspaces/<name>/` (absolute paths plus `source:`), every other verb reads the installed copy, and nobody edits it by hand. `sb 0.8.0+` splits the repository files in two kinds: **`sb.workspace.yaml`**, the inventory (modules and messages), and **flow files**, `sb.flow.yaml` (the flow named `default`) and any `<name>.flow.yaml`, each a diagram of what runs (nodes), how many times (instances), with what per instance (instance settings), and wired to what (wires, publisher first). Through 0.7 one `sb.flow.yaml` held both; 0.8.0 reads that shape with a deprecation warning, 0.8.1 will not (§Deprecations).

```bash
sb install https://github.com/acme/ws_ips.git --dir ~/ws_ips   # clone, install (or: cd clone && sb install .)
sb ws create ws_ips              # write ./sb.workspace.yaml and ./sb.flow.yaml, then install them
sb ws install [<url|path>]       # workspace-only form of sb install (default .)
sb ws check [ws_ips]             # validate a workspace (default: the active one)
sb ws update ws_ips [--force]    # git pull --ff-only on the source, then reinstall
sb ws uninstall ws_ips [--clean] # remove the installed copy; --clean also deletes each module's io dir and the clone sb made
sb ws set    ws_ips              # write ws_ips to ~/.swarmbotix/active
sb ws list                       # installed workspaces, with source and a stale marker
sb ws delete ws_ips              # rm -rf ~/.swarmbotix/workspaces/ws_ips
sb up [FLOW]                     # launch the nodes of one flow (default: default); records the active flow
```

**`<repo>/sb.workspace.yaml`** (sb 0.8.0+, `sb_core::Workspace`, `WORKSPACE_FILE`):

```yaml
name: ws_ips
sb: ">=0.8"                         # optional
modules:
  ips_cam_gcam_pub: modules/ips_cam_gcam_pub
  ips_detect: modules/ips_detect
  ips_track: modules/ips_track
  ips_viz: modules/ips_viz
  ips_calib: modules/ips_calib
messages:                           # map keyed by a label
  custom:
    style: ros2
    path: messages/custom
    backends: [proto, iox2]
```

- **`name:`** *(optional)*: the workspace name; default the name of the directory holding the file. Must pass `validate_identifier`, so a folder such as `ws-ips` needs an explicit `name:`.
- **`sb:`** *(optional)*: free text, not enforced.
- **`modules:`**: module name → path (string only) relative to the file, naming the module directory (its manifest found by `module_file_candidates`) or the yaml itself; a path that does not exist yet is a directory unless it ends in `.yml` / `.yaml`. No map form: the 0.7 `{ path, up, instances }` entry is gone. A null value means empty.
- **`messages:`**: map label → `MessageSource { label, style, path, backends }`; an inner `label:` must equal the key (`messages.<label>: label <x> does not match the key`); the 0.7 list form is still read, the label defaulting to the path's last component with `_2`, `_3`, ... appended on a collision (`messages_from_map_or_list`, written back by `messages_to_map`). `style` is `[A-Za-z0-9_]+`; `backends` is any of `proto`, `fb`, `iox2`. On install, `Vault::import_style_dir` copies `<path>/<ns>/<Leaf>.proto` to `<style>/<ns>/<Leaf>` and a flat `<path>/<Leaf>.proto` to `<style>/<dirname of path>/<Leaf>`; hidden entries and non-`.proto` files are ignored; identical content is unchanged, different content is a conflict left alone unless `--force`, `ros2/std/` is never overwritten; any file that cannot be named `<style>/<ns>/<Leaf>` aborts that entry with nothing written. The style is then compiled with `backends`, or the normal backend selection when absent. A null value means empty.
- `deny_unknown_fields` on the file and on each `messages` entry. `Workspace::validate` errors: `workspace name "<n>" is invalid: <reason>`, `module "<m>" has an empty path`, and the `messages` entry errors (`style "<s>" is invalid (allowed: [A-Za-z0-9_])`, `path is empty`, `unknown backend "<b>" (expected one of: proto, fb, iox2)`).

**`<repo>/<name>.flow.yaml`** (sb 0.8.0+, `sb_core::Flow`; `DEFAULT_FLOW_FILE` `sb.flow.yaml` is the flow `DEFAULT_FLOW_NAME` `default`, and `default.flow.yaml` is not a flow file; `flow_name_of` / `flow_file_name` map file names and flow names; a `name:` that differs from the file's flow name makes `sb install` refuse the flow):

```yaml
name: calib                         # optional; defaults to the file stem, "default" for sb.flow.yaml
nodes: [ips_cam_gcam_pub, ips_detect, ips_track, ips_calib]
instances:                          # optional; a node absent here is a singleton
  ips_cam_gcam_pub: [sb00, sb01, sb02, sb03]
  ips_detect: [sb00, sb01, sb02, sb03]
instance_settings:                  # optional; node -> id -> settings
  ips_detect:
    sb02:
      label: East wall
      args: --debug --fps 30        # string (split like scripts) or [..]
      env:
        RUST_LOG: debug
wires:                              # publisher on the left, one subscriber or [subscribers]
  ips_cam_gcam_pub[{id}].image: ips_detect.image
  ips_detect[*].markers: [ips_track.markers, ips_calib.markers]
```

- **`nodes`**: the modules this flow runs. `sb up <flow>` builds and starts exactly these; `sb run` refuses a module that is not a node of the active flow (`module {m} is not a node of flow {f}`). This replaces 0.7's per-module `up:` flag.
- **`instances`**: node → ids (`validate_instance_id`, `[A-Za-z0-9_-]+`, unique per node). One tmux window `<module>-<id>` per instance, `--id <id>` appended to the start command, `SB_INSTANCE_ID` exported (§Launch). A node absent here is a single un-suffixed process. `Flow::instances_for`, `Flow::resolved_instances` (→ `ModuleInstance { id, label, args, env }`, no docker field).
- **`instance_settings`**: node → id → `InstanceSettings { label, args, env }`; `args` is a string split by `Script::parse_line` or a list (`argv_from_str_or_list`, written back as one string). `args` go to `{args}` of the start command, `env` over the manifest's `env`. `Flow::settings_for`.
- **`wires`**: `<module>[<sel>].<entry>: <module>.<entry>` or a list of them (`wires_from_map` / `wires_to_map`, `parse_wire(lhs, rhs) -> Wire { source: WireSource { module, selector, entry }, sinks: Vec<WireEnd> }`). Selector on the publisher side only: `{id}` (`WireSelector::SameId`, the publisher instance with the subscriber instance's id; the subscriber node must have instances), `*` (`All`, Zenoh only), a literal id (`Id`), or none (`None`). Not read by codegen yet.
- `Flow::validate` (no manifests needed), exact texts: nodes are identifiers and unique (`nodes: <n> is listed more than once`); every `instances` / `instance_settings` key is a node (`instances: unknown node <n>`, `instance_settings: unknown node <n>`) and every wire module, both sides, is a node (`wires.<lhs>: unknown node <n>`); ids valid and unique per node; every settings id is in that node's instances (`instance_settings.<node>: <id> is not an instance of <node>`); selector grammar (`wires.<lhs>: subscriber "<s>" takes no selector; selectors go on the publisher (left) side`, `wires.<lhs>: no subscriber on the right side`) and the `{id}` rule (`wires.<lhs>: {id} but <sink> has no instances`). Port existence, transport, and type checks need manifests and run in `sb ws check` (`ws flows`).
- Written shape (installed copy): `nodes` and multi-sink wires as block lists; `args` as one string unless a token holds whitespace or quotes.
- `deny_unknown_fields` on the file and each settings map.

**Installed copy** `~/.swarmbotix/workspaces/<name>/`: `sb.workspace.yaml` (`SbHome::workspace_file`) with `name:` always set, every path absolute (canonical when it exists), every module path naming its manifest file, and `source: { repo_workspace, url, installed_at }` (`WorkspaceSource`, renamed from 0.7's `FlowSource` with `repo_flow`), where `url` is absent for a local-path install and `installed_at` is RFC 3339 UTC to the second; `flows/<file>` (`SbHome::flows_dir`, `SbHome::flow_file`) holding every repository flow; `active_flow` (`active_flow` / `set_active_flow`), written by `sb up`. Written by every install (`sb_workspace::install_workspace`, which replaces `install_flow` and takes the repository `sb.workspace.yaml` or a legacy 0.7 `sb.flow.yaml`), never by hand; the repository files are never written by an install. An installed copy is refused as an install source. A workspace of the same name installed from a **different** repository that still exists is refused; one whose previous source no longer exists is replaced. `read_workspace`, `read_flow` (error `no flow named {flow} in workspace {ws} (flows: a, b)`, or `(flows: none)`), `list_flows`. A workspace installed by sb 0.7 or earlier (a single installed `sb.flow.yaml`) is refused: ``workspace "{ws}" was installed by an older sb (<path> holds its modules and wiring in one file); install it again from its repository with `sb install <repo folder>` ``.

**Legacy 0.7 shape** (read in 0.8.0 only): `probe_legacy_flow_shape(yaml)` detects a `sb.flow.yaml` with no `nodes:` and any of `modules:`, `messages:`, or `source:`; `convert_legacy_flow(yaml) -> (Workspace, Flow)` puts modules and messages into the workspace, makes nodes of the modules with `up != false`, moves inline instance ids to `instances` and per-instance `args` / `env` / `label` to `instance_settings`, drops the instance docker blocks, flips wires to publisher first, and drops wire ends on modules whose `up` was false. A file that does not parse: `sb.flow.yaml in the 0.7 shape does not parse: <error>`. A `Deprecation` is pushed (`item: "sb.flow.yaml in the 0.7 shape"`, `hint: "split it into sb.workspace.yaml and sb.flow.yaml; see sbcli_workspace.md §Migrating"`). Removed in 0.8.1.

Without `wires:`, edges between modules are **derived** by matching publisher keys to subscriber keys across the manifests. `wires:` declares them explicitly; they are validated only.

---

## Per-App Config Files

### `sb.module.yaml` — development time
Generated by `sb init` at the app's project root (`sb 0.6.0+`; `sb.dev.yml` before, read with a deprecation warning in 0.6.0, not read since 0.7.0). **Not shipped with binary.** Holds no host-specific path and is committed with the module; `sb install` and `sb io update` regenerate `swarmbotix_io/` from it, and `sb up` / `sb run` / `sb stop` build, start, and stop the module from it. Shipped doc: `documents/sbcli_module.md`.

```yaml
module: ips_detect
language: rust
version: 0.3.0                      # optional
sb: ">=0.8"                         # optional
description: ...                    # optional
scripts:                            # optional, language defaults
  build: cargo build --release
  start: "{bin} {args}"
  test: cargo test
  stop: [echo stopping, rm -f /tmp/x.lock]
env:
  RUST_LOG: info
requires: [cargo]
platforms:
  linux:
    env:
      LIBCLANG_PATH: ${LIBCLANG_PATH:-/usr/lib/llvm-18/lib}
    requires: [cmake]
  windows:
    env:
      LIBCLANG_PATH: ${LIBCLANG_PATH:-C:/Program Files/LLVM/bin}
io_dir: swarmbotix_io
publishers:                         # map keyed by port name
  markers:
    type: ros2/custom/MarkerDetections
    transport: zenoh
  status:
    topic: health                   # optional, defaults to the key
    type: ros2/std/StringStamped
    transport: zenoh
    encoding: raw
    mime: text/plain
subscribers:
  image:
    type: ros2/sensor_msgs/Image1280x1024
    transport: iceoryx2
  commands:
    key: dev01/ops/console/zenoh/cmd
    type: ros2/std/StringStamped
    transport: zenoh
services: [cam_param]               # list of names; a map keyed by name is also read
```

- Ports *(sb 0.8.0+)*: `publishers` / `subscribers` are maps keyed by port name (`ModuleDevConfig.publishers: Vec<PubSpec>` with `deserialize_with = "ports_from_map_or_list"`, `serialize_with = "ports_to_map"`; `RawSpec.name` is `Option`, the key fills it). The 0.7 dash list (one `name:` per item) is still read, silently. In the map form an inner `name:` is allowed only if equal to the key (`publishers.<key>: name <x> does not match the key`, likewise `subscribers.` / `services.`). Duplicate keys: `<path>: duplicate entry with key "<k>"`; duplicate names across the list form stay the existing validation. Error paths read `publishers.markers: ...` in the map form and `publishers[0]: ...` in the list form. `services` (`Vec<ServiceSpec>`, `services_from_any` / `services_to_list`) accepts `[a, b]`, a map keyed by name (only `name:` may sit under a key; a service has no options), or the 0.7 list of `{name}` maps. Every write (`sb io`) uses the map form for ports, still writing `topic:` even when it equals the key; `services` is written as a block list (serde_yaml emits no inline brackets).

- `root:` *(optional, sb 0.5.0+)*: absent means the directory holding the manifest; relative is resolved against that directory; absolute is used as is (pre-0.5.0 files). `sb init` no longer writes it. Always resolved through `ModuleDevConfig::module_root`.
- `scripts:` *(sb 0.6.0+)*: `build`, `start`, `stop`, `test`, each a `Script` (string = one command, list = one command per string). A command is argv, never shell: split on whitespace, `"..."` / `'...'` group (also inside a token), quotes removed, no backslash escapes, no `&&` / `export` / redirection; `ScriptError::UnterminatedQuote`, `ScriptError::Empty`. `start` must resolve to exactly one command (`ManifestError::StartNotSingle`, top level or any overlay).
- `env:`, `requires:`, `platforms:` *(sb 0.6.0+)*: `platforms` keys are `<os>` or `<os>-<arch>` with os in `linux|windows|macos`, arch in `x86_64|aarch64` (`ManifestError::BadPlatformKey`); each value is an `Overlay { scripts, env, requires }`. Absent block: every host supported; present: the host must match `<os>` or `<os>-<arch>` (`Platforms::supports`), else `sb up` skips the module with a line, `sb run` refuses (`ManifestError::Unsupported`), `sb ws check` warns (`ws platforms`). Machine-specific paths go in as `${VAR:-default}`, never as literals; there is no local override file.
- Resolution (`ModuleDevConfig::resolve(host) -> Resolved`), lowest priority first: language defaults, top level, `platforms.<os>`, `platforms.<os>-<arch>`, then (at launch) instance `env` / `args` from the flow's `instance_settings`. `scripts` per key, `env` per key (later wins), `requires` appended then de-duplicated keeping the first, `bin` from the language defaults only. Language defaults (`language_defaults`), raw, `{exe}` = `.exe` on Windows:

| language | build | start | bin | test |
|---|---|---|---|---|
| rust | `cargo build --release` | `{bin} {args}` | `target/release/{module}{exe}` | `cargo test` |
| cpp | `cmake -S . -B build -DCMAKE_BUILD_TYPE=Release` then `cmake --build build --config Release` | `{bin} {args}` | linux/macos `build/{module}`; windows `build/Release/{module}.exe` | none |
| python | none | `{python} main.py {args}` | empty | none |
| flutter | none | `flutter run -d {os}` | empty | none |
| unity | none | none (resolve returns `NoStart`) | empty | none |

- Expansion (`sb_core::expand`), one left-to-right pass over each argv token, env value, and `bin`: `{module}` `{root}` (absolute, forward slashes) `{bin}` `{exe}` `{id}` `{args}` `{ws}` `{os}` `{python}` (`.venv/bin/python`, `.venv/Scripts/python.exe` on Windows, if present under the root; else `python3`, `python` on Windows); `${VAR}` (sb's environment, empty when unset), `${VAR:-default}` (default may hold placeholders); `{{` / `}}` literal braces. `{args}` must be a whole token and splices zero or more tokens. Errors (`ExpandError`): `unknown placeholder {x}`, `{id} used but this module has no instances`, `{args} must be a whole token`, `unterminated placeholder in "<s>"`.
- `test` is declared only; no `sb` verb runs it in 0.6.0.
- `services:` *(sb 0.5.0+)*: one entry per name (list of names since 0.8.0); each renders `<io_dir>/services/<name>.<ext>` (Rust and Python).
- Per publisher / subscriber `encoding: proto | fb | raw` *(sb 0.5.0+)*, default `proto` (omitted on disk), zenoh only. `raw` has no `type:` and an optional `mime:` (e.g. `text/json`). Load errors, exact, prefixed `publishers.<name>: ` / `subscribers.<name>: ` (map form, sb 0.8.0+) or `publishers[<i>]: ` / `subscribers[<i>]: ` (list form): `encoding: raw takes no type:`, `type: is required unless encoding: raw`, `mime: is only valid with encoding: raw`, `encoding: is only valid for transport: zenoh` (an explicit `encoding: proto` on iceoryx2 is accepted). The topic rules are checked first, then transport, then type, then mime.
- Names are unique across publishers, subscribers, and services within a module (`ModuleDevConfig::find_name`); checked when an entry is added, not at load.

### `sb.prd.yml` — production / runtime
Generated by `sb gopro`. **Ships with the binary.** Language-agnostic static interface definition.

```yaml
module: camera
publishers:
  - name: image_raw
    topic: image_raw
    type: ImageMsg
    transport: iceoryx2
subscribers:
  - name: cmd_vel
    key: dev01/nav/teleop/zenoh/cmd_vel
    type: TwistMsg
    transport: zenoh
```

`ModulePrdConfig` is not part of the 0.8.0 map-form change: `sb gopro` still writes `sb.prd.yml` with list-form ports, as above.

---

## Module IO Folder Structure

Each app has a `swarmbotix_io/` IO folder (default name; user can override via the manifest's `io_dir`). Contains generated pub/sub files using raw transport APIs.

```
camera_app/
  swarmbotix_io/            ← io_dir (default: swarmbotix_io/; user-overridable)
    publishers/
      image_raw.py          ← generated, uses raw zenoh/iceoryx2 API
    subscribers/
      cmd_vel.py            ← generated
    services/
      calib.py              ← generated router (sb 0.5.0+); user-edited, regenerated only on request
  sb.module.yaml            ← module manifest (sb 0.6.0+); build and launch come from it
  sb.prd.yml                ← after sb gopro
  main.py                   ← user's algorithm, free structure
```

The `swarmbotix_io/` folder is the **only structural imposition** on the app. Everything else is free.

---

## CLI Commands

Conventions used in every command spec below:
- `<positional>` — required positional argument
- `[<bracketed>]` — optional positional or flag
- `-x | --xxx` — short and long form of the same flag
- Defaults are shown explicitly under each option

### Workspaces

#### `sb install <git-url | path> [--dir <path>] [--force]` (workspace, `sb 0.5.0+`)
`sb install` reads the manifest at the source's root (`package_kind`): `sb.app.yml` installs an app package (§App Packages), `sb.workspace.yaml` installs a workspace with every `*.flow.yaml` beside it (`sb 0.8.0+`; in 0.8.0 a root with only a 0.7-shape `sb.flow.yaml` installs too, converted, with the deprecation warning; `sb 0.7.0+`: a root with only `flow.yaml` fails with `sb.flow.yaml not found at <dir>; flow.yaml is no longer read since 0.7.0, rename it (see sbcli_module.md §Migrating)`). Both: `<source> has both sb.app.yml and sb.workspace.yaml; a package is one or the other`; a new-shape `sb.flow.yaml` with no `sb.workspace.yaml`: `<source> has sb.flow.yaml but no sb.workspace.yaml; a workspace package lists its modules in sb.workspace.yaml and keeps sb.flow.yaml for the flow (see sbcli_workspace.md)`; neither: `<source> has neither sb.app.yml (an app package) nor sb.workspace.yaml (a workspace package) at its root`. A 0.7-shape flow is read only when no `sb.workspace.yaml` sits beside it; next to one it is refused: `<path> is a flow in the 0.7 shape (top-level modules:), which is read only when no sb.workspace.yaml sits beside it; move modules: and messages: into sb.workspace.yaml and keep nodes, instances and wires in the flow (see sbcli_workspace.md §Migrating)`. For a workspace:
- `<git-url>` (`@<ref>` pins): shallow clone into `--dir <path>`, else staged under `~/.swarmbotix/repos/` and moved to `~/.swarmbotix/repos/<name>/`, `<name>` being the flow's `name:` or else the URL's repository folder name. The same URL again (`source.url` match) fast-forwards that clone and reinstalls. `--dir` must be new, empty, or already a clone of the same URL (fast-forwarded); it is refused for a local folder and for an app. `<path>`: used in place, recording no URL; `sb install .` inside a clone is the common author form.
- `--force`: overwrite vault messages that differ from the package's copies; the bundled `ros2/std` set is never overwritten.
- Steps (`finish_workspace_install`), one stdout line each: `-- workspace <n>: created|updated, active`; `-- modules: <k> registered` (a module without a manifest warns on stderr); `sb 0.8.0+`: `-- flows: default, calib` (every installed flow, default first; `-- flows: none` when there is none, which is allowed); per `messages:` entry `-- messages: <style> <- <path>: <i> imported, <u> unchanged, <c> conflicts; compiled [<backends>]` (`; not compiled` on a conflict), imported with `Vault::import_style_dir` into the configured vault (never a cwd-discovered one) and compiled with the entry's backends (or the style default); per module `-- io: <m> <w> written, <u> unchanged[, <s> skipped]` (`sb_pubsub::update_all` with `ServiceRender::IfMissing`: a service router is rendered only where its file does not exist, printing `  rendered services/<name> for <module>`; skipped entries warn; a module with no manifest yet prints `-- io: <m> skipped (no module manifest)`); then `done. next: sb up` (`sb init <module dir>` when no modules). Every registered module's io is regenerated, node of a flow or not. A legacy flow name or a legacy manifest name fails the install with the §Deprecations errors; `sb 0.8.0` warns (stderr) on a 0.7-shape `sb.flow.yaml`.
- Conflicts fail the command after every step has run: `<n> message(s) differ from the vault copies and were not imported (their styles were not compiled); re-run with --force to overwrite the vault copies`.
- Idempotent; re-run after `git pull`.

#### `sb ws create <name>`
`sb 0.8.0+`: write `./sb.workspace.yaml` (`name: <name>`, `modules:` empty, no messages) and `./sb.flow.yaml` (`nodes: []`), print `wrote sb.workspace.yaml and sb.flow.yaml`, and install them, which creates `~/.swarmbotix/workspaces/<name>/` and makes it active; a failed install removes the files again. Refuses an existing workspace file in the folder and an already installed `<name>`. (0.5.0 to 0.7 wrote one `./sb.flow.yaml`.) Before 0.5.0 it created an empty installed copy only and did not change the active workspace.
- `<name>` (required) — workspace identifier, no spaces or slashes.

#### `sb ws install [<git-url | path>] [--dir <path>] [--force]` (`sb 0.5.0+`)
The workspace-only form of `sb install` (default `.`); an app package is refused with ``<source> is an app package (sb.app.yml), not a workspace; install it with `sb install <source>` ``.

#### `sb ws check [<name>]` (`sb 0.5.0+`)
Validate an installed workspace (default: the active one) with `sb_doctor::check_workspace`, rows in order: `workspace` (summary: module count and source; `[FAIL]` if the installed copy cannot be read), `ws modules` (`[FAIL]` per missing or unparseable module manifest; a module dir with only `sb.dev.yml` fails with `module manifest sb.module.yaml not found at <dir>; sb.dev.yml is no longer read since 0.7.0, rename it (see sbcli_module.md §Migrating)`), `ws types` (`[FAIL]` per proto/fb `type:` not in the configured vault nor an `iox2_variants` name), `ws encoding` (`[FAIL]` per wire key whose entries disagree on `encoding:`), `ws io files` (`[WARN]` per module with generated files missing), then `sb 0.6.0+`: `ws requires` (`[FAIL]`, `<module>: missing <bin>[, <bin>]` per module joined by `; `, resolved with `which::which` after `ModuleDevConfig::resolve(host)`, skipping modules the host does not support; OK `all required tools present`), `ws platforms` (`[WARN]`, `<module>: not supported on <os>-<arch> (declares: linux, windows)`; OK `all modules support this host`), `sb 0.8.0+` `ws flows` (replaces 0.6/0.7's `ws instances` and `ws wires`; one line per flow, default first; OK `<flow>: <n> node(s), <i> instance(s), <w> wire(s) resolve`; `[WARN]` ``no flows installed; `sb up` needs one (add sb.flow.yaml beside sb.workspace.yaml)``; one `[FAIL]` row per problem, `<flow>: <wire or item>: <reason>` (an unknown node has the item `nodes`), the item being the publisher key for a problem at the publisher end and `<key> -> <module>.<entry>` for one at a subscriber, reason one of `unknown node <n>`, `no publisher <entry> in <module>`, `no subscriber <entry> in <module>`, `[*] on iceoryx2 is not supported`, `{id} but <module> has no instances`, `no instance <id> in <module>`, `transport mismatch (<a> vs <b>)`, `type mismatch (<a> vs <b>)`), `deprecated` (`[WARN]`, one leftover legacy file per line: `sb.dev.yml at <path>`, `flow.yaml at <path>`, `runscript.bash at <path>`, `stopscript.bash at <path>`, each ending `is ignored since 0.7.0 (see sbcli_module.md §Migrating)`, and in 0.8.0 `sb.flow.yaml at <path> is in the 0.7 shape, which 0.8.1 no longer reads (split it into sb.workspace.yaml and sb.flow.yaml; see sbcli_workspace.md §Migrating)`; OK `nothing deprecated in use`), and last `ws source` (`[WARN]` when the repository files are newer than the installed copy, `[SKIP]` without `source:`). `WorkspaceView { name, workspace, flows, vault_root, stale_repo, host }`. Exit 1 on any `[FAIL]`. The same rows, for the active workspace, are part of `sb doctor`.

#### `sb ws update <name> [--force]` (`sb 0.5.0+`)
`git -C <repo dir> pull --ff-only` on the directory holding the installed copy's `source.repo_workspace` (`repo_flow` before 0.8.0), then reinstall (`sb_workspace::update_from_source`, then the remaining install steps). Requires `source.url`: a local-path install errors with `workspace "<n>" was installed from a local path (<flow>) with no URL; update that checkout yourself and install it again`; no `source:` errors with `workspace "<n>" was not installed from a repository flow`.

#### `sb ws uninstall <name> [--clean]` (`sb 0.5.0+`)
Remove `~/.swarmbotix/workspaces/<name>/` and clear the active marker if it named it (`uninstall_workspace`). `--clean` first deletes each module's io dir (`io_dir:` of its manifest); an absolute or escaping `io_dir`, or a missing manifest, is skipped and reported (`left <module>: <why>`). A clone `sb` made under `~/.swarmbotix/repos/` (URL recorded, folder under `repos/`) is deleted with `--clean` and otherwise reported as `left clone <path> (pass --clean to delete it)`; `--dir` clones and local folders are never deleted. Imported vault messages are left alone.

#### `sb ws set <name>`
Mark a workspace active by writing its name to `~/.swarmbotix/active`.
- `<name>` (required) — must already exist.

#### `sb ws list`
List all workspaces. The active one is prefixed with `*`. No arguments. `sb 0.5.0+`: each row is `<*| > <name>  <source>[  [stale]]`, the source being the URL or else the repository folder; `[stale]` when the repository files are newer than the installed copy (`is_stale`, by modification time), with the footer ``[stale]: a repository file changed after install; run `sb install <source>` ``. Empty: ``(no workspaces; run `sb ws create <name>` or `sb install <repo>`)``.

#### `sb ws delete <name>`
Remove `~/.swarmbotix/workspaces/<name>/`. If active, clears the `active` marker with a warning. Equivalent to `sb ws uninstall <name>` without `--clean`.
- `<name>` (required)

---

### Module init

#### `sb init [<rootpath>] [OPTIONS]`
Adopt an existing project directory as a swarmbotix module. sb does **not** create the project itself — the user already has a Rust crate, a Python project, a C++ tree, a Flutter project, or a Unity project. `sb init` just drops swarmbotix's manifest in and registers the module with the active workspace.

What sb writes into `<rootpath>` (only the missing pieces; existing files are preserved unless `--force`):
- `sb.module.yaml` (sb 0.6.0+): `module: <basename of rootpath>`, `language: <lang>`, `io_dir: swarmbotix_io/` (or `Assets/Scripts/swarmbotix_io/` for Unity). No `scripts:`: the language defaults launch the module. `sb 0.7.0+`: a directory with only `sb.dev.yml` fails before anything is written with `module manifest sb.module.yaml not found at <dir>; sb.dev.yml is no longer read since 0.7.0, rename it (see sbcli_module.md §Migrating)` (0.6.0 adopted it read-only).
- `swarmbotix_io/` (or `Assets/Scripts/swarmbotix_io/` for Unity) — empty IO directory; L3 codegen fills it.
- No launch script (sb 0.6.0+). The 0.6.0 `--runscript` flag and its stub template (`crates/sb-workspace/src/runscript.rs`) are removed in 0.7.0.

stdout: `adopted module <m> (language: <l>, root: <abs>)`, then `  wrote sb.module.yaml`, `  created IO directory`, `  registered in <path to sb.workspace.yaml>` or `  (already registered in <path>)` (without a repository workspace file: `  registered in workspace <ws>` / `  (already registered in workspace <ws>)`), and under a repository `  installed workspace <ws> (active)`.

Then it registers the module. `sb 0.8.0+`: when a repo `sb.workspace.yaml` sits at or above `<rootpath>` (`find_workspace_upwards`; never an installed copy, never `$HOME` itself unless the search starts there), the module is added there as `<module>: <relative/path>` (`register_in_workspace`), appended as one line under `modules:` so comments survive (falling back to a full rewrite when `modules:` is a flow mapping), and the repository is reinstalled, keeping a previously recorded URL. A same-named entry pointing at another existing module is an error (`module "<m>" is already registered in <file> for <path>; module names must be unique within a workspace`); one pointing at a missing module is repointed. Otherwise it inserts `module → /abs/path/<rootpath>/sb.module.yaml` into the active workspace's installed `sb.workspace.yaml`, as before. No flow file is touched: the module runs once a flow lists it in `nodes:`. Idempotent: re-running with the same rootpath does not duplicate the registration. The manifest is written without `root:` (sb 0.5.0+).

Arguments:
- `<rootpath>` (default: `.`) — project root to adopt. For Unity, this is the project root that contains `Assets/`. Module name is `basename(rootpath)`.
- `--rust | --python | --cpp | --flutter | --unity` — language tag written to `sb.module.yaml`. **Required** when no manifest exists yet at `<rootpath>`. **Optional** when one already exists; if given, must match the existing `language:` tag (else error).
- A repo `sb.workspace.yaml` above `<rootpath>` or an active workspace (required): with neither, `sb init` errors (``no active workspace and no sb.workspace.yaml above <root>; run `sb ws create <name>` at the repository root first, or `sb ws set <name>` to pick an installed workspace``). A 0.7-shape `sb.flow.yaml` found instead stops it: ``<path> is a flow in the 0.7 shape; sb init registers modules in sb.workspace.yaml, so split it into sb.workspace.yaml and sb.flow.yaml first (see sbcli_workspace.md §Migrating)``.
- `--force` (default: refuse to overwrite) — overwrite an existing `sb.module.yaml` at `<rootpath>`; the rewritten manifest keeps only `module`, `language`, `io_dir`.
- `--docker`: removed in 0.8.0 (`InitOptions.docker` deleted; clap rejects the flag). A module that runs in a container calls `docker compose` from its own `scripts`, and the compose file reads `${SB_INSTANCE_ID}` (shipped doc: `documents/sbcli_module.md` §14).

---

### Pub/Sub

`sb 0.7.0+`: `sb io` (§Module IO) is the only pub/sub and service surface; `sb pub`, `sb sub`, `sb list`, and `sb service` were removed (§Deprecations). This section holds the rules every `sb io` verb shares.

`-m` is reserved for `MsgType`. Module selection uses the long-only flag `--module`.

#### Module resolution (highest priority first)
Used by every `sb io` verb (`sb_pubsub::resolve_module`; `sb io update` takes the module as a positional):
1. `--module <name>` and `./<name>/sb.module.yaml` exists → that path
2. `--module <name>` and the active workspace's installed `sb.workspace.yaml` has it → that path
3. No `--module` → `./sb.module.yaml` in cwd
4. Otherwise → error with the paths searched

`sb 0.7.0+` probes the one name only; a directory with only `sb.dev.yml` fails with `module manifest sb.module.yaml not found at <dir>; sb.dev.yml is no longer read since 0.7.0, rename it (see sbcli_module.md §Migrating)`.

The module root is then `ModuleDevConfig::module_root` of that file (`root:` absent → the file's directory).

After every mutation: updates `sb.module.yaml` **and** regenerates the language file under `<io_dir>/publishers/<name>.<ext>` (or `subscribers/`, `services/`). `sb 0.5.0+`: a write also rewrites every legacy `topic: /...` entry in the file as `key:` and prints `  rewrote N legacy topic entry|entries as key: in <path>`.

#### Conflict policy
An add that collides with existing entries on name (any kind) or topic (publishers and subscribers) resolves a `ConflictPolicy`: `--force` removes every collider and its generated file, then adds (printing `  overwrote <role> <name>` per collider); `--skip-existing` writes nothing and exits 0 (`<role> skipped on module <m> (already exists)`); neither flag on a TTY prompts `[o]verwrite, [s]kip, [c]ancel?`; neither flag off a TTY is an error listing the colliders with their reason (`same name`, `same topic`, `same name + topic`) and the two flags (`sb_pubsub::apply_conflict_policy`).

`sb 0.5.0+` load errors for an entry (prefixed `publishers[<i>]: ` or `subscribers[<i>]: `): `exactly one of topic: or key: is required (both given)`, `topic "a/b" may not contain '/'; use key: for a verbatim wire key` (for a legacy `topic: a/b`, followed by a parenthetical naming `key: <device>/<ws>/<module>/<transport>/a/b`), `key: is empty`, and the encoding rules: `encoding: raw takes no type:`, `type: is required unless encoding: raw`, `mime: is only valid with encoding: raw`, `encoding: is only valid for transport: zenoh`. `sb 0.6.0+`: an entry with neither `topic:` nor `key:` reads as `topic: <name>`.

---

### Module IO (`sb io`, `sb 0.5.0+`)

The only pub/sub and service surface since 0.7.0 (it replaced `sb pub`, `sb sub`, `sb list`, and `sb service init`, removed in 0.7.0). Module resolution and conflict policy as in §Pub/Sub; publishers, subscribers, and services share one name space per module. Shipped doc: `documents/sbcli_pubsub.md` §14.

#### `sb io add pub (<topic> | --key <WIRE_KEY>) -m <Type> [--zenoh | --iox2] [--proto | --fb] [-n <name>] [--force | --skip-existing] [--module <name>]`
#### `sb io add pub (<topic> | --key <WIRE_KEY>) --raw [<mime>] [--zenoh] ...`
#### `sb io add sub ...` (same forms)
Add a publisher / subscriber and render `<io_dir>/{publishers,subscribers}/<name>.<ext>`.
- `<topic>` bare (no `/`), stored as `topic:`; `--key` verbatim, stored as `key:`.
- `-m <Type>` required unless `--raw`, refused with `--raw`.
- Transport: `--zenoh` / `--iox2`; without either, `--fb` / `--raw` imply zenoh and a proto entry takes `transport_on_device` (iceoryx2 by default).
- `--proto` (default, not written to disk) / `--fb` / `--raw [<mime>]` set `encoding:` (and `mime:`); `--raw [<mime>]` goes after the topic. `--raw --iox2`: `iceoryx2 payloads are typed; use a message with a bytes field and --iox2`; `--fb --iox2`: refused by `sb_pubsub`. `--fb` requires `<message_targets>/fb/<ns>/<Leaf>/` to exist: `flatbuffers backend not compiled for style <s>; run sb message compile <type> --fb`. `<mime>` must be `type/subtype` (`;` parameters allowed, no whitespace); `application/protobuf` and `application/flatbuffers` are refused for raw.
- `-n, --name` default: the bare topic or the key's last segment; refused when already used by any publisher, subscriber, or service on the module: `name "<n>" is already used by <kind> "<n>" on module "<m>"; publishers, subscribers, and services share one name space`.
- `--force` / `--skip-existing` / TTY prompt: the §Pub/Sub conflict policy (same name, or same topic among publishers and subscribers).
- stdout: `added publisher|subscriber on module <m>`, then `  wrote <file>` and `  updated <manifest>`.

#### `sb io add service <name>`
Append `<name>` to the `services:` list and render `<io_dir>/services/<name>.<ext>` (Rust, Python). Base key `<device>/<workspace>/<module>/zenoh/service/<name>`; a service named `service` uses the bare `.../zenoh/service` base (the default service).

#### `sb io edit <name> [-m <Type>] [-t <topic> | --key <WIRE_KEY>] [--proto | --fb | --raw [<mime>]] [--module <name>]`
Edit a publisher or subscriber by name; with no flag, only regenerate its file. Switching to raw drops `type:`; switching away from raw drops `mime:` and needs `-m`. No transport change (rm + add). Re-renders the file, writes `sb.module.yaml`. stdout: `edited publisher on module <m>` / `regenerated subscriber on module <m>` (a change), `regenerated on module <m>` (no flags; the manifest is written only to rewrite legacy `topic: /...` entries), then the file lines. A service: `services have no editable fields; edit the file in <io_dir>/services`. Unknown name: `no publisher, subscriber, or service named "<n>" on module "<m>"`.

#### `sb io rm <name> [--module <name>]` / `sb io list [--module <name>]` (alias `ls`)
`rm`: remove the entry of any kind and delete its generated file. `list`: a table with columns `role`, `name`, `type`, `topic`, `transport`, `encoding` over publishers, subscribers, and services (`(no publishers or subscribers)` when empty); read-only.

#### `sb io update [<module> | --all] [--services]`
Regenerate `swarmbotix_io/` from `sb.module.yaml` without changing it (`sb_pubsub::update_all`). Targets `<module>` (resolved as `--module`), else the cwd module when `./sb.module.yaml` exists, else (or with `--all`) every module of the active workspace's installed `sb.workspace.yaml`, node of a flow or not; an active workspace is required in every form. One line per module, `<module>: <w> written, <u> unchanged, <s> skipped[ (<entry>: <why>; ...)]`. A file whose rendered bytes match is `unchanged` and untouched; an entry that cannot render is `skipped` with its reason and the rest continue; any skip or module failure exits 1 (`<n> entr(y/ies) or module(s) could not be updated`). Services only with `--services` (their files are user-edited). The only `sb.module.yaml` write is rewriting legacy `topic: /...` entries as `key:`.

**Generated Zenoh wrappers** (`sb-codegen`): wire tag (`wire_tag`) `application/protobuf;<type>` for proto, `application/flatbuffers;<type>` for fb, the `mime:` (else `application/octet-stream`) for raw, exported as `WIRE_TAG`. Proto wrappers get a typed `publish(msg)` plus `publish_bytes(...)` and a typed receive (`try_recv_latest_msg`) in Rust, Python, and C++, in addition to the bytes API; fb and raw are bytes-only; Dart and C# are bytes-only and set no tag.

---

### Services (request/response)

REST/FastAPI-style request-response over Zenoh queryables, JSON payloads only.
Independent of pub/sub. Full reference: [documents/sbcli_reqres.md](documents/sbcli_reqres.md).

`sb 0.5.0+`: a service is a `services:` entry in `sb.module.yaml` (`sb io add service <name>`), rendered to `<io_dir>/services/<name>.<ext>` and served under `<device>/<workspace>/<module>/zenoh/service/<name>/<route>`; a service named `service` is the default service, served under `<device>/<workspace>/<module>/zenoh/service/<route>`. Since 0.7.0 that is the only way to add one. The user defines routes against the rendered router (`@app.get("/path")` / `app.get("/path", handler)`); clients call a route with a plain Zenoh `get` (no codegen).

`sb service init` (deprecated 0.5.0, removed 0.7.0) wrote the default router to `<io_dir>/service.<ext>` with no `services:` entry. A file it wrote keeps working as user code; nothing regenerates it.

---

### Message Vault

Vault location: `<messages_root>/<style>/message_definitions/`, where the style comes from the **message name** (`ros2/std/Header`). `messages_root` defaults to `~/.swarmbotix/messages`.

**Vault resolution order.** Every `sb message *` command resolves its vault before anything else. Highest priority first:

1. **`$SB_CONFIG` set** → skip discovery entirely; use that file's `messages_root`. This is the escape hatch for CI and for pointing `sb` at a tree it would not find on its own.
2. **Discovery by walking up from cwd.** The first level matching one of these shapes wins:
   - `./message_definitions/` — **cwd is itself a style.** The vault roots at cwd's *parent*, and the style name is cwd's folder name. A bare `sb message compile` then compiles **only that style**, writing to `./message_targets/`. This is what lets a standalone message repo — cloned to any path, registered with nothing — build in place.
   - `./messages/<style>/message_definitions/` — the conventional layout the forge itself uses. Roots at `./messages`; no style is pinned, so all styles compile.
   - `./<style>/message_definitions/` — cwd holds styles directly.
3. **`messages_root` from config** → `~/.swarmbotix/messages`.

Matching is on the `<style>/message_definitions/` **shape**, never on the folder name, so an unrelated directory called `messages` is not mistaken for a vault.

Two consequences of a *discovered* vault:

- Generated output defaults to `<discovered_root>/<style>/message_targets/` — the tree that was actually compiled, never the global one.
- The forge's embedded `std/` bundle is **not** installed into it. Auto-install applies only to the configured global vault; a discovered root is the user's own repo and is never written to beyond `message_targets/` and `message_definitions/.cache/`.

#### `sb message list`
List all messages in the vault, grouped by namespace. No arguments.

#### `sb message new <Name>`
Write a `.proto` skeleton under the user namespace.
- `<Name>` (required) — must be `Namespace/PascalCase`, e.g. `custom/Foo`.

#### `sb message rm <Name>`
Delete a message. Refuses if `<Name>` is under `std/`.
- `<Name>` (required)

#### `sb message compile [OPTIONS] [<Name>]`
Always writes the `FileDescriptorSet` IR to `<defs>/.cache/descriptor.bin`, then emits the backends selected by the order in §Backend selection order. `iox2` is an in-tree emitter (no external tool); `proto` and `fb` shell out to `protoc` / `flatc` from `sb.config.yml`.
- `<Name>` (default: every message) — compile a single message, fully qualified (`ros2/std/Header`). The style comes from the name.
- `--style <name>` — compile only that style. A **filter**, not a mode: it persists nothing. **Mutually exclusive with `<Name>`**, which already carries its style. An unknown style errors and lists the real ones.
- `--iox2` / `--proto` / `--fb`: emit these backends explicitly. An explicit flag naming a missing tool errors rather than being silently dropped. `sb 0.5.0+`: the `fb` backend also writes `<Leaf>.bfbs` (`flatc -b --schema`) next to the bindings, for `sb topic listen`.
- `-o, --out <dir>` — relocate the generated `iox2/` / `proto/` / `fb/` trees. Defaults to `<messages_root>/<style>/message_targets/`, the sibling of `message_definitions/`. `.cache/descriptor.bin` and the `.proto` sources never move.
- `--string-cap` / `--bytes-cap` / `--vec-cap` / `--string-array-cap <N>` — iox2-only capacity overrides for this invocation; they beat every config-derived default.

---

### Topic Introspection

Snapshot-based. No rolling sampler. Output layout mirrors the `swarmbotix_old/sb-introspect` prototype.

#### `sb topic list [OPTIONS]`
Enumerate currently-active topics on either transport.
- `-t, --timeout <sec>` (default: `0.5`) — Zenoh discovery window. iceoryx2 uses `Service::list` and ignores the window.
- `-k, --keyword <str>` (default: none): substring filter on the topic as printed. A leading `/` in the keyword is ignored (`sb 0.5.0+`), so a keyword copied from a slashed path still matches.
- `--case-sensitive` (default: insensitive) — toggle keyword case.
- `--transport zenoh | iceoryx2` (default: both): restrict discovery. Aliases `z` (zenoh), `i` and `iox2` (iceoryx2) are `sb 0.4.1+`. Long form only: `-t` is `--timeout` on this verb. There is no `all` value; omit the flag for both.
- `--json` (default: pretty table): one JSON line per topic instead. JSON keys: `topic`, `transport`, `schema`, `hash`, `rate_hz`, `publisher_pid`, `is_dead`. `topic` is the wire key as is, as in the table (`sb 0.5.0+`).

`sb 0.5.0+`: the `schema` column also reads non-protobuf Zenoh wire tags (`sb_discover::WireTag::type_label`): `application/flatbuffers;<type>` fills it with `<type>`; `text/*` and `application/json` fill it with the mime; `application/octet-stream` and anything else leave it to the other sources.

Table columns: `[transport-tag] rate Hz  PID <n>[ (dead)]  schema  topic`. `sb 0.5.0+`: `topic` is the Zenoh key expression or iceoryx2 service name as is, which is also the canonical form, so a row pastes into `sb topic listen` / `sb topic pub` unchanged. History: `sb 0.4.2` began prepending a leading `/` to both the table and the JSON `topic`; 0.5.0 reverted that when the canonical form dropped its `/`. The PID column is populated for iceoryx2 via `Service::list` → `dynamic_details.nodes` → `UniqueNodeId::pid()`; alive owner shown as `PID <n>`, stale on-disk service whose owning process is gone shown as `PID <n> (dead)`. Zenoh entries render `-` in the PID column (the wire format does not yet carry publisher PID).

`sb 0.4.1+`: a Zenoh row with neither a Header `msg_type` nor an encoding schema takes its `schema` from the owning module's `sb.module.yaml`, by the same lookup as `sb topic listen`'s `"config"` source below (`fill_config_schemas` → `sb_discover::config_schema` on the row's topic). Any miss (no active workspace, unregistered module, unreadable `sb.module.yaml`, no matching entry) leaves the row blank. Table and JSON shapes are unchanged.

`sb 0.4.2+`: the Header probe behind the `schema` and `rate_hz` columns (`sb_discover::probe_header`, shared with `sb topic listen`'s `"header"` source) reads both Header layouts. `swarmbotix/header/Header` carries `metadata` at tag 4 (tag 3 is `uint64 seq`); `ros2/std/Header` carries it at tag 3. `HeaderProbe` implements `prost::Message` by hand to tell tag 3 apart by wire type (length-delimited is `metadata`, varint is `seq`) and prefers tag 4. Before 0.4.2 only tag 3 was read, so a swarmbotix-layout Header left both columns to the fallbacks: its tag-4 metadata was skipped, and a non-zero `seq` failed the decode outright.

#### `sb topic listen <topic> [OPTIONS]`
Subscribe and print frames as JSON lines.
- `<topic>` (required): bare names resolved via cwd's `sb.module.yaml`. `sb 0.4.2+`: any argument containing a `/` is fully qualified; before 0.4.2 only a leading `/` marked one. `sb 0.5.0+`: a leading `/` is accepted and dropped (0.4.2 prepended one when missing). An argument without a `/` is a bare name.
- `-t, --transport zenoh | iceoryx2` (default: inferred, below): which transport to subscribe on. Aliases `z` (zenoh), `i` and `iox2` (iceoryx2).
- `--raw` (default: decode via vault) — emit hex bytes; payload becomes `{kind: "hex", bytes: "…"}`.
- `-n, --count <N>` (default: `0` — run until SIGINT) — stop after N frames.
- `--type <style>/<ns>/<Leaf>` (default: none): decode every frame as this vault message, overriding the wire schema and the Header probe.
- `--no-data` (default: off): drop `bytes` field contents and keep only `<name>_len`.
- `--show` (default: off): open a window and draw image frames (below). Conflicts with `--raw`.
- `--pretty` (default: off): indent each frame's JSON across several lines, with a blank line between frames, instead of one compact line per frame. Whitespace only; the content is identical. `sb 0.9.0+`.

`--type`, `--no-data`, `--show`, the `schema_source` key, the `bytes` summary, the wire-schema source, and typed iceoryx2 opens are `sb 0.4.0+`. The `-t` short, the transport aliases, transport inference, and the `"config"` source are `sb 0.4.1+`; before 0.4.1, `--transport` defaulted to `zenoh`.

Transport, when `-t` is absent (`pick_transport`): the `<transport>` segment of a fully-qualified `<device>/<ws>/<module>/<zenoh|iox2>/<topic>`, or, for a bare topic, the transport of the cwd `sb.module.yaml` entry it resolved to. There is no default. A fully-qualified path not in that five-segment form, without `-t`, exits 1 with
`error: cannot infer transport from <topic>: expected <device>/<ws>/<module>/<zenoh|iox2>/<topic>, or pass -t zenoh|iox2` (`sb 0.5.0+` text; e.g. `cannot infer transport from not/canonical: ...`).
A `-t` that disagrees with the path wins, with a stderr note such as `note: path says zenoh, --transport says iceoryx2; using iceoryx2`.

Output: one JSON line per frame, keys in this order: `transport`, `topic`, `schema`, `schema_source`, `hash`, `encoding`, `payload`. `topic` is the wire key without a leading `/` (`sb 0.5.0+`), as are the `--show` window title and the stderr warnings that name the topic. `schema_source` is `"flag"`, `"wire"`, `"header"`, `"config"` (`sb 0.4.1+`), or `null` (exactly when `schema` is `null`). `encoding` is `"protobuf"`, `"iox2"` (an iceoryx2 `#[repr(C)]` struct), or `null`; `sb 0.5.0+` adds `"flatbuffers"` and `"text"` on Zenoh. `payload` is `{kind: "json", json: {…}}` or `{kind: "hex", bytes: "…"}`, and `sb 0.5.0+` adds `{kind: "text", text: "…"}`.

Zenoh wire tags (`sb 0.5.0+`, beyond protobuf; only `--type` overrides them): `application/flatbuffers;<type>` names the type (`schema_source: "wire"`) and decodes through `flatc --json --raw-binary --strict-json` against `<message_targets>/fb/<ns>/<Leaf>/<Leaf>.bfbs` when that file and `flatc` exist, else hex with `schema` set and a one-time stderr note (`encoding: "flatbuffers"` either way); `text/*` and `application/json` print `payload: {kind: "text", text: …}` with the mime as `schema` (`schema_source: "wire"`, `encoding: "text"`; a body that is not UTF-8 prints hex); `application/octet-stream` adds nothing, so the Header probe and `"config"` sources apply as for an untagged sample. `hash` is reserved and always `null`. A `bytes` field longer than 64 bytes prints as `{len, head_hex}` (first 16 bytes).

Type resolution, per frame, first hit wins:
1. `--type` → `"flag"`.
2. The transport's wire schema → `"wire"`. Zenoh: the sample encoding `application/protobuf;<style>/<ns>/<Leaf>`, set by generated Rust, Python, and C++ Zenoh publishers (Flutter and Unity set none). iceoryx2: the payload type name the service was registered with, mapped to the vault iox2 struct with the same leaf and the live payload's size.
3. The Header probe (`metadata.msg_type`, either Header layout `sb 0.4.2+`, see `sb topic list`) → `"header"`.
4. The topic's entry in the module's `sb.module.yaml` → `"config"` (`sb 0.4.1+`; `ResolveOutcome::schema`). The entry's `type` is used verbatim. Bare topic: the cwd module's matched entry. Fully-qualified: `<module>` is looked up in the active workspace's installed `sb.workspace.yaml` (`active_module_registry`), as written, then without its `-<instance>` suffix (everything from the first `-`; `ips_track-sb00` → `ips_track`); publishers then subscribers, the first entry whose transport equals the path's and whose `topic:` equals the path's last segment or whose `key:` equals the whole path (`sb_discover::config_schema`). Only the five-segment form is looked up, so a four-segment legacy key such as `dev01/demo/pubber/hello` never matches. The cwd `sb.module.yaml` is never consulted for a fully-qualified path. Any miss (no active workspace, unregistered module, unreadable `sb.module.yaml`, no entry, path not in the five-segment form) skips the source silently. On iceoryx2 a typed service always supplies a wire type first, so the source matters only for an untyped `[u8]` service; there, as with `--type`, the name matches no iox2 layout (the element is one byte), so the bytes are tried as protobuf, else hex with `schema` kept.

No hit: hex, `schema: null`, and a one-time stderr hint to re-render the publisher (`sb io edit <name> -m <type>`) or pass `--type`. A resolved type that does not decode prints hex with `schema` kept. `--raw` skips resolution entirely.

iceoryx2: the service is opened with its own registered payload type (read via `Service::list`, opened over `CustomPayloadMarker`), so typed sb-generated publishers are readable. The pre-0.4.0 `[u8]`-only open and its `IncompatibleTypes` failure are gone. The listener never creates the service; until it exists it waits, printing `waiting for iox2 service <name>...` once on stderr. Bodies decode against layouts flattened from the same per-style descriptor sets with the `sb.config.yml` caps (`string_array_cap`, `string_array_caps`, `bytes_caps`, `vec_caps`, `iox2_variants`), natural layout first, then packed. `schema` is the vault name (the base message for an `iox2_variants` type).

`--show`: built-in viewer (`sb_listen::view`; `minifb` + `image`). Nothing is linked from the system at build time: on Linux, libX11, libwayland-client and libxkbcommon are loaded at runtime; Windows uses user32 / gdi32. Draws frames whose decoded body has, at the top level or one level down (a nested message such as the `image` field of `ros2/std/ImageStamped`; nothing deeper), `width`, `height`, `encoding`, `step`, `data` (encodings `mono8`, `mono16`, `rgb8`, `bgr8`, `rgba8`, `bgra8`, `8UC1`, `8UC3`, `16UC1`) or `format` + `data` (JPEG, PNG). Works on both transports. The body is decoded with `bytes` whole for the viewer, then summarized for stdout, so JSON lines are unchanged (including under `--no-data`). An undrawable frame is skipped with one stderr warning. Escape or `q` or closing the window ends the command with exit 0. No display: fails with `no display available for --show` before subscribing.

Decoding uses a `DescriptorPool` built from the vault at startup, one `protoc` run per style with that style's `message_definitions/` as the include root (`sb_listen::build_pool`), so intra-style imports resolve exactly as they do for `sb message compile`. A style that fails to compile prints `warning: dynamic decode skipped for style <style> (<protoc error>)` on stderr and is left out; the pool is only abandoned (`warning: dynamic decode disabled`, hex payloads) when `protoc` is unconfigured, the vault is empty, or no style compiled.

#### `sb topic pub <topic> <bytes-hex> [-t, --transport zenoh | iceoryx2]`
Publish raw bytes once.
- `<topic>` (required): bare or fully-qualified, by the same rule as `sb topic listen` (`sb 0.5.0+`: a leading `/` is accepted and dropped). On success prints the wire key without a leading `/`, e.g. `published 4 byte(s) on test/x/y/zenoh/p via Zenoh`.
- `<bytes-hex>` (required) — hex string (no `0x` prefix).
- `-t, --transport zenoh | iceoryx2` (default: inferred): aliases `z` (zenoh), `i` and `iox2` (iceoryx2). Inference, the `cannot infer transport` error, and the disagreement note are exactly as for `sb topic listen` (`sb 0.4.1+`; before it the default was `zenoh`). No schema lookup: raw bytes need none.

#### `sb topic prune [--json]`
Remove iceoryx2 on-disk node registrations whose owning process is gone — the entries `sb topic list` flags `(dead)`. Walks `Node::list` for the global iceoryx2 config and calls `try_remove_stale_resources()` on every `NodeState::Dead`. Zenoh is session-based and has no on-disk state to prune, so this verb is iceoryx2-only.
- `--json` (default: human table) — emit one JSON object `{cleaned_pids: [u32], failed: [[pid, reason], …]}`.

Human output: one `Pruned <N> dead iceoryx2 node(s):` block listing the cleaned PIDs, plus a `warning: failed to prune PID <n>: <reason>` line per failure on stderr. Exit code is non-zero iff any node failed to clean (insufficient permissions, version mismatch). When nothing is stale, prints `No dead iceoryx2 nodes found.` and exits 0.

---

### Launch

`sb 0.6.0+` (native launch): `sb` reads the installed workspace and (`sb 0.8.0+`) one installed flow, resolves the manifest of each of the flow's nodes for the host (`ModuleDevConfig::resolve(Host::current())`), runs every module's `build` inline (inherited stdio, in order, fail fast) **before any tmux call**, then opens one tmux/psmux window per instance in session `sb-<active_workspace>`. Window name: `instance_module(module, id)`, i.e. `<module>-<id>`, or `<module>` without instances. Each window runs the expanded `start` argv directly (no shell), with `-c <module root>`, one `-e KEY=VAL` per env entry (merged `env`, instance `env`, then `SB_WORKSPACE`, `SB_MODULE`, `SB_MODULE_ROOT`, and `SB_INSTANCE_ID` for an instance), and `--id <id>` appended after expansion for an instance; `remain-on-exit on`. Targets are exact: sessions as `=<session>`, windows by `#{window_id}`. Shipped docs: `documents/sbcli_launch.md`, `documents/sbcli_module.md`.

`sb 0.7.0+`: every module is native. `runscript.bash` and `stopscript.bash` at a module root are never probed by `sb-launch` and never run; `sb ws check` / `sb doctor` list them in the `deprecated` row. 0.6.0 also had a legacy mode (`runscript.bash` and no `scripts` → `cd <root> && ./runscript.bash [args]` in one window per module, `stopscript.bash` on stop, with a `Deprecation`), removed in 0.7.0 with `PaneCmd::Legacy` and `StopPlan.legacy`.

Before each build step stdout prints `building <module>: <argv joined by spaces>`.

#### `sb up [FLOW]`
`sb 0.8.0+` (`Cmd::Up { flow: Option<String> }`, `sb_launch::plan_up(home, ws, flow, host)`): build and launch the nodes of one flow of the active workspace, `FLOW` defaulting to `default` (`sb.flow.yaml`); a registered module that is not a node is neither built nor started. Requires an active workspace. No passthrough args; instance `args` apply. Errors: `no flow named {flow} in workspace {ws} (flows: a, b)` (or `(flows: none)`); `flow {f} has no nodes`. Skips, printing `skipped <module>: <reason>` before any build, a node whose `platforms:` exclude the host (`not supported on <os>-<arch> (declares: <keys>)`) and a Unity module (`unity modules have no start command (launch them from their editor)`); when everything is skipped it errors with `flow <f> has nothing to launch on <os>-<arch>: <module>: <reason>; ...`. The session check (`ensure_no_session`) runs before the builds. Refuses an existing session (the session is per workspace, so switching flows is `sb down && sb up <flow>`). Warns on stderr (and continues) when a repository file is newer than the installed copy (`sb_workspace::is_stale`): `warning: <file> at <repo dir> is newer than the installed copy; run sb install`. On success records the flow as the active flow (`set_active_flow`, `<sb_home>/workspaces/<ws>/active_flow`); `LaunchPlan.flow` carries it. stdout:

```
flow <name>: launched <n> window(s) in tmux session "<session>"
  - <window>
attach with: tmux attach -t <session>
```

#### `sb run [--fg] <module>[:<id>] [args...]`
Build one module, then start every instance (`<module>`) or one (`<module>:<id>`) in `sb-<workspace>`, creating the session on demand; windows that exist are respawned (`respawn-window -k`), the rest created, then windows named `<module>` / `<module>-*` not in the plan are closed. `sb 0.8.0+` (`plan_run`): instances and instance settings come from the active flow, else `default`; a module that is not a node of that flow is refused (`module {m} is not a node of flow {f}`). Args after the target fill `{args}` after the instance's own `args`; `--` disambiguates from `sb`'s flags. No `--id` flag (a passthrough `--id` is just an arg). stdout, one line per window: `launched <window> in tmux session "<session>"` or `restarted <window> ...`, with ` (args: a b)` when args were given. Unsupported platform: refused. `--fg`: exactly one native pane; builds, then runs it in the foreground with inherited stdio (Unix `exec`, Windows spawn-and-wait) and exits with its code (the replacement for the 0.5 launch scripts' `SB_NO_TMUX=1`).

```bash
sb run cam_gige_ht                    # all instances
sb run cam_gige_ht:left --exposure 4  # one instance: <bin> --exposure 4 --id left
sb run cam_gige_ht -- --foo --bar     # `--` disambiguates if flags clash with sb's
sb run --fg ips_viz                   # foreground, no tmux
```

#### `sb stop <module> [args...]`
Native: kill every window named `<module>` or `<module>-<anything>` by window id, then run `scripts.stop` inline (module root, module env, `{args}` from the args) when declared. stdout: `stopped <module>: closed <k> window(s) [<w1>, <w2>]`, then `ran scripts.stop for <module>` when it ran. If that empties the session, tmux ends it. `sb 0.7.0+` never runs a `stopscript.bash` (0.6.0 still ran it, by respawning the window, for a legacy module). `sb stop <module>:<id>` is refused. `sb 0.8.0+`: reads `sb.workspace.yaml` only, never a flow.

#### `sb down`
Kill `sb-<workspace>` (exact target) and all its windows. No arguments. Coarser than `sb stop`: runs no `scripts.stop`.

#### `sb attach`
`tmux attach -t sb-<workspace>`. No arguments.

---

### Production

#### `sb gopro [--all] [--module <name>]`
Export the module manifest + `<io_dir>/` to `sb.prd.yml` (sanitized, language-agnostic, ships with binary). Strips `root:`, absolute paths, and other host-specific fields.
- Default target: cwd's module.
- `--module <name>` (default: cwd) — target module.
- `--all` (default: single module) — export every module in the active workspace.

---

### App Packages

An **app package** is a folder with an `sb.app.yml` at its root. It is not a module: it is a foreground tool (calibration, converter, generator) that users run as `sb <name> [args...]` after `sb install`, with no change to `PATH`. Apps are global to the host and independent of workspaces. Shipped doc: `documents/sbcli_app.md`.

#### `sb app init [path] [--name <n>] [--entry <file>] [--kind docker|host|none] [--image <img>] [--description <t>] [--force]`
Write a fully commented `sb.app.yml` into `path` (default cwd) plus executable stubs for a missing entry and, for `--kind host`, a missing `install.bash`. Prompts for name / entry / kind / image on a TTY when `--kind` is absent; otherwise kind defaults to `none` (or `docker` when `--image` is given). Name defaults to the sanitised folder name and must pass `validate_identifier` and not be an `sb` built-in. The manifest is validated before being written. Existing scripts are never overwritten; `sb.app.yml` only with `--force`.

#### `sb install <url|path> [--prefetch]` (alias `sb app install`)
- `sb 0.5.0+`: the source's root manifest decides: `sb.app.yml` → app (below); `sb.workspace.yaml` (`sb 0.8.0+`; `sb.flow.yaml` before, and a 0.7-shape `sb.flow.yaml` in 0.8.0) → workspace (§Workspaces, `sb install`); both or neither → error (`sb 0.7.0+`: a root with only `flow.yaml` fails with the §Deprecations flow error).
- Source: git URL (`https://`, `http://`, `ssh://`, `git://`, `file://`, `git@…`, or anything ending in `.git`), optionally `@<branch-or-tag>`; else a local folder, tilde-expanded and canonicalised.
- Git sources are shallow-cloned into `<sb_home>/apps/<manifest.name>/`; the same URL again fast-forwards, a different `@ref` replaces the clone. Local folders are registered in place: never copied, never deleted.
- Steps: resolve source → load + validate `sb.app.yml` → claim name (built-in or taken by another folder = error) → kind step (`none`: nothing; `docker`: `docker pull <image>` only if `prefetch: true` or `--prefetch`; `host`: `bash <host.install>` from the package root, non-zero aborts) → check `requires` on PATH (warnings) → write `<sb_home>/apps.yml`.

#### `sb <name> [args...]`
clap `external_subcommand`: a name that is not a built-in is looked up in `apps.yml`. On Unix the `sb` process is replaced by the entry (`exec`); on Windows it is spawned via `bash` and its exit code returned. cwd = user's cwd, args verbatim, env `SB_HOME`, `SB_APP_DIR`, `SB_APP_NAME`. Unknown name: exit 1 with a did-you-mean against built-ins and installed apps.

#### `sb app list | info <name> | update [name] | remove <name>`
`update`: git sources `git pull --ff-only` (pinned tags stay), then the kind step reruns and `version` / `commit` are refreshed; no name = every app, stop at first failure. `remove`: unregister; delete the folder only when it lives under `<sb_home>/apps/`.

#### `sb.app.yml` schema
```yaml
name: camcalib            # identifier, not a built-in
version: 0.1.0
description: …            # optional
entry: ./camcalib         # relative, inside the package
kind: docker              # docker | host | none
docker:                   # kind: docker
  image: swarmbotix/sb_kalibr:latest
  prefetch: false
host:                     # kind: host
  install: ./install.bash
requires: [docker]        # binaries checked on PATH by install and doctor
```
Unknown keys are rejected. `apps.yml` schema: `apps.<name>.{path, source: path|git, url, git_ref, commit, version, installed_at}`.

---

### Self-Update

Releases live at `github.com/swarmbotix/sbcli`, tag `vX.Y.Z`, assets `swarmbotix-<ver>-<arch>.zip` + `.sha256`, installer inside the zip. `sb` resolves the latest version from the `releases/latest` redirect (no API, no token). Shipped doc: `documents/sbcli_update.md`.

#### `sb --version` / `-V`
Prints `sb <version>`; then, unless `SB_NO_UPDATE_CHECK` is set (non-empty, not `0`), prints a second line: `update available: X.Y.Z   run `sb update`` when a newer release exists, `(latest)` when equal; nothing when ahead or when the lookup fails. Lookup is cached 24 h in `<sb_home>/update-check.json`, hard 4 s limit including DNS, never affects the exit code.

#### `sb update --check`
Always fetches, refreshes the cache, prints `installed / latest / platform / asset / status`. Exit 0 for `up to date` or `ahead`, 10 for `update available`. Not combinable with `--version` / `--force`.

#### `sb update [--version X.Y.Z] [-y|--yes] [--force]`
Guard: `current_exe` must be `<sb_home>/bin/sb[.exe]` (`SB_UPDATE_ALLOW_ANY_EXE=1` for tests). Target = `--version` or latest; equal = refused unless `--force`; lower = downgrade warning. Confirm `sb <old> -> <new>` on a TTY unless `--yes`; no TTY without `--yes` = error. Download zip + sidecar to a temp dir outside `<sb_home>`, verify SHA-256, unpack, require one top folder `swarmbotix-<ver>-<arch>/` with the installer, run `bash install.sh --yes` / `powershell -NoProfile -ExecutionPolicy Bypass -File install.ps1 -Yes` with `SB_HOME` set, keep the last 20 output lines for the error path, then require `<sb_home>/bin/sb --version` == target. Windows: rename running `sb.exe` to `sb.exe.old` first, restore on failure; every `sb` start deletes a stale `.old`. Platform map: `x86_64-linux` → `linux-x86_64`, `aarch64-linux` → `linux-aarch64`, `x86_64-windows` → `windows-x86_64`. `SB_RELEASE_BASE` overrides the release location (`https://` fork or `file:///dir` with `latest.txt` + zips) for tests.

---

### Host Tools (`sb 0.9.0+`)

`protoc` and `flatc` are not bundled; `sb tools install` fetches the pinned releases (§Pinned third-party versions). Shipped doc: `documents/sbcli_tools.md`. A fresh Linux machine reaches a green `sb doctor` non-interactively with:

```bash
curl -fsSL https://github.com/swarmbotix/sbcli/releases/latest/download/get-sb.sh | bash -s -- --yes
export PATH="$HOME/.swarmbotix/bin:$PATH"
sudo mkdir -p /opt/protobuf /opt/flatbuffers && sudo chown "$USER" /opt/protobuf /opt/flatbuffers
sb tools install --yes
sb message compile
sb doctor
```

Line 2 is needed because a running or non-interactive shell does not read `~/.bashrc`, where the installer appended it. `sb` never invokes `sudo`; line 3 is the only privileged step and is the user's.

#### `get-sb.sh [--yes] [--version X.Y.Z]`
Linux bootstrap published beside the zips of every release (not inside them), so `releases/latest/download/get-sb.sh` always resolves. Version: `--version`, else the tag from the `Location` header of `<base>/releases/latest`. Arch map as `install.sh` (`x86_64|amd64` → `linux-x86_64`, `aarch64|arm64` → `linux-aarch64`, else error). Requires `curl`, `unzip`, `sha256sum` (names the missing one, exit 1). Downloads `swarmbotix-<v>-<arch>.zip` + `.sha256` into `mktemp -d`, verifies, unzips, runs `install.sh [--yes]`, removes the temp dir on exit. Passes `SB_HOME` through; honours `SB_RELEASE_BASE` with the same meaning as `sb update`. Ends with `Next steps:`: the `export PATH` line when `sb` is not on `PATH`, the `sudo` line listing only the `/opt` parents that are missing or unwritable, then `sb tools install --yes`, `sb message compile`, `sb doctor`.

#### `sb tools install [--prefix DIR] [--only protoc|flatc] [-y|--yes]`
Install the pinned `protoc` and `flatc` for this platform and point the global `sb.config.yml` at them.
- `--prefix DIR` (default: `/opt`; Windows `C:/cmake_ext_libs`) — install root.
- `--only protoc|flatc` (default: both) — case-insensitive; anything else fails with ``unknown tool `<x>`; expected protoc or flatc``.
- `-y | --yes` — do not ask. Required when stdin is not a terminal.

Pins and assets (platform = the one `sb` was built for, as `sb update`):

| Platform | Tool | Asset | SHA-256 |
|---|---|---|---|
| `linux-x86_64` | protoc | `protoc-35.0-linux-x86_64.zip` | `a45cda0989c17dd950db55f6fbe1e5814c50fda08e87aa422980ac1f89dddbbc` |
| `linux-aarch64` | protoc | `protoc-35.0-linux-aarch_64.zip` | `36b518ac14d90351cc6598228ed2bbe5afe4e357b1af470b07e0ec1609875de2` |
| `windows-x86_64` | protoc | `protoc-35.0-win64.zip` | `d1cede9e308cc3eb072392af1c02ccae4bdd3d2f374ec2970dbd8cdfdaa91363` |
| `linux-x86_64` | flatc | `Linux.flatc.binary.g++-13.zip` | `9f87066dc5dfa7fe02090b55bab5f3e55df03e32c9b0cdf229004ade7d091039` |
| `linux-aarch64` | flatc | none published | unavailable: `no prebuilt flatc 25.12.19 for linux-aarch64; build it from source: https://github.com/google/flatbuffers` |
| `windows-x86_64` | flatc | `Windows.flatc.binary.zip` | `fff9445c9db907227bc64b54cc98743084c4949282aa4e576cff6a955724ddc8` |

URLs: `https://github.com/protocolbuffers/protobuf/releases/download/v35.0/<asset>` and `https://github.com/google/flatbuffers/releases/download/v25.12.19/<asset>`.

Layout (`dir` relative to the prefix, executable relative to `dir`): Linux protoc `protobuf/protoc-35.0-linux-x86_64` (aarch64: `protobuf/protoc-35.0-linux-aarch_64`) + `bin/protoc`; Linux flatc `flatbuffers/flatc-25.12.19` + `flatc`; Windows protoc `protoc/35.0` + `bin/protoc.exe`; Windows flatc `flatbuffers/25.12.19` + `flatc.exe`.

Tool state: `installed` (exe exists and the first line of `exe --version` contains the pin), `missing` (no exe), `stale (<line>)` (exe exists, other line or `--version` failed), `unavailable (<reason>)` (no asset for this platform).

Behaviour, in order:
1. Resolve prefix and `--only`; plan every tool (in `protoc`, `flatc` order).
2. Print to stdout, `{:<10}: {:<10} {:<12} -> {}`, unavailable rows without arrow and path:
   ```
   prefix    : /opt
   protoc    : 35.0       missing      -> /opt/protobuf/protoc-35.0-linux-x86_64/bin/protoc
   flatc     : 25.12.19   missing      -> /opt/flatbuffers/flatc-25.12.19/flatc
   ```
3. No tool `missing` or `stale`: print `nothing to do: every available tool is at its pinned version`, still set the config key of any installed tool whose configured path differs, print the doctor rows, exit 0.
4. Privilege: the parents `<prefix>/<first component of dir>` that do not exist or are not writable, for tools not `installed` (dedup, tool order). Non-empty: exit 1 with ``<dirs joined by ", "> not writable; run this once, then re-run `sb tools install`:`` and, on the next line, two-space indented, `sudo mkdir -p <dirs> && sudo chown "$USER" <dirs>` (dirs space-separated; Windows: `mkdir` lines instead). Nothing is downloaded first.
5. Confirm `Proceed? [y/N]` unless `--yes`; no terminal without `--yes` → `not a terminal; pass --yes`; no → `cancelled`.
6. Per tool to install, progress on stderr (`downloading <url> ...`, `verifying sha256`, `unpacking into <dest>`): temp dir → fetch → SHA-256 compare (mismatch deletes the download and fails with `sha256 mismatch for <asset>: pinned <hex>, download is <hex>`) → unzip → `chmod 0755` the exe (unix) → create the parent → remove an existing `dest` → move into place (rename, else recursive copy) → `exe --version` must contain the pin. Then stdout `installed: <tool> <version> -> <exe>` per tool.
7. `sb config set <tool> <exe>` (global file) for each installed tool and each `installed`-state tool whose configured path differs; prints `set <tool> in <path>`.
8. Reload the config; print `sb_doctor::tool_checks` (§Doctor 1-2).
9. Per unavailable tool, stderr `note: <tool>: <reason>`. Exit 0.

`SB_TOOLS_BASE=file:///<dir>`: copy `<dir>/<asset>` instead of downloading; a `<dir>/<asset>.sha256` sidecar's first token replaces the pinned digest (offline tests only). Any other non-empty value is an error; unset or empty downloads the pinned URLs.

---

### Web UI

#### `sb ui [OPTIONS]`
Start the `swarmctl` server in a tmux pane and open the browser. Idempotent — re-running reuses the existing pane.
- `--port <p>` (default: `7878`) — bind port.
- `--no-browser` (default: opens browser) — start server only.

---

### Doctor

#### `sb doctor`
Print `sb <version>`, then read merged `sb.config.yml` and run twelve checks, in this order:
1-2. Executables: `protoc`, `flatc`, each against its `versions.json` pin (sb 0.9.0+). Rows, exactly: unset → `[SKIP]` ``not set in sb.config.yml (run `sb tools install`)``; path missing on disk → `[FAIL]` ``<name> not found at <path> (run `sb tools install`)``; `--version` fails → `[FAIL]` `<path> --version exited <status>: <stderr>` (or `<path> --version failed: <error>`); first stdout line contains the pin, or the pin is `unknown` → `[OK]` `<path> (<line>)`; otherwise → `[WARN]` ``<path> (<line>; sb pins <name> <pinned>, run `sb tools install`)``. `sb_doctor::tool_checks(&cfg)` returns exactly these two rows; `sb tools install` prints them.
3. `transports` — the zenoh and iceoryx2 **crate** versions linked into this binary, baked in from the workspace lockfile by `sb-doctor`'s build script (`SB_ZENOH_VERSION` / `SB_ICEORYX2_VERSION`; `unknown` when the build tree had no lockfile). Reads nothing from the config and never fails.
4-5. Shared libraries (file exists + `dlopen` succeeds): `libzenohc.so`, `libiceoryx2_ffi_c.so`. Each also reports the install's own version, probed in order from `<libdir>/pkgconfig/*.pc`, `<libdir>/cmake/*/…ConfigVersion.cmake`, a version component of the canonicalized path, then a `.so.X.Y.Z` suffix; `version unknown` when every probe misses. A version that disagrees with check 3 **warns**, never fails: zenoh on differing `major.minor`, iceoryx2 on any differing digit (segments are opened on exact version equality).
6. `tmux` (or `psmux`), probed with `-V`, not `--version`. Unset falls back to `tmux` on `PATH`; neither → `[SKIP]` ``not on PATH and not set in sb.config.yml (needed by `sb up`)`` (sb 0.9.0+; `[FAIL]` before). Unset `libzenohc` / `libiceoryx2` (4-5) → `[SKIP]` `not set in sb.config.yml`.
7. `message styles` — one row per style under the resolved root with its `.proto` count and whether its targets are built. Fails only when the root holds no styles at all.
8. `unity targets` — stale generated C# inside a Unity project (see §Generated-code language levels). **Warns**, never fails.
9. `config keys` — the dead `message_style` / `message_definitions` / `message_targets` keys, if still present.
10. `std vault` — `~/.swarmbotix/messages/ros2/message_definitions/std/` populated; names `sb message list` as the re-install path.
11. `topics` (`sb 0.5.0+`): legacy `topic: /...` entries in the modules of the active workspace. `` [OK]   topics       no legacy `topic: /...` entries in the active workspace ``; `[SKIP] topics       no active workspace`; otherwise one `[WARN] topics` row per offending module, naming the module, then a continuation line per entry with both forms, then the rewrite command (`sb 0.7.0+`: ``rewrite: `sb io edit <first entry> --module <module>` ``). **Warns**, never fails.
12. `workspace` and `ws *` (`sb 0.5.0+`): the `sb ws check` rows for the active workspace (`workspace`, `ws modules`, `ws types`, `ws encoding`, `ws io files`, and `sb 0.6.0+` `ws requires`, `ws platforms`, `sb 0.8.0+` `ws flows`, then `deprecated` and `ws source`; see §Workspaces). `WorkspaceView` carries the `Host`. `[SKIP] workspace no active workspace` without one. A `[FAIL]` here **counts toward the exit code** (it breaks `sb up` or codegen); `ws io files`, `ws platforms`, `deprecated`, and `ws source` only warn.

No Python-package check exists, so a `pip`-installed transport is never compared against check 3. An unset path field is `[SKIP]`, not `[FAIL]`.

Tags are `[OK]` / `[FAIL]` / `[SKIP]` / `[WARN]`, colored when stdout is a TTY and `NO_COLOR` is unset or empty. **Only `[FAIL]` affects the exit code** — a run whose worst rows are `[WARN]` and `[SKIP]` exits 0.

**`sb doctor` reports the vault that `sb message *` will actually use**, resolved by the same rules (see §Message Vault) — not `messages_root` from config. The two diverge whenever cwd discovery wins, and a doctor describing a different tree than the next command touches is worse than no doctor. Consequences when the vault was discovered:

- The `message styles` line is labeled `(discovered from cwd — overrides messages_root)`, and narrows to a single style when cwd is itself one.
- `std vault` **skips** instead of failing when a discovered root has no `ros2` style. A standalone message repo legitimately has no `std/`; the forge bundle installs into the configured vault only.

Rust and Flutter toolchains are skipped — cargo and pub manage those per-project. On failure the checklist still prints in full on stdout and a single `error: <n> check(s) failed` summary goes to stderr.

---

### Config

#### `sb config`
Open the merged `sb.config.yml` in `$EDITOR`. Edits land in `~/.swarmbotix/sb.config.yml` unless invoked inside an active workspace, in which case the workspace's override file is opened.

#### `sb config set <key> <value> [--workspace]`
Write a single key non-interactively.
- `<key>` (required) — e.g. `protoc`, `transport_on_device`.
- `<value>` (required)
- `--workspace` (default: writes to global `~/.swarmbotix/sb.config.yml`) — write to active workspace's `sb.config.yml` instead.

---

## Message System

### Default: Protobuf IDL

Ships with standard message definitions in `.proto` format. Protobuf chosen as the canonical source because:
1. Protobuf → JSON (built-in)
2. `.proto` → `.fbs` (flatc converter)
3. Flatbuffers → JSON
4. Other protocols → use LLM to derive from `.proto`
5. Iceoryx2 Rust/Python/C++ types → Rust-based converter script

### Message Vault

Messages are addressed by a **fully-qualified** name: `<style>/<namespace>/<Leaf>` (e.g. `ros2/std/Header`, `swarmbotix/images/Image480pMono8`). The name resolves to `<messages_root>/<style>/message_definitions/<namespace>/<Leaf>.proto`. There is no ambient "active style": one module routinely publishes one style and subscribes to another, and `Header.metadata.msg_type` travels on the wire to consumers that share no config with the publisher. The forge bundles the canonical `std/` set under `messages/ros2/message_definitions/std/` and installs it into the ros2 style on first run.

```bash
sb message list        # list local messages
sb message new  <Name> # create new .proto locally
sb message rm   <Name> # remove local message
```

#### `sb message backends <style> [<backend>...] [--clear]`
Show or pin which codegen backends `sb message compile` emits for a style.

- `<style>` (required) — e.g. `vrobots_msgs`.
- `<backend>...` (optional) — any of `iox2`, `proto`, `fb`; or the single word `none` for an empty list. Omit to show the current setting.
- `--clear` — remove the pin and fall back to automatic selection.

Writes `<messages_root>/<style>/sb.style.yml`, a sibling of `message_definitions/`. The pin lives in the style tree rather than in `sb.config.yml` because a style is often a standalone repo cloned onto machines that share no config — the answer has to survive a `git clone`. Commit the file.

#### Backend selection order

`sb message compile` resolves backends per style, first match wins:

1. **Explicit `--iox2` / `--proto` / `--fb`** — honored exactly; never second-guessed. Naming a backend whose tool is missing is an error.
2. **`backends:` in `<style>/sb.style.yml`** — the persisted pin. A pin naming an unavailable tool errors rather than silently dropping it. `backends: []` means "emit nothing automatically" and is distinct from no key.
3. **Tool availability** — `iox2` always, `proto` iff `protoc` is configured, `fb` iff `flatc` is.

Rule 2 prints why it applied. A narrowed selection is never silent.

**Selection is not narrowed by the host project.** A style's generated tree feeds C++, Python and Rust clients as well as whatever project it happens to sit inside, so dropping a backend to suit one consumer would silently deprive the rest. Host compatibility is instead a property of the **emitters**, which target the oldest level any consumer uses.

#### Generated-code language levels

| Language | Target | Constraint it satisfies |
|---|---|---|
| C# | **C# 9 / .NET Standard 2.1** | Unity 6 caps here and auto-compiles every `.cs` under `Assets/` |
| C++ | C++17 | |
| Rust | 2021 edition | |
| Python | 3.8+ | |

The C# emitter therefore avoids file-scoped namespaces (C# 10, `error CS8773`) and `[InlineArray]` (.NET 8) — a repeated-message field emits a wrapper struct of `N` explicitly-named fields under `LayoutKind.Sequential, Pack = 1`, which is the same memory layout, plus a `ref` indexer so `arr[i]` is unchanged at the call site. Nothing emitted is newer than C# 9, so the output stays valid on modern .NET too.

**The whole generated tree is one assembly**, since a host compiles every `.cs` under it together. A type may therefore be declared exactly once across the *entire* output, not merely once per file. This binds the C# array wrappers above: `iox2/_arrays/<pkg>_<Leaf>_Array<N>.cs` holds one file per `(element type, capacity)` under namespace `sb_iox2_arrays`, and consuming structs reference it as `global::sb_iox2_arrays.<Name>`. Emitting it inline instead — as `sb` did through 0.1.38 — declares the same struct in every message that uses it, which is `error CS0101` as soon as two messages in one package share an element type.

`proto/` C# additionally needs a `Google.Protobuf` assembly, which a Unity project must vendor itself; codegen cannot supply it.

`sb doctor`'s `unity targets` check warns — without failing — when generated `.cs` inside a Unity project was produced by an older `sb`: file-scoped namespaces, `[InlineArray]`, or an inline `_Array<N>` wrapper outside `iox2/_arrays/`. Detection is by file content, not directory name. The fix is to re-run `sb message compile`.

---

## Web UI (swarmctl)

A REST + MCP server that wraps `sb-core` logic, serving the visual flow editor.

```bash
sb ui     # start swarmctl + open browser
```

### Design-Time View (no app running)
- Reads `sb.module.yaml` or `sb.prd.yml` per module via `sb.workspace.yaml`, and one flow file per view
- Draws module boxes with ports (publishers = output, subscribers = input), one node per instance
- Draws the edges declared in the flow's `wires:` (sb 0.8.0+, publisher first), else derives them by matching topic names across modules
- No manual edge declaration needed

### Runtime View (apps running)
- Overlays live Zenoh discovery on top of design-time view
- Designed port live → lit up
- Designed port not live → shown in orange/red (mismatch)
- Live port not in design → shown as unknown

### Interaction
- Right-click module box → add/remove publisher or subscriber
- All actions call swarmctl REST → same as running `sb` CLI commands
- AI agents interact via MCP endpoint

---

## swarmctl Architecture

```
Web UI
  ↓ REST
swarmctl (REST + MCP server)
  ↓
sb-core (shared logic: naming, workspace and flow R/W, manifest resolution, config R/W)
  ↓
sb.workspace.yaml + *.flow.yaml + sb.module.yaml + io_dir files
```

Three surfaces, one core:

| Surface | For |
|---|---|
| CLI (`sb`) | Humans, scripts, CI |
| REST | Web UI, curl, dashboards |
| MCP | AI agents (Cursor, Claude, etc.) |

---

## tmux / psmux

`sb up` creates a named tmux session with one window per instance.

| Platform | Tool | Install |
|---|---|---|
| Linux / macOS | tmux | `apt/brew install tmux` |
| Windows | psmux | `winget install psmux` |

psmux is tmux-compatible — ships as the `tmux` command on Windows. `sb` calls `tmux` on all platforms; on Windows `resolve_tmux` also falls back to `psmux` on `PATH`. `sb 0.6.0+` hands tmux the start argv as separate trailing arguments (two or more are executed directly, without a shell, from tmux 3.0; a single-token argv is passed as one quoted string), which is what lets psmux run native modules (0.6.0's legacy `runscript.bash` panes were a POSIX command line psmux could not run; they are gone in 0.7.0). psmux's handling of `-e`, multi-argument exec, and `remain-on-exit` is to be verified on a Windows box.

---

## Deprecations (sb 0.6.0, 0.8.0) and removals (sb 0.7.0, 0.8.0)

`sb 0.8.0` has one active deprecation, the 0.7 shape of `sb.flow.yaml` (removed in 0.8.1). Every other item below is removed in the release named in the table.

| item | deprecated since | removed in | replacement |
|---|---|---|---|
| `sb pub`, `sb sub`, `sb list`, `sb service` | 0.5.0 (help only), 0.6.0 (warning) | 0.7.0 | `sb io` |
| `sb.dev.yml` | 0.6.0 | 0.7.0 | `sb.module.yaml` |
| `flow.yaml` | 0.6.0 | 0.7.0 | `sb.flow.yaml` (since 0.8.0 with `sb.workspace.yaml`) |
| `runscript.bash` | 0.6.0 | 0.7.0 | `scripts:` in `sb.module.yaml` |
| `stopscript.bash` | 0.6.0 | 0.7.0 | `scripts.stop`, or nothing |
| `sb init --runscript` and its stub template | 0.6.0 | 0.7.0 | none (`runscript.rs` deleted) |
| top-level `instances:` in the 0.5 flow | 0.6.0 | 0.7.0 | the flow file's `instances:` (0.8.0) |
| `SB_NO_TMUX=1` | 0.6.0 | 0.7.0 | `sb run --fg` |
| `sb.flow.yaml` in the 0.7 shape (modules, `up:`, inline instances, subscriber-first wires) | 0.8.0 (warning) | 0.8.1 | `sb.workspace.yaml` + flow files, wires publisher first |
| instance `docker:` | 0.6.0 (not launched) | 0.8.0 (dropped by `convert_legacy_flow`) | the module's `scripts` call `docker compose`; compose reads `${SB_INSTANCE_ID}` |
| `sb init --docker` | 0.6.0 (bash template removed, flag kept as an instance preflight) | 0.8.0 | none |

What 0.8.0 does with each:

- The 0.7 shape: read as workspace + default flow (§Workspace, "Legacy 0.7 shape"), with the `Deprecation` `item: "sb.flow.yaml in the 0.7 shape"`, `hint: "split it into sb.workspace.yaml and sb.flow.yaml; see sbcli_workspace.md §Migrating"`, printed as `warning: sb.flow.yaml in the 0.7 shape is deprecated since 0.8.0 and will be removed in 0.8.1; split it into sb.workspace.yaml and sb.flow.yaml; see sbcli_workspace.md §Migrating`, and listed by the doctor `deprecated` row.
- `sb init --docker`: unknown to clap.
- The four verbs: gone from `Cmd`, with no hidden alias. The name falls through to the app launcher, which answers ``error: `<verb>` is not an sb command or an installed app.`` (with a spelling guess when one is close) and exits 1.
- `sb.dev.yml` with no `sb.module.yaml` (`sb install`, `sb init`, `sb io`, `sb doctor`): `module manifest sb.module.yaml not found at <dir>; sb.dev.yml is no longer read since 0.7.0, rename it (see sbcli_module.md §Migrating)`.
- `flow.yaml` with no `sb.flow.yaml`: `sb.flow.yaml not found at <dir>; flow.yaml is no longer read since 0.7.0, rename it (see sbcli_module.md §Migrating)`.
- `runscript.bash` / `stopscript.bash`: no error; ignored, and listed by the doctor `deprecated` row as `runscript.bash at <path> is ignored since 0.7.0 (see sbcli_module.md §Migrating)` (likewise `sb.dev.yml at`, `flow.yaml at`, `stopscript.bash at` for any leftover file). OK text unchanged: `nothing deprecated in use`.
- The 0.7 list forms of `publishers` / `subscribers` / `services` and of `messages` are read silently and are not deprecated.

`sb_core::Deprecation`, `deprecation::warn_deprecated` / `warn_all` (`crates/sb-cli/src/deprecation.rs`) carry the warning, stderr, once per item per process, never on stdout:

```
warning: <item> is deprecated since <SINCE> and will be removed in <UNTIL>; <hint>
```

Shipped table: `documents/sbcli.md` §10. Shipped migration guides: `documents/sbcli_module.md` §11 (from 0.5, required since 0.7.0; ports from 0.7) and `documents/sbcli_workspace.md` §7 (the 0.7 workspace file).

---

## Non-Goals

- `sb` does not wrap Zenoh or Iceoryx2
- `sb` does not own your build system (`sb build` does not exist); `sb up` / `sb run` only run the build command the module declares, or its language default (sb 0.6.0+)
- `sb` does not manage your app's dependencies
- `sb` does not dictate your app's internal folder structure
- `sb` does not impose a message protocol — Protobuf is the default, others are optional
- `sb` does not require a running daemon — swarmctl starts on demand
