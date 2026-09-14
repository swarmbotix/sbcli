<h1 align="center">swarmbotix CLI</h1>

<h3 align="center">One toolchain for building, launching, and inspecting a polyglot pub/sub swarm</h3>

<h4 align="center">
  <a href="#installation">Install</a>
  |
  <a href="#quick-start">Quick Start</a>
  |
  <a href="documents/sbcli.md">Docs</a>
  |
  <a href="installguide_ubuntu.md">Linux Guide</a>
  |
  <a href="installguide_windows.md">Windows Guide</a>
  |
  <a href="https://github.com/swarmbotix/sbcli/releases">Releases</a>
</h4>

<div align="center">
  <a href="https://github.com/swarmbotix/sbcli/actions/workflows/build.yml"><img src="https://github.com/swarmbotix/sbcli/actions/workflows/build.yml/badge.svg" alt="Build"/></a>
  <a href="https://github.com/swarmbotix/sbcli/releases/latest"><img src="https://img.shields.io/github/v/release/swarmbotix/sbcli?label=release" alt="Latest release"/></a>
  <img src="https://img.shields.io/badge/rust-1.82%2B-orange" alt="Rust 1.82+"/>
  <img src="https://img.shields.io/badge/platforms-Linux%20x86__64%20%7C%20Linux%20aarch64%20%7C%20Windows%20x86__64-blue" alt="Platforms"/>
  <img src="https://img.shields.io/badge/license-Apache--2.0-green" alt="License"/>
</div>

<br/>

# swarmbotix CLI (`sb`)

