# `sb.module.yaml`: Module Manifest Reference

Official reference for `sb.module.yaml`, the one file a module carries:
its identity, how it is built and started, the environment it runs in,
the tools it needs, the platforms it supports, and its publishers,
subscribers, and services.

This document is normative for behavior shipped in `sb 0.6.0+`.

The canonical spec is `requirements.md` in the sbcli **source
repository**, a maintainer document that is not part of your install;
when it and this page disagree, it wins.

Related: [sbcli_launch.md](sbcli_launch.md) (what `sb up`, `sb run`
and `sb stop` do with the manifest), [sbcli_ws.md](sbcli_ws.md)
(`sb.flow.yaml`: modules, instances, wires), [sbcli_init.md](sbcli_init.md)
(which writes the first manifest), [sbcli_pubsub.md](sbcli_pubsub.md)
(the publisher, subscriber, and service entries).

**Added in sb 0.6.0.** `sb.module.yaml` replaces the pre-0.6.0 module
file, and its `scripts:` block replaces the per-module launch and stop
scripts. `sb 0.7.0+` reads only `sb.module.yaml` and runs no such
script; §11 is the migration guide, required since 0.7.0.

---

## 1. Conceptual model

A module folder holds one manifest and no launch scripts. `sb` reads the
manifest, merges in the defaults for the module's `language:` and the
overlay for the host it runs on, and from the result knows how to build
the module, how to start one process per instance, and what to clean up
after stopping it. The same file drives Linux and Windows.

The smallest valid manifest is two lines:

```yaml
module: ips_track
language: rust
```

With only that, `sb up` builds the module with `cargo build --release`
and starts `target/release/ips_track` in a tmux window (§3.2). Declare a
key only when the language default does not fit.

The manifest is committed with the module and is shareable: it carries
no path that is true on only one machine (§6.3).

### 1.1 Where it lives

`<module root>/sb.module.yaml`. The module root is the directory that
holds the file, unless `root:` says otherwise (§2). A workspace's
`sb.flow.yaml` names the module directory, and `sb` looks for
`sb.module.yaml` in it. `sb 0.7.0+` looks for no other name (§11.1).

### 1.2 What `sb` does with it

| Verb | Uses |
|---|---|
| `sb up`, `sb run` | `scripts.build` (run inline, before any window opens), then `scripts.start` (one window per instance), with `env:` applied ([sbcli_launch.md](sbcli_launch.md) §1) |
| `sb run --fg` | the same build and start, in your terminal instead of tmux |
| `sb stop` | closes the module's windows, then runs `scripts.stop` if declared |
| `sb doctor`, `sb ws check` | `requires:` (`ws requires`), `platforms:` (`ws platforms`), the entries for `ws wires` ([sbcli_doctor.md](sbcli_doctor.md) §4.11) |
| `sb io`, `sb install`, `sb gopro` | `io_dir`, `publishers`, `subscribers`, `services`, as before ([sbcli_pubsub.md](sbcli_pubsub.md)) |

---

## 2. Schema

```yaml
module: ips_detect                 # required
language: rust                     # required: rust | python | cpp | flutter | unity
version: 0.3.0                     # optional, for sharing
sb: ">=0.6"                        # optional, the sb version it needs (not enforced in 0.6.0)
description: ArUco detector        # optional

scripts:                           # optional; every key has a language default (§3.2)
  build: cargo build --release     # a string is one command; a list is several, run in order
  start: "{bin} {args}"            # exactly one command; sb appends --id <id> per instance
  stop:  []                        # extra teardown only; sb closes the windows itself
  test:  cargo test
env:                               # applied to every script
  RUST_LOG: info
requires: [cargo]                  # executables sb doctor must find on PATH

platforms:                         # key present = supported; no block = every platform
  linux:
    env: { LIBCLANG_PATH: "${LIBCLANG_PATH:-/usr/lib/llvm-18/lib}" }
  linux-aarch64: {}                # more specific key, merged after `linux`
  windows:
    env: { LIBCLANG_PATH: "${LIBCLANG_PATH:-C:/Program Files/LLVM/bin}" }
  macos: {}

io_dir: swarmbotix_io
publishers:                        # `topic:` is optional; it defaults to `name`
  - { name: markers, type: ros2/custom/MarkerDetections, transport: zenoh }
subscribers:
  - { name: image, type: ros2/sensor_msgs/Image1280x1024, transport: iceoryx2 }
services: []
```

