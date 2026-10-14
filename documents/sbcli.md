# `sb` — swarmbotix CLI Overview

The high-level entry point. This document explains what `sb` is, the
mental model behind it, and which reference doc to open for each
command group.

Everything here describes the `sb` you have installed. Where it is
overruled, it is overruled by `requirements.md` in the sbcli **source
repository**, alongside the per-level build plans (`plan/levelN.html`).
Those are maintainer documents: they are not shipped in this
`documents/` set and you will not find them under `~/.swarmbotix/`.

---

## 1. What `sb` is

`sb` is a single binary that gives you one toolchain for building,
launching, and inspecting a heterogeneous pub/sub system across
Rust, Python, C++, Flutter, and Unity over two transports
(Zenoh for the network, iceoryx2 for shared memory on a single host).

Concretely, it does seven things:

1. Manages a **message vault** — `.proto` source-of-truth files plus
   per-language generated bindings (iox2 ctypes / protobuf / FlatBuffers).
2. Manages **workspaces**: named, isolated collections of modules
   (think Python venvs, but for swarmbotix projects). `sb 0.5.0+`: a
   workspace is committed at a repository root, and
   `sb install <git-url | path>` installs it in one step. `sb 0.8.0+`
   splits it in two kinds of file: `sb.workspace.yaml` lists the
   modules and messages, and each flow file (`sb.flow.yaml`, or
   `<name>.flow.yaml`) says which of them run, how many times, and wired
   to what.
3. **Adopts** an existing user project (Cargo crate, Python project,
   C++ tree, Flutter app, Unity game) as a swarmbotix *module* by
   dropping a manifest (`sb.module.yaml`, sb 0.6.0+) and an IO
   directory into it. It never scaffolds the project itself; you bring
   your own.
4. Generates **pub/sub stubs** per language and transport against the
   vault's types (`sb io`, sb 0.5.0+), with a protobuf, FlatBuffers, or
   raw payload encoding on Zenoh.
5. **Builds and launches** the nodes of a flow under tmux, one window
   per instance, from each module's `sb.module.yaml` (sb 0.6.0+): no
   launch scripts, the same file on Linux and Windows, and `sb stop`
   closes a module's windows again. `sb up [FLOW]` picks the flow
   (sb 0.8.0+).
6. **Introspects** the live swarm — list topics, listen for frames,
   one-shot publish, prune dead iceoryx2 registrations — across both
   transports.
7. Drops **request/response routers** into a module (`services:`
   entries via `sb io add service`, sb 0.5.0+) for REST-style calls over
   Zenoh queryables, independent of pub/sub.

It does **not** mock, wrap, or hide either transport. Generated code
calls `zenoh::Session::open` and `iceoryx2::node::NodeBuilder`
directly. There is no "swarmbotix runtime" — `sb` is purely
build-time + launch-time orchestration.

---

## 2. Mental model — three on-disk roots

Every `sb` invocation reads from and writes to one of three roots.
Knowing which root holds which file is the fastest way to navigate.

