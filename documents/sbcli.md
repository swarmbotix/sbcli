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
2. Manages **workspaces** — named, isolated collections of modules
   (think Python venvs, but for swarmbotix projects).
3. **Adopts** an existing user project (Cargo crate, Python project,
   C++ tree, Flutter app, Unity game) as a swarmbotix *module* by
   dropping bookkeeping files into it. It never scaffolds the project
   itself — you bring your own.
4. Generates **pub/sub stubs** — per language, per transport —
   against the vault's types.
5. **Launches** every module in a workspace under tmux, one window per
   module, driven by the module's `runscript.bash` (and tears one down
   again via its `stopscript.bash`).
6. **Introspects** the live swarm — list topics, listen for frames,
   one-shot publish, prune dead iceoryx2 registrations — across both
   transports.
7. Drops a **request/response router** into a module (`sb service`) for
   REST-style calls over Zenoh queryables, independent of pub/sub.

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
├── apps/<name>/                     ←       clones made by `sb install <git-url>`
├── update-check.json                ←     latest release seen by `sb --version` (sb 0.2.1+)
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
└── workspaces/<name>/               ←     one dir per workspace
    ├── flow.yaml                    ←       module name → sb.dev.yml path
    └── sb.config.yml                ←       per-workspace config override (optional)

/path/to/<user_project>/             ← (3) a user-adopted module (lives anywhere)
├── sb.dev.yml                       ←     dev-time interface (gitignored)
├── sb.prd.yml                       ←     production interface (ships with binary)
├── swarmbotix_io/                   ←     generated pub/sub files
│   ├── publishers/<name>.py
│   └── subscribers/<name>.py
├── runscript.bash                   ←     user-written launch script (sb provides a stub)
└── …user's own source tree…
```

The vault (root 2) is global to the host. Workspaces (root 1's
`workspaces/`) are isolated registries. Modules (root 3) live wherever
the user keeps source code — `sb init` doesn't move them.

The full layout, config schema, and layering rules live in
[sbcli_config.md](sbcli_config.md) §2-§3.

---

## 3. Topic naming

Every topic is fully qualified at the wire:

```
/<device>/<workspace>/<module>/<transport>/<topic>
```

`<transport>` is the literal `iox2` or `zenoh`, chosen from the
`--iox2` / `--zenoh` flag on the pub/sub entry. Users type bare names
(`image_raw`) at the CLI; `sb` resolves the prefix from the active
workspace + the module's `sb.dev.yml` + the config's `device:` field,
and fills in the transport segment from the entry being added.
Generated code embeds the fully-qualified form so two modules in the
same workspace can find each other without the user typing the
prefix once.

A leading `/` in a topic argument bypasses the prefix and addresses
the wire path verbatim — useful for cross-workspace traffic or
legacy ROS2 topics. See [sbcli_pubsub.md](sbcli_pubsub.md) §13.7.

---

## 4. Command groups

The `sb` binary exposes **twelve** verb groups today. The table below is
the index; detailed behavior, flag-by-flag semantics, error catalogue,
and edge cases live in the per-group reference. The last row (`sb ui`,
L6) is a roadmap entry and is **not** in `sb --help` yet — `sb --help`
is always the authority on what this build actually accepts.

| Group | What it does | Reference doc | Build level |
|---|---|---|---|
| `sb doctor` | Validate this host's tooling and vault — `protoc` / `flatc` / `libzenohc` / `libiceoryx2` / `tmux`, the zenoh + iceoryx2 versions this `sb` links and whether the installed libraries match them, plus the resolved message styles, stale Unity C#, dead config keys, and the `std/` bundle. Ten checks plus one `app` line per installed app package; `[FAIL]` alone sets the exit code. | [sbcli_doctor.md](sbcli_doctor.md) | L1 |
| `sb config *` | Read, write, and inspect the layered `sb.config.yml` (env override → workspace → global → built-in). `show` / `get` expose the merged result to downstream LLMs and build scripts. | [sbcli_config.md](sbcli_config.md) | L1 (`open`/`set`/`show`/`get`); L5 (`--workspace`) |
| `sb message *` | Manage the vault: list, create, edit, remove `.proto` files; compile to per-language bindings. | [sbcli_messages.md](sbcli_messages.md) | L1 |
| `sb ws *` | Workspace lifecycle — create, set-active, list, delete. The active workspace's `flow.yaml` is the module registry every other verb reads. | [sbcli_ws.md](sbcli_ws.md) | L2 |
| `sb init` | Adopt an existing user project as a swarmbotix module. Drops `sb.dev.yml`, a `runscript.bash` stub, and an empty `swarmbotix_io/`; registers the module in the active workspace's `flow.yaml`. | [sbcli_init.md](sbcli_init.md) | L2 |
| `sb pub *` / `sb sub *` / `sb list` | The codegen surface. Each `add` / `edit` / `rm` mutates `sb.dev.yml` AND regenerates the file under `swarmbotix_io/{publishers,subscribers}/`. Generated classes expose a `TopicOverrides` API for runtime per-segment override (swarm instances, see consuming-doc §3). | [sbcli_pubsub.md](sbcli_pubsub.md) (codegen reference) · [sbcli_pubsub_consuming.md](sbcli_pubsub_consuming.md) (consumer guide: main, build glue, swarm pattern) | L3 |
| `sb service *` | REST-style request/response over Zenoh queryables, JSON payloads, independent of pub/sub. `init` drops a self-contained router (`service.py` / `service.rs`) into the module's io dir with its namespace baked in; you write the routes. Rust and Python only. | [sbcli_reqres.md](sbcli_reqres.md) | L4 |
| `sb topic *` | Introspect the live swarm. `list` enumerates active topics on either transport; `listen` decodes frames as JSON lines; `pub` does a one-shot raw publish; `prune` clears iceoryx2 registrations left behind by dead publishers (the `(dead)` rows in `list`). | (ref TBD — see `requirements.md` §"Topic Introspection") | L4 |
| `sb up` / `sb run` / `sb stop` / `sb down` / `sb attach` | Launch every module in the active workspace under tmux, or a single module, or kill the session. `up` / `run` execute a module's `runscript.bash`; `stop` is `run`'s symmetric counterpart and executes its `stopscript.bash` in the same window. | [sbcli_launch.md](sbcli_launch.md) | L5 |
| `sb install` / `sb app *` | Install app packages from a git URL or a local folder and run them as `sb <name> [args...]`, with no `PATH` change. `app init` makes any folder a package by writing `sb.app.yml`; `list` / `info` / `update` / `remove` manage what is installed. Global to the host, independent of workspaces. | [sbcli_app.md](sbcli_app.md) | `sb 0.2.1+` |
| `sb update` / `sb --version` | Self-update from the GitHub release page: `--version` adds an `update available` line (cached 24 h, silent offline, `SB_NO_UPDATE_CHECK=1` to skip); `update --check` compares and exits 10 when newer exists; `update` downloads, verifies the checksum and runs the package installer in place. | [sbcli_update.md](sbcli_update.md) | `sb 0.2.1+` |
| `sb gopro` | Freeze a module for production. Reads `sb.dev.yml` + `swarmbotix_io/` and writes a sanitized, language-agnostic `sb.prd.yml` that ships with the binary. | (ref TBD — see `requirements.md` §"Production") | L5 |
| `sb ui` *(not shipped)* | Start `swarmctl` (REST + MCP server + visual flow editor) in a tmux pane and open the browser. | (ref TBD — `requirements.md` §"Web UI" + `plan/level6.html`, both in the source repo) | L6 |

---

## 5. Typical lifecycle

Order of operations from a fresh host to a running swarm:

```
┌─ Setup (once per host) ────────────────────────────────────────────┐
│ sb doctor                # check tooling                            │
│ sb config set protoc /usr/local/bin/protoc                          │
│ sb message list          # triggers std/* install                   │
│ sb message compile       # generate bindings into <message_targets> │
└─────────────────────────────────────────────────────────────────────┘
            ↓