| Field | Required | Type | Meaning |
|---|---|---|---|
| `module` | yes | identifier | Module name, `[A-Za-z_][A-Za-z0-9_]*`. The `<module>` segment of every owned wire key, and the base of every window name. |
| `language` | yes | `rust` / `python` / `cpp` / `flutter` / `unity` | Selects the codegen templates and the defaults in §3.2. |
| `version` | no | string | The module's own version, for sharing. Free text; `sb` does not read it. |
| `sb` | no | string | The `sb` version the module needs, such as `">=0.6"`. Free text; not enforced in 0.6.0. |
| `description` | no | string | One line for people and tools. |
| `scripts` | no | map | `build`, `start`, `stop`, `test` (§4). Each is a string (one command) or a list of strings (several commands, run in order). `start` must be exactly one command. A key left out takes the language default (§3.2). |
| `env` | no | map | Environment variables for every script. Values are expanded (§5). |
| `requires` | no | list | Executables that must be on `PATH` (§7). |
| `platforms` | no | map | Per-host overlays of `scripts`, `env`, `requires`, and the list of supported hosts (§6). |
| `root` | no | path | Module root. Absent: the directory holding the file. Relative: resolved against that directory. Absolute: used as is. |
| `io_dir` | no | relative path | Where generated files go. Default `swarmbotix_io`; `sb init` writes `Assets/Scripts/swarmbotix_io` for Unity. |
| `publishers`, `subscribers` | no | lists | Pub/sub entries ([sbcli_pubsub.md](sbcli_pubsub.md) §4). `sb 0.6.0+`: `topic:` is optional; an entry with neither `topic:` nor `key:` uses its `name` as the topic. |
| `services` | no | list of `{ name }` | Request/response routers ([sbcli_reqres.md](sbcli_reqres.md) §2). |

Load rules:

- `deny_unknown_fields` applies to the file, to `scripts`, and to every
  `platforms` overlay, so a typo such as `scirpts:` or `enviroment:` is
  reported by name rather than ignored.
- An `sb` older than 0.6.0 does not look for `sb.module.yaml` at all,
  and rejects the new keys if you rename the file back. A repository on
  this schema needs `sb 0.6.0+` on every host.
- `sb io add`, `edit`, and `rm` write the file back from its parsed
  form (§13.1).

---

## 3. The three rules

### 3.1 Every command is argv, not shell

A command is a program and its arguments, run directly. It never passes
through `bash`, `sh`, `cmd.exe`, or PowerShell, which is what lets one
manifest start the same way under tmux on Linux and under psmux on
Windows.

A string is split into arguments like this:

- Whitespace separates arguments.
- `"..."` and `'...'` group an argument that contains spaces, and may
  sit inside a word: `a"b c"d` is the single argument `ab cd`.
- The quotes themselves are removed. There are no backslash escapes.
- An empty command is an error (`empty command`), and so is an
  unclosed quote (`unterminated quote in command: <line>`).

So `start: python main.py --name "front cam"` runs `python` with the
three arguments `main.py`, `--name`, `front cam`.

There is no `&&`, no `;`, no `export`, no redirection (`>`, `2>&1`),
no pipe, and no `$(...)`. Their replacements:

| Shell construct | In the manifest |
|---|---|
| `a && b` | a list: `build: [a, b]`. Each step must exit 0 before the next runs. |
| `export VAR=value` | `env: { VAR: value }` |
| `cd <dir>` | nothing: every command runs in the module root |
| `"$@"` | `{args}` (§5) |
| `$HOME`, `${VAR:-x}` | `${HOME}`, `${VAR:-x}`, expanded by `sb` (§5) |
| `cmd > log 2>&1`, pipes | not supported; tmux keeps each window's output, and `tmux capture-pane` reads it back |

### 3.2 Every key has a default

Each `scripts` key falls back to a default chosen by `language:`. The
defaults, raw, before expansion (`{exe}` is `.exe` on Windows, empty
elsewhere):

| language | build | start | bin | test |
|---|---|---|---|---|
| rust | `cargo build --release` | `{bin} {args}` | `target/release/{module}{exe}` | `cargo test` |
| cpp | `cmake -S . -B build -DCMAKE_BUILD_TYPE=Release` then `cmake --build build --config Release` | `{bin} {args}` | linux/macos `build/{module}`; windows `build/Release/{module}.exe` | none |
| python | none | `{python} main.py {args}` | empty | none |
| flutter | none | `flutter run -d {os}` | empty | none |
| unity | none | none (resolve returns `NoStart`) | empty | none |

Reading the table:

- **bin** is what `{bin}` expands to: the built binary's path inside
  the module root, made absolute. It comes from the language and cannot
  be overridden;
  a binary with another name or location is started by overriding
  `start` instead (`start: "target/release/my_tool{exe} {args}"`).
- **none** means the step does nothing. A Python module is not built;
  `sb stop` runs no extra teardown unless `stop` is declared.
- **unity** has no start command: Unity modules launch from the editor.
  `sb up` skips one with
  `skipped <module>: unity modules have no start command (launch them from their editor)`,
  and `sb run` refuses it. Mark a Unity module `up: false` in
  `sb.flow.yaml` ([sbcli_ws.md](sbcli_ws.md) §4.4) to keep it out of the
  plan altogether.
