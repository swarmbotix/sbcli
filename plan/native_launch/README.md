# Native launch: `sb.module.yaml` + `sb.flow.yaml`, no run scripts

Status: phases 1 and 2 implemented on 2026-10-13 and cut as 0.6.0. Phase 3
(wires in codegen) is not started; 0.7.0 (removal) is not started.

Verification record (2026-10-13, this box, Ubuntu, tmux 3.4):

- `cargo test --workspace`: all suites green, including the four new tests;
  `level5_e2e_hello_world` (ignored, run by hand) green through the rewritten
  example `run.sh` with no run scripts.
- ws-ips against an isolated `SB_HOME`: `sb install .` rendered the missing
  `ips_track` service router; `sb ws check` all OK (the `[*]` iceoryx2 wire
  was removed from the fixture as unsupported by design); `sb run
  ips_cam_gcam_pub` built once and opened four `--id` windows; `sb stop`
  closed exactly those four; `sb run ips_cam_gcam_pub:sb00` opened one;
  `sb run --fg ips_viz` ran inline; `sb down` cleared the session.
- `sb up` on ws-ips fails at the `ips_detect` build because OpenCV
  development files are not installed on this box; sb stopped before any
  tmux call, as designed. `ips_track` exits at startup with an application
  error (`publish() takes a prost message`), a ws-ips issue, and its dead
  pane stayed readable.
- Not verified: psmux on Windows.

Rollback point before any work starts: `sbcli` commit `637f2bb`, `sbcli_forge`
commit `7d45f02`. If integration goes wrong, `git checkout 637f2bb` in `sbcli/`
is the recovery, not another round of fixes.

Files in this folder:

| file | purpose |
|---|---|
| `README.md` | this overview: goal, design, phases, execution model, decisions |
| `contract.md` | the exact Rust surface every agent codes against; names are fixed here, not guessed |
| `briefs.md` | the five agent briefs for the single writing wave |

---

## 1. Goal

A module folder holds one manifest and no bash. sb builds, launches, stops and
fans out instances itself, on Linux and Windows, from two files:

- `sb.module.yaml`: one module, committed with it, shareable. Identity,
  scripts, env, requirements, supported platforms, ports.
- `sb.flow.yaml`: one workspace, at the repo root. Modules, instances, messages,
  wires.

The motivating case is `ws-ips` (four cameras, four detectors, a tracker, a
visualiser, a one-shot calibrator). Today every module carries a 60-line
`runscript.bash` and a `stopscript.bash`, of which 1 to 3 lines are
module-specific; the rest re-implements tmux fan-out that sb already owns.
`flow.yaml::instances` exists but is never read at launch, so the camera list
lives in six places. `sb up` does not work on Windows at all because the pane
command is a POSIX line that psmux cannot run.

## 2. Design

### 2.1 `sb.module.yaml`

```yaml
module: ips_detect                 # required
language: rust                     # required: rust | python | cpp | flutter | unity
version: 0.3.0                     # optional, for sharing
sb: ">=0.6"                        # optional, sb version requirement (string, not enforced in 0.6.0)
description: ArUco detector        # optional

scripts:                           # optional; every key has a language default
  build: cargo build --release     # string = one command; list = several, in order
  start: "{bin} {args}"            # exactly one command; sb appends --id <id> per instance
  stop:  []                        # extra teardown only; sb closes windows itself
  test:  cargo test
env:                               # applied to every script
  RUST_LOG: info
requires: [cargo]                  # executables sb doctor must find on PATH

platforms:                         # key present = supported; absent block = all platforms
  linux:
    env: { LIBCLANG_PATH: "${LIBCLANG_PATH:-/usr/lib/llvm-18/lib}" }
    requires: [cargo]
  linux-aarch64: {}                # more specific key merges after the general one
  windows:
    env: { LIBCLANG_PATH: "${LIBCLANG_PATH:-C:/Program Files/LLVM/bin}" }
  macos: {}

io_dir: swarmbotix_io              # unchanged
publishers:                        # unchanged; `topic:` now optional, defaults to `name`
  - { name: markers, type: ros2/custom/MarkerDetections, transport: zenoh }
subscribers:
  - { name: image, type: ros2/sensor_msgs/Image1280x1024, transport: iceoryx2 }
services: []
```

