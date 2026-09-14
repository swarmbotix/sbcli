# L2 — Workspaces & Module Init (running report)

Source of truth for L2 progress. Update on every state change.

Last updated: 2026-05-27

---

## Status: instances + docker runscript extension landed

All sixteen L2 TDD tests from [level2.html](level2.html) are covered. The original eleven (workspace + module-init scope) still pass; the new five (tests 12–16) cover `flow.yaml::instances`, the `--docker` preflight + codegen, regeneration, and the generated runscript's runtime behavior.

Run just L2's integration tests:

```bash
cargo test -p sb-cli --test 'level2_*'
```

12 test binaries, 33 tests pass (27 original + 6 new `level2_init_docker.rs`).

---

## 2026-05-27 — Multi-instance modules + docker-aware runscripts

Plan file: `~/.claude/plans/but-now-modules-cam-gige-ht-adaptive-marble.md`.

### Why
`flow.yaml` previously mapped `module → sb.dev.yml` and nothing else. The `--id N` runtime override from `documents/sbcli_pubsub_consuming.md` §3 (which lets one binary run as N instances, each rewriting its own wire `<module>` segment) was invisible to the registry — so:

1. The planned L6 Simulink-style graph editor had no place to read "this module has these instances" and could not draw `cam_gige_ht-left` / `cam_gige_ht-right` as distinct nodes.
2. Containerized modules (`cam_gige_ht` runs in Docker; each camera needs its own image tag / `--device` / env / GPU) had no place to declare per-instance container settings, so `runscript.bash` was a one-size-fits-nobody shell stub.

### What landed

- **`WorkspaceFlow` extended** ([crates/sb-core/src/config.rs](../crates/sb-core/src/config.rs)) with an optional `instances: BTreeMap<String, Vec<ModuleInstance>>` map. New types: `ModuleInstance { id, label?, docker?, args }` and `DockerSpec { image?, env, devices, mounts, gpus?, network? }`. `#[serde(default)]` everywhere — pre-instances `flow.yaml` keeps parsing unchanged.
- **`WorkspaceFlow::validate()`** added; called from `read_flow` ([crates/sb-workspace/src/lib.rs](../crates/sb-workspace/src/lib.rs)) so malformed `flow.yaml` (invalid ids, duplicate ids per module) fails at load time. New error type `FlowValidationError`.
- **`validate_instance_id`** + `InstanceIdError` in [crates/sb-core/src/topic.rs](../crates/sb-core/src/topic.rs) — character class matches the wire-segment validator (`[A-Za-z0-9_-]+`), wider than `validate_identifier` because `-` must be legal for IDs like `front-left`.
- **Docker variant of the runscript** ([crates/sb-workspace/src/runscript.rs](../crates/sb-workspace/src/runscript.rs)) — new `render_docker(runscript_path, module, instances, flow_yaml_path)` emits a case-per-instance `docker run` dispatcher. Image / env / `--device` / mounts / `--gpus` / `--network` are baked in at render time; no flow.yaml lookup at runtime and no dependency on `sb` being on PATH inside the runscript's environment. Embedded staleness check: if `flow.yaml` mtime > runscript mtime, the script emits a one-line stderr warning and continues. Container name + hostname both default to `<module>-<id>`.
- **`sb init --docker` flag** ([crates/sb-cli/src/main.rs](../crates/sb-cli/src/main.rs)) with `InitOptions.docker` ([crates/sb-workspace/src/lib.rs](../crates/sb-workspace/src/lib.rs)) and a preflight that errors actionably when `flow.yaml::instances.<module>` is empty or when an instance lacks `docker.image` — each error names the missing element and points at the new consuming doc.
- **New worked-examples doc**: [documents/sbcli_docker_runscript.md](../documents/sbcli_docker_runscript.md) — `flow.yaml::instances` schema, `sb init --docker` walkthrough, runscript anatomy, per-language in-container entry-points (Rust / Python / C++ / Flutter / Unity), composition with the `TopicOverrides` API, and pitfalls (iox2 shm passthrough, GPU passthrough, USB device hot-plug).
- **Cross-references updated**:
  - [documents/sbcli_pubsub_consuming.md](../documents/sbcli_pubsub_consuming.md) §3 — note that `--id` is a string (`left` and `2` are interchangeable forms), cross-link to the new doc; §3.3 — string-form ids do not change `is_owned` semantics.
  - [requirements.md](../requirements.md) — `flow.yaml::instances` schema reference, `sb init --docker` flag.
  - `CLAUDE.md` (since removed from the repo) — Filename/type/owner table now mentions instances; new entry in the "When uncertain" table; codegen-API rule extended to cover the docker runscript contract.
  - [plan/level2.html](level2.html) — `--docker` added to the `sb init` signature, new `flow.yaml::instances` section, four new TDD tests (12–16), `init_docker.rs` added to the crate layout.
  - [plan/level5.html](level5.html) — "one pane per module" → "one pane per `(module, instance)` pair"; explicit note that `sb-launch` will fan out across `flow.modules` × `flow.instances` when L5 lands.
  - [plan/level6.html](level6.html) — node identity is `(module, instance)`; runtime overlay colors per-instance; explicit note that `sb pub add` flag surface is module-only (instance IDs are launch-time).