- **`{python}`** is `.venv/bin/python` when that file exists under the
  module root (`.venv/Scripts/python.exe` on Windows), else `python3` on
  Linux and macOS and `python` on Windows. A module with a project venv
  needs no `start` override.
- The C++ default expects the CMake target to be named after the
  module. The Rust default expects the binary to be named after the
  module (the `[[bin]]` name, or the package name when there is no
  `[[bin]]`).

A module whose build and binary follow those conventions declares no
`scripts:` at all.

### 3.3 OS differences live only under `platforms`

Anything that differs between hosts (a library path, an extra build
step, a tool that exists on one OS only) goes under
`platforms.<os>` or `platforms.<os>-<arch>`, never in the top-level
keys and never in a local, uncommitted override file. §6 covers the
keys, §8 the merge order.

---

## 4. `scripts`

| Key | When it runs | Where | Default |
|---|---|---|---|
| `build` | `sb up` and `sb run` (including `--fg`), before anything starts | inline, in your terminal, in the module root, output visible | §3.2 |
| `start` | once per instance, after every build succeeded | its own tmux window (`sb run --fg`: your terminal) | §3.2 |
| `stop` | `sb stop`, after the module's windows are closed | inline, in your terminal, in the module root | none |
| `test` | not run by any `sb` verb in 0.6.0; declared for tooling and people | | `cargo test` for Rust, else none |

- **`build`** may hold several commands; they run in order and the
  first failure stops everything. Builds of all modules run before the
  first tmux call, so **a failed build leaves no session behind**
  ([sbcli_launch.md](sbcli_launch.md) §2). Before each build `sb` prints
  `building <module>: <command>`.
- **`start`** must be exactly one command (a list with two entries is
  rejected with `scripts.start must be exactly one command`). For each
  instance `sb` appends `--id <id>` to it after expansion, so even a
  start command without `{args}` receives its id (§9).
- **`stop`** is extra teardown only: containers, mounted devices, lock
  files. `sb stop` closes the module's windows itself, which ends the
  processes it started; a module with nothing outside those processes
  declares no `stop`. See [sbcli_launch.md](sbcli_launch.md) §4.
- Overriding one key leaves the others at their defaults: declaring
  only `start` keeps the default `build`.

```yaml
# A Rust module whose binary is not named after the module:
scripts:
  start: "target/release/hello_pubber{exe} {args}"

# A C++ module with an extra code-generation step before the CMake build:
scripts:
  build:
    - python tools/gen_tables.py
    - cmake -S . -B build -DCMAKE_BUILD_TYPE=Release
    - cmake --build build --config Release
```

---

## 5. Placeholders and environment expansion

`sb` expands every command argument, every `env:` value, and the `{bin}`
path in one left-to-right pass.

| Placeholder | Expands to |
|---|---|
| `{module}` | the manifest's `module:` |
| `{root}` | the absolute module root, with forward slashes on every OS |
| `{bin}` | the language's binary path (§3.2), absolute |
| `{exe}` | `.exe` on Windows, empty elsewhere |
| `{id}` | the instance id; an error for a module without instances |
| `{args}` | the instance's `args` from the flow, then the passthrough args of `sb run` (§9) |
| `{ws}` | the workspace name |
| `{os}` | `linux`, `windows`, or `macos` |
| `{python}` | the module's Python interpreter (§3.2) |
| `{{` and `}}` | a literal `{` and `}` |

| Environment form | Expands to |
|---|---|
| `${VAR}` | the value of `VAR` in `sb`'s own environment, or empty when unset |
| `${VAR:-default}` | the value of `VAR`, or `default` when it is unset or empty; `default` may itself contain `{...}` placeholders |

Rules:

- `{args}` must be a whole argument. It expands to zero or more
  arguments, so `{bin} {args}` with no args is just the binary.
  `--flag={args}` is rejected (`{args} must be a whole token`), and
  `{args}` inside an `env:` value is an error too.
- An unknown name is an error (`unknown placeholder {nmae}`), not
  literal text, so a typo cannot reach the started process silently.
- `{id}` in a module without instances is an error
  (`{id} used but this module has no instances`). `scripts.stop` runs
  once per module, not per instance, so it has no `{id}` either.
- The pass is single: `${LIBCLANG_PATH:-/usr/lib}` is read as one
  environment reference, and a variable whose value happens to contain
  `{root}` is passed through as is, never expanded a second time.
- `${VAR}` is read when `sb` runs, from the environment of the `sb`
  process. A variable exported in your shell before `sb up` reaches the
  manifest; a variable set inside a tmux session does not.

---

## 6. `platforms`

### 6.1 Keys

A key is an OS, or an OS and an architecture:

| OS | Architectures |
|---|---|
| `linux`, `windows`, `macos` | `x86_64`, `aarch64` |

So `linux`, `windows`, `macos`, `linux-x86_64`, `linux-aarch64`,
`windows-x86_64`, and so on. Any other key is rejected when the file is
read:

