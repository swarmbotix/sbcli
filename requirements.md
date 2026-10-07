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
/<device>/<workspace>/<module>/<transport>/<topic>
```

Example: `/dev01/perception/camera/iox2/image_raw` (or `.../zenoh/image_raw`)

- `device` — identifies the machine
- `workspace` — the active swarmbotix workspace
- `module` — the app/module name
- `transport` — literal `iox2` or `zenoh`, picked from the `--iox2` / `--zenoh` flag at `sb pub/sub add` time
- `topic` — the pub/sub channel name

The transport segment lets the same bare name (e.g. `image_raw`) coexist on both transports for the same module without collision, and makes the transport visible directly in the key. `sb` enforces this naming automatically. Users never type the full path.

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
    <workspace_name>/    ← one directory per workspace
      sb.config.yml      ← optional per-workspace override
      flow.yaml          ← module registry
  apps.yml               ← installed app packages: name → path, source, commit, version
  update-check.json      ← cache for the `sb --version` update notice: latest, checked_at
  apps/<name>/           ← clones made by `sb install <git-url>`; sb owns and deletes these
```

### Per-app state — `<module_root>/`

```
camera_app/
  sb.dev.yml       ← dev-time interface (gitignored — host-specific paths)
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

Workspaces isolate projects. Analogous to Python virtual environments. Each workspace is a real directory at `~/.swarmbotix/workspaces/<name>/`.

```bash
sb ws create workspace1   # mkdir ~/.swarmbotix/workspaces/workspace1, init flow.yaml
sb ws set    workspace1   # write workspace1 to ~/.swarmbotix/active
sb ws list                # list directories under ~/.swarmbotix/workspaces/
sb ws delete workspace1   # rm -rf ~/.swarmbotix/workspaces/workspace1
```

**`flow.yaml`** at `~/.swarmbotix/workspaces/<workspace>/flow.yaml` — the module registry. Minimal:

```yaml
modules:
  camera:   /path/to/camera/sb.dev.yml
  detector: /path/to/detector/sb.dev.yml
  tablet:   /path/to/tablet/sb.prd.yml
```

`flow.yaml` stores two things:
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
Generated by `sb init` at the app's project root. **Not shipped with binary.** May contain absolute paths (machine-specific — add to `.gitignore`).

```yaml
module: camera
root: /home/user/projects/camera_app
io_dir: swarmbotix_io/
```

### `sb.prd.yml` — production / runtime
Generated by `sb gopro`. **Ships with the binary.** Language-agnostic static interface definition.

```yaml
module: camera
publishers:
  - name: image_raw
    type: ImageMsg
    transport: iceoryx2
subscribers:
  - name: cmd_vel
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

#### `sb ws create <name>`
Create a workspace at `~/.swarmbotix/workspaces/<name>/` with an empty `flow.yaml`.
- `<name>` (required) — workspace identifier, no spaces or slashes.

#### `sb ws set <name>`
Mark a workspace active by writing its name to `~/.swarmbotix/active`.
- `<name>` (required) — must already exist.

#### `sb ws list`
List all workspaces. The active one is prefixed with `*`. No arguments.

#### `sb ws delete <name>`
Remove `~/.swarmbotix/workspaces/<name>/`. If active, clears the `active` marker with a warning.
- `<name>` (required)

---

### Module init

#### `sb init [<rootpath>] [OPTIONS]`
Adopt an existing project directory as a swarmbotix module. sb does **not** create the project itself — the user already has a Rust crate, a Python project, a C++ tree, a Flutter project, or a Unity project. `sb init` just drops swarmbotix's bookkeeping files in and registers the module with the active workspace.

What sb writes into `<rootpath>` (only the missing pieces; existing files are preserved unless `--force`):
- `sb.dev.yml` — `module: <basename of rootpath>`, `language: <lang>`, `io_dir: swarmbotix_io/` (or `Assets/Scripts/swarmbotix_io/` for Unity), empty publishers/subscribers lists.
- `runscript.bash` — executable stub. Contents are a comment block explaining what the file is for plus per-language example commands; the body itself prints a TODO and exits non-zero until the user fills it in. An LLM agent pointed at the file + the project tree can typically complete it.
- `swarmbotix_io/` (or `Assets/Scripts/swarmbotix_io/` for Unity) — empty IO directory; L3 codegen fills it.

Then it appends `module → /abs/path/<rootpath>/sb.dev.yml` to the active workspace's `flow.yaml`. Idempotent: re-running with the same rootpath does not duplicate the registration.