### Non-goals (explicit deferrals)

- No `sb instance add/edit/rm` management subcommand at this stage. Users hand-edit `flow.yaml`. Add later if friction shows.
- `sb up/down/run` does not yet fan out across instances. L5 plan now reflects the intended behavior; `crates/sb-launch/src/lib.rs:87-91`'s `for (module, dev_yml_path) in &flow.modules` loop will be expanded as part of L5.
- L6 graph editor implementation is still future; plan doc updated to make the data model explicit so it can be built directly.
- No per-module `docker.base:` block with per-instance overrides. YAML anchors handle dedup today; add a `base:` later if duplication becomes painful.

### Test counts after this change

- [crates/sb-core/src/config.rs](../crates/sb-core/src/config.rs) — 5 new in-file unit tests (round-trip with instances; backwards compat without `instances:` key; resolver lookup; validator rejects bad ids; validator rejects duplicates). 42 sb-core unit tests pass in total.
- [crates/sb-workspace/src/runscript.rs](../crates/sb-workspace/src/runscript.rs) — 2 new in-file unit tests (docker codegen smoke, shell-quote escape).
- [crates/sb-cli/tests/level2_init_docker.rs](../crates/sb-cli/tests/level2_init_docker.rs) — 6 new integration tests:
  - `init_docker_emits_runscript_with_branches` — happy-path codegen produces both case branches with correct flags
  - `init_docker_requires_instances_block` — missing `instances:` → actionable error referencing `flow.yaml` and the consuming doc, no runscript written
  - `init_docker_force_regenerates_from_updated_flow` — no-force ≠ regen; --force does
  - `docker_runscript_unknown_id_exits_nonzero_with_help` — `--id ghost` exits 2, names the unknown id and the known list
  - `docker_runscript_missing_id_exits_nonzero` — missing `--id` exits 2 with the known list
  - `docker_runscript_warns_when_flow_yaml_is_newer` — staleness check fires on stderr

---

---

## What landed

### sb-core extensions

- [config.rs](../crates/sb-core/src/config.rs) — added `Language` enum
  (`rust`, `python`, `cpp`, `flutter`, `unity`; serializes lowercase) and a
  `language: Language` field on `ModuleDevConfig`. `Language::default_io_dir()`
  encapsulates the Unity-vs-everyone-else split:
  - Unity → `Assets/Scripts/swarmbotix_io`
  - Rust / Python / C++ / Flutter → `swarmbotix_io`
- Added `sb_home_dir: Option<PathBuf>` to `SbCliConfig`. The single
  knob that points the CLI at a different `~/.swarmbotix/` root.
  Resolution lives in sb-config (`resolve_sb_home_dir`).
- Changed `default_io_dir()` from `swarmbotix` to `swarmbotix_io`
  (matches requirements.md `<module>/swarmbotix_io/` and the L2 spec).
- [ident.rs](../crates/sb-core/src/ident.rs) — `validate_identifier` /
  `is_valid_identifier` enforce the workspace + module name rule:
  non-empty, starts with ASCII letter or `_`, then ASCII letters / digits /
  `_`. Stricter than the prose hint ("no spaces/slashes") so we can reject
  `demo-app`, `123demo`, etc. per L2 TDD test #11.