```
platforms: unknown key "linx" (expected <os> or <os>-<arch>, os in linux|windows|macos, arch in x86_64|aarch64)
```

Each value is an overlay with the same three optional keys as the top
level: `scripts`, `env`, `requires`. An empty overlay (`macos: {}`)
only declares support.

### 6.2 Supported hosts

- **No `platforms:` block**: the module is supported everywhere.
- **A `platforms:` block**: the host must be listed, as `<os>` or as
  `<os>-<arch>`. A module with only `linux-aarch64` is supported on a
  Jetson and nowhere else.

On a host the module does not list:

| Verb | Behavior |
|---|---|
| `sb up` | skips the module with a `skipped <module>: <reason>` line, and launches the rest |
| `sb run <module>` | refuses: `module "<m>" is not supported on <os>-<arch> (declares: linux, windows)` |
| `sb ws check`, `sb doctor` | `[WARN] ws platforms <module>: not supported on <os>-<arch> (declares: linux, windows)` |
| `sb doctor` `ws requires` | skips the module: nothing it requires is needed on this host |

Listing one platform therefore restricts the module to it. To add an
overlay for one OS while staying supported on the others, list the
others too, even as `{}`.

### 6.3 Machine-specific paths

A path that differs between boxes never goes in the committed file as a
literal. Write the common location as the default of an environment
reference, and let a box that differs set one variable:

```yaml
platforms:
  linux:   { env: { LIBCLANG_PATH: "${LIBCLANG_PATH:-/usr/lib/llvm-18/lib}" } }
  windows: { env: { LIBCLANG_PATH: "${LIBCLANG_PATH:-C:/Program Files/LLVM/bin}" } }
```

A host with LLVM elsewhere runs `export LIBCLANG_PATH=/opt/llvm/lib`
(or sets it in the Windows user environment) once. There is no local
override file to forget to copy.

---

## 7. `requires`

A list of executables the module needs on `PATH`: compilers, `cmake`,
`flutter`, `docker`, a vendor tool. `sb doctor` and `sb ws check` look
each one up and report the missing ones in the `ws requires` row
([sbcli_doctor.md](sbcli_doctor.md) §4.11):

```
[FAIL] ws requires  ips_detect: missing cmake, clang; ips_viz: missing docker
```

- Entries are executable names, not packages or libraries. A library
  such as libclang cannot be found on `PATH`; express its location
  through `env:` instead (§6.3).
- Top-level `requires` apply everywhere; `platforms.<os>.requires` add
  to them on that host (§8).
- A module not supported on the host is skipped by this row (§6.2).
- `sb up` does not check `requires` itself. A missing tool shows up as
  a failed build or a window whose process could not start; run
  `sb doctor` to see it ahead of time.

---

## 8. Merge order

For one module on one host, lowest priority first:

1. the language defaults (§3.2)
2. the top-level `scripts`, `env`, `requires`
3. `platforms.<os>`
4. `platforms.<os>-<arch>`
5. the instance's `env` and `args`, from `sb.flow.yaml` ([sbcli_ws.md](sbcli_ws.md) §4.6)

How each key merges:

| Key | Rule |
|---|---|
| `scripts` | per key: a later `build` replaces an earlier `build` and leaves `start` alone |
| `env` | per variable: a later value wins |
| `requires` | appended, then de-duplicated, keeping the first occurrence |
| `bin` | language defaults only |

Example, on a Linux aarch64 host:

```yaml
module: cam
language: cpp
env: { LOG: info, MODE: sim }
platforms:
  linux:         { env: { MODE: hw }, requires: [v4l2-ctl] }
  linux-aarch64: { scripts: { build: [cmake -S . -B build -DJETSON=ON, cmake --build build] } }
  windows:       {}
```

resolves to the C++ default `start` (`{bin} {args}`, with `{bin}` =
`build/cam`), the Jetson `build` list, `env` `LOG=info` and `MODE=hw`,
and `requires` `[v4l2-ctl]`. On Windows the same file resolves to the
C++ default build and `MODE=sim`; on macOS the module is unsupported.

---

## 9. What a started process sees

For each instance, `sb` starts the expanded `start` command:

- **argv**: the `start` argv, with `{args}` replaced by the instance's
  `args` followed by any passthrough args from `sb run`, and then, for
  an instance, the two arguments `--id <id>` appended at the end.
- **working directory**: the module root.
- **environment**: the merged `env:` (§8), then the instance's `env`,
  then the variables below, later entries winning.

| Variable | Value | Set when |
|---|---|---|
| `SB_WORKSPACE` | the workspace name | always |
| `SB_MODULE` | the manifest's `module:` | always |
| `SB_MODULE_ROOT` | the absolute module root | always |
| `SB_INSTANCE_ID` | the instance id | the module has instances |

