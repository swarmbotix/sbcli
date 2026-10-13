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
4. Manages the workspace registry (`flow.yaml`)
5. Generates per-app config files (`sb.dev.yml`, `sb.prd.yml`)
6. Provides live topic introspection via raw Zenoh (`sb topic list/listen`)
7. Launches modules via tmux/psmux (`sb up`)

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
- `transport` — literal `iox2` or `zenoh`, picked from the `--iox2` / `--zenoh` flag at `sb pub/sub add` time
- `topic` — the pub/sub channel name

The transport segment lets the same bare name (e.g. `image_raw`) coexist on both transports for the same module without collision, and makes the transport visible directly in the key. `sb` enforces this naming automatically. Users type the full path only to address a topic they do not own, with `--key`.

`sb 0.5.0+`: the canonical form has no leading `/` and is byte-identical to the wire key on both transports. Zenoh forbids a leading `/` in key expressions, and iceoryx2 service names use the same form. Every publisher / subscriber entry in `sb.dev.yml` / `sb.prd.yml` carries exactly one of:

- `topic: <bare>`: owned; codegen prefixes it with `<device>/<workspace>/<module>/<transport>/`. May not contain `/`. Generated code has `is_owned = true`.
- `key: <wire key>`: verbatim, used as is. Generated code has `is_owned = false`.

A pre-0.5.0 `topic: /x/y` is read as `key: x/y` (the identical wire key); `sb doctor` warns about it (`topics` check) and the next `sb pub` / `sb sub` write rewrites it. A pre-0.5.0 `topic: a/b` (inner `/`, no leading one) is refused at load; the equivalent entry is `key: <device>/<ws>/<module>/<transport>/a/b`.

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
      flow.yaml          ← installed copy of a repo flow.yaml: absolute paths + source: (sb 0.5.0+); never hand-edited
  repos/<name>/          ← default clone location for `sb install <git-url>` of a workspace (sb 0.5.0+)
  apps.yml               ← installed app packages: name → path, source, commit, version
  update-check.json      ← cache for the `sb --version` update notice: latest, checked_at
  apps/<name>/           ← clones made by `sb install <git-url>`; sb owns and deletes these
```

### Repository state: `<repo>/` (sb 0.5.0+)

```
ws_ips/                  ← a workspace repository, cloned anywhere
  flow.yaml              ← repo copy: the source of truth for the workspace; committed
  messages/custom/       ← optional .proto sources, imported by `sb install` via `messages:`
  modules/<module>/      ← one module root per module (see below)
```

### Per-app state: `<module_root>/`

```
camera_app/
  sb.dev.yml       ← dev-time interface; sb 0.5.0+ writes no host path (no `root:`), so it is committed
  sb.prd.yml       ← production interface (ships with binary)
  swarmbotix_io/         ← generated IO files (default io_dir)
  runscript.bash         ← user-written launch script
```

### `sb.config.yml` schema

```yaml
# Host-specific tool paths
protoc:      /usr/local/bin/protoc
flatc:       /usr/local/bin/flatc
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

Missing paths are explicit errors from `sb doctor` — never silently swallowed.

### Forge → user install

This repository (the forge) bundles the canonical `std/` message set under `messages/ros2/message_definitions/`. On install (`cargo install sb-cli` or release tarball), `sb` copies the bundle into `~/.swarmbotix/messages/ros2/message_definitions/std/` if not already present. The forge is the source of truth; user machines hold a copy.

---

## Workspace

Workspaces isolate projects. Analogous to Python virtual environments. Each installed workspace is a real directory at `~/.swarmbotix/workspaces/<name>/`. Shipped doc: `documents/sbcli_ws.md`.

`sb 0.5.0+`: a workspace is defined by a **repo copy** of `flow.yaml` at the root of a repository, committed with the code. `sb install` derives an **installed copy** at `~/.swarmbotix/workspaces/<name>/flow.yaml` (absolute paths plus `source:`); every other verb reads the installed copy, and nobody edits it by hand.

```bash
sb install https://github.com/acme/ws_ips.git --dir ~/ws_ips   # clone, install (or: cd clone && sb install .)
sb ws create ws_ips              # write ./flow.yaml, then install it
sb ws install [<url|path>]       # workspace-only form of sb install (default .)
sb ws check [ws_ips]             # validate a workspace (default: the active one)
sb ws update ws_ips [--force]    # git pull --ff-only on the source, then reinstall
sb ws uninstall ws_ips [--clean] # remove the installed copy; --clean also deletes each module's io dir and the clone sb made
sb ws set    ws_ips              # write ws_ips to ~/.swarmbotix/active
sb ws list                       # installed workspaces, with source and a stale marker
sb ws delete ws_ips              # rm -rf ~/.swarmbotix/workspaces/ws_ips
```

**Repo copy** `<repo>/flow.yaml` (sb 0.5.0+):

```yaml
name: ws_ips                       # optional; default = the repo folder name
modules:
  ips_track: modules/ips_track                      # path relative to this file: a module dir or its sb.dev.yml
  ips_calib: { path: modules/ips_calib, up: false } # registered, rendered, but skipped by sb up
messages:
  - { style: ros2, path: messages/custom, backends: [proto, iox2] }   # imported into the vault and compiled by sb install; backends optional
instances: ...                     # unchanged
```