Arguments:
- `<rootpath>` (default: `.`) — project root to adopt. For Unity, this is the project root that contains `Assets/`. Module name is `basename(rootpath)`.
- `--rust | --python | --cpp | --flutter | --unity` — language tag written to `sb.dev.yml` and used to pick `runscript.bash` example commments. **Required** when no `sb.dev.yml` exists yet at `<rootpath>`. **Optional** when one already exists; if given, must match the existing `language:` tag (else error).
- Active workspace (required) — set via `sb ws set` first; `sb init` without one errors with an actionable message.
- `--force` (default: refuse to overwrite) — overwrite an existing `sb.dev.yml` / `runscript.bash` at `<rootpath>`.
- `--docker` — emit a docker-flavored `runscript.bash` whose `--id <name>` branches dispatch to one pre-resolved `docker run` invocation per instance declared in `flow.yaml::instances.<module>`. Preflight requires the block to exist and each instance to have a `docker.image`. See [documents/sbcli_docker_runscript.md](documents/sbcli_docker_runscript.md) for the per-language in-container entry-point.

---

### Pub/Sub

`-m` is reserved for `MsgType`. Module selection uses the long-only flag `--module`.

#### Module resolution (highest priority first)
1. `--module <name>` and active workspace's `flow.yaml` has it → that path
2. `--module <name>` and `./<name>/sb.dev.yml` exists → that path
3. No `--module` → `./sb.dev.yml` in cwd
4. Otherwise → error with the paths searched

After every mutation: updates `sb.dev.yml` **and** regenerates the language file under `<io_dir>/publishers/<name>.<ext>` (or `subscribers/`).

#### `sb list [--module <name>]`
Render a table of all publishers and subscribers for the resolved module.
- `--module <name>` (default: cwd) — target module.

#### `sb pub list [--module <name>]`
Same as `sb list` but filters to publishers only.

#### `sb pub add -m <MsgType> <topic> [OPTIONS]`
Add a publisher.
- `<topic>` (required) — bare (`image_raw`) or fully-qualified (`/dev01/ws/cam/iox2/image_raw`). When fully-qualified, the `<transport>` segment must match the `--iox2` / `--zenoh` flag.
- `-m, --msg <MsgType>` (required) — message type from vault, e.g. `std/ImageStamped`.
- `-n, --name <name>` (default: derived from last segment of `<topic>`) — identifier used in `sb.dev.yml` and as the generated filename.
- `--iox2 | --zenoh` (default: from `sb.config.yml` — `transport_on_device` for same-device, `transport_cross_device` otherwise) — transport.
- `--module <name>` (default: cwd) — target module.

#### `sb pub edit <name> [OPTIONS]`
Modify an existing publisher in-place. At least one of `-m` / `-t` is required.
- `<name>` (required) — existing publisher identifier.
- `-m, --msg <MsgType>` (optional) — change message type.
- `-t, --topic <topic>` (optional) — change topic.
- `--module <name>` (default: cwd) — target module.

#### `sb pub rm <name> [--module <name>]`
Remove a publisher and its generated file.
- `<name>` (required)
- `--module <name>` (default: cwd)

#### `sb sub list [--module <name>]`
List subscribers only.

#### `sb sub add -m <MsgType> <topic> [OPTIONS]`
Add a subscriber.
- `<topic>` (required) — bare or fully-qualified.
- `-m, --msg <MsgType>` (required) — message type from vault, e.g. `std/StringStamped`. Mirrors `pub add` for consistency.
- `--iox2 | --zenoh` (default: from `sb.config.yml`) — transport.
- `--module <name>` (default: cwd) — target module.

#### `sb sub edit <topic> [--module <name>]`
Re-render the subscriber for `<topic>` from current `sb.dev.yml` (useful after `sb.config.yml` defaults change).

#### `sb sub rm <topic> [--module <name>]`
Remove a subscriber and its generated file.

---

### Services (request/response)

REST/FastAPI-style request-response over Zenoh queryables, JSON payloads only.
Independent of pub/sub. Full reference: [documents/sbcli_reqres.md](documents/sbcli_reqres.md).

#### `sb service init [--module <name>]`
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
- `--iox2` / `--proto` / `--fb` — emit these backends explicitly. An explicit flag naming a missing tool errors rather than being silently dropped.
- `-o, --out <dir>` — relocate the generated `iox2/` / `proto/` / `fb/` trees. Defaults to `<messages_root>/<style>/message_targets/`, the sibling of `message_definitions/`. `.cache/descriptor.bin` and the `.proto` sources never move.
- `--string-cap` / `--bytes-cap` / `--vec-cap` / `--string-array-cap <N>` — iox2-only capacity overrides for this invocation; they beat every config-derived default.

---

### Topic Introspection

Snapshot-based. No rolling sampler. Output layout mirrors the `swarmbotix_old/sb-introspect` prototype.