So a module declared with `instances: [sb00, sb01]` and the default
Rust start runs `target/release/<module> --id sb00` in window
`<module>-sb00` and `target/release/<module> --id sb01` in window
`<module>-sb01`, each with `SB_INSTANCE_ID` set. A program can take its
identity from either: parsing `--id` keeps working exactly as before,
and `SB_INSTANCE_ID` needs no argument parsing at all. How that id
becomes a per-instance wire key is in
[sbcli_pubsub_consuming.md](sbcli_pubsub_consuming.md) §3 and §10.

In a tmux window, variables not set by any of the above come from the
tmux server's environment, which tmux captured when the server first
started, not from the shell you ran `sb up` in. Under `sb run --fg`
they come from your shell. Declare what the program depends on in
`env:` rather than relying on either.

---

## 10. Worked example: `ws-ips`

Four cameras, four ArUco detectors, a tracker, a visualiser, and a
one-shot calibrator. Before 0.6.0 every module carried a 60-line launch
script and a stop script of which one to three lines were specific to
the module, and the camera list was repeated in six places. After:

```yaml
# modules/ips_cam_gcam_pub/sb.module.yaml
module: ips_cam_gcam_pub
language: rust
platforms:
  linux: {}
publishers:
  - { name: image, type: ros2/sensor_msgs/Image1280x1024, transport: iceoryx2 }
```

```yaml
# modules/ips_detect/sb.module.yaml
module: ips_detect
language: rust
platforms:
  linux:   { env: { LIBCLANG_PATH: "${LIBCLANG_PATH:-/usr/lib/llvm-18/lib}" } }
  windows: { env: { LIBCLANG_PATH: "${LIBCLANG_PATH:-C:/Program Files/LLVM/bin}" } }
publishers:
  - { name: markers,     type: ros2/custom/MarkerDetections,        transport: zenoh }
  - { name: debug_image, type: ros2/sensor_msgs/Image1280x1024Color, transport: iceoryx2 }
subscribers:
  - { name: image,       type: ros2/sensor_msgs/Image1280x1024,      transport: iceoryx2 }
```

```yaml
# modules/ips_track/sb.module.yaml
module: ips_track
language: rust
publishers:
  - { name: cube_poses, type: ros2/custom/CubePoses, transport: zenoh }
subscribers:
  - { name: markers,    type: ros2/custom/MarkerDetections, transport: zenoh }
services:
  - { name: cam_param }
```

```yaml
# modules/ips_viz/sb.module.yaml
module: ips_viz
language: rust
subscribers:
  - { name: cube_poses,  type: ros2/custom/CubePoses,                 transport: zenoh }
  - { name: debug_image, type: ros2/sensor_msgs/Image1280x1024Color,  transport: iceoryx2 }
```

```yaml
# modules/ips_calib/sb.module.yaml
module: ips_calib
language: rust
subscribers:
  - { name: markers, type: ros2/custom/MarkerDetections, transport: zenoh }
```

```yaml
# sb.flow.yaml
name: ws_ips
modules:
  ips_cam_gcam_pub: { path: modules/ips_cam_gcam_pub, instances: [sb00, sb01, sb02, sb03] }
  ips_detect:       { path: modules/ips_detect,       instances: [sb00, sb01, sb02, sb03] }
  ips_track:        modules/ips_track
  ips_viz:          modules/ips_viz
  ips_calib:        { path: modules/ips_calib, up: false }
messages:
  - { style: ros2, path: messages/custom, backends: [proto, iox2] }
wires:
  ips_detect.image:    "ips_cam_gcam_pub[{id}].image"
  ips_track.markers:   "ips_detect[*].markers"
  ips_calib.markers:   "ips_detect[*].markers"
  ips_viz.cube_poses:  ips_track.cube_poses
  # ips_viz.debug_image is iceoryx2, which has no wildcard: main() opens one
  # subscriber per camera itself, so it is not wired.
```

No module declares `scripts:`: every binary is named after its module,
so the Rust defaults apply. The camera list exists once, in
`sb.flow.yaml`. Deleted: every per-module launch and stop script, and
the pre-0.6.0 flow and module files (§11).

What `sb install .` then `sb up` does on Linux:

```console
$ sb up
building ips_cam_gcam_pub: cargo build --release
building ips_detect: cargo build --release
building ips_track: cargo build --release
building ips_viz: cargo build --release
launched 10 window(s) in tmux session "sb-ws_ips"
  - ips_cam_gcam_pub-sb00
  - ips_cam_gcam_pub-sb01
  - ips_cam_gcam_pub-sb02
  - ips_cam_gcam_pub-sb03
  - ips_detect-sb00
  - ips_detect-sb01
  - ips_detect-sb02
  - ips_detect-sb03
  - ips_track
  - ips_viz
attach with: tmux attach -t sb-ws_ips
```