### sb-config

- [`expand_tilde`](../crates/sb-config/src/lib.rs) — public utility for
  the `~/...` prefix.
- [`resolve_sb_home_dir`](../crates/sb-config/src/lib.rs) — the single
  source of truth for "where is the swarmbotix tree on this machine?"
  Reads `sb_home_dir` from a merged config; tilde-expands; falls back
  to the per-user default.
- `apply_key` learned the `sb_home_dir` key so
  `sb config set sb_home_dir /foo` works.

### sb-workspace — new crate ([crates/sb-workspace/](../crates/sb-workspace/))

- [lib.rs](../crates/sb-workspace/src/lib.rs):
  - `SbHome::from_config(&SbCliConfig)` — single constructor used by
    the CLI. All `~/.swarmbotix/` paths derive from this value type.
  - `SbHome::at(PathBuf)` — convenience for direct unit tests.
  - `create_workspace` / `list_workspaces` / `set_active` / `delete_workspace`
    / `active_workspace` — every workspace lifecycle action.
    `delete_workspace` returns a `DeleteOutcome { cleared_active }` so
    the CLI can emit the stderr warning when the active workspace was
    deleted.
  - `read_flow` / `write_flow` — YAML R/W for `flow.yaml` under each
    workspace dir.
  - `init_module(&SbHome, &InitOptions)` — the load-bearing `sb init`
    logic. Idempotent: only writes missing artifacts unless `--force`;
    only appends to `flow.yaml` when the module isn't already
    registered or points elsewhere. Returns `InitOutcome` flagging
    which artifacts changed.
- [runscript.rs](../crates/sb-workspace/src/runscript.rs) — single
  shared `runscript.bash` template (comment block with per-language
  examples; body `echo TODO; exit 1`). Test #8 golden-snippets the
  comment block, so this file is the spec.

### sb-cli — `sb` binary

- [main.rs](../crates/sb-cli/src/main.rs) — added two top-level
  subcommands:
  - `sb ws create|set|list|delete <name>`
  - `sb init [<rootpath>] [--rust|--python|--cpp|--flutter|--unity] [--force]`
- Internal `sb_home()` helper loads the merged `SbCliConfig` and
  passes it to `SbHome::from_config` — same code path the tests use.

### Test helper

- [tests/common/l2_sandbox.rs](../crates/sb-cli/tests/common/l2_sandbox.rs)
  — `WsSandbox`: builds a tempdir, writes a temp `sb.config.yml`
  inheriting the real dev-box config with `sb_home_dir: <tempdir>`
  appended. `cmd()` sets `SB_CONFIG` on the spawned `sb` process.

### Topic naming format note

`io_dir` is serialized without a trailing slash. Requirements.md shows
`io_dir: swarmbotix_io/` (with slash); we emit `swarmbotix_io` because
`PathBuf` normalizes the trailing slash away. Functionally identical
(every consumer joins onto it); cosmetic only.

---

## TDD test → location map

| # | Spec | Integration test |
|---|---|---|
| 1 | Workspace lifecycle round-trip | [level2_workspace_lifecycle.rs](../crates/sb-cli/tests/level2_workspace_lifecycle.rs) |
| 2 | `sb init` with no active workspace | [level2_init_no_active_workspace.rs](../crates/sb-cli/tests/level2_init_no_active_workspace.rs) |
| 3 | `sb init` on empty dir writes 3 artifacts | [level2_init_empty_dir.rs](../crates/sb-cli/tests/level2_init_empty_dir.rs) |
| 4 | Idempotent re-run | [level2_init_idempotent.rs](../crates/sb-cli/tests/level2_init_idempotent.rs) |
| 5 | Preserves user project files (Rust/Python/C++/Flutter/Unity matrix) | [level2_init_preserves_user_files.rs](../crates/sb-cli/tests/level2_init_preserves_user_files.rs) |
| 6 | Language flag handling (3 cases) | [level2_init_language_flag.rs](../crates/sb-cli/tests/level2_init_language_flag.rs) |
| 7 | `sb.dev.yml` shape (serde round-trip per language) | [level2_dev_yml_shape.rs](../crates/sb-cli/tests/level2_dev_yml_shape.rs) |
| 8 | `runscript.bash` stub contents + executable bit | [level2_runscript_stub.rs](../crates/sb-cli/tests/level2_runscript_stub.rs) |
| 9 | Unity `io_dir` = `Assets/Scripts/swarmbotix_io/` | [level2_init_unity_io_dir.rs](../crates/sb-cli/tests/level2_init_unity_io_dir.rs) |
| 10 | Refuse-to-overwrite vs `--force` | [level2_refuse_overwrite.rs](../crates/sb-cli/tests/level2_refuse_overwrite.rs) |
| 11 | Identifier validation (table test) | [level2_identifier_validation.rs](../crates/sb-cli/tests/level2_identifier_validation.rs) |