**`sb` is a single Rust binary** that gives you one toolchain for building,
launching, and inspecting a heterogeneous pub/sub system across **Rust, Python,
C++, Flutter, and Unity**, over two transports: **[Zenoh](https://zenoh.io/)**
for the network and **[iceoryx2](https://github.com/eclipse-iceoryx/iceoryx2)**
for zero-copy shared memory on one device.

> `sb` does **not** mock, wrap, or hide either transport. Generated code calls
> `zenoh::Session::open` and `iceoryx2::node::NodeBuilder` directly. There is no
> swarmbotix runtime and nothing is added to your application's dependencies:
> `sb` is build-time and launch-time orchestration only, and it never touches
> your build system.

---

## Table of Contents

- [Features](#features)
- [Installation](#installation)
- [Quick Start](#quick-start)
- [CLI Commands](#cli-commands)
- [On-Disk Layout](#on-disk-layout)
- [Architecture](#architecture)
- [Language Support](#language-support)
- [Versions](#versions)
- [Documentation](#documentation)
- [Development](#development)
- [Contributing](#contributing)
- [License](#license)

## Features

### Two transports, one workflow

- **Zenoh for the network** -- cross-device pub/sub; the same generated code
  runs on a laptop, a Jetson, or a phone
- **iceoryx2 for shared memory** -- zero-copy, typed payloads between processes
  on one host, for image and point-cloud rates
- **Pick per publisher** -- `--zenoh` or `--iox2` on each `sb pub add` /
  `sb sub add`; a module can use both at once
- **Uniform topic naming** -- every topic is
  `/<device>/<workspace>/<module>/<transport>/<topic>`, so
  `/dev01/perception/camera/iox2/image_raw` tells you where it lives and how it
  travels

### Developer experience

- **Bring your own project** -- `sb init` adopts an existing Cargo crate,
  Python project, C++ tree, Flutter app, or Unity game by dropping bookkeeping
  files into it; it never scaffolds or moves your code
- **Message vault** -- `.proto` files are the single source of truth; `sb`
  compiles them into iceoryx2 ctypes, protobuf, and FlatBuffers bindings per
  language, with a shipped ROS2-style bundle (`std/*`, `geometry_msgs`,
  `sensor_msgs`, ...) and a `swarmbotix/` style
- **Pub/sub codegen** -- each `sb pub add` / `sb sub add` writes one file under
  `swarmbotix_io/` that you wire into your build; edit the message or the
  transport and the file regenerates
- **Workspaces** -- named, isolated collections of modules, comparable to
  Python virtual environments but for a swarm
- **Layered config** -- one `sb.config.yml` per host, an optional override per
  workspace, environment variables on top; `sb config show` prints the merged
  result for scripts and LLMs
- **`sb doctor`** -- validates `protoc`, `flatc`, `tmux`, the two transport
  libraries, and whether their versions match what this `sb` links against

### Launch and production

- **`sb up`** -- starts every module in the active workspace under tmux, one
  window per module, driven by that module's `runscript.bash`
- **Per-module lifecycle** -- `sb run` restarts one module with passthrough
  args, `sb stop` runs its `stopscript.bash`, `sb attach` drops you into the
  session, `sb down` tears it all down
- **Docker dispatch** -- `sb init --docker` lets a module run as a container
  through the same lifecycle
- **`sb gopro`** -- freezes a module's interface into a language-agnostic
  `sb.prd.yml` that ships with the binary
- **Request/response** -- `sb service init` drops a REST-style router over
  Zenoh queryables into a Rust or Python module, independent of pub/sub

### Introspection

- **`sb topic list`** -- enumerates live topics on both transports, marking
  iceoryx2 registrations left behind by dead publishers
- **`sb topic listen`** -- decodes frames as JSON lines
- **`sb topic pub`** -- one-shot raw publish for smoke tests
- **`sb topic prune`** -- clears dead iceoryx2 registrations

## Installation

### From a GitHub release (recommended)

A release is one zip per architecture with the installer inside it. Download
the one matching your machine from the
[Releases](https://github.com/swarmbotix/sbcli/releases) page.

**Linux** (x86_64 or aarch64; `uname -m` tells you which):

```bash
sha256sum -c swarmbotix-<version>-linux-x86_64.zip.sha256   # optional
unzip swarmbotix-<version>-linux-x86_64.zip
./swarmbotix-<version>-linux-x86_64/install.sh
```

**Windows** (x86_64):

```powershell
Expand-Archive swarmbotix-<version>-windows-x86_64.zip
.\swarmbotix-<version>-windows-x86_64\install.ps1
```

The installer creates `~/.swarmbotix/{bin,messages,documents}/`, installs the
binary to `~/.swarmbotix/bin/`, merges the `.proto` vault without overwriting
your edits, replaces `documents/`, seeds `sb.config.yml` only if none exists,
places an uninstaller, and offers to add `~/.swarmbotix/bin` to your `PATH`.
Unzip anywhere except inside `~/.swarmbotix` itself.

Linux binaries are linked against glibc 2.17, so they load on Ubuntu 20.04 and
22.04, Debian 11, JetPack 5 and 6, and anything newer, with no distro floor to
check.

Verify:

```bash
sb --version
sb doctor
```

### From source

Requires a stable Rust toolchain (`rust-version = 1.82`).

```bash
git clone https://github.com/swarmbotix/sbcli
cd sbcli
cargo build --release
./target/release/sb --version
```

The build alone gives you the binary. The vault, docs, and config template come
from a packaged release; see [Development](#development) for cutting one.

### Runtime prerequisites

The binary is self-contained, but some commands shell out. Install what you
need; `sb doctor` reports what is missing:

| Tool | Needed for | Minimum |
|---|---|---|
| `protoc` | `.proto` codegen | libprotoc 3.21+ |
| `flatc` | FlatBuffers codegen | 24.x+ |
| `libzenohc.so` | cross-device pub/sub | matches Zenoh 1.9 |
| `libiceoryx2_ffi_c.so` | same-device pub/sub | matches iceoryx2 0.9 |
| `tmux` | `sb up` and friends | 3.2+ |

Per-language clients: Rust adds `zenoh` / `iceoryx2` to `Cargo.toml`; Python
needs `pip install eclipse-zenoh iceoryx2`; C++ needs the two shared libraries
system-wide; Flutter uses `zenoh_flutter` (Zenoh only); Unity needs `zenoh.dll`
in `Assets/Plugins/` (Zenoh only).

## Quick Start

From a fresh host to a running swarm. Every stage reads what the previous one
wrote to disk; no state lives in memory between `sb` invocations.

**1. Set up the host** (once)

```bash
sb doctor                                    # check tooling
sb config set protoc /usr/local/bin/protoc   # point at tools doctor could not find
sb message list                              # first run installs the std/* bundle
sb message compile                           # generate bindings into message_targets/
```

**2. Create a workspace** (once per project)

```bash
sb ws create my_project
sb ws set    my_project
```

**3. Adopt your modules** (once per app)

```bash
sb init --rust   /path/to/camera_app
sb init --python /path/to/detector
```

Each module gets `sb.dev.yml`, a `runscript.bash` stub, and an empty
`swarmbotix_io/`, and is registered in the workspace's `flow.yaml`.

**4. Wire pub/sub** (iterate freely)

```bash
cd /path/to/camera_app
sb pub add -m ros2/std/ImageStamped image_raw --iox2

cd /path/to/detector
sb sub add -m ros2/std/ImageStamped /dev01/my_project/camera_app/iox2/image_raw --iox2
```

Each command regenerates one file under `swarmbotix_io/`. Wire it into your
build as described in
[sbcli_pubsub_consuming.md](documents/sbcli_pubsub_consuming.md).

**5. Run and inspect**

```bash
# edit each module's runscript.bash so it actually launches the app, then:
sb up                                                    # tmux session comes up
sb attach                                                # look inside
sb topic list                                            # what is live, on both transports
sb topic listen /dev01/my_project/camera_app/iox2/image_raw
sb run camera_app --id 2                                 # restart one module with args
sb stop camera_app                                       # run its stopscript.bash
```

**6. Freeze for production**

```bash
sb gopro --all   # writes sb.prd.yml next to each sb.dev.yml
sb down          # tear down the session
```

## CLI Commands

`sb --help` is the authority on what a given build accepts. All clap
definitions live in [crates/sb-cli/src/main.rs](crates/sb-cli/src/main.rs).

### Introspection

| Command | Purpose |
|---|---|
| `sb topic list` | Enumerate live topics on both transports |
| `sb topic listen <topic>` | Decode frames as JSON lines |
| `sb topic pub <topic> <bytes-hex>` | One-shot raw publish |
| `sb topic prune` | Clear iceoryx2 registrations left by dead publishers |

### Setup

| Command | Purpose |
|---|---|
| `sb doctor` | Validate this host's tooling and vault against `sb.config.yml` |
| `sb config <open\|set\|show\|get>` | Read and edit the layered configuration |
| `sb message <list\|new\|edit\|rm\|backends\|compile>` | Manage the message vault and compile bindings |

### Workspaces and modules

| Command | Purpose |
|---|---|
| `sb ws <create\|set\|list\|delete>` | Workspace lifecycle under `~/.swarmbotix/workspaces/` |
| `sb init [path] [--rust\|--python\|--cpp\|--flutter\|--unity] [--docker] [--force]` | Adopt an existing project as a module |
| `sb list` | List publishers and subscribers on the resolved module |
| `sb gopro [--all \| --module <name>]` | Freeze a module for production (`sb.prd.yml`) |

### Pub/sub codegen

| Command | Purpose |
|---|---|
| `sb pub <list\|add\|edit\|rm>` | Manage publishers; each mutation regenerates the file |
| `sb sub <list\|add\|edit\|rm>` | Manage subscribers |
| `sb service init` | Scaffold REST-style request/response over Zenoh queryables |

With no `--zenoh` / `--iox2` flag, `sb pub add` and `sb sub add` default to
iceoryx2, while `sb topic listen` and `sb topic pub` default to Zenoh.

### Launch

| Command | Purpose |
|---|---|
| `sb up` | Launch every module in the active workspace in tmux |
| `sb run <module> [args...]` | Launch or restart a single module, passthrough args |
| `sb stop <module> [args...]` | Stop a module by running its `stopscript.bash` |
| `sb attach` | Attach to the workspace's tmux session |
| `sb down` | Kill the session |

## On-Disk Layout

Every `sb` invocation reads from and writes to one of three roots:

```
~/.swarmbotix/                       (1) swarmbotix home
├── sb.config.yml                        host-wide config (tool paths, defaults)
├── active                               name of the active workspace
├── messages/                        (2) message vault, one subdir per style
│   ├── ros2/
│   │   ├── message_definitions/         source .proto files
│   │   └── message_targets/             generated bindings (iox2 / proto / fb)
│   └── swarmbotix/
└── workspaces/<name>/
    ├── flow.yaml                        module name -> sb.dev.yml path
    └── sb.config.yml                    per-workspace override (optional)

/path/to/<user_project>/             (3) an adopted module, located anywhere
├── sb.dev.yml                           dev-time interface (gitignored)
├── sb.prd.yml                           production interface (ships with binary)
├── swarmbotix_io/                       generated pub/sub files
├── runscript.bash                       user-written launch script
└── ...your own source tree...
```

The vault is global to the host, workspaces are isolated registries, and
modules stay wherever you keep your source. Configuration resolves as
environment override, then workspace, then global, then built-in defaults; see
[sbcli_config.md](documents/sbcli_config.md).

## Architecture

One Cargo workspace, 13 crates. Only packaging differs between platforms; the
Rust source is not split per OS.

| Crate | Role |
|---|---|
| [sb-core](crates/sb-core) | Pure logic: topic naming, config types, transport enum. No IO. |
| [sb-config](crates/sb-config) | Layered `sb.config.yml` loader (env, workspace, global, built-in) |
| [sb-vault](crates/sb-vault) | Message vault: embedded standard bundle, CRUD, `protoc` invocation |
| [sb-iox2-typegen](crates/sb-iox2-typegen) | Flattens protobuf descriptors into iceoryx2-compatible fixed-size types |
| [sb-doctor](crates/sb-doctor) | Environment validation behind `sb doctor` |
| [sb-workspace](crates/sb-workspace) | Workspace lifecycle, `flow.yaml`, module adoption |
| [sb-codegen](crates/sb-codegen) | Per-language pub/sub codegen (minijinja templates) |
| [sb-pubsub](crates/sb-pubsub) | `sb pub` / `sb sub` mutation and codegen invocation |
| [sb-discover](crates/sb-discover) | Snapshot topic discovery (Zenoh enumeration, iceoryx2 service list) |
| [sb-listen](crates/sb-listen) | Per-transport frame iterators and one-shot raw publishers |
| [sb-launch](crates/sb-launch) | tmux session launch (`sb up` / `run` / `stop` / `down` / `attach`) |
| [sb-gopro](crates/sb-gopro) | Production export to `sb.prd.yml` |
| [sb-cli](crates/sb-cli) | The `sb` binary itself |

Top-level directories:

- [documents/](documents/) -- shipped reference docs, packaged into every
  release and installed to `~/.swarmbotix/documents/`
- [messages/](messages/) -- the shipped vault source, in `ros2/` and
  `swarmbotix/` styles
- [platforms/](platforms/) -- per-OS packaging: `version.json`,
  `dist-tooling/`, committed `dist/`
- [plan/](plan/) -- build-stage plans and level reports
- [requirements.md](requirements.md) -- the canonical specification, which
  takes precedence over `documents/` where they disagree

## Language Support

What `sb pub add` / `sb sub add` can generate, per language and transport:

| | Zenoh | iceoryx2 |
|---|---|---|
| Rust | pub ✓ · sub ✓ | pub ✓ · sub ✓ |
| Python | pub ✓ · sub ✓ | pub ✓ · sub ✓ |
| C++ | pub ✓ · sub ✓ | pub ✓ · sub ✓ |
| Flutter | pub ✓ · sub ✓ | -- (mobile sandbox) |
| Unity | pub ✓ · sub ✓ | -- (game sandbox) |

Zenoh payloads are raw bytes at the API boundary; iceoryx2 payloads are always
typed, fixed-size structs generated from the vault. The two empty cells are
structural: a mobile or game sandbox has no shared memory to map.

### Platform support

| Platform | Architecture | Binary | Notes |
|---|---|---|---|
| Linux | x86_64 | `sb` | glibc 2.17 floor |
| Linux | aarch64 | `sb` | Jetson, Raspberry Pi 64-bit, ARM servers; same glibc floor |
| Windows | x86_64 | `sb.exe` | PowerShell 5.1+ for the installer |
| macOS | -- | -- | slot reserved under `platforms/mac/`, not built yet |

## Versions

| | |
|---|---|
| **Current version** | **0.1.41** |
| **Main dependency (Zenoh)** | **1.9.0** (network transport, cross-device) |
| **Main dependency (iceoryx2)** | **0.9.3** (shared-memory transport, on-device) |
| Edition / MSRV | Rust 2021, `rust-version = 1.82`, stable toolchain |
| License | Apache-2.0 |

The `sb` version is canonical in `[workspace.package]` of the root
[Cargo.toml](Cargo.toml#L21) and mirrored into
[platforms/linux/version.json](platforms/linux/version.json) and
[platforms/windows/version.json](platforms/windows/version.json). It is what
`sb --version` prints and what names the release zip.

The two transport versions are canonical in [versions.json](versions.json):

```json
{
  "zenoh": "1.9.0",
  "iceoryx2": "0.9.3"
}
```

That file is the single source of truth for the third-party transport pins. It
is synced into `[workspace.dependencies]` as **exact** pins (`zenoh = "=1.9.0"`,
`iceoryx2 = "=0.9.3"`). Exactness is required, not stylistic: iceoryx2 writes its
full `major.minor.patch` into every shared-memory segment and compares it for
equality on open, so a process built against 0.9.0 cannot see the services of a
process built against 0.9.3. The pin must also match the wheel installed by
`pip install iceoryx2` and the C library `libiceoryx2_ffi_c.so` on the target
machine. `sb doctor` checks both.

## Documentation

Shipped reference docs, installed to `~/.swarmbotix/documents/` with every
release. Start with [sbcli.md](documents/sbcli.md).

| Document | Covers |
|---|---|
| [sbcli.md](documents/sbcli.md) | Overview: what `sb` is, the three on-disk roots, topic naming, the command map |
| [sbcli_config.md](documents/sbcli_config.md) | `sb config`: file locations, layering chain, key reference |
| [sbcli_doctor.md](documents/sbcli_doctor.md) | `sb doctor`: every check, OK / FAIL / SKIP semantics |
| [sbcli_init.md](documents/sbcli_init.md) | `sb init`: adopting a project in each supported language |
| [sbcli_ws.md](documents/sbcli_ws.md) | `sb ws`: workspace lifecycle, `active` marker, `flow.yaml` |
| [sbcli_messages.md](documents/sbcli_messages.md) | `sb message`: vault lifecycle, codegen, capacity configuration |
| [sbcli_pubsub.md](documents/sbcli_pubsub.md) | `sb pub` / `sb sub` / `sb list`: resolution, validation, conflicts |
| [sbcli_pubsub_consuming.md](documents/sbcli_pubsub_consuming.md) | Per-language guide to wiring generated files into your build |
| [sbcli_launch.md](documents/sbcli_launch.md) | `sb up` / `run` / `stop` / `down` / `attach`: the tmux session and lifecycle scripts |
| [sbcli_reqres.md](documents/sbcli_reqres.md) | `sb service`: request/response over Zenoh queryables |
| [sbcli_docker_runscript.md](documents/sbcli_docker_runscript.md) | `sb init --docker`: `flow.yaml` instances and docker dispatch |

Maintainer docs that stay in the repo: [installguide_ubuntu.md](installguide_ubuntu.md),
[installguide_windows.md](installguide_windows.md), [requirements.md](requirements.md),
and [platforms/README.md](platforms/README.md).

## Development

### Build and test

```bash
cargo build --release
cargo test --workspace
```

### Lint and format

The same three gates CI runs on every tag, on both Linux and Windows:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Tests that need `protoc`, a live transport, or shared memory are `#[ignore]`d
in-tree; run them by hand per their doc comments.

### Package a release

Build-machine prerequisites: `cargo`, `jq`, `zip`, `rsync`, `sha256sum`.

Set the target platform in a repository-root `.env` (copy
[.env.example](.env.example); `OS` is required and accepts `linux`, `windows`,
or `mac`), then run the platform's staging script:

```bash
./platforms/linux/dist-tooling/build.sh                # Linux, both architectures
./platforms/linux/dist-tooling/build.sh --arch linux-aarch64
pwsh -File platforms\windows\dist-tooling\build.ps1    # Windows
```

The script reads every build fact from `platforms/<os>/version.json`, refuses
to stage if that version has drifted from `cargo metadata`, builds against an
explicit `--target`, re-checks `sb --version` on the produced binary, stages the
payload with the installer inside it, then zips and checksums it into
`platforms/<os>/dist/<version>/`. Useful flags: `--skip-build`, `--no-zip`.

### Release

Pushing a `v*` tag runs [build.yml](.github/workflows/build.yml): `check` on
both platforms, `build` for all three targets through the same staging scripts,
then a GitHub Release with one zip and checksum per architecture. A tag whose
number disagrees with `Cargo.toml` fails in seconds, before anything is built.
A manual dispatch runs the same pipeline without publishing.

Full maintainer procedure: [installguide_ubuntu.md](installguide_ubuntu.md)
Part 2, [installguide_windows.md](installguide_windows.md), and
[platforms/README.md](platforms/README.md).

## Contributing

Issues and pull requests are welcome. Before opening a PR, run the three gates
under [Lint and format](#lint-and-format); CI runs exactly those and nothing is
packaged unless they pass. Changes to a command's flags, output, or generated
code shape should update the owning document under [documents/](documents/) in
the same PR, since those docs ship with the binary.

## License

Apache-2.0.