(Cargo's own output appears between the `building` lines.) `ips_calib`
is neither built nor started, because of `up: false`;
`sb run ips_calib` starts it when needed. On Windows the camera module
is not supported, so `sb up` prints
`skipped ips_cam_gcam_pub: not supported on windows-x86_64 (declares: linux)`
and launches the other six windows.

Day-to-day commands on the same workspace:

```bash
sb stop ips_detect            # closes ips_detect-sb00 .. -sb03
sb run ips_detect:sb00        # rebuilds, then starts only that instance
sb run --fg ips_viz           # rebuilds, then runs the visualiser in this terminal
```

About `wires:`: in 0.6.0 `sb ws check` parses and validates them
(`ws wires`, [sbcli_ws.md](sbcli_ws.md) §4.7), but code generation does
not read them yet. Until a later release generates subscriber keys from
`wires:`, the modules still pair the way they did before 0.6.0: through
`key:` entries ([sbcli_pubsub.md](sbcli_pubsub.md) §13.7) or the
`TopicOverrides` a module's `main()` builds from its `--id`
([sbcli_pubsub_consuming.md](sbcli_pubsub_consuming.md) §3).

---

## 11. Migrating from 0.5 (required since 0.7.0)

Every item below was deprecated in 0.6.0 and is removed in 0.7.0
([sbcli.md](sbcli.md) §10 holds the table). `sb 0.7.0+` refuses the old
file names and the top-level `instances:` map with an error that ends
`(see sbcli_module.md §Migrating)`, which is this section, and ignores
the old launch and stop scripts. There is no migration command: each
change is a rename or a few lines of YAML, made by hand, and none of it
needs an older `sb`.

### 11.1 `sb.dev.yml` becomes `sb.module.yaml`

The contents are compatible; only the name changes.

```bash
git mv modules/ips_track/sb.dev.yml modules/ips_track/sb.module.yaml
sb install .        # the installed copy records manifest paths; refresh it
```

`sb 0.7.0+` refuses a module directory that holds `sb.dev.yml` and no
`sb.module.yaml`; `sb install`, `sb init`, `sb io`, and `sb doctor` stop
with

```
module manifest sb.module.yaml not found at <dir>; sb.dev.yml is no longer read since 0.7.0, rename it (see sbcli_module.md §Migrating)
```

When both files exist, `sb.module.yaml` is read and `sb.dev.yml` is
ignored; `sb ws check` lists it in the `deprecated` row until you delete
it. An installed copy written before the rename may still record the
old path; `sb` reads such an entry as the `sb.module.yaml` in the same
directory, and the next `sb install .` records the new name.

### 11.2 `flow.yaml` becomes `sb.flow.yaml`

```bash
git mv flow.yaml sb.flow.yaml
sb install .
```

Before, at the repository root: `flow.yaml`. After: `sb.flow.yaml`, same
contents. `sb 0.7.0+` reads only `sb.flow.yaml`; a directory that holds
`flow.yaml` and no `sb.flow.yaml` is refused with

```
sb.flow.yaml not found at <dir>; flow.yaml is no longer read since 0.7.0, rename it (see sbcli_module.md §Migrating)
```

The same applies to an installed copy written before 0.6.0 under
`<sb_home>/workspaces/<name>/`: rename it there too, or reinstall the
repository, which writes `sb.flow.yaml`. A leftover `flow.yaml` beside an
`sb.flow.yaml` is ignored; delete it.

### 11.3 `runscript.bash` becomes `scripts:`

Before (abridged; the generated wrapper was about 60 lines):

```bash
#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"
# ... tmux re-exec, --id parsing, session naming ...
export LIBCLANG_PATH=/usr/lib/llvm-18/lib
cargo build --release
exec ./target/release/ips_detect "$@"
```

After, in `sb.module.yaml`:

```yaml
module: ips_detect
language: rust
platforms:
  linux:   { env: { LIBCLANG_PATH: "${LIBCLANG_PATH:-/usr/lib/llvm-18/lib}" } }
  windows: { env: { LIBCLANG_PATH: "${LIBCLANG_PATH:-C:/Program Files/LLVM/bin}" } }
```

then `git rm runscript.bash`. Line by line:

| In the script | In `sb.module.yaml` |
|---|---|
| `cd "$(dirname ...)"` | nothing: commands run in the module root |
| the tmux re-exec and session naming | nothing: `sb` owns the windows, named `<module>-<id>` |
| parsing `--id` in bash | nothing: `sb` appends `--id <id>` and sets `SB_INSTANCE_ID` |
| `export VAR=value` | `env: { VAR: value }`, or `platforms.<os>.env` when it differs per OS |
| `cargo build --release`, `cmake ...` | nothing when it matches §3.2; else `scripts.build` |
| `exec ./target/release/<module> "$@"` | nothing when it matches §3.2; else `scripts.start: "<path> {args}"` |
| `"$@"` | `{args}` |
| `a && b` | `build: [a, b]` |
| an absolute path true on one box | `${VAR:-default}` (§6.3) |
| pipes, redirection, `$(...)` | not available (§3.1); move the logic into the program or a build step |

`sb 0.7.0+` launches every module natively and never runs
`runscript.bash`, whether or not the manifest declares `scripts:`. A
module whose defaults fit therefore migrates by deleting the script and
nothing else; one whose script did more needs the lines in the table
above, or it starts the wrong program (or none) after the upgrade.
Until the script is deleted, `sb ws check` lists it in its `deprecated`
row.

### 11.4 `stopscript.bash` becomes `scripts.stop`, or nothing

`sb stop` now closes the module's windows itself, which ends the
processes `sb` started. A stop script that only killed those processes
is deleted without replacement. One that also cleaned up something
outside them (a container, a mount, a lock file) becomes `scripts.stop`:

```yaml
scripts:
  stop: docker rm -f cam_gige_ht-left cam_gige_ht-right
```

The difference in timing: the old script ran inside the module's tmux
window after `sb` had killed the process in it; `scripts.stop` runs in
your terminal after the windows are closed. Both are teardown, not a
graceful signal ([sbcli_launch.md](sbcli_launch.md) §4.1). `sb 0.7.0+`
never runs `stopscript.bash`, so cleanup it still performs must move to
`scripts.stop` before the upgrade.

### 11.5 Top-level `instances:` moves under the module entry

Before:

```yaml
modules:
  ips_detect: modules/ips_detect
instances:
  ips_detect: [{ id: sb00 }, { id: sb01 }]
```

After:

```yaml
modules:
  ips_detect: { path: modules/ips_detect, instances: [sb00, sb01] }
```

An instance is a bare id, or a map `{ id, label, args, env, docker }`
([sbcli_ws.md](sbcli_ws.md) §4.6). `sb 0.7.0+` refuses a flow that still
has a top-level `instances:` map, before anything else in it is read:

```
top-level instances: is no longer read since 0.7.0; move each list under modules.<name>.instances (see sbcli_module.md §Migrating)
```

An installed copy written by 0.6.0 from such a flow carries the map
too, so move the lists in the repository's `sb.flow.yaml`, then
`sb install .`.

### 11.6 `sb init --docker` no longer writes a bash template

`sb init --docker` still checks the module's instances (each needs a
`docker.image`) before writing anything, but writes only the manifest.
An instance's `docker:` block stays valid and is validated, but `sb`
does not start a container from it yet, and rather than run the host
binary in its place, `sb up` **skips** a module whose instances declare
`docker:` (`sb run` refuses it). The docker launch script that releases
before 0.6.0 generated is not run either (sb 0.7.0+), so until native
container launch lands there is one way forward: **move the container
into `scripts.start`** and drop the `docker:` blocks, carrying the
per-instance differences as instance `args`:

