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
   workspace is a flow file committed at a repository root
   (`sb.flow.yaml` since 0.6.0), and `sb install <git-url | path>`
   installs it in one step.
3. **Adopts** an existing user project (Cargo crate, Python project,
   C++ tree, Flutter app, Unity game) as a swarmbotix *module* by
   dropping a manifest (`sb.module.yaml`, sb 0.6.0+) and an IO
   directory into it. It never scaffolds the project itself; you bring
   your own.
4. Generates **pub/sub stubs** per language and transport against the
   vault's types (`sb io`, sb 0.5.0+; `sb pub` / `sb sub` before), with
   a protobuf, FlatBuffers, or raw payload encoding on Zenoh.
5. **Builds and launches** every module in a workspace under tmux, one
   window per instance, from each module's `sb.module.yaml`
   (sb 0.6.0+): no launch scripts, the same file on Linux and Windows,
   and `sb stop` closes a module's windows again.
6. **Introspects** the live swarm — list topics, listen for frames,
   one-shot publish, prune dead iceoryx2 registrations — across both
   transports.
7. Drops **request/response routers** into a module (`services:`
   entries via `sb io add service`, sb 0.5.0+; `sb service init` before)
   for REST-style calls over Zenoh queryables, independent of pub/sub.

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
    ├── sb.flow.yaml                 ←       installed copy: absolute paths + source: (never hand-edited)
    └── sb.config.yml                ←       per-workspace config override (optional)

/path/to/<repo>/                     ← (3) a workspace repository (lives anywhere)
├── sb.flow.yaml                     ←     the workspace: modules, instances, messages, wires (committed)
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

`sb 0.6.0+` names the two committed files `sb.flow.yaml` and
`sb.module.yaml`; the pre-0.6.0 names still work until 0.7.0 (§10).

The vault (root 2) is global to the host. A workspace is defined by the
`sb.flow.yaml` in its repository (root 3) and installed into root 1's
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

`--key <wire key>` on `sb io add` (or `sb pub add` / `sb sub add`) bypasses the prefix
and addresses the wire key verbatim: the way to subscribe to another
module's publisher, and useful for cross-workspace traffic or legacy
ROS2 topics (sb 0.5.0+; a positional topic may not contain `/`). See
[sbcli_pubsub.md](sbcli_pubsub.md) §13.7.

`sb topic list` prints every topic as its wire key, in the table and in
the `--json` `topic` field alike (sb 0.5.0+; 0.4.2 printed it with a
leading `/`), and `sb topic listen` / `sb topic pub` accept a path with
or without a leading `/`, so a listed row pastes in unchanged.

---

## 4. Command groups

The `sb` binary exposes **thirteen** verb groups today. The table below is
the index; detailed behavior, flag-by-flag semantics, error catalogue,
and edge cases live in the per-group reference. The last row (`sb ui`,
L6) is a roadmap entry and is **not** in `sb --help` yet — `sb --help`
is always the authority on what this build actually accepts.