- **`name:`** *(optional)*: the workspace name; default the name of the directory holding the file. Must pass `validate_identifier`, so a folder such as `ws-ips` needs an explicit `name:`.
- **`modules:`**: module name → path relative to the file, naming the module directory (`sb.dev.yml` appended) or the yaml itself; a path that does not exist yet is a directory unless it ends in `.yml` / `.yaml`. Map form `{ path, up }`, `up` defaulting to `true` (`ModuleRef`); a bare path serialises back whenever `up` is true. `up: false`: registered, io regenerated, `sb run` / `sb stop` work, `sb up` skips it.
- **`messages:`** *(optional)*: `{ style, path, backends? }` (`MessageSource`). `style` is `[A-Za-z0-9_]+`; `backends` is any of `proto`, `fb`, `iox2`. On install, `Vault::import_style_dir` copies `<path>/<ns>/<Leaf>.proto` to `<style>/<ns>/<Leaf>` and a flat `<path>/<Leaf>.proto` to `<style>/<dirname of path>/<Leaf>`; hidden entries and non-`.proto` files are ignored; identical content is unchanged, different content is a conflict left alone unless `--force`, `ros2/std/` is never overwritten; any file that cannot be named `<style>/<ns>/<Leaf>` aborts that entry with nothing written. The style is then compiled with `backends`, or the normal backend selection when absent.
- **`instances:`** *(optional)*: unchanged, below.
- `deny_unknown_fields` applies to the file, to the map form of a module entry, and to each `messages:` entry, so an `sb` older than 0.5.0 cannot read a `flow.yaml` with the new keys.
- Load errors (`FlowValidationError`): `workspace name "<n>" is invalid: <reason>`, `module "<m>" has an empty path`, `messages[<i>]: style "<s>" is invalid (allowed: [A-Za-z0-9_])`, `messages[<i>]: path is empty`, `messages[<i>]: unknown backend "<b>" (expected one of: proto, fb, iox2)`, `module "<m>" declares instance id "<id>" more than once`.

**Installed copy** `~/.swarmbotix/workspaces/<name>/flow.yaml`: `name:` always set; every path absolute (canonical when it exists) and every module path naming its `sb.dev.yml`; `source: { repo_flow, url, installed_at }` (`FlowSource`), where `url` is absent for a local-path install and `installed_at` is RFC 3339 UTC to the second. Written in full by every install (`sb_workspace::install_flow`), never by hand; the repo copy is never written by an install. An installed copy is refused as an install source. A workspace of the same name installed from a **different** repo flow that still exists is refused; one whose previous source no longer exists is replaced.

A pre-0.5.0 installed copy (bare `modules:` with absolute paths, optional `instances:`) still loads; with no `source:`, `sb ws update` and the stale check do not apply to it.

`flow.yaml` (either copy) stores:
- **`modules:`** — module name → path to its config file.
- **`instances:`** *(optional)* — module name → list of declared instances. Each entry is `{ id, label?, docker?, args? }`. Instances are the unit of node identity in the L6 graph editor; they also drive docker-aware runscript codegen at L2 (`sb init --docker`). A module without an `instances:` entry runs as a single un-suffixed process. Full schema: see [documents/sbcli_docker_runscript.md](documents/sbcli_docker_runscript.md) §2.

```yaml
modules:
  cam_gige_ht: /path/to/cam_gige_ht/sb.dev.yml

instances:
  cam_gige_ht:
    - id: left
      label: "Left Camera"
      docker:
        image: cam_gige_ht:latest
        env: { CAM_SERIAL: A123, LOG_LEVEL: info }
        devices: [/dev/video0]
        gpus: all
    - id: right
      docker:
        image: cam_gige_ht:latest
        env: { CAM_SERIAL: B456 }
        devices: [/dev/video1]
```

Edges between modules are **derived automatically** by matching publisher topic names to subscriber topic names. Never manually declared.

---

## Per-App Config Files

### `sb.dev.yml` — development time
Generated by `sb init` at the app's project root. **Not shipped with binary.** `sb 0.5.0+`: holds no host-specific path and is committed with the module; `sb install` and `sb io update` regenerate `swarmbotix_io/` from it.

```yaml
module: camera
language: python
io_dir: swarmbotix_io
publishers:
  - name: image_raw
    topic: image_raw
    type: ros2/std/ImageStamped
    transport: iceoryx2
  - name: status
    topic: status
    transport: zenoh
    encoding: raw
    mime: text/json
subscribers:
  - name: cmd_vel
    key: dev01/nav/teleop/zenoh/cmd_vel
    type: ros2/std/TwistStamped
    transport: zenoh
    encoding: fb
services:
  - name: calib
```

- `root:` *(optional, sb 0.5.0+)*: absent means the directory holding `sb.dev.yml`; relative is resolved against that directory; absolute is used as is (pre-0.5.0 files). `sb init` no longer writes it. Always resolved through `ModuleDevConfig::module_root`.
- `services:` *(sb 0.5.0+)*: list of `{ name }`; each renders `<io_dir>/services/<name>.<ext>` (Rust and Python).
- Per publisher / subscriber `encoding: proto | fb | raw` *(sb 0.5.0+)*, default `proto` (omitted on disk), zenoh only. `raw` has no `type:` and an optional `mime:` (e.g. `text/json`). Load errors, exact, prefixed `publishers[<i>]: ` or `subscribers[<i>]: `: `encoding: raw takes no type:`, `type: is required unless encoding: raw`, `mime: is only valid with encoding: raw`, `encoding: is only valid for transport: zenoh` (an explicit `encoding: proto` on iceoryx2 is accepted). The topic rules are checked first, then transport, then type, then mime.
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

---

## Module IO Folder Structure