```yaml
# modules/cam_gige_ht/sb.module.yaml
module: cam_gige_ht
language: cpp
scripts:
  build: []
  start: "docker run --rm --init {args} cam_gige_ht:latest"
requires: [docker]
```

```yaml
# sb.flow.yaml
modules:
  cam_gige_ht:
    path: camera_publishers/cam_gige_ht
    instances:
      - { id: left,  args: [--device, /dev/video0, -e, CAM_SERIAL=A123] }
      - { id: right, args: [--device, /dev/video1, -e, CAM_SERIAL=B456] }
```

`{args}` places the instance's flags among the `docker run` options, and
the `--id left` that `sb` appends lands after the image name, where the
container's entry point receives it as its own `--id`. `build: []`
replaces the C++ default build, since the image is built elsewhere.

Closing the window ends the `docker run` client. Whether the container
ends with it depends on how its process handles the signal `docker`
forwards; `--init` gives it a process that passes signals on. If a
container outlives its window, add a `scripts.stop` that removes it
(§11.4).

### 11.7 `SB_NO_TMUX=1` becomes `sb run --fg`

The variable only ever switched off the tmux wrapper inside a generated
launch script, and nothing reads it since 0.7.0. To run a module in the
current terminal, with its build first and its exit code passed
through:

```bash
sb run --fg ips_viz
sb run --fg ips_detect:sb00      # one instance of an instanced module
```

### 11.8 `sb pub`, `sb sub`, `sb list`, `sb service` become `sb io`

Deprecated in 0.5.0, warned about from 0.6.0, removed in 0.7.0: each
now fails, exit 1, with
``error: `<verb>` is not an sb command or an installed app.``
(plus a spelling guess, which does not point here). Use the right-hand
column.

| Before | After |
|---|---|
| `sb pub add -m <T> <topic>` | `sb io add pub <topic> -m <T>` |
| `sb sub add -m <T> --key <key>` | `sb io add sub --key <key> -m <T>` |
| `sb pub edit <name>`, `sb sub edit <topic-or-key>` | `sb io edit <name>` |
| `sb pub rm <name>`, `sb sub rm <topic-or-key>` | `sb io rm <name>` |
| `sb list`, `sb pub list`, `sb sub list` | `sb io list` |
| `sb service init` | `sb io add service service` (the default service; any other name adds a named one) |