| Group | What it does | Reference doc | Build level |
|---|---|---|---|
| `sb doctor` | Validate this host's tooling and vault: `protoc` / `flatc` / `libzenohc` / `libiceoryx2` / `tmux`, the zenoh + iceoryx2 versions this `sb` links and whether the installed libraries match them, plus the resolved message styles, stale Unity C#, dead config keys, the `std/` bundle, and (sb 0.5.0+) legacy `topic: /...` entries and the `sb ws check` rows for the active workspace (sb 0.6.0+: including required tools, platform support, wires, and deprecated files). Twelve checks plus one `app` line per installed app package; `[FAIL]` alone sets the exit code. | [sbcli_doctor.md](sbcli_doctor.md) | L1 |
| `sb config *` | Read, write, and inspect the layered `sb.config.yml` (env override → workspace → global → built-in). `show` / `get` expose the merged result to downstream LLMs and build scripts. | [sbcli_config.md](sbcli_config.md) | L1 (`open`/`set`/`show`/`get`); L5 (`--workspace`) |
| `sb message *` | Manage the vault: list, create, edit, remove `.proto` files; compile to per-language bindings. | [sbcli_messages.md](sbcli_messages.md) | L1 |
| `sb ws *` | Workspace lifecycle. `sb 0.5.0+`: a workspace is a repository flow file (`sb.flow.yaml` since 0.6.0, with instances and wires per module). `create` writes `./sb.flow.yaml` and installs it; `install` installs a repository (the workspace-only form of `sb install`); `check [<name>]` validates a workspace (default: the active one); `update` runs `git pull --ff-only` and reinstalls; `uninstall [--clean]` removes the installed copy (and, with `--clean`, each module's io dir and the clone `sb` made). `set` / `list` / `delete` as before; `list` shows each workspace's source and a stale marker. The active workspace's installed `sb.flow.yaml` is the module registry every other verb reads. | [sbcli_ws.md](sbcli_ws.md) | L2; `sb 0.5.0+` (repository flow) |
| `sb init` | Adopt an existing user project as a swarmbotix module. Drops `sb.module.yaml` (sb 0.6.0+; no launch script, no `root:`) and an empty `swarmbotix_io/`; registers the module in the repository `sb.flow.yaml` above it and reinstalls (sb 0.5.0+), or else in the active workspace. What else the manifest can hold: [sbcli_module.md](sbcli_module.md). | [sbcli_init.md](sbcli_init.md) | L2 |
| `sb io *` | The codegen surface (sb 0.5.0+). `add pub` / `add sub` / `add service`, `edit`, `rm`, `list` mutate `sb.module.yaml` and regenerate the matching file under `swarmbotix_io/{publishers,subscribers,services}/`; `--proto` / `--fb` / `--raw [<mime>]` pick the Zenoh payload encoding; `update [<module>] [--services]` regenerates without changing the YAML. Publishers, subscribers, and services share one name space per module. | [sbcli_pubsub.md](sbcli_pubsub.md) §14 | `sb 0.5.0+` |
| `sb pub *` / `sb sub *` / `sb list` | Deprecated in favour of `sb io` (sb 0.5.0+; a warning on every use since 0.6.0, removed in 0.7.0, §10); unchanged, no new features. The codegen surface. Each `add` / `edit` / `rm` mutates `sb.module.yaml` AND regenerates the file under `swarmbotix_io/{publishers,subscribers}/`. Generated classes expose a `TopicOverrides` API for runtime per-segment override (swarm instances, see consuming-doc §3). | [sbcli_pubsub.md](sbcli_pubsub.md) (codegen reference) · [sbcli_pubsub_consuming.md](sbcli_pubsub_consuming.md) (consumer guide: main, build glue, swarm pattern) | L3 |
| `sb service *` | Deprecated in favour of `sb io add service` (sb 0.5.0+; a warning on every use since 0.6.0, removed in 0.7.0); unchanged. REST-style request/response over Zenoh queryables, JSON payloads, independent of pub/sub. `init` drops a self-contained router (`service.py` / `service.rs`) into the module's io dir with its namespace baked in; you write the routes. Rust and Python only. | [sbcli_reqres.md](sbcli_reqres.md) | L4 |
| `sb topic *` | Introspect the live swarm. `list` enumerates active topics on either transport; `listen` decodes frames as JSON lines; `pub` does a one-shot raw publish; `prune` clears iceoryx2 registrations left behind by dead publishers (the `(dead)` rows in `list`). `listen` and `pub` read the transport from the topic path's `zenoh` / `iox2` segment, so `-t zenoh` or `-t iox2` is needed only for a path without one (sb 0.4.1+). | (ref TBD — see `requirements.md` §"Topic Introspection") | L4 |
| `sb up` / `sb run` / `sb stop` / `sb down` / `sb attach` | Build and launch every module in the active workspace under tmux, one window per instance (`<module>-<id>`), or a single module or instance (`sb run <module>[:<id>]`, `--fg` for the current terminal), or kill the session. `sb 0.6.0+`: driven by each module's `sb.module.yaml`, with no launch scripts; `stop` closes the module's windows and runs its optional `scripts.stop`. | [sbcli_launch.md](sbcli_launch.md) · [sbcli_module.md](sbcli_module.md) | L5; native launch `sb 0.6.0+` |
| `sb install` / `sb app *` | Install app packages from a git URL or a local folder and run them as `sb <name> [args...]`, with no `PATH` change. `app init` makes any folder a package by writing `sb.app.yml`; `list` / `info` / `update` / `remove` manage what is installed. Global to the host, independent of workspaces. `sb 0.5.0+`: `sb install` on a source whose root holds `sb.flow.yaml` (or the pre-0.6.0 `flow.yaml`) instead of `sb.app.yml` installs a workspace (clone, install the flow, import and compile `messages:`, regenerate every module's io). | [sbcli_app.md](sbcli_app.md) · [sbcli_ws.md](sbcli_ws.md) §5.2 | `sb 0.2.1+`; workspaces `sb 0.5.0+` |
| `sb update` / `sb --version` | Self-update from the GitHub release page: `--version` adds an `update available` line (cached 24 h, silent offline, `SB_NO_UPDATE_CHECK=1` to skip); `update --check` compares and exits 10 when newer exists; `update` downloads, verifies the checksum and runs the package installer in place. | [sbcli_update.md](sbcli_update.md) | `sb 0.2.1+` |
| `sb gopro` | Freeze a module for production. Reads `sb.module.yaml` + `swarmbotix_io/` and writes a sanitized, language-agnostic `sb.prd.yml` that ships with the binary. | (ref TBD — see `requirements.md` §"Production") | L5 |
| `sb ui` *(not shipped)* | Start `swarmctl` (REST + MCP server + visual flow editor) in a tmux pane and open the browser. | (ref TBD — `requirements.md` §"Web UI" + `plan/level6.html`, both in the source repo) | L6 |

---

## 5. Typical lifecycle

**Install a published workspace** (sb 0.5.0+). The shortest path from a
fresh host to a running swarm, for a repository whose root holds an
`sb.flow.yaml`:

```bash
sb config set device <name>
sb install https://github.com/<org>/<workspace>.git --dir ~/<workspace>   # or: cd clone && sb install .
sb up
```

`sb install` clones the repository (into `--dir`, or
`~/.swarmbotix/repos/<workspace>/` without it), installs its `sb.flow.yaml`
as the active workspace, imports and compiles its `messages:`, and
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
│ sb ws create my_project  # writes ./sb.flow.yaml, installs, active  │
└─────────────────────────────────────────────────────────────────────┘
            ↓
┌─ Adopt modules (once per app) ──────────────────────────────────────┐
│ sb init --rust   modules/camera_app                                 │
│ sb init --python modules/detector                                   │
│   (each module gets sb.module.yaml + swarmbotix_io/ and a line      │
│    in ./sb.flow.yaml; the workspace is reinstalled)                 │
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
│ sb up                    # build, then tmux session sb-my_project   │
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
│ git add sb.flow.yaml modules && git commit && git push              │
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
| "What goes in a repository's `sb.flow.yaml`, and what does the installed copy hold?" | [sbcli_ws.md](sbcli_ws.md) §4 |
| "What can a module's `sb.module.yaml` declare, and what are the defaults?" | [sbcli_module.md](sbcli_module.md) §2 + §3.2 |
| "How do I run four instances of one module?" | [sbcli_ws.md](sbcli_ws.md) §4.6 + [sbcli_launch.md](sbcli_launch.md) §1.1 |
| "How do I move a 0.5 workspace to the 0.6 file names?" | [sbcli_module.md](sbcli_module.md) §11 |
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

## 10. Deprecations (sb 0.6.0+)

Everything below still works in 0.6.0 and prints a warning when used;
it is removed in the release named in the table. The migration, item by
item, is [sbcli_module.md](sbcli_module.md) §11.

| item | since | removed | replacement |
|---|---|---|---|
| `sb pub`, `sb sub`, `sb list`, `sb service` | 0.5.0 (help only), 0.6.0 (warning) | 0.7.0 | `sb io` |
| `sb.dev.yml` | 0.6.0 | 0.7.0 | `sb.module.yaml` |
| `flow.yaml` | 0.6.0 | 0.7.0 | `sb.flow.yaml` |
| `runscript.bash` | 0.6.0 | 0.7.0 | `scripts:` in `sb.module.yaml` |
| `stopscript.bash` | 0.6.0 | 0.7.0 | `scripts.stop`, or nothing |
| top-level `instances:` in the flow | 0.6.0 | 0.7.0 | `instances` under the module entry |
| `sb init --docker` bash template | 0.6.0 | 0.6.0 | `instances[].docker`, launched natively in a later release; until then `sb up` skips a native module whose instances declare `docker:`, so keep the module's existing launch script, or start the container from `scripts.start` ([sbcli_module.md](sbcli_module.md) §11.6) |
| `SB_NO_TMUX=1` | 0.6.0 | 0.7.0 | `sb run --fg` |

Warning format, on stderr, once per item per process:

```
warning: <item> is deprecated since 0.6.0 and will be removed in 0.7.0; <hint>
```

Warnings never change stdout, so scripts that parse `sb` output are
unaffected. `sb ws check` and `sb doctor` list the deprecated files a
workspace still uses in their `deprecated` row
([sbcli_doctor.md](sbcli_doctor.md) §4.11). From 0.7.0 the old names
are no longer read, and for one minor version the error names the
replacement.