Rules:

1. Every command is argv, not shell. A string is split on whitespace with
   double- and single-quote grouping. No `&&`, no `export`, no redirection.
2. Placeholders, expanded by sb: `{module}` `{root}` `{bin}` `{exe}` `{id}`
   `{args}` `{ws}` `{os}` `{python}`. `${VAR}` and `${VAR:-default}` are read
   from the process environment. `{args}` must be a whole token and expands to
   zero or more tokens. `{{` and `}}` are literal braces.
3. Every key has a default from `language:` (table in `contract.md` §3).
   A module with nothing special declares nothing beyond `module` and
   `language`.
4. OS differences live only under `platforms.<os>` or `platforms.<os>-<arch>`.
   Merge order, lowest to highest: language defaults, top level,
   `platforms.<os>`, `platforms.<os>-<arch>`, instance `env`/`args` from the
   flow. `scripts` keys override individually; `env` keys override
   individually; `requires` append and dedupe.
5. `platforms:` absent means supported everywhere. Present means the host must
   be listed: `sb up` skips unsupported modules with a warning, `sb run`
   refuses with the supported list, `sb ws check` reports.
6. Machine-specific paths never go in the committed file. The author writes
   `${VAR:-sensible default}`; a box that differs sets one environment
   variable. No local override file.

### 2.2 `sb.flow.yaml`

```yaml
name: ws_ips
modules:
  ips_cam_gcam_pub: { path: modules/ips_cam_gcam_pub, instances: [sb00, sb01, sb02, sb03] }
  ips_detect:
    path: modules/ips_detect
    instances:
      - { id: sb00 }
      - { id: sb01 }
      - { id: sb02, args: [--debug] }
      - { id: sb03, env: { RUST_LOG: debug } }
  ips_track: modules/ips_track
  ips_viz:   modules/ips_viz
  ips_calib: { path: modules/ips_calib, up: false }
messages:
  - { style: ros2, path: messages/custom, backends: [proto, iox2] }
wires:                                           # subscriber <- publisher
  ips_detect.image:   "ips_cam_gcam_pub[{id}].image"   # same instance id
  ips_track.markers:  "ips_detect[*].markers"          # every instance (zenoh only)
  ips_viz.cube_poses: ips_track.cube_poses             # singleton
```

- `instances` sit under the module entry. The pre-0.6 top-level `instances:`
  map is still read, with a deprecation warning. Declaring both for one module
  is an error.
- An instance is a bare id or a map `{id, label, args, env, docker}`.
- `wires` grammar: `<module>.<entry>: <module>[<selector>].<entry>` with
  selector `{id}` (same id as the subscriber instance), `*` (all instances),
  a literal id, or no brackets (singleton). Values with `[` or `{` must be
  quoted in YAML. In 0.6.0 wires are parsed, validated and reported by
  `sb ws check`; code generation honours them in phase 3.

### 2.3 What sb does at launch

1. Read the installed flow, resolve each module's manifest, merge for the host.
2. Mode per module: `scripts:` declared, or no `runscript.bash` present, means
   **native**. `runscript.bash` present and no `scripts:` means **legacy**
   (old behaviour, deprecation warning). Both present: native, warning that
   the script is ignored.
3. Run every module's `build` inline, in order, fail fast, **before any tmux
   call**. A failed build leaves no session.
4. One tmux window per instance, named `<module>-<id>` (or `<module>` for a
   singleton), started with `-c <root>`, `-e` for every env var, and the
   `start` argv with `--id <id>` and the instance's `args` appended.
   Exported: `SB_WORKSPACE`, `SB_MODULE`, `SB_INSTANCE_ID` (instances only),
   `SB_MODULE_ROOT`.