```
~/.swarmbotix/                       ← (1) the swarmbotix home
├── sb.config.yml                    ←     host-wide config (tool paths, defaults)
├── active                           ←     name of the active workspace
├── apps.yml                         ←     installed app packages (sb 0.2.1+)
├── apps/<name>/                     ←       app clones made by `sb install <git-url>`
├── update-check.json                ←     latest release seen by `sb --version` (sb 0.2.1+)
├── repos/<name>/                    ←     workspace clones made by `sb install <git-url>` (sb 0.5.0+)
├── messages/                        ←     (2) message styles — one subdir per style
│   ├── ros2/                        ←       shipped style
│   │   ├── message_definitions/     ←         source .proto files
│   │   │   ├── std/                 ←           forge-bundled (installed on first run)
│   │   │   ├── sensor_msgs/         ←           ROS2-mirror packages
│   │   │   └── <user_pkg>/          ←           user-created namespaces
│   │   └── message_targets/         ←       (2b) generated bindings
│   │       ├── iox2/<pkg>/<Leaf>/<Leaf>.py
│   │       ├── proto/<pkg>/<Leaf>/<Leaf>_pb2.py
│   │       └── fb/<pkg>/<Leaf>/<Leaf>.py
│   └── swarmbotix/                  ←       shipped style (main)
│       ├── message_definitions/
│       │   ├── header/              ←         Header (UNIX ns timestamp)
│       │   ├── primitives/          ←         Vector3/4, Quaternion, Mat33/44, ...
│       │   ├── images/              ←         Image -> 8 fixed-size iox2 types
│       │   └── sensors/             ←         Imu, LaserScan, PointCloud, GnssFix
│       └── message_targets/
└── workspaces/<name>/               ←     one dir per installed workspace
    ├── sb.workspace.yaml            ←       installed copy: absolute paths + source: (never hand-edited)
    ├── flows/                       ←       installed flows: sb.flow.yaml (default), <name>.flow.yaml
    ├── active_flow                  ←       the flow the last `sb up` launched
    └── sb.config.yml                ←       per-workspace config override (optional)

/path/to/<repo>/                     ← (3) a workspace repository (lives anywhere)
├── sb.workspace.yaml                ←     the inventory: modules, messages (committed)
├── sb.flow.yaml                     ←     the default flow: nodes, instances, wires (committed)
├── <name>.flow.yaml                 ←     any other flow, `sb up <name>` (optional)
├── messages/<dir>/*.proto           ←     optional; imported into (2) by `sb install`
└── modules/<module>/                ←     a user-adopted module
    ├── sb.module.yaml               ←       the module manifest: interface, build, launch (committed)
    ├── sb.prd.yml                   ←       production interface (ships with binary)
    ├── swarmbotix_io/               ←       generated files
    │   ├── publishers/<name>.py
    │   ├── subscribers/<name>.py
    │   └── services/<name>.py       ←         sb 0.5.0+
    └── …user's own source tree…
```

`sb 0.8.0+` reads three kinds of committed file:

| File | Holds | Reference |
|---|---|---|
| `sb.module.yaml` | one module: identity, build and start, environment, platforms, ports | [sbcli_module.md](sbcli_module.md) |
| `sb.workspace.yaml` | one workspace: the modules it holds and the messages it imports | [sbcli_workspace.md](sbcli_workspace.md) |
| `sb.flow.yaml`, `<name>.flow.yaml` | one way to run it: nodes, instances, instance settings, wires | [sbcli_flow.md](sbcli_flow.md) |

`sb 0.7.0+` reads no pre-0.6.0 file name: a folder that still holds
only one is refused with an error that names the rename (§10). A
`sb.flow.yaml` in the 0.7 shape, which held the modules too, is still
read by 0.8.0 with a warning (§10).

The vault (root 2) is global to the host. A workspace is defined by the
files in its repository (root 3) and installed into root 1's
`workspaces/`, which every other verb reads
([sbcli_ws.md](sbcli_ws.md) §1). Modules live inside the repository, or
wherever the user keeps source code; `sb init` doesn't move them.

The full layout, config schema, and layering rules live in
[sbcli_config.md](sbcli_config.md) §2-§3.

---

## 3. Topic naming

Every topic is fully qualified at the wire:

```
<device>/<workspace>/<module>/<transport>/<topic>
```

There is no leading `/` (sb 0.5.0+): the canonical form is the wire
form, byte for byte the Zenoh key expression and the iceoryx2 service
name.

`<transport>` is the literal `iox2` or `zenoh`, chosen from the
`--iox2` / `--zenoh` flag on the pub/sub entry. Users type bare names
(`image_raw`) at the CLI; `sb` resolves the prefix from the active
workspace + the module's `sb.module.yaml` + the config's `device:` field,
and fills in the transport segment from the entry being added.
Generated code embeds the fully-qualified form so two modules in the
same workspace can find each other without the user typing the
prefix once.