Every L2 test uses the
[`WsSandbox`](../crates/sb-cli/tests/common/l2_sandbox.rs) helper.

---

## End-to-end verification (this dev box)

```text
$ cat /tmp/sb_l2_smoke2/sb.config.yml
sb_home_dir: /tmp/sb_l2_smoke2
device: dev01
transport_on_device: iceoryx2
transport_cross_device: zenoh

$ SB_CONFIG=/tmp/sb_l2_smoke2/sb.config.yml sb ws create demo
created workspace demo at /tmp/sb_l2_smoke2/workspaces/demo

$ SB_CONFIG=/tmp/sb_l2_smoke2/sb.config.yml sb ws set demo
active workspace = demo

$ SB_CONFIG=/tmp/sb_l2_smoke2/sb.config.yml sb ws list
* demo

$ SB_CONFIG=/tmp/sb_l2_smoke2/sb.config.yml sb init --python /tmp/sb_l2_smoke2_pubber
adopted module sb_l2_smoke2_pubber (language: python, ...)
  wrote sb.dev.yml
  wrote runscript.bash (executable)
  created IO directory
  registered in flow.yaml

$ find /tmp/sb_l2_smoke2 -type f
/tmp/sb_l2_smoke2/sb.config.yml
/tmp/sb_l2_smoke2/active
/tmp/sb_l2_smoke2/workspaces/demo/flow.yaml
```

---

## Known gaps / follow-ups (not blocking L2)

### `sb-workspace` follow-ups

- **Workspace-scoped `sb.config.yml`.** `SbHome` now lets the per-
  workspace `sb.config.yml` slot in at
  `<sb_home_dir>/workspaces/<ws>/sb.config.yml`, but
  `sb config --workspace` still errors with "not yet implemented".
  Wire it up once at least one L2+ consumer needs it.
- **No `sb-workspace` unit tests.** Every assertion lives in the L2
  integration tests, which double as the spec. Pure-logic helpers
  (flow.yaml read/write) could grow direct tests later — kept thin for
  now.
- **Cross-platform executable bit.** `set_executable` is `#[cfg(unix)]`
  and a no-op on Windows. When L5 lands `sb run` we'll need a story
  for Windows runscripts (probably emit `runscript.cmd` or call bash
  via WSL).

---

## What L3 inherits from L2

These primitives are now stable and L3 should consume them as-is:

- `sb_core::Language` — `--module <name>` codegen will switch on this
  to pick file extensions (`.rs`, `.py`, `.cc`, `.dart`, `.cs`).
- `sb_core::ModuleDevConfig` — the canonical struct that `sb pub/sub
  add` mutates. L3 reads it from `sb.dev.yml`, appends to `publishers`
  / `subscribers`, writes back.
- `sb_workspace::active_workspace`, `read_flow`, `write_flow` — module
  resolution for the `--module <name>` flag at L3 (rule 1 in
  requirements.md: "active workspace's `flow.yaml` has it → that
  path").
- `sb_workspace::SbHome::from_config` — every `~/.swarmbotix/` path
  derives from a merged config; L3 codegen targets
  `<module_root>/<io_dir>/{publishers,subscribers}/` where
  `module_root` and `io_dir` come from `ModuleDevConfig`.
- `sb_config::resolve_sb_home_dir` — the resolver L3 tests should
  reuse if they need a hermetic root.