5. `sb run <module>` starts all its windows; `sb run <module>:<id>` one;
   `sb run --fg <module>[:<id>]` runs one inline with no tmux.
6. `sb stop <module>` kills every `<module>` and `<module>-*` window by id,
   then runs `scripts.stop` inline if declared. Legacy modules keep the old
   stopscript path.
7. tmux targets are exact (`=name`, window ids), fixing the prefix-match bug
   where `sb-demo` matched `sb-demo2`.
8. tmux on Linux and macOS, psmux on Windows, both invoked as `tmux`;
   `resolve_tmux` falls back to `which psmux` on Windows.

## 3. Phases

| phase | release | content |
|---|---|---|
| 1 schema | 0.6.0 | new filenames with legacy fallback; new manifest and flow fields; optional `topic`; expander and platform merge; install carries instances and wires; init writes the new manifest and no runscript by default; missing service routers rendered at install |
| 2 launch | 0.6.0 | native launch as in §2.3; doctor rows `ws requires`, `ws platforms`, `ws wires`, `deprecated`; deprecation warnings on the four old verbs, both legacy filenames, both scripts, top-level instances; docs |
| 3 wires | 0.6.x | install-time wire resolver; `GenCtx` plumbing; subscriber templates honour `Exact`, `PerInstance` (via `SB_INSTANCE_ID`), `Wildcard` (zenoh, with `try_recv_all`); publisher side of an instanced module honours `SB_INSTANCE_ID` |
| removal | 0.7.0 | delete legacy filename reads, script paths, `sb pub/sub/list/service`, top-level `instances:`, `SB_NO_TMUX` mentions; errors name the replacement for one minor version |

Phases 1 and 2 ship together and are what removes every ws-ips script. Phase 3
is what removes the `main()` key overrides.

## 4. Execution model: one-shot check

Per `.claude/skills/sb-delegate/SKILL.md` §1.

- Main thread writes `contract.md`, dispatches five `coder` agents at once
  from `briefs.md`, each in its own worktree, each told "Write only. Do not run
  cargo". Docs are written in the same wave from the design.
- Main thread merges, runs `cargo check --workspace`, sends each error to the
  agent that owns the file (by `SendMessage`, same agent, same context), loops
  until clean. Then `cargo build`, then one `cargo test --workspace`, failures
  routed to the owner of the code under test.
- Verification on ws-ips: write five `sb.module.yaml` files and `sb.flow.yaml`
  (texts in §6), delete ten scripts, `sb install .`, `sb up`,
  `sb stop ips_detect`, `sb run ips_detect:sb00`, `sb run --fg ips_viz`.
- Post-build docs pass for strings that moved. `/sb-sync-check`. Version
  bump to 0.6.0 on the main thread, bundle, install.

Agent ownership (disjoint file sets; `main.rs` is the one shared file, split
by function and merged by git):

| agent | owns |
|---|---|
| core | `crates/sb-core/**` |
| workspace | `crates/sb-workspace/**`, `crates/sb-pubsub/src/lib.rs`, `crates/sb-gopro/src/lib.rs`, `main.rs`: init / ws / install handlers and the two filename joins, `tests/level2_*.rs`, `tests/common/l3_sandbox.rs`, `tests/level5_gopro_round_trip.rs` |
| launch | `crates/sb-launch/**`, `main.rs`: `RunArgs`, `StopArgs`, up / run / stop / down / attach handlers, `tests/level5_*.rs` except gopro, `tests/common/l5_sandbox.rs` |
| doctor | `crates/sb-doctor/**`, new `crates/sb-cli/src/deprecation.rs`, `main.rs`: `mod deprecation;`, the four deprecated verb arms and their help strings |
| docs | `documents/**`, `README.md`, `requirements.md`, `installguide_*.md`, `platforms/README.md`, `examples/**` |