Two differences to know. `sb io` names every entry, a subscriber
included, by its `name:` (`sb io list` shows it), not by its topic or
key. And `sb io add service service` renders the default router to
`<io_dir>/services/service.<ext>` and records it under `services:`,
where the old verb wrote `<io_dir>/service.<ext>` and recorded nothing;
both serve on the same base key ([sbcli_reqres.md](sbcli_reqres.md) §3).
A router file the old verb wrote keeps working as your own file, but
nothing regenerates it. Details in [sbcli_pubsub.md](sbcli_pubsub.md)
§14.

### 11.9 Confirming you are done

`sb 0.7.0+` prints no deprecation warnings. What remains is the
`deprecated` row of `sb ws check` and `sb doctor`, which lists every
leftover pre-0.6.0 file it finds, one per line:

```
[WARN] deprecated   runscript.bash at /home/el/ws_ips/modules/ips_track/runscript.bash is ignored since 0.7.0 (see sbcli_module.md §Migrating)
```

A migrated workspace shows no `[WARN] deprecated` row in `sb ws check`
or `sb doctor`; the row reads `nothing deprecated in use`
([sbcli_doctor.md](sbcli_doctor.md) §4.11).

The whole migration, for one repository:

1. `git mv` each module's `sb.dev.yml` to `sb.module.yaml` (§11.1).
2. `git mv flow.yaml sb.flow.yaml` (§11.2).
3. Move any top-level `instances:` under their module entries (§11.5).
4. Translate each launch script into `scripts:`, or delete it when the
   defaults fit; delete each stop script, or turn it into
   `scripts.stop` (§11.3, §11.4).
5. `sb install .`, then `sb ws check` until `ws modules` passes and
   the `deprecated` row reads `nothing deprecated in use`.

---

## 12. Error catalogue

Strings below are the core wording; `sb` prints them after `error:`,
usually behind the module or file they concern.

| Cause | Message |
|---|---|
| bad `platforms` key | `platforms: unknown key "<k>" (expected <os> or <os>-<arch>, os in linux\|windows\|macos, arch in x86_64\|aarch64)` |
| `start` with more than one command (top level or an overlay) | `scripts.start must be exactly one command` |
| a Unity module asked to start (`sb run`) | `language unity has no start command (unity modules launch from the editor)` |
| host not listed under `platforms` (`sb run`) | `module "<m>" is not supported on <os>-<arch> (declares: <keys>)` |
| unclosed quote in a command | `unterminated quote in command: <line>` |
| empty command | `empty command` |
| unknown placeholder | `unknown placeholder {<name>}` |
| `{id}` without instances | `{id} used but this module has no instances` |
| `{args}` inside a word or an `env:` value | `{args} must be a whole token` |
| unclosed `{` or `${` | `unterminated placeholder in "<text>"` |
| a module directory with only the pre-0.6.0 manifest (sb 0.7.0+, §11.1) | `module manifest sb.module.yaml not found at <dir>; sb.dev.yml is no longer read since 0.7.0, rename it (see sbcli_module.md §Migrating)` |

---

## 13. Edge cases and gotchas

### 13.1 `sb io` rewrites the file

`sb io add`, `edit`, and `rm` write `sb.module.yaml` back from its
parsed form. Every key survives,
`scripts:` included, but YAML comments and your own key order do not,
and `topic:` is written out even where you omitted it. Keep notes about
the module in a README rather than in comments.

### 13.2 A start command without `{args}` drops passthrough args

`{args}` is the only place the instance's `args` and `sb run`'s
passthrough args go. A custom `start` without it ignores both. The
`--id <id>` of an instance is still appended, since it is added after
expansion.

### 13.3 Every instance gets `--id`

The program must accept a trailing `--id <id>`. Most argument parsers
need it declared (`ap.add_argument("--id")`); a tool that rejects
unknown flags, such as `flutter run`, cannot be the start command of an
instanced module as is.

### 13.4 When you genuinely need a shell

A command may name a shell explicitly, as in
`start: bash -c "exec ./run.sh"`, because that is still a program with
arguments. The module then starts only where that shell exists, which is
exactly the portability §3.1 is there to keep. Prefer moving the logic
into the program or into a build step.

### 13.5 `build` runs on every `sb up` and `sb run`

The default builds are incremental (`cargo build`, `cmake --build`), so
an unchanged module costs a second or two. A slow custom build step runs
every time too; make it incremental, or move it out of `build`.

### 13.6 Window output survives a crash

A native window stays open after its process exits, so a crash leaves
its last output readable (`sb attach`, or
`tmux capture-pane -p -t '=sb-<ws>:=<window>'`). `sb stop` and `sb down`
close it.

### 13.7 Library paths are `env`, not `requires`

`requires` finds executables on `PATH`. Shared libraries and SDKs (a
libclang for bindgen, an iceoryx2 install) are located through `env:`,
with a `${VAR:-default}` default per OS (§6.3).