┌─ Workspace (once per project) ──────────────────────────────────────┐
│ sb ws create my_project                                             │
│ sb ws set    my_project                                             │
└─────────────────────────────────────────────────────────────────────┘
            ↓
┌─ Adopt modules (once per app) ──────────────────────────────────────┐
│ sb init --rust   /path/to/camera_app                                │
│ sb init --python /path/to/detector                                  │
│   (each module gets sb.dev.yml + runscript.bash + swarmbotix_io/)   │
└─────────────────────────────────────────────────────────────────────┘
            ↓
┌─ Wire pub/sub (iterative) ──────────────────────────────────────────┐
│ ( cd /path/to/camera_app                                            │
│   && sb pub add -m ros2/std/ImageStamped image_raw --iox2 )              │
│ ( cd /path/to/detector                                              │
│   && sb sub add -m ros2/std/ImageStamped /dev01/my_project/camera_app/iox2/image_raw --iox2 ) │
│   (each command regenerates one file under swarmbotix_io/)          │
└─────────────────────────────────────────────────────────────────────┘
            ↓
┌─ Run and inspect ───────────────────────────────────────────────────┐
│ # edit each module's runscript.bash to actually launch the app      │
│ sb up                    # tmux session sb-my_project comes up      │
│ sb attach                # attach to the tmux session               │
│ sb topic list            # enumerate live topics                    │
│ sb topic listen /dev01/my_project/camera_app/iox2/image_raw         │
│ sb run camera_app --id 2 # restart one module with passthrough args  │
│ sb stop camera_app       # run its stopscript.bash (teardown)        │
└─────────────────────────────────────────────────────────────────────┘
            ↓
┌─ Freeze for production ─────────────────────────────────────────────┐
│ sb gopro --all           # writes sb.prd.yml next to each sb.dev.yml │
│ sb down                  # tear down the tmux session                │
└─────────────────────────────────────────────────────────────────────┘
```

Each arrow is a real handoff — the next stage reads what the
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
| "What's the exact flag list for `sb pub add`?" | [sbcli_pubsub.md](sbcli_pubsub.md) §9.1 |
| "What does `sb message compile --iox2` actually emit?" | [sbcli_messages.md](sbcli_messages.md) §5.1 |
| "Where does `~/.swarmbotix/messages` come from?" | [sbcli_config.md](sbcli_config.md) §2 + [sbcli_messages.md](sbcli_messages.md) §1 |
| "What's in the active workspace's `flow.yaml`?" | [sbcli_ws.md](sbcli_ws.md) §4 |
| "How do I add Python message bindings to `sys.path` / VS Code?" | [sbcli_config.md](sbcli_config.md) §4.5.1 + §4.5.2 |
| "Why is `sb doctor` saying `[SKIP] flatc`?" | [sbcli_doctor.md](sbcli_doctor.md) §3 + §4 |
| "How do I expose a REST-style call instead of a topic?" | [sbcli_reqres.md](sbcli_reqres.md) §2 + §6 |
| "What does `sb init --python` actually write?" | [sbcli_init.md](sbcli_init.md) §4 + §5 |
| "Why does `sb stop` need a `stopscript.bash` I have to write myself?" | [sbcli_launch.md](sbcli_launch.md) §4.2 |
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