## 5. Tests: minimal

Fix what breaks; add four. Nothing else.

Broken by the change, fixed by the owner:

- `level2_init_empty_dir`, `level2_init_preserves_user_files` (filename,
  no runscript), `level2_ws_install` (fixture filename, exact stdout),
  `common/l3_sandbox.rs` (filename), `level5_gopro_round_trip` (filename),
  four sb-core unit tests (`spec_rejects_neither`, two instance tests,
  `module_ref_string_and_map_forms`).
- Deleted: `level2_runscript_stub.rs`.
- Untouched on purpose: the five level 3 codegen snapshot suites
  (`CodegenInput` does not change in 0.6.0), `level3_pub_add_mutation`
  (`topic:` is still written back), `level5_run_stop_args` and
  `level5_launch_two_modules` (legacy path must keep passing).

New, one each:

1. `level5_native_up.rs`: a module with `scripts.start` and two instances;
   `sb up` creates windows `m-a` and `m-b`; a sentinel written by the start
   command proves `SB_INSTANCE_ID` was set in the pane; a module whose `build`
   fails leaves no session.
2. `level5_native_stop.rs`: `sb stop m` closes both windows and runs
   `scripts.stop`.
3. `level2_legacy_manifest.rs`: a workspace with `sb.dev.yml` + `flow.yaml` +
   `runscript.bash` installs and runs, and stderr carries the three
   deprecation warnings.
4. sb-core unit: platform merge and placeholder expansion round trip on one
   manifest.

## 6. ws-ips after migration (verification fixture)

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
```

Deleted: `modules/*/runscript.bash`, `modules/*/stopscript.bash`, `flow.yaml`,
`modules/*/sb.dev.yml`. Until phase 3 lands, the `main()` key overrides stay
as they are.

## 7. Decisions taken

| # | decision | choice |
|---|---|---|
| 1 | extension of the two new files while `sb.config.yml` and `sb.app.yml` stay `.yml` | `.yaml`; the old two stay as they are |
| 2 | old sb binaries reject files with new keys (`deny_unknown_fields`) | accepted; a repo on the new schema needs sb 0.6+ |
| 3 | `[*]` fan-in on iceoryx2 | left at today's behaviour; zenoh-only in phase 3, error on iox2 |
| 4 | `sb init --force` wipes pub/sub/service entries | left as is, documented |
| 5 | 0.6.0 and 0.7.0 as minor bumps | yes |
| 6 | migration tooling | none; warnings point at the docs, user edits by hand |
| 7 | `topic:` on write | still written back; optional on read only, so files written by 0.6 still parse on 0.5 |

## 8. Deprecation table (goes into `documents/sbcli.md`)

| item | since | removed | replacement |
|---|---|---|---|
| `sb pub`, `sb sub`, `sb list`, `sb service` | 0.5.0 (help only), 0.6.0 (warning) | 0.7.0 | `sb io` |
| `sb.dev.yml` | 0.6.0 | 0.7.0 | `sb.module.yaml` |
| `flow.yaml` | 0.6.0 | 0.7.0 | `sb.flow.yaml` |
| `runscript.bash` | 0.6.0 | 0.7.0 | `scripts:` in `sb.module.yaml` |
| `stopscript.bash` | 0.6.0 | 0.7.0 | `scripts.stop`, or nothing |
| top-level `instances:` in the flow | 0.6.0 | 0.7.0 | `instances` under the module entry |
| `sb init --docker` bash template | 0.6.0 | 0.6.0 | none yet: `sb up` skips and `sb run` refuses native modules whose instances declare `docker:`; keep a legacy `runscript.bash` for those until a later release launches containers natively |
| `SB_NO_TMUX=1` | 0.6.0 | 0.7.0 | `sb run --fg` |

Warning format, stderr, once per item per process:

```
warning: <item> is deprecated since 0.6.0 and will be removed in 0.7.0; <hint>
```