#### `sb topic list [OPTIONS]`
Enumerate currently-active topics on either transport.
- `-t, --timeout <sec>` (default: `0.5`) — Zenoh discovery window. iceoryx2 uses `Service::list` and ignores the window.
- `-k, --keyword <str>` (default: none) — substring filter on topic name.
- `--case-sensitive` (default: insensitive) — toggle keyword case.
- `--transport zenoh | iceoryx2 | all` (default: `all`) — restrict discovery.
- `--json` (default: pretty table) — one JSON line per topic instead. JSON keys: `topic`, `transport`, `schema`, `hash`, `rate_hz`, `publisher_pid`, `is_dead`.

Table columns: `[transport-tag] rate Hz  PID <n>[ (dead)]  schema  topic`. The PID column is populated for iceoryx2 via `Service::list` → `dynamic_details.nodes` → `UniqueNodeId::pid()` — alive owner shown as `PID <n>`, stale on-disk service whose owning process is gone shown as `PID <n> (dead)`. Zenoh entries render `-` in the PID column (the wire format does not yet carry publisher PID).

#### `sb topic listen <topic> [OPTIONS]`
Subscribe and print frames as JSON lines.
- `<topic>` (required) — bare names resolved via cwd's `sb.dev.yml`.
- `--transport zenoh | iceoryx2` (default: `zenoh`) — which transport to subscribe on.
- `--raw` (default: decode via vault) — emit hex bytes; payload becomes `{kind: "hex", bytes: "…"}`.
- `-n, --count <N>` (default: `0` — run until SIGINT) — stop after N frames.

#### `sb topic pub <topic> <bytes-hex> [--transport zenoh | iceoryx2]`
Publish raw bytes once.
- `<topic>` (required)
- `<bytes-hex>` (required) — hex string (no `0x` prefix).
- `--transport zenoh | iceoryx2` (default: `zenoh`)

#### `sb topic prune [--json]`
Remove iceoryx2 on-disk node registrations whose owning process is gone — the entries `sb topic list` flags `(dead)`. Walks `Node::list` for the global iceoryx2 config and calls `try_remove_stale_resources()` on every `NodeState::Dead`. Zenoh is session-based and has no on-disk state to prune, so this verb is iceoryx2-only.
- `--json` (default: human table) — emit one JSON object `{cleaned_pids: [u32], failed: [[pid, reason], …]}`.

Human output: one `Pruned <N> dead iceoryx2 node(s):` block listing the cleaned PIDs, plus a `warning: failed to prune PID <n>: <reason>` line per failure on stderr. Exit code is non-zero iff any node failed to clean (insufficient permissions, version mismatch). When nothing is stale, prints `No dead iceoryx2 nodes found.` and exits 0.

---

### Launch

Each module is launched by executing its `runscript.bash` (or `stopscript.bash`) in a dedicated tmux/psmux pane. Session name: `sb-<active_workspace>`. One window per module — `sb run` and `sb stop` both target the same window, idempotently swapping in whichever script the verb implies.

#### `sb up`
Launch every module in the active workspace's `flow.yaml`. One tmux pane per module. Requires an active workspace. No per-module passthrough args — use `sb run <module> [args...]` for that.

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
Print `sb <version>`, then read merged `sb.config.yml` and run ten checks, in this order:
1-2. Executables (`--version` succeeds): `protoc`, `flatc`.
3. `transports` — the zenoh and iceoryx2 **crate** versions linked into this binary, baked in from the workspace lockfile by `sb-doctor`'s build script (`SB_ZENOH_VERSION` / `SB_ICEORYX2_VERSION`; `unknown` when the build tree had no lockfile). Reads nothing from the config and never fails.
4-5. Shared libraries (file exists + `dlopen` succeeds): `libzenohc.so`, `libiceoryx2_ffi_c.so`. Each also reports the install's own version, probed in order from `<libdir>/pkgconfig/*.pc`, `<libdir>/cmake/*/…ConfigVersion.cmake`, a version component of the canonicalized path, then a `.so.X.Y.Z` suffix; `version unknown` when every probe misses. A version that disagrees with check 3 **warns**, never fails: zenoh on differing `major.minor`, iceoryx2 on any differing digit (segments are opened on exact version equality).
6. `tmux` (or `psmux`), probed with `-V`, not `--version`.
7. `message styles` — one row per style under the resolved root with its `.proto` count and whether its targets are built. Fails only when the root holds no styles at all.
8. `unity targets` — stale generated C# inside a Unity project (see §Generated-code language levels). **Warns**, never fails.
9. `config keys` — the dead `message_style` / `message_definitions` / `message_targets` keys, if still present.
10. `std vault` — `~/.swarmbotix/messages/ros2/message_definitions/std/` populated; names `sb message list` as the re-install path.

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