Each app has a `swarmbotix_io/` IO folder (default name; user can override via `sb.dev.yml`'s `io_dir`). Contains generated pub/sub files using raw transport APIs.

```
camera_app/
  swarmbotix_io/            ← io_dir (default: swarmbotix_io/; user-overridable)
    publishers/
      image_raw.py          ← generated, uses raw zenoh/iceoryx2 API
    subscribers/
      cmd_vel.py            ← generated
    services/
      calib.py              ← generated router (sb 0.5.0+); user-edited, regenerated only on request
  sb.dev.yml
  sb.prd.yml          ← after sb gopro
  runscript.bash            ← stubbed by `sb init`, user fills in; sb uses to launch
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
`sb install` reads the manifest at the source's root (`package_kind`): `sb.app.yml` installs an app package (§App Packages), `flow.yaml` installs a workspace. Both: `<source> has both sb.app.yml and flow.yaml; a package is one or the other`; neither: `<source> has neither sb.app.yml (an app package) nor flow.yaml (a workspace package) at its root`. For a workspace:
- `<git-url>` (`@<ref>` pins): shallow clone into `--dir <path>`, else staged under `~/.swarmbotix/repos/` and moved to `~/.swarmbotix/repos/<name>/`, `<name>` being the flow's `name:` or else the URL's repository folder name. The same URL again (`source.url` match) fast-forwards that clone and reinstalls. `--dir` must be new, empty, or already a clone of the same URL (fast-forwarded); it is refused for a local folder and for an app. `<path>`: used in place, recording no URL; `sb install .` inside a clone is the common author form.
- `--force`: overwrite vault messages that differ from the package's copies; the bundled `ros2/std` set is never overwritten.
- Steps (`finish_workspace_install`), one stdout line each: `-- workspace <n>: created|updated, active`; `-- modules: <k> registered[, <j> with up: false (<names>)]` (a module without `sb.dev.yml` warns on stderr); per `messages:` entry `-- messages: <style> <- <path>: <i> imported, <u> unchanged, <c> conflicts; compiled [<backends>]` (`; not compiled` on a conflict), imported with `Vault::import_style_dir` into the configured vault (never a cwd-discovered one) and compiled with the entry's backends (or the style default); per module `-- io: <m> <w> written, <u> unchanged[, <s> skipped]` (`sb_pubsub::update_all`, services excluded; skipped entries warn); then `done. next: sb up` (`sb init <module dir>` when no modules).
- Conflicts fail the command after every step has run: `<n> message(s) differ from the vault copies and were not imported (their styles were not compiled); re-run with --force to overwrite the vault copies`.
- Idempotent; re-run after `git pull`.

#### `sb ws create <name>`
`sb 0.5.0+`: write `./flow.yaml` (`name: <name>` and `modules: {}`) and install it, which creates `~/.swarmbotix/workspaces/<name>/` and makes it active; a failed install removes `./flow.yaml` again. Refuses an existing `./flow.yaml` and an already installed `<name>`. Before 0.5.0 it created an empty installed copy only and did not change the active workspace.
- `<name>` (required) — workspace identifier, no spaces or slashes.

#### `sb ws install [<git-url | path>] [--dir <path>] [--force]` (`sb 0.5.0+`)
The workspace-only form of `sb install` (default `.`); an app package is refused with ``<source> is an app package (sb.app.yml), not a workspace; install it with `sb install <source>` ``.

#### `sb ws check [<name>]` (`sb 0.5.0+`)
Validate an installed workspace (default: the active one) with `sb_doctor::check_workspace`, rows in order: `workspace` (summary: module count and source; `[FAIL]` if the installed copy cannot be read), `ws modules` (`[FAIL]` per missing or unparseable `sb.dev.yml`), `ws types` (`[FAIL]` per proto/fb `type:` not in the configured vault nor an `iox2_variants` name), `ws instances` (`[FAIL]` per duplicate id), `ws encoding` (`[FAIL]` per wire key whose entries disagree on `encoding:`), `ws io files` (`[WARN]` per module with generated files missing), `ws source` (`[WARN]` when the repo `flow.yaml` is newer than the installed copy, `[SKIP]` without `source:`). Exit 1 on any `[FAIL]`. The same rows, for the active workspace, are part of `sb doctor`.

#### `sb ws update <name> [--force]` (`sb 0.5.0+`)
`git -C <repo dir> pull --ff-only` on the directory holding the installed copy's `source.repo_flow`, then reinstall (`sb_workspace::update_from_source`, then the remaining install steps). Requires `source.url`: a local-path install errors with `workspace "<n>" was installed from a local path (<flow>) with no URL; update that checkout yourself and install it again`; no `source:` errors with `workspace "<n>" was not installed from a repository flow.yaml`.

#### `sb ws uninstall <name> [--clean]` (`sb 0.5.0+`)
Remove `~/.swarmbotix/workspaces/<name>/` and clear the active marker if it named it (`uninstall_workspace`). `--clean` first deletes each module's io dir (`io_dir:` of its `sb.dev.yml`); an absolute or escaping `io_dir`, or a missing `sb.dev.yml`, is skipped and reported (`left <module>: <why>`). A clone `sb` made under `~/.swarmbotix/repos/` (URL recorded, folder under `repos/`) is deleted with `--clean` and otherwise reported as `left clone <path> (pass --clean to delete it)`; `--dir` clones and local folders are never deleted. Imported vault messages are left alone.

#### `sb ws set <name>`
Mark a workspace active by writing its name to `~/.swarmbotix/active`.
- `<name>` (required) — must already exist.

#### `sb ws list`
List all workspaces. The active one is prefixed with `*`. No arguments. `sb 0.5.0+`: each row is `<*| > <name>  <source>[  [stale]]`, the source being the URL or else the repository folder; `[stale]` when the repo `flow.yaml` is newer than the installed copy (`is_stale`, by modification time), with a footer ``[stale]: the repo flow.yaml changed after install; run `sb install <source>` ``. Empty: ``(no workspaces; run `sb ws create <name>` or `sb install <repo>`)``.

#### `sb ws delete <name>`
Remove `~/.swarmbotix/workspaces/<name>/`. If active, clears the `active` marker with a warning. Equivalent to `sb ws uninstall <name>` without `--clean`.
- `<name>` (required)

---

### Module init

#### `sb init [<rootpath>] [OPTIONS]`
Adopt an existing project directory as a swarmbotix module. sb does **not** create the project itself — the user already has a Rust crate, a Python project, a C++ tree, a Flutter project, or a Unity project. `sb init` just drops swarmbotix's bookkeeping files in and registers the module with the active workspace.

What sb writes into `<rootpath>` (only the missing pieces; existing files are preserved unless `--force`):
- `sb.dev.yml` — `module: <basename of rootpath>`, `language: <lang>`, `io_dir: swarmbotix_io/` (or `Assets/Scripts/swarmbotix_io/` for Unity), empty publishers/subscribers lists.
- `runscript.bash` — executable stub. Contents are a comment block explaining what the file is for plus per-language example commands; the body itself prints a TODO and exits non-zero until the user fills it in. An LLM agent pointed at the file + the project tree can typically complete it.
- `swarmbotix_io/` (or `Assets/Scripts/swarmbotix_io/` for Unity) — empty IO directory; L3 codegen fills it.

Then it registers the module. `sb 0.5.0+`: when a repo `flow.yaml` sits at or above `<rootpath>` (`find_flow_upwards`; never an installed copy, never `$HOME` itself unless the search starts there), the module is added there as `<module>: <relative/path>`, appended as one line under `modules:` so comments survive (falling back to a full rewrite when `modules:` is a flow mapping), and the repo flow is reinstalled, keeping a previously recorded URL. A same-named entry pointing at another existing module is an error (`module "<m>" is already registered in <flow> for <path>; module names must be unique within a workspace`); one pointing at a missing module is repointed, keeping `up:`. Otherwise it inserts `module → /abs/path/<rootpath>/sb.dev.yml` into the active workspace's installed `flow.yaml`, as before. Idempotent: re-running with the same rootpath does not duplicate the registration. `sb.dev.yml` is written without `root:` (sb 0.5.0+).

Arguments:
- `<rootpath>` (default: `.`) — project root to adopt. For Unity, this is the project root that contains `Assets/`. Module name is `basename(rootpath)`.
- `--rust | --python | --cpp | --flutter | --unity` — language tag written to `sb.dev.yml` and used to pick `runscript.bash` example commments. **Required** when no `sb.dev.yml` exists yet at `<rootpath>`. **Optional** when one already exists; if given, must match the existing `language:` tag (else error).
- A repo `flow.yaml` above `<rootpath>` or an active workspace (required): with neither, `sb init` errors with ``no active workspace and no flow.yaml above <root>; run `sb ws create <name> && sb ws set <name>` first, or add a flow.yaml at the repository root`` (sb 0.5.0+ text).
- `--force` (default: refuse to overwrite) — overwrite an existing `sb.dev.yml` / `runscript.bash` at `<rootpath>`.
- `--docker`: emit a docker-flavored `runscript.bash` whose `--id <name>` branches dispatch to one pre-resolved `docker run` invocation per instance declared in `flow.yaml::instances.<module>` (`sb 0.5.0+`: read from the repo `flow.yaml` when the module sits under one, else from the active workspace's). Preflight requires the block to exist and each instance to have a `docker.image`. See [documents/sbcli_docker_runscript.md](documents/sbcli_docker_runscript.md) for the per-language in-container entry-point.

---

### Pub/Sub

`sb 0.5.0+`: `sb pub`, `sb sub`, and `sb list` are deprecated in favour of `sb io` (§Module IO); they keep working unchanged and receive no new features.

`-m` is reserved for `MsgType`. Module selection uses the long-only flag `--module`.

#### Module resolution (highest priority first)
Shared by `sb io`, `sb pub`, `sb sub`, `sb list`, and `sb service` (`sb_pubsub::resolve_module`):
1. `--module <name>` and `./<name>/sb.dev.yml` exists → that path
2. `--module <name>` and the active workspace's installed `flow.yaml` has it → that path
3. No `--module` → `./sb.dev.yml` in cwd
4. Otherwise → error with the paths searched

The module root is then `ModuleDevConfig::module_root` of that file (`root:` absent → the file's directory).

After every mutation: updates `sb.dev.yml` **and** regenerates the language file under `<io_dir>/publishers/<name>.<ext>` (or `subscribers/`). `sb 0.5.0+`: a write also rewrites every legacy `topic: /...` entry in the file as `key:` and prints `  rewrote N legacy topic entry|entries as key: in <path>`.

`sb 0.5.0+` load errors for an entry (prefixed `publishers[<i>]: ` or `subscribers[<i>]: `): `exactly one of topic: or key: is required (both given)`, `exactly one of topic: or key: is required (neither given)`, `topic "a/b" may not contain '/'; use key: for a verbatim wire key` (for a legacy `topic: a/b`, followed by a parenthetical naming `key: <device>/<ws>/<module>/<transport>/a/b`), `key: is empty`, and the encoding rules: `encoding: raw takes no type:`, `type: is required unless encoding: raw`, `mime: is only valid with encoding: raw`, `encoding: is only valid for transport: zenoh`.

#### `sb list [--module <name>]`
Render a table of all publishers and subscribers for the resolved module.
- `--module <name>` (default: cwd) — target module.

#### `sb pub list [--module <name>]`
Same as `sb list` but filters to publishers only.

#### `sb pub add -m <MsgType> (<topic> | --key <WIRE_KEY>) [OPTIONS]`
Add a publisher.
- `<topic>` (required unless `--key`): a bare name (`image_raw`), stored as `topic:`. `sb 0.5.0+`: may not contain `/`; a slashed value errors with `topic "/dev01/ws/mod/iox2/hello" may not contain '/'; for a verbatim wire key use --key dev01/ws/mod/iox2/hello`.
- `--key <WIRE_KEY>` (`sb 0.5.0+`; conflicts with `<topic>`): a verbatim wire key (`dev01/ws/cam/iox2/image_raw`), stored as `key:` and used as is; a leading `/` is dropped. `sb` injects no `<transport>` segment, so the key must carry the one the flag implies.
- `-m, --msg <MsgType>` (required) — message type from vault, e.g. `std/ImageStamped`.
- `-n, --name <name>` (default: the bare `<topic>`, or the last segment of `--key`): identifier used in `sb.dev.yml` and as the generated filename.
- `--iox2 | --zenoh` (default: from `sb.config.yml` — `transport_on_device` for same-device, `transport_cross_device` otherwise) — transport.
- `--module <name>` (default: cwd) — target module.

#### `sb pub edit <name> [OPTIONS]`
Modify an existing publisher in-place. At least one of `-m` / `-t` / `--key` is required; with none, it errors `at least one of -m/--msg, -t/--topic, or --key is required`.
- `<name>` (required) — existing publisher identifier.
- `-m, --msg <MsgType>` (optional) — change message type.
- `-t, --topic <topic>` (optional): change to an owned bare topic (`topic:`). `sb 0.5.0+`: bare only, may not contain `/`.
- `--key <WIRE_KEY>` (optional, `sb 0.5.0+`; conflicts with `-t`): change to a verbatim wire key (`key:`); a leading `/` is dropped.
- `--module <name>` (default: cwd) — target module.

#### `sb pub rm <name> [--module <name>]`
Remove a publisher and its generated file.
- `<name>` (required)
- `--module <name>` (default: cwd)

#### `sb sub list [--module <name>]`
List subscribers only.

#### `sb sub add -m <MsgType> (<topic> | --key <WIRE_KEY>) [OPTIONS]`
Add a subscriber.
- `<topic>` (required unless `--key`): a bare name, as for `sb pub add` (`sb 0.5.0+`: may not contain `/`).
- `--key <WIRE_KEY>` (`sb 0.5.0+`; conflicts with `<topic>`): a verbatim wire key, as for `sb pub add`. The way to subscribe to another module's publisher.
- `-m, --msg <MsgType>` (required) — message type from vault, e.g. `std/StringStamped`. Mirrors `pub add` for consistency.
- `--iox2 | --zenoh` (default: from `sb.config.yml`) — transport.
- `--module <name>` (default: cwd) — target module.

#### `sb sub edit <topic-or-key> [--module <name>]`
Re-render the subscriber whose `topic:` or `key:` equals `<topic-or-key>` from current `sb.dev.yml` (useful after `sb.config.yml` defaults change). `sb 0.5.0+`: a leading `/` in the argument is ignored; a miss errors with `subscriber for topic/key "<x>" not found on module "<m>"`.

#### `sb sub rm <topic-or-key> [--module <name>]`
Remove a subscriber and its generated file. Matches `<topic-or-key>` as `sb sub edit` does.

---

### Module IO (`sb io`, `sb 0.5.0+`)

Replaces `sb pub`, `sb sub`, `sb list`, and `sb service init`. Same `sb.dev.yml`, same templates, same module resolution (above); publishers, subscribers, and services share one name space per module. Shipped doc: `documents/sbcli_pubsub.md` §14.

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

#### `sb io add service <name>`
Append `{ name }` to `services:` and render `<io_dir>/services/<name>.<ext>` (Rust, Python). Base key `<device>/<workspace>/<module>/zenoh/service/<name>`; a service named `service` uses the bare `.../zenoh/service` base (the default service).

#### `sb io edit <name> [-m <Type>] [-t <topic> | --key <WIRE_KEY>] [--proto | --fb | --raw [<mime>]] [--module <name>]`
Edit a publisher or subscriber by name; with no flag, only regenerate its file. Switching to raw drops `type:`; switching away from raw drops `mime:` and needs `-m`. No transport change (rm + add). Re-renders the file, writes `sb.dev.yml`. A service: `services have no editable fields; edit the file in <io_dir>/services`. Unknown name: `no publisher, subscriber, or service named "<n>" on module "<m>"`.

#### `sb io rm <name> [--module <name>]` / `sb io list [--module <name>]` (alias `ls`)
`rm`: remove the entry of any kind and delete its generated file. `list`: the `sb list` table over publishers, subscribers, and services; read-only.

#### `sb io update [<module> | --all] [--services]`
Regenerate `swarmbotix_io/` from `sb.dev.yml` without changing it (`sb_pubsub::update_all`). Targets `<module>` (resolved as `--module`), else the cwd module when `./sb.dev.yml` exists, else (or with `--all`) every module of the active workspace's installed `flow.yaml`, `up: false` included; an active workspace is required in every form. One line per module, `<module>: <w> written, <u> unchanged, <s> skipped[ (<entry>: <why>; ...)]`. A file whose rendered bytes match is `unchanged` and untouched; an entry that cannot render is `skipped` with its reason and the rest continue; any skip or module failure exits 1 (`<n> entr(y/ies) or module(s) could not be updated`). Services only with `--services` (their files are user-edited). The only `sb.dev.yml` write is rewriting legacy `topic: /...` entries as `key:`.

**Generated Zenoh wrappers** (`sb-codegen`): wire tag (`wire_tag`) `application/protobuf;<type>` for proto, `application/flatbuffers;<type>` for fb, the `mime:` (else `application/octet-stream`) for raw, exported as `WIRE_TAG`. Proto wrappers get a typed `publish(msg)` plus `publish_bytes(...)` and a typed receive (`try_recv_latest_msg`) in Rust, Python, and C++, in addition to the bytes API; fb and raw are bytes-only; Dart and C# are bytes-only and set no tag.

---

### Services (request/response)

REST/FastAPI-style request-response over Zenoh queryables, JSON payloads only.
Independent of pub/sub. Full reference: [documents/sbcli_reqres.md](documents/sbcli_reqres.md).

`sb 0.5.0+`: a service is a `services:` entry in `sb.dev.yml` (`sb io add service <name>`), rendered to `<io_dir>/services/<name>.<ext>` and served under `<device>/<workspace>/<module>/zenoh/service/<name>/<route>`. `sb service init` below is deprecated in its favour.

#### `sb service init [--module <name>]` (deprecated, `sb 0.5.0+`)
Copy the static service router into the resolved module's io dir as
`service.<ext>` (Rust/Python only), with the module's `device`/`workspace`/`module`
namespace filled in. Re-run to refresh; overwrites the file. The user then
defines routes against it (`@app.get("/path")` / `app.get("/path", handler)`),
served on `<device>/<workspace>/<module>/zenoh/service/<route>`. Clients call a
route with a plain Zenoh `get` (no codegen).

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

`sb 0.4.1+`: a Zenoh row with neither a Header `msg_type` nor an encoding schema takes its `schema` from the owning module's `sb.dev.yml`, by the same lookup as `sb topic listen`'s `"config"` source below (`fill_config_schemas` → `sb_discover::config_schema` on the row's topic). Any miss (no active workspace, unregistered module, unreadable `sb.dev.yml`, no matching entry) leaves the row blank. Table and JSON shapes are unchanged.

`sb 0.4.2+`: the Header probe behind the `schema` and `rate_hz` columns (`sb_discover::probe_header`, shared with `sb topic listen`'s `"header"` source) reads both Header layouts. `swarmbotix/header/Header` carries `metadata` at tag 4 (tag 3 is `uint64 seq`); `ros2/std/Header` carries it at tag 3. `HeaderProbe` implements `prost::Message` by hand to tell tag 3 apart by wire type (length-delimited is `metadata`, varint is `seq`) and prefers tag 4. Before 0.4.2 only tag 3 was read, so a swarmbotix-layout Header left both columns to the fallbacks: its tag-4 metadata was skipped, and a non-zero `seq` failed the decode outright.

#### `sb topic listen <topic> [OPTIONS]`
Subscribe and print frames as JSON lines.
- `<topic>` (required): bare names resolved via cwd's `sb.dev.yml`. `sb 0.4.2+`: any argument containing a `/` is fully qualified; before 0.4.2 only a leading `/` marked one. `sb 0.5.0+`: a leading `/` is accepted and dropped (0.4.2 prepended one when missing). An argument without a `/` is a bare name.
- `-t, --transport zenoh | iceoryx2` (default: inferred, below): which transport to subscribe on. Aliases `z` (zenoh), `i` and `iox2` (iceoryx2).
- `--raw` (default: decode via vault) — emit hex bytes; payload becomes `{kind: "hex", bytes: "…"}`.
- `-n, --count <N>` (default: `0` — run until SIGINT) — stop after N frames.
- `--type <style>/<ns>/<Leaf>` (default: none): decode every frame as this vault message, overriding the wire schema and the Header probe.
- `--no-data` (default: off): drop `bytes` field contents and keep only `<name>_len`.
- `--show` (default: off): open a window and draw image frames (below). Conflicts with `--raw`.

`--type`, `--no-data`, `--show`, the `schema_source` key, the `bytes` summary, the wire-schema source, and typed iceoryx2 opens are `sb 0.4.0+`. The `-t` short, the transport aliases, transport inference, and the `"config"` source are `sb 0.4.1+`; before 0.4.1, `--transport` defaulted to `zenoh`.

Transport, when `-t` is absent (`pick_transport`): the `<transport>` segment of a fully-qualified `<device>/<ws>/<module>/<zenoh|iox2>/<topic>`, or, for a bare topic, the transport of the cwd `sb.dev.yml` entry it resolved to. There is no default. A fully-qualified path not in that five-segment form, without `-t`, exits 1 with
`error: cannot infer transport from <topic>: expected <device>/<ws>/<module>/<zenoh|iox2>/<topic>, or pass -t zenoh|iox2` (`sb 0.5.0+` text; e.g. `cannot infer transport from not/canonical: ...`).
A `-t` that disagrees with the path wins, with a stderr note such as `note: path says zenoh, --transport says iceoryx2; using iceoryx2`.

Output: one JSON line per frame, keys in this order: `transport`, `topic`, `schema`, `schema_source`, `hash`, `encoding`, `payload`. `topic` is the wire key without a leading `/` (`sb 0.5.0+`), as are the `--show` window title and the stderr warnings that name the topic. `schema_source` is `"flag"`, `"wire"`, `"header"`, `"config"` (`sb 0.4.1+`), or `null` (exactly when `schema` is `null`). `encoding` is `"protobuf"`, `"iox2"` (an iceoryx2 `#[repr(C)]` struct), or `null`; `sb 0.5.0+` adds `"flatbuffers"` and `"text"` on Zenoh. `payload` is `{kind: "json", json: {…}}` or `{kind: "hex", bytes: "…"}`, and `sb 0.5.0+` adds `{kind: "text", text: "…"}`.

Zenoh wire tags (`sb 0.5.0+`, beyond protobuf; only `--type` overrides them): `application/flatbuffers;<type>` names the type (`schema_source: "wire"`) and decodes through `flatc --json --raw-binary --strict-json` against `<message_targets>/fb/<ns>/<Leaf>/<Leaf>.bfbs` when that file and `flatc` exist, else hex with `schema` set and a one-time stderr note (`encoding: "flatbuffers"` either way); `text/*` and `application/json` print `payload: {kind: "text", text: …}` with the mime as `schema` (`schema_source: "wire"`, `encoding: "text"`; a body that is not UTF-8 prints hex); `application/octet-stream` adds nothing, so the Header probe and `"config"` sources apply as for an untagged sample. `hash` is reserved and always `null`. A `bytes` field longer than 64 bytes prints as `{len, head_hex}` (first 16 bytes).

Type resolution, per frame, first hit wins:
1. `--type` → `"flag"`.
2. The transport's wire schema → `"wire"`. Zenoh: the sample encoding `application/protobuf;<style>/<ns>/<Leaf>`, set by generated Rust, Python, and C++ Zenoh publishers (Flutter and Unity set none). iceoryx2: the payload type name the service was registered with, mapped to the vault iox2 struct with the same leaf and the live payload's size.
3. The Header probe (`metadata.msg_type`, either Header layout `sb 0.4.2+`, see `sb topic list`) → `"header"`.
4. The topic's entry in the module's `sb.dev.yml` → `"config"` (`sb 0.4.1+`; `ResolveOutcome::schema`). The entry's `type` is used verbatim. Bare topic: the cwd module's matched entry. Fully-qualified: `<module>` is looked up in the active workspace's `flow.yaml` (`active_module_registry`), as written, then without its `-<instance>` suffix (everything from the first `-`; `ips_track-sb00` → `ips_track`); publishers then subscribers, the first entry whose transport equals the path's and whose `topic:` equals the path's last segment or whose `key:` equals the whole path (`sb_discover::config_schema`). Only the five-segment form is looked up, so a four-segment legacy key such as `dev01/demo/pubber/hello` never matches. The cwd `sb.dev.yml` is never consulted for a fully-qualified path. Any miss (no active workspace, unregistered module, unreadable `sb.dev.yml`, no entry, path not in the five-segment form) skips the source silently. On iceoryx2 a typed service always supplies a wire type first, so the source matters only for an untyped `[u8]` service; there, as with `--type`, the name matches no iox2 layout (the element is one byte), so the bytes are tried as protobuf, else hex with `schema` kept.

No hit: hex, `schema: null`, and a one-time stderr hint to re-render the publisher (`sb pub edit <name> -m <type>`) or pass `--type`. A resolved type that does not decode prints hex with `schema` kept. `--raw` skips resolution entirely.

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

Each module is launched by executing its `runscript.bash` (or `stopscript.bash`) in a dedicated tmux/psmux pane. Session name: `sb-<active_workspace>`. One window per module — `sb run` and `sb stop` both target the same window, idempotently swapping in whichever script the verb implies.

#### `sb up`
Launch every module in the active workspace's `flow.yaml`. One tmux pane per module. Requires an active workspace. No per-module passthrough args; use `sb run <module> [args...]` for that. `sb 0.5.0+`: skips modules with `up: false`, erroring with ``workspace "<ws>" has no modules to launch: every module in flow.yaml has `up: false` (use `sb run <module>` to start one)`` when that leaves none; warns on stderr (and continues) when the workspace's repo `flow.yaml` is newer than the installed copy (`sb_workspace::is_stale`). `sb run` gives the same warning and ignores `up:`.

#### `sb run <module> [args...]`
Launch a single module in the existing `sb-<workspace>` session (creating it if absent). Re-running the same module kills and restarts its pane (no duplicates). All args after `<module>` are forwarded verbatim to `runscript.bash` — use `--` to disambiguate args that start with `-` from `sb`'s own flags.
- `<module>` (required) — must be in active workspace's `flow.yaml`.
- `[args...]` (optional) — appended verbatim to the runscript's argv. The module's path is resolved via `flow.yaml`, so `sb run` works from any cwd.

```bash
sb run cam_gige_ht                 # default args
sb run cam_gige_ht --id sb02       # passthrough: ./runscript.bash --id sb02
sb run cam_gige_ht -- --foo --bar  # `--` disambiguates if flags clash with sb's
```

#### `sb stop <module> [args...]`
Stop a single module by executing its `stopscript.bash` in the same tmux session — symmetric counterpart of `sb run` (same module resolution, same window, idempotent respawn).
- `<module>` (required) — must be in active workspace's `flow.yaml`.
- `[args...]` (optional) — appended verbatim to the stopscript's argv.
- Errors clearly if the module has no `stopscript.bash` next to its `sb.dev.yml`. (`sb init` writes a runscript stub but no stopscript stub — author one if you want a clean teardown.)

#### `sb down`
Kill `sb-<workspace>` and all its panes. No arguments. Coarser than `sb stop`: tears down the whole workspace session in one go without running any per-module stopscripts.

#### `sb attach`
`tmux attach -t sb-<workspace>`. No arguments.

---

### Production

#### `sb gopro [--all] [--module <name>]`
Export `sb.dev.yml` + `<io_dir>/` to `sb.prd.yml` (sanitized, language-agnostic, ships with binary). Strips `root:`, absolute paths, and other host-specific fields.
- Default target: cwd's module.
- `--module <name>` (default: cwd) — target module.
- `--all` (default: single module) — export every module in the active workspace.

---

### App Packages

An **app package** is a folder with an `sb.app.yml` at its root. It is not a module: it is a foreground tool (calibration, converter, generator) that users run as `sb <name> [args...]` after `sb install`, with no change to `PATH`. Apps are global to the host and independent of workspaces. Shipped doc: `documents/sbcli_app.md`.

#### `sb app init [path] [--name <n>] [--entry <file>] [--kind docker|host|none] [--image <img>] [--description <t>] [--force]`
Write a fully commented `sb.app.yml` into `path` (default cwd) plus executable stubs for a missing entry and, for `--kind host`, a missing `install.bash`. Prompts for name / entry / kind / image on a TTY when `--kind` is absent; otherwise kind defaults to `none` (or `docker` when `--image` is given). Name defaults to the sanitised folder name and must pass `validate_identifier` and not be an `sb` built-in. The manifest is validated before being written. Existing scripts are never overwritten; `sb.app.yml` only with `--force`.

#### `sb install <url|path> [--prefetch]` (alias `sb app install`)
- `sb 0.5.0+`: the source's root manifest decides: `sb.app.yml` → app (below); `flow.yaml` → workspace (§Workspaces, `sb install`); both or neither → error.
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
docker: { image: swarmbotix/sb_kalibr:latest, prefetch: false }   # kind: docker
host:   { install: ./install.bash }                               # kind: host
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

### Web UI

#### `sb ui [OPTIONS]`
Start the `swarmctl` server in a tmux pane and open the browser. Idempotent — re-running reuses the existing pane.
- `--port <p>` (default: `7878`) — bind port.
- `--no-browser` (default: opens browser) — start server only.

---

### Doctor

#### `sb doctor`
Print `sb <version>`, then read merged `sb.config.yml` and run twelve checks, in this order:
1-2. Executables (`--version` succeeds): `protoc`, `flatc`.
3. `transports` — the zenoh and iceoryx2 **crate** versions linked into this binary, baked in from the workspace lockfile by `sb-doctor`'s build script (`SB_ZENOH_VERSION` / `SB_ICEORYX2_VERSION`; `unknown` when the build tree had no lockfile). Reads nothing from the config and never fails.
4-5. Shared libraries (file exists + `dlopen` succeeds): `libzenohc.so`, `libiceoryx2_ffi_c.so`. Each also reports the install's own version, probed in order from `<libdir>/pkgconfig/*.pc`, `<libdir>/cmake/*/…ConfigVersion.cmake`, a version component of the canonicalized path, then a `.so.X.Y.Z` suffix; `version unknown` when every probe misses. A version that disagrees with check 3 **warns**, never fails: zenoh on differing `major.minor`, iceoryx2 on any differing digit (segments are opened on exact version equality).
6. `tmux` (or `psmux`), probed with `-V`, not `--version`.
7. `message styles` — one row per style under the resolved root with its `.proto` count and whether its targets are built. Fails only when the root holds no styles at all.
8. `unity targets` — stale generated C# inside a Unity project (see §Generated-code language levels). **Warns**, never fails.
9. `config keys` — the dead `message_style` / `message_definitions` / `message_targets` keys, if still present.
10. `std vault` — `~/.swarmbotix/messages/ros2/message_definitions/std/` populated; names `sb message list` as the re-install path.
11. `topics` (`sb 0.5.0+`): legacy `topic: /...` entries in the modules of the active workspace. `` [OK]   topics       no legacy `topic: /...` entries in the active workspace ``; `[SKIP] topics       no active workspace`; otherwise one `[WARN] topics` row per offending module, naming the module, then a continuation line per entry with both forms, then the rewrite command. **Warns**, never fails.
12. `workspace` and `ws *` (`sb 0.5.0+`): the `sb ws check` rows for the active workspace (`workspace`, `ws modules`, `ws types`, `ws instances`, `ws encoding`, `ws io files`, `ws source`, see §Workspaces). `[SKIP] workspace no active workspace` without one. A `[FAIL]` here **counts toward the exit code** (it breaks `sb up` or codegen); `ws io files` and `ws source` only warn.

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
- Reads `sb.dev.yml` or `sb.prd.yml` per module via `flow.yaml`
- Draws module boxes with ports (publishers = output, subscribers = input)
- Derives edges by matching topic names across modules
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
sb-core (shared logic: naming, flow.yaml R/W, config R/W)
  ↓
flow.yaml + sb.dev.yml + io_dir files
```

Three surfaces, one core:

| Surface | For |
|---|---|
| CLI (`sb`) | Humans, scripts, CI |
| REST | Web UI, curl, dashboards |
| MCP | AI agents (Cursor, Claude, etc.) |

---

## tmux / psmux

`sb up` creates a named tmux session with one pane per module.

| Platform | Tool | Install |
|---|---|---|
| Linux / macOS | tmux | `apt/brew install tmux` |
| Windows | psmux | `winget install psmux` |

psmux is tmux-compatible — ships as the `tmux` command on Windows. `sb` calls `tmux` on all platforms.

---

## Non-Goals

- `sb` does not wrap Zenoh or Iceoryx2
- `sb` does not build your app (`sb build` does not exist)
- `sb` does not manage your app's dependencies
- `sb` does not dictate your app's internal folder structure
- `sb` does not impose a message protocol — Protobuf is the default, others are optional
- `sb` does not require a running daemon — swarmctl starts on demand
