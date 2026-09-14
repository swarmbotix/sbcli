# swarmbotix CLI (`sb`)

A single Rust binary that provides one toolchain for building, launching, and
inspecting a heterogeneous pub/sub system across **Rust, Python, C++, Flutter,
and Unity**, over two transports: **Zenoh** (network, cross-device) and
**iceoryx2** (zero-copy shared memory, same device).
 
| | |
|---|---|
| **Current version** | **0.1.41** |
| **Main dependency (Zenoh)** | **1.9.0** (network transport, cross-device) |
| **Main dependency (iceoryx2)** | **0.9.3** (shared-memory transport, on-device) |
| Edition / MSRV | Rust 2021, `rust-version = 1.82`, stable toolchain |
| License | Apache-2.0 |
| Platforms | Linux x86_64, Windows x86_64 |

The version number is canonical in `[workspace.package]` of the root
[Cargo.toml](Cargo.toml#L21) and mirrored into
[platforms/linux/version.json](platforms/linux/version.json) and
[platforms/windows/version.json](platforms/windows/version.json).

The two transport versions above are canonical in [versions.json](versions.json):

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
machine.

---

## What `sb` does

1. **Message vault.** Manages `.proto` source-of-truth files plus per-language
   generated bindings (iox2 ctypes, protobuf, FlatBuffers).
2. **Workspaces.** Named, isolated collections of modules, comparable to Python
   virtual environments but for swarmbotix projects.
3. **Module adoption.** Turns an existing project (Cargo crate, Python project,
   C++ tree, Flutter app, Unity game) into a swarmbotix *module* by adding
   bookkeeping files to it. It never scaffolds the project for you.
4. **Pub/sub codegen.** Generates publisher and subscriber stubs per language and
   per transport against the vault's types.
5. **Launch.** Starts every module in a workspace under tmux, one pane per
   module, driven by that module's `runscript.bash`.
6. **Introspection.** Lists topics, listens for frames, and performs one-shot
   publishes across both transports.

What it explicitly does **not** do: it does not mock, wrap, or hide either
transport. Generated code calls `zenoh::Session::open` and
`iceoryx2::node::NodeBuilder` directly. There is no swarmbotix runtime and no
runtime dependency added to your application; `sb` is purely build-time and
launch-time orchestration, and it does not touch your build system.

### Topic naming

```
/<device>/<workspace>/<module>/<transport>/<topic>
example: /dev01/perception/camera/iox2/image_raw
```

---

## Installation (end user)

A release consists of exactly three files in `platforms/<os>/dist/<version>/`:
the package zip, its `.sha256`, and a version-agnostic installer.

```bash
cd platforms/linux/dist/0.1.41
sha256sum -c swarmbotix-0.1.41-linux-x86_64.zip.sha256   # optional integrity check
./install.sh
```

On Windows, run `install.ps1` from `platforms\windows\dist\0.1.41` instead.

The installer extracts the sibling zip, creates `~/.swarmbotix/{bin,messages,documents}/`,
installs the binary to `~/.swarmbotix/bin/sb`, merges `.proto` files without
overwriting local edits, replaces `documents/`, seeds `sb.config.yml` from the
template only if it is absent, writes an `uninstall.sh`, and offers to add
`~/.swarmbotix/bin` to your `PATH`.

Verify:

```bash
sb --version    # sb 0.1.41
sb doctor       # validates host tooling against sb.config.yml
```

Full procedure: [installguide_ubuntu.md](installguide_ubuntu.md) (Linux) and
[installguide_windows.md](installguide_windows.md).

### Runtime prerequisites

`sb doctor` reports which of these are missing:

| Tool | Minimum |
|---|---|
| `protoc` | libprotoc 3.21+ |
| `flatc` | 24.x+ |
| `libzenohc.so` | matching Zenoh 1.9 |
| `libiceoryx2_ffi_c.so` | matching iceoryx2 0.9 |
| `tmux` | 3.2+ |

Per-language client requirements: Rust adds `zenoh` / `iceoryx2` to `Cargo.toml`;
Python needs `pip install eclipse-zenoh iceoryx2`; C++ needs the two shared
libraries system-wide; Flutter uses `zenoh_flutter` (Zenoh only); Unity needs
`zenoh.dll` in `Assets/Plugins/` (Zenoh only).

---

## Command surface

All clap definitions live in [crates/sb-cli/src/main.rs](crates/sb-cli/src/main.rs).

| Command | Purpose |
|---|---|
| `sb doctor` | Validate this host's tooling against `sb.config.yml` |
| `sb message <list\|new\|edit\|rm\|backends\|compile>` | Manage the message vault and compile bindings |
| `sb config <open\|set\|show\|get>` | Read and edit layered configuration |
| `sb ws <create\|set\|list\|delete>` | Manage workspaces under `~/.swarmbotix/workspaces/` |
| `sb init [rootpath]` | Adopt an existing project directory as a module |
| `sb list` | List publishers and subscribers on the resolved module |
| `sb pub <list\|add\|edit\|rm>` | Manage publishers |
| `sb sub <list\|add\|edit\|rm>` | Manage subscribers |
| `sb service init` | Scaffold REST-style request/response routing over Zenoh queryables |
| `sb topic <list\|listen\|pub\|prune>` | Inspect the live swarm |
| `sb up` / `sb run` / `sb stop` / `sb down` / `sb attach` | tmux launch lifecycle |
| `sb gopro` | Freeze a module for production (`sb.prd.yml`) |

**Transport defaults.** When no `--zenoh` / `--iox2` flag is given, `sb pub add`
and `sb sub add` default to iceoryx2 (the on-device transport), while
`sb topic listen` and `sb topic pub` default to Zenoh.

---

## On-disk layout

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

The vault is global to the host, workspaces are isolated registries, and modules
stay wherever you keep your source code (`sb init` never moves them).

---

## Repository layout

One Cargo workspace, 13 crates. Only packaging differs between platforms; the
Rust source is not split per platform.

| Crate | Role |
|---|---|
| [crates/sb-core](crates/sb-core) | Pure logic: topic naming, config types, transport enum. No IO. |
| [crates/sb-config](crates/sb-config) | Layered `sb.config.yml` loader (env, workspace, global, builtin) |
| [crates/sb-vault](crates/sb-vault) | Message vault: embedded standard bundle, CRUD, `protoc` invocation |
| [crates/sb-iox2-typegen](crates/sb-iox2-typegen) | Flattens protobuf descriptors into iceoryx2-compatible types |
| [crates/sb-doctor](crates/sb-doctor) | Environment validation behind `sb doctor` |
| [crates/sb-workspace](crates/sb-workspace) | Workspace lifecycle, `flow.yaml`, module adoption |
| [crates/sb-codegen](crates/sb-codegen) | Per-language pub/sub codegen (minijinja templates) |
| [crates/sb-pubsub](crates/sb-pubsub) | `sb pub` / `sb sub` mutation and codegen invocation |
| [crates/sb-discover](crates/sb-discover) | Snapshot topic discovery (Zenoh enumeration, iceoryx2 service list) |
| [crates/sb-listen](crates/sb-listen) | Per-transport frame iterators and one-shot raw publishers |
| [crates/sb-launch](crates/sb-launch) | tmux session launch (`sb up/run/down/attach`) |
| [crates/sb-gopro](crates/sb-gopro) | Production export to `sb.prd.yml` |
| [crates/sb-cli](crates/sb-cli) | The `sb` binary itself |

Top-level directories:

- [documents/](documents/) shipped reference docs; these are packaged into the
  release and installed to `~/.swarmbotix/documents/`
- [examples/](examples/) runnable example applications, one per
  (language, transport) pair, each with a `run.sh`
- [messages/](messages/) the shipped vault source, in `ros2/` and `swarmbotix/` styles
- [platforms/](platforms/) per-OS packaging: `version.json`, `dist-tooling/`, committed `dist/`
- [plan/](plan/) build-stage plans (`level1..level6.html`) and level reports
- [requirements.md](requirements.md) the canonical specification, which takes
  precedence over `documents/` where they disagree

---

## Documentation

Shipped reference docs, all under [documents/](documents/):

| Document | Covers |
|---|---|
| [sbcli.md](documents/sbcli.md) | Overview and entry point: what `sb` is and which doc to open |
| [sbcli_config.md](documents/sbcli_config.md) | `sb config`: file locations, layering chain, key reference |
| [sbcli_doctor.md](documents/sbcli_doctor.md) | `sb doctor`: what is checked, OK/FAIL/SKIP semantics |
| [sbcli_init.md](documents/sbcli_init.md) | `sb init`: adopting a project of each supported language |
| [sbcli_ws.md](documents/sbcli_ws.md) | `sb ws`: workspace lifecycle, `active` marker, `flow.yaml` |
| [sbcli_messages.md](documents/sbcli_messages.md) | `sb message`: vault lifecycle, codegen, capacity configuration |
| [sbcli_pubsub.md](documents/sbcli_pubsub.md) | `sb pub` / `sb sub` / `sb list`: resolution, validation, conflicts |
| [sbcli_pubsub_consuming.md](documents/sbcli_pubsub_consuming.md) | Per-language guide to wiring generated files into your build |
| [sbcli_launch.md](documents/sbcli_launch.md) | `sb up` / `run` / `stop` / `down` / `attach`: tmux session, lifecycle scripts, passthrough args |
| [sbcli_reqres.md](documents/sbcli_reqres.md) | `sb service`: request/response over Zenoh queryables |
| [sbcli_docker_runscript.md](documents/sbcli_docker_runscript.md) | `sb init --docker`: `flow.yaml` instances and docker dispatch |

Maintainer-only documents that are not shipped: [installguide_ubuntu.md](installguide_ubuntu.md),
[installguide_windows.md](installguide_windows.md), [requirements.md](requirements.md),
and [platforms/README.md](platforms/README.md).

---

## Building from source

Build-machine prerequisites: `cargo` (stable toolchain), `jq`, `zip`, `rsync`,
`sha256sum`.

```bash
cargo build --release
cargo test --workspace
```

To produce a release package, first set the target platform in a repository-root
`.env` (copy from [.env.example](.env.example); `OS` is required and accepts
`linux`, `windows`, or `mac`), then run the packaging script:

```bash
./platforms/linux/dist-tooling/build.sh              # Linux
pwsh -File platforms\windows\dist-tooling\build.ps1  # Windows
```

The script reads every build fact from `platforms/<os>/version.json` rather than
detecting the OS at runtime, fails if the declared version has drifted from
`cargo metadata`, builds against an explicit `--target`, re-checks `sb --version`
on the produced artifact, stages the payload into
`dist/<version>/<package_stem>/`, then zips and checksums it. Useful flags:
`--skip-build`, `--no-zip`.

Staging only clears the single `<version>/` slot being built, so Linux and
Windows packages can be cut on separate machines and merged by commit. `dist/`
is committed to the repository; `target/` is not.

Full maintainer procedure: [installguide_ubuntu.md](installguide_ubuntu.md) Part 2 and
[platforms/README.md](platforms/README.md).

---

## License

Apache-2.0.