`--key <wire key>` on `sb io add` bypasses the prefix and addresses
the wire key verbatim: the way to subscribe to another
module's publisher, and useful for cross-workspace traffic or legacy
ROS2 topics (sb 0.5.0+; a positional topic may not contain `/`). See
[sbcli_pubsub.md](sbcli_pubsub.md) §13.7.

`sb topic list` prints every topic as its wire key, in the table and in
the `--json` `topic` field alike (sb 0.5.0+; 0.4.2 printed it with a
leading `/`), and `sb topic listen` / `sb topic pub` accept a path with
or without a leading `/`, so a listed row pastes in unchanged.

---

## 4. Command groups

The `sb` binary exposes **eleven** verb groups today. The table below is
the index; detailed behavior, flag-by-flag semantics, error catalogue,
and edge cases live in the per-group reference. The last row (`sb ui`,
L6) is a roadmap entry and is **not** in `sb --help` yet — `sb --help`
is always the authority on what this build actually accepts.

| Group | What it does | Reference doc | Build level |
|---|---|---|---|
| `sb doctor` | Validate this host's tooling and vault: `protoc` / `flatc` / `libzenohc` / `libiceoryx2` / `tmux`, the zenoh + iceoryx2 versions this `sb` links and whether the installed libraries match them, plus the resolved message styles, stale Unity C#, dead config keys, the `std/` bundle, and (sb 0.5.0+) legacy `topic: /...` entries and the `sb ws check` rows for the active workspace (sb 0.6.0+: including required tools, platform support, and deprecated files; sb 0.8.0+: `ws flows`, one line per flow with its nodes, instances, and wires). Twelve checks plus one `app` line per installed app package; `[FAIL]` alone sets the exit code. | [sbcli_doctor.md](sbcli_doctor.md) | L1 |
| `sb config *` | Read, write, and inspect the layered `sb.config.yml` (env override → workspace → global → built-in). `show` / `get` expose the merged result to downstream LLMs and build scripts. | [sbcli_config.md](sbcli_config.md) | L1 (`open`/`set`/`show`/`get`); L5 (`--workspace`) |
| `sb message *` | Manage the vault: list, create, edit, remove `.proto` files; compile to per-language bindings. | [sbcli_messages.md](sbcli_messages.md) | L1 |
| `sb ws *` | Workspace lifecycle. `sb 0.5.0+`: a workspace lives in a repository; `sb 0.8.0+`: as `sb.workspace.yaml` (modules, messages) plus flow files (nodes, instances, wires). `create` writes `./sb.workspace.yaml` and `./sb.flow.yaml` and installs them; `install` installs a repository (the workspace-only form of `sb install`); `check [<name>]` validates a workspace (default: the active one); `update` runs `git pull --ff-only` and reinstalls; `uninstall [--clean]` removes the installed copy (and, with `--clean`, each module's io dir and the clone `sb` made). `set` / `list` / `delete` as before; `list` shows each workspace's source and a stale marker. The active workspace's installed `sb.workspace.yaml` is the module registry every other verb reads. | [sbcli_ws.md](sbcli_ws.md) · [sbcli_workspace.md](sbcli_workspace.md) · [sbcli_flow.md](sbcli_flow.md) | L2; `sb 0.5.0+` (repository); `sb 0.8.0+` (workspace and flow files) |
| `sb init` | Adopt an existing user project as a swarmbotix module. Drops `sb.module.yaml` (sb 0.6.0+; no launch script, no `root:`) and an empty `swarmbotix_io/`; registers the module in the repository `sb.workspace.yaml` above it and reinstalls (sb 0.8.0+), or else in the active workspace. It does not add the module to a flow. What else the manifest can hold: [sbcli_module.md](sbcli_module.md). | [sbcli_init.md](sbcli_init.md) | L2 |
| `sb io *` | The codegen surface (sb 0.5.0+). `add pub` / `add sub` / `add service`, `edit`, `rm`, `list` mutate `sb.module.yaml` and regenerate the matching file under `swarmbotix_io/{publishers,subscribers,services}/`; `--proto` / `--fb` / `--raw [<mime>]` pick the Zenoh payload encoding; `update [<module>] [--services]` regenerates without changing the YAML. Publishers, subscribers, and services share one name space per module. Generated pub/sub classes expose a `TopicOverrides` API for runtime per-segment override (swarm instances). A service is a REST-style request/response router over Zenoh queryables with JSON payloads, independent of pub/sub (Rust and Python); you write its routes. | [sbcli_pubsub.md](sbcli_pubsub.md) §14 (codegen reference) · [sbcli_pubsub_consuming.md](sbcli_pubsub_consuming.md) (consumer guide: main, build glue, swarm pattern) · [sbcli_reqres.md](sbcli_reqres.md) (services) | L3, L4; `sb io` `sb 0.5.0+` |
| `sb topic *` | Introspect the live swarm. `list` enumerates active topics on either transport; `listen` decodes frames as JSON lines; `pub` does a one-shot raw publish; `prune` clears iceoryx2 registrations left behind by dead publishers (the `(dead)` rows in `list`). `listen` and `pub` read the transport from the topic path's `zenoh` / `iox2` segment, so `-t zenoh` or `-t iox2` is needed only for a path without one (sb 0.4.1+). | (ref TBD — see `requirements.md` §"Topic Introspection") | L4 |
| `sb up` / `sb run` / `sb stop` / `sb down` / `sb attach` | Build and launch the nodes of one flow of the active workspace under tmux, one window per instance (`<module>-<id>`), or a single module or instance (`sb run <module>[:<id>]`, `--fg` for the current terminal), or kill the session. `sb 0.6.0+`: driven by each module's `sb.module.yaml`, with no launch scripts; `stop` closes the module's windows and runs its optional `scripts.stop`. `sb 0.8.0+`: `sb up [FLOW]` (default `default`) records the active flow, and `sb run` follows it. | [sbcli_launch.md](sbcli_launch.md) · [sbcli_flow.md](sbcli_flow.md) · [sbcli_module.md](sbcli_module.md) | L5; native launch `sb 0.6.0+`; flows `sb 0.8.0+` |
| `sb install` / `sb app *` | Install app packages from a git URL or a local folder and run them as `sb <name> [args...]`, with no `PATH` change. `app init` makes any folder a package by writing `sb.app.yml`; `list` / `info` / `update` / `remove` manage what is installed. Global to the host, independent of workspaces. `sb 0.5.0+`: `sb install` on a source whose root holds a workspace file instead of `sb.app.yml` installs a workspace (clone, install `sb.workspace.yaml` and every flow, import and compile `messages:`, regenerate every module's io; `sb.workspace.yaml` since 0.8.0). | [sbcli_app.md](sbcli_app.md) · [sbcli_ws.md](sbcli_ws.md) §5.2 | `sb 0.2.1+`; workspaces `sb 0.5.0+` |
| `sb update` / `sb --version` | Self-update from the GitHub release page: `--version` adds an `update available` line (cached 24 h, silent offline, `SB_NO_UPDATE_CHECK=1` to skip); `update --check` compares and exits 10 when newer exists; `update` downloads, verifies the checksum and runs the package installer in place. | [sbcli_update.md](sbcli_update.md) | `sb 0.2.1+` |
| `sb gopro` | Freeze a module for production. Reads `sb.module.yaml` + `swarmbotix_io/` and writes a sanitized, language-agnostic `sb.prd.yml` that ships with the binary. | (ref TBD — see `requirements.md` §"Production") | L5 |
| `sb ui` *(not shipped)* | Start `swarmctl` (REST + MCP server + visual flow editor) in a tmux pane and open the browser. | (ref TBD — `requirements.md` §"Web UI" + `plan/level6.html`, both in the source repo) | L6 |

`sb 0.7.0+`: the four older codegen verbs that `sb io` replaced are no
longer part of the binary; §10 names them, and `sb --help` no longer
lists them.

---

## 5. Typical lifecycle

**Install a published workspace** (sb 0.5.0+). The shortest path from a
fresh host to a running swarm, for a repository whose root holds an
`sb.workspace.yaml` (sb 0.8.0+):

```bash
sb config set device <name>
sb install https://github.com/<org>/<workspace>.git --dir ~/<workspace>   # or: cd clone && sb install .
sb up
```

`sb install` clones the repository (into `--dir`, or
`~/.swarmbotix/repos/<workspace>/` without it), installs its
`sb.workspace.yaml` and flows as the active workspace, imports and
compiles its `messages:`, and
regenerates every module's `swarmbotix_io/` ([sbcli_ws.md](sbcli_ws.md) §5.2).

**Author a workspace.** Order of operations from a fresh host to a
running swarm you build yourself:

```
┌─ Setup (once per host) ─────────────────────────────────────────────┐
│ sb doctor                # check tooling                            │
│ sb config set protoc /usr/local/bin/protoc                          │
│ sb config set device dev01                                          │
│ sb message list          # triggers std/* install                   │
│ sb message compile       # generate bindings into <message_targets> │
└─────────────────────────────────────────────────────────────────────┘
            ↓
┌─ Workspace (once per project, sb 0.5.0+) ───────────────────────────┐
│ cd ~/my_project          # the repository root                      │
│ sb ws create my_project  # writes ./sb.workspace.yaml and           │
│                          #   ./sb.flow.yaml, installs, active       │
└─────────────────────────────────────────────────────────────────────┘
            ↓
┌─ Adopt modules (once per app) ──────────────────────────────────────┐
│ sb init --rust   modules/camera_app                                 │
│ sb init --python modules/detector                                   │
│   (each module gets sb.module.yaml + swarmbotix_io/ and a line      │
│    in ./sb.workspace.yaml; the workspace is reinstalled)            │
│ # sb 0.8.0+: list them in ./sb.flow.yaml so sb up runs them:        │
│ #   nodes: [camera_app, detector]                                   │
│ sb install .             # the installed flow follows               │
└─────────────────────────────────────────────────────────────────────┘
            ↓
┌─ Wire pub/sub (iterative, sb 0.5.0+) ───────────────────────────────┐
│ ( cd modules/camera_app                                             │
│   && sb io add pub image_raw -m ros2/std/ImageStamped --iox2 )      │
│ ( cd modules/detector                                               │
│   && sb io add sub -m ros2/std/ImageStamped --iox2 \                │
│        --key dev01/my_project/camera_app/iox2/image_raw )           │
│   (each command regenerates one file under swarmbotix_io/;          │
│    sb io update regenerates a whole module)                         │
└─────────────────────────────────────────────────────────────────────┘
            ↓
┌─ Run and inspect ───────────────────────────────────────────────────┐
│ # defaults build + start each module; add scripts: to its           │
│ # sb.module.yaml only where they don't fit (sbcli_module.md)        │
│ sb up                    # build the default flow, then tmux        │
│                          #   session sb-my_project (sb up <flow>)   │
│ sb attach                # attach to the tmux session               │
│ sb topic list            # enumerate live topics                    │
│ sb topic listen dev01/my_project/camera_app/iox2/image_raw \        │
│     --show               # JSON lines; iceoryx2, from the path      │
│   # sb 0.4.1+: no transport flag needed; the path's iox2 / zenoh    │
│   #   segment picks it. -t zenoh|iox2 only for other paths.         │
│   # sb 0.4.0+ flags:                                                │
│   #   --type <style>/<ns>/<Leaf>  decode as this vault message      │
│   #   --no-data                   omit bytes data, keep <name>_len  │
│   #   --show                      open a window for image frames    │
│ sb run camera_app        # rebuild + restart one module             │
│ sb stop camera_app       # close its windows (+ its scripts.stop)   │
└─────────────────────────────────────────────────────────────────────┘
            ↓
┌─ Share it (sb 0.5.0+) ──────────────────────────────────────────────┐
│ git add sb.workspace.yaml *.flow.yaml modules                       │
│ git commit && git push                                              │
│   (others run: sb install <git-url>)                                │
└─────────────────────────────────────────────────────────────────────┘
            ↓
┌─ Freeze for production ─────────────────────────────────────────────┐
│ sb gopro --all           # writes sb.prd.yml next to each manifest  │
│ sb down                  # tear down the tmux session               │
└─────────────────────────────────────────────────────────────────────┘
```

Each arrow is a real handoff: the next stage reads what the
previous stage wrote (on disk; no in-memory state survives between
`sb` invocations).

---
## 6. Configuration layering at a glance

Every `sb` verb reads a single merged `sb.config.yml`. The layers,
highest priority first:

1. `$SB_CONFIG` — explicit file path. Used by tests, CI, ad-hoc
   overrides.
2. `<sb_home>/workspaces/<active>/sb.config.yml` — per-workspace
   override.
3. `<sb_home>/sb.config.yml` — per-user global.
4. Built-in defaults in `sb_core::SbCliConfig::builtin_defaults`.

To see what the merged result actually looks like on your host:

```bash
sb config show                # YAML
sb config show --sources      # which layer supplied each field
sb config get message_targets # one field, bare value
```

Full schema, layering rules, and `show` / `get` output modes
(including the recommended pattern for downstream LLMs setting up
Python `.pth` + IDE analyzer paths) live in
[sbcli_config.md](sbcli_config.md).

---

## 7. Three surfaces, one core

Every behavior `sb` exposes via the CLI is also reachable via two
other surfaces (planned for L6):

| Surface | For | Reference |
|---|---|---|
| `sb` CLI | Humans, scripts, CI, LLM agents in a shell | this doc + the `sbcli_*.md` references |
| REST | Web UI, `curl`, dashboards | `requirements.md` §"swarmctl Architecture" |
| MCP | AI agents (Cursor, Claude Code, etc.) | same as REST |

The shared logic lives under `crates/sb-core` and the per-feature
crates (`sb-vault`, `sb-pubsub`, `sb-codegen`, `sb-discover`,
`sb-listen`, `sb-launch`, `sb-gopro`, `sb-workspace`, `sb-config`,
`sb-doctor`). The CLI binary (`crates/sb-cli`) is a thin clap-driven
wrapper that calls into those crates. swarmctl (L6) will be a
second thin wrapper around the same core, exposing REST + MCP.

---

## 8. Where to look when

| Question | Open |
|---|---|
| "What's the exact flag list for `sb io add`?" | [sbcli_pubsub.md](sbcli_pubsub.md) §14.1 |
| "How do I install a workspace someone published?" | [sbcli_ws.md](sbcli_ws.md) §5.2 + §6.1 |
| "Which payload encoding should a topic use?" | [sbcli_pubsub.md](sbcli_pubsub.md) §14.3 |
| "What does `sb message compile --iox2` actually emit?" | [sbcli_messages.md](sbcli_messages.md) §5.1 |
| "Where does `~/.swarmbotix/messages` come from?" | [sbcli_config.md](sbcli_config.md) §2 + [sbcli_messages.md](sbcli_messages.md) §1 |
| "What goes in `sb.workspace.yaml`, and what goes in a flow file?" | [sbcli_workspace.md](sbcli_workspace.md) §1 + [sbcli_flow.md](sbcli_flow.md) §1 |
| "What does the installed copy hold?" | [sbcli_workspace.md](sbcli_workspace.md) §6 + [sbcli_ws.md](sbcli_ws.md) §4.3 |
| "What can a module's `sb.module.yaml` declare, and what are the defaults?" | [sbcli_module.md](sbcli_module.md) §2 + §3.2 |
| "How do I run four instances of one module?" | [sbcli_flow.md](sbcli_flow.md) §4 + [sbcli_launch.md](sbcli_launch.md) §1.1 |
| "I ran `sb init`, but `sb up` does not start the module." | [sbcli_flow.md](sbcli_flow.md) §3 (add it to `nodes:`, then `sb install .`) |
| "How do I keep a tool such as a calibrator out of `sb up`, but still run it?" | [sbcli_flow.md](sbcli_flow.md) §7 + §9 (a flow of its own, `sb up calib`) |
| "Which publisher feeds which subscriber, and how do I declare it?" | [sbcli_flow.md](sbcli_flow.md) §6 |
| "How do I run a module in a Docker container?" | [sbcli_module.md](sbcli_module.md) §14 |
| "How do I move a 0.7 `sb.flow.yaml` to the 0.8 files?" | [sbcli_workspace.md](sbcli_workspace.md) §7 |
| "How do I move a 0.5 workspace to the current file names (required since 0.7.0)?" | [sbcli_module.md](sbcli_module.md) §11 |
| "How do I add Python message bindings to `sys.path` / VS Code?" | [sbcli_config.md](sbcli_config.md) §4.5.1 + §4.5.2 |
| "Why is `sb doctor` saying `[SKIP] flatc`?" | [sbcli_doctor.md](sbcli_doctor.md) §3 + §4 |
| "How do I expose a REST-style call instead of a topic?" | [sbcli_reqres.md](sbcli_reqres.md) §2 + §6 |
| "What does `sb init --python` actually write?" | [sbcli_init.md](sbcli_init.md) §4 + §5 |
| "Do I need to write a stop script for `sb stop`?" (no) | [sbcli_launch.md](sbcli_launch.md) §4.2 |
| "Why is my module still running after `sb stop`?" | [sbcli_launch.md](sbcli_launch.md) §4.1 + §8 |
| "How do I ship a tool so users run it as `sb <name>` without touching `PATH`?" | [sbcli_app.md](sbcli_app.md) §3 + §4 |
| "Is there a newer `sb`, and how do I get it without redownloading by hand?" | [sbcli_update.md](sbcli_update.md) §2 + §4 |
| "Authoritative spec for any flag, file, or path" | `requirements.md` (sbcli source repo — not shipped) |
| "When does feature X land?" | `plan/levelN.html` (same repo) |

---

## 9. What this doc deliberately does *not* cover

- **Exact flag semantics, error messages, exit codes** — those live
  in the per-group references. This doc would go stale every release
  if it tried to mirror them.
- **Build-system integration** (CMake, Cargo.toml, pyproject.toml) —
  `sb init` adopts a project but does not generate or edit those
  files. Users wire their own build glue using `sb config get` to
  read sb's paths. See [sbcli_config.md](sbcli_config.md) §4.5 for
  the recommended downstream-LLM patterns.
- **Transport internals** — Zenoh and iceoryx2 are external
  projects. `sb` generates code that calls their public APIs; if
  you're debugging the wire, read their docs first.

---

## 10. Deprecations and removals (sb 0.8.0+)

`sb 0.8.0` has one active deprecation: a `sb.flow.yaml` in the 0.7
shape is still read, with a warning, and will not be read by 0.8.1.
Every other item below is **removed** in the release named in the
table: a removed verb or flag is unknown to the binary, and a removed
file or form is no longer read. The 0.5 items were deprecated in 0.6.0
(the four verbs already in 0.5.0); their migration, item by item, is
[sbcli_module.md](sbcli_module.md) §11. The 0.7 items are migrated by
[sbcli_workspace.md](sbcli_workspace.md) §7.

| item | deprecated since | removed in | replacement |
|---|---|---|---|
| `sb pub`, `sb sub`, `sb list`, `sb service` | 0.5.0 (help only), 0.6.0 (warning) | 0.7.0 | `sb io` |
| `sb.dev.yml` | 0.6.0 | 0.7.0 | `sb.module.yaml` |
| `flow.yaml` | 0.6.0 | 0.7.0 | `sb.flow.yaml` (and, since 0.8.0, `sb.workspace.yaml`) |
| `runscript.bash` | 0.6.0 | 0.7.0 | `scripts:` in `sb.module.yaml`, or nothing when the language defaults fit |
| `stopscript.bash` | 0.6.0 | 0.7.0 | `scripts.stop`, or nothing |
| `sb init --runscript` | 0.6.0 | 0.7.0 | none: `sb init` writes no launch script |
| top-level `instances:` in the 0.5 flow | 0.6.0 | 0.7.0 | the flow file's `instances:` (sb 0.8.0+) |
| `SB_NO_TMUX=1` | 0.6.0 | 0.7.0 | `sb run --fg` |
| `sb.flow.yaml` in the 0.7 shape (modules, `up:`, instances, and subscriber-first wires in one file) | 0.8.0 (warning) | 0.8.1 | `sb.workspace.yaml` plus flow files with publisher-first wires ([sbcli_workspace.md](sbcli_workspace.md) §7) |
| `up: false` on a module entry | 0.8.0 (part of the 0.7 shape; read as "not a node") | 0.8.1 | leave the module out of the flow's `nodes:`, or give it a flow of its own ([sbcli_flow.md](sbcli_flow.md) §3) |
| an instance's `docker:` block | 0.6.0 (not launched) | 0.8.0 (dropped when a 0.7 file is read) | the module's `scripts` call `docker compose` ([sbcli_module.md](sbcli_module.md) §14) |
| `sb init --docker` | 0.6.0 (bash template removed; the flag stayed as an instance check) | 0.8.0 | none; `sb init` without it, and compose as above |

What `sb 0.8.0` does when it meets a deprecated or removed item:

- **A `sb.flow.yaml` in the 0.7 shape** at a repository root:
  `sb install` reads it as a workspace and a default flow
  ([sbcli_workspace.md](sbcli_workspace.md) §7.7) and prints, on stderr,
  ``warning: sb.flow.yaml in the 0.7 shape is deprecated since 0.8.0 and will be removed in 0.8.1; split it into sb.workspace.yaml and sb.flow.yaml; see sbcli_workspace.md §Migrating``.
  `sb ws check` and `sb doctor` list it in their `deprecated` row.
- **`sb init --docker`**: clap rejects the unknown flag before `sb`
  does anything.
- **A removed verb** (the first row): it is no longer a built-in
  command, so `sb` looks it up as an installed app
  ([sbcli_app.md](sbcli_app.md)) and, finding none, exits 1 with
  ``error: `<verb>` is not an sb command or an installed app.``
  followed by a `Did you mean ...?` guess when one is close, the list of
  installed apps, and how to install one. The guess is by spelling, not a
  pointer to `sb io`; there is no alias.
- **`sb.dev.yml` without `sb.module.yaml`** beside it: `sb install`,
  `sb init`, `sb io`, and `sb doctor` stop with
  `module manifest sb.module.yaml not found at <dir>; sb.dev.yml is no longer read since 0.7.0, rename it (see sbcli_module.md §Migrating)`.
- **`flow.yaml` without `sb.flow.yaml`** beside it (a repository root,
  or an installed copy written before 0.6.0):
  `sb.flow.yaml not found at <dir>; flow.yaml is no longer read since 0.7.0, rename it (see sbcli_module.md §Migrating)`.
- **`runscript.bash` or `stopscript.bash`** at a module root: no error.
  The module launches natively from its `sb.module.yaml` (language
  defaults included) and the script is never run. `sb ws check` and
  `sb doctor` list each leftover file in their `deprecated` row
  ([sbcli_doctor.md](sbcli_doctor.md) §4.11) so it can be deleted or
  translated.
- **`SB_NO_TMUX=1`**: nothing reads it.

The 0.7 list forms inside a module manifest (`publishers:` and
`subscribers:` as dash lists, `services:` as a list of maps) and the
0.7 list form of `messages:` are **not** deprecated: `sb 0.8.0+` reads
them silently and writes the map forms
([sbcli_module.md](sbcli_module.md) §11.10).

Deprecation warnings go to stderr, once per item per process, never to
stdout:

```
warning: <item> is deprecated since <release> and will be removed in <release>; <hint>
```
