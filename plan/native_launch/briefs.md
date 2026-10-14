# Briefs: the single writing wave

Five `coder` agents, dispatched together, each in its own worktree
(`isolation: "worktree"`), each at the sbcli submodule root. The main thread
sends each brief verbatim plus the two shared blocks below. Compile and test
errors come back to the same agent by message.

## Shared block A: mode

> Build mode: **one-shot check**. Write only. Do not run cargo, clippy, or
> any test. The main thread builds once after every agent has finished and
> sends compile or test errors back to you by message; fix them in place when
> they arrive. Code against `plan/native_launch/contract.md`; every name
> there is fixed. If you need a name that is not there, add it in your own
> crate and list it under `ADDED` in your report. Do not rename anything in
> the contract. Read `plan/native_launch/README.md` §2 for the design. Version
> bump and install are handled elsewhere; never touch `Cargo.toml` versions.
> Tests are minimal: fix the tests your change breaks; add only what your
> brief lists. Report with `CHANGED / ADDED / SKILLS / TESTS / DOCS /
> DEFERRED / BLOCKED`.

## Shared block B: ownership

> You own only the files listed under "owns". Do not edit any other file. If
> you believe another file must change, write the exact edit under `DEFERRED`
> with the file and line; the main thread routes it. `crates/sb-cli/src/main.rs`
> is shared by several agents, split by function; edit only the functions
> and items your brief names.

---

## Brief 1: core

**Owns:** `crates/sb-core/**`.

**Task:** implement contract §1 to §5: filename constants and helpers,
`Host`, `Deprecation`, the new `ModuleDevConfig` fields (`version`, `sb`,
`description`, `scripts`, `env`, `requires`, `platforms`), `Script` with
string-or-list serde and the §2.1 splitter, `Scripts`, `Overlay`,
`Platforms` with key validation, `Resolved`, `ModuleDevConfig::resolve`,
`language_defaults` (table §3), `python_venv_relpath`, `python_fallback`,
the `expand` module (§4), flow changes (§5: `ModuleRef.instances`,
`ModuleInstance.env` and scalar form including numeric ids, `wires`,
`instances_for`, `has_legacy_instances`, `wires()`, `parse_wire`, new
`FlowValidationError` variants), `topic::instance_module`, optional `topic:`
on read.

**Where:** `crates/sb-core/src/config.rs` (`ModuleDevConfig` at 371,
`RawSpec::topic_ref` at 795, `WorkspaceFlow` at 1036, `ModuleRef` and its
hand-written `Deserialize` at 1150 to 1253, `ModuleInstance` at 1324),
`crates/sb-core/src/topic.rs` (`validate_instance_id` at 131,
`is_valid_segment_char` at 116), new `host.rs` and `expand.rs`, re-exports in
`lib.rs`.

**Constraints:** keep `deny_unknown_fields` on every struct that has it. No
`untagged`, no `flatten`. No new dependencies (sb-core has serde, serde_yaml,
thiserror only). `topic:` is still written back on serialize; optional on
read only. `Platforms` keys validated against `KNOWN_OS` / `KNOWN_ARCH`.
`Script` serialization rule as in contract §2. The `ModuleRef` short form
`name: path` is written only when `up && instances.is_empty()`.

**Done when:** every item in contract §1 to §5 exists with the given name and
signature; `validate()` on both configs covers the new rules; the four unit
tests that break (`spec_rejects_neither` at 1422, the two instance tests at
1470 and 1494, `module_ref_string_and_map_forms` at 1628) are updated; one
new unit test covers platform merge plus expansion on a single manifest
(rust module, linux overlay with a `${VAR:-default}` env, two instances,
`{bin} {args}` start with `--id` appended by the caller is not tested here).
Also fix the stale `[A-Za-z0-9_]` text at `topic.rs:26` and `:188` to
include `-`.

**Leave alone:** `SbCliConfig`, `ModulePrdConfig` beyond what the shared
`RawSpec` change implies, `transport.rs`, `style.rs`, `ident.rs`, any file
outside sb-core.

---

## Brief 2: workspace

**Owns:** `crates/sb-workspace/**`, `crates/sb-pubsub/src/lib.rs`,
`crates/sb-gopro/src/lib.rs`, in `crates/sb-cli/src/main.rs` only: `InitArgs`
and `cmd_init` (884 to 908, 2107 to 2144), `cmd_ws_create` (around 1944),
`cmd_install` / `install_from_git` / `finish_workspace_install` /
`install_io` (2192 to 2770), the two filename joins at 1851 and 5155,
`sb io update` service flag plumbing at 1871; tests
`crates/sb-cli/tests/level2_*.rs`, `tests/common/l3_sandbox.rs`,
`tests/level5_gopro_round_trip.rs`.

**Task:** contract §6. New filenames with legacy fallback everywhere
sb-workspace, sb-pubsub, sb-gopro and the named main.rs functions resolve a
manifest or flow; `sb ws create` and `sb install` write `sb.flow.yaml` (repo
template and installed copy); `read_flow` and `find_flow_upwards` probe both
names; `install_flow` carries inline `instances` and `wires` into the
installed copy and pushes `Deprecation`s for legacy names and top-level
instances; `init_module` writes `sb.module.yaml`, adopts an existing
`sb.dev.yml` read-only with a `Deprecation`, writes `runscript.bash` only
with the new `--runscript` flag, runs docker preflight before any write,
and no longer renders the docker bash template; delete `render_docker`,
`render_case_branch`, `preflight_docker_instances` stays (moved to a pure
fn returning the instance list for sb-launch to use later); `update_all`
takes `ServiceRender`, `sb install` renders missing service routers and
prints the new line; `cmd_init` prints per contract §10 and calls
`crate::deprecation::warn_all(&outcome.deprecations)` (the function exists
in the doctor agent's new module; just call it); the install path calls the
same for `report.deprecations`.

**Where:** `crates/sb-workspace/src/lib.rs` (`DEV_YML` 261, `FLOW_FILE` 258,
`flow_path` 89, `read_flow` 217, `find_flow_upwards` 269, `install_flow` 334,
`resolve_module_yaml` 484, `init_module` 787 to 964,
`register_in_repo_flow` 984), `crates/sb-workspace/src/runscript.rs`,
`crates/sb-pubsub/src/lib.rs` (`resolve_module` 77 to 122, `update_all` 1138,
`write_dev_yml` 1469), `crates/sb-gopro/src/lib.rs` (269, 279, 292, 297).

**Constraints:** registry paths recorded by 0.5 installs point at
`.../sb.dev.yml` explicitly and must keep resolving. Do not normalize a
legacy top-level `instances:` map into inline form when rewriting a flow;
copy it as is. `sb init --force` behaviour (wipes entries) stays. Keep the
stub runscript template for `--runscript`. Service router: render only when
the target file does not exist; never overwrite.

**Done when:** every filename resolution in your files goes through the
sb-core constants or the sb-workspace `find_*` helpers; no string literal
`"sb.dev.yml"` or `"flow.yaml"` remains in your files except inside the
constants and the legacy-detection helpers; `level2_init_empty_dir`,
`level2_init_preserves_user_files`, `level2_ws_install`, `l3_sandbox`,
`level5_gopro_round_trip` are updated for the new names and the absent
runscript; `level2_runscript_stub.rs` is deleted; `level2_refuse_overwrite`
still passes unchanged (legacy file adopted, never rewritten); new
`tests/level2_legacy_manifest.rs`: a workspace with `sb.dev.yml`,
`flow.yaml` and a stub `runscript.bash` installs, and stderr of `sb install`
contains `sb.dev.yml is deprecated` and `flow.yaml is deprecated`.

**Leave alone:** sb-launch, sb-doctor, every other main.rs function, the
level 3 codegen snapshots and goldens, `GenCtx`, templates.

---

## Brief 3: launch

**Owns:** `crates/sb-launch/**`; in `crates/sb-cli/src/main.rs` only:
`Cmd::Up/Run/Stop/Down/Attach` doc strings (81 to 98), `RunArgs` and
`StopArgs` (377 to 395), `cmd_up`, `cmd_run`, `cmd_stop`, `cmd_down`,
`cmd_attach`, `require_active_workspace`, `warn_if_stale` (5173 to 5301);
tests `crates/sb-cli/tests/level5_*.rs` except `level5_gopro_round_trip.rs`,
`tests/common/l5_sandbox.rs`.

**Task:** contract §7 and §10. Replace the pane model with
`Pane { module, window, instance, cwd, cmd: PaneCmd }`, add `BuildStep`,
the new `LaunchPlan`, `plan_workspace(home, ws, host)`, `plan_run`, `StopPlan`,
`plan_stop`, `run_builds`, `up`, `run`, `stop`, `run_foreground`,
exact-target `has_session`/`down`, window ids from `list-windows`,
`-c`/`-e` on every tmux spawn, `remain-on-exit` for native windows, psmux
fallback in `resolve_tmux`. Mode rule per module as in §7. Native env and
argv assembly as in §7. `{python}` resolution per contract §3. Handlers:
`sb up` runs `run_builds` then `up` and prints §10 lines including `skipped`
and the deprecation warnings via `crate::deprecation::warn_all`; `sb run`
parses `<module>[:<id>]`, honours `--fg`, builds, then `run` or
`run_foreground`; `sb stop` plans, prints §10, calls `warn_if_stale` too.
Host comes from `sb_core::Host::current()`.

**Where:** `crates/sb-launch/src/lib.rs` (entire file; `plan_module` 179,
`Pane::shell_command` 82, `run_single` 314, `new_window` 422,
`respawn_window` 463, `shell_quote` 488, unit tests 506 to 595).

**Constraints:** legacy path (`runscript.bash`, `stopscript.bash`) must keep
passing `level5_launch_two_modules` and `level5_run_stop_args` unchanged,
including `--id sb01` arriving as passthrough argv. No clap `--id`. All
preconditions (manifest parse, unsupported platform, build) are checked or
run before the first tmux call. `cd` for native panes is tmux `-c`, not a
shell string. Confirm multi-argument direct exec and `-e` support against
`man tmux` on the box (tmux 3.4) and write what you found in your report;
psmux is unverified, state so. Windows: `run_foreground` spawns and waits.

**Done when:** every item in contract §7 exists with the given name; the two
existing unit tests are updated to the new `Pane` shape; new
`tests/level5_native_up.rs` (module with `scripts.start` writing a sentinel
that includes `$SB_INSTANCE_ID`, two instances in the flow, `sb up` yields
windows `m-a` and `m-b` and both sentinels; a second module whose `build`
is `false` makes `sb up` fail with no session created) and
`tests/level5_native_stop.rs` (`sb stop m` closes both windows and
`scripts.stop` writes its sentinel). Both guarded by
`tmux_launch_supported()`.

**Leave alone:** sb-core, sb-workspace, sb-doctor, every other main.rs
function, sb-pubsub, templates.

---

## Brief 4: doctor

**Owns:** `crates/sb-doctor/**`; new file `crates/sb-cli/src/deprecation.rs`;
in `crates/sb-cli/src/main.rs` only: the `mod deprecation;` line, the four
`Cmd::List/Pub/Sub/Service` doc strings (59 to 76) and their dispatch arms
(1164 to 1180), `workspace_checks` (3212 to 3242) to pass `host`.

**Task:** contract §8 and §9. The four doctor rows (`ws requires`,
`ws platforms`, `ws wires`, `deprecated`) with the exact texts; `WorkspaceView`
gains `host`; existing rows say "module manifest" instead of `sb.dev.yml`
(`check_module_topics` at 426 to 487 probes `module_file_candidates()`);
`deprecation.rs` with `warn_deprecated`, `warn_all`, the constants, once-per-
item dedupe; the four verbs call `warn_deprecated` at dispatch before doing
their old work; help strings updated.

**Where:** `crates/sb-doctor/src/lib.rs` (`check_workspace` 562 to 611,
`load_dev_yml` 614, `check_ws_instances` 686, `check_ws_encoding` 713,
`check_ws_io_files` 810, `check_deprecated_keys` 389 as the pattern),
`crates/sb-cli/src/main.rs` (the named items only).

**Constraints:** sb-doctor depends on sb-core, sb-config, sb-vault only; do
not add sb-launch or sb-workspace. `requires` lookup is `which::which`
(already a workspace dependency; add it to sb-doctor's `Cargo.toml` deps,
not versions). The `deprecated` row is WARN; `ws requires` and `ws wires`
are FAIL; `ws platforms` is WARN. A module unsupported on the host is
skipped by `ws requires`. Wire existence checks use the loaded manifests
(`LoadedModule`) and `flow.instances_for`; do not resolve keys. Message
format of existing rows otherwise unchanged (`level2_ws_install` asserts no
`[FAIL]` on a fixture with no `requires`, so an empty `requires` must be OK).

**Done when:** all four rows appear in `sb ws check` and `sb doctor` output
in the order given; `sb pub list` prints the warning line on stderr exactly
once and still works; `deprecation::warn_all` is callable from other
handlers.

**Leave alone:** every other main.rs function, sb-launch, sb-workspace,
sb-core.

---

## Brief 5: docs

**Owns:** `documents/**`, `README.md`, `requirements.md`,
`installguide_ubuntu.md`, `installguide_windows.md`, `platforms/README.md`,
`examples/**`.

**Task:** document the 0.6.0 surface from `README.md` §2, §3, §7, §8 and
`contract.md` §3, §8, §10, before the binary exists. Specifically:

1. New `documents/sbcli_module.md`: the `sb.module.yaml` reference. Every
   field, the three rules (argv not shell, defaults, platforms), the
   placeholder list, `${VAR:-default}`, the language defaults table verbatim
   from contract §3, the merge order, `platforms` semantics, `requires`, a
   "Migrating from 0.5" section with before/after for each deprecated item
   (table in README §8) and the `ws-ips` fixture from README §6 as the
   worked example. Add an "added in 0.6.0" marker per `sb-doc` skill.
2. `documents/sbcli_launch.md`: rewrite §1 (what `sb up` does: build, then
   windows per instance), §1.1 window naming `<module>-<id>`, §3 `sb run`
   with `<module>:<id>` and `--fg`, §4 `sb stop` native semantics, §4.2
   (the "why do I write a stopscript" FAQ becomes "you don't"), §7
   passthrough args, exact output lines from contract §10, legacy section
   marked deprecated, Windows paragraph: psmux, works natively now.
3. `documents/sbcli_init.md`: writes `sb.module.yaml`, no runscript unless
   `--runscript`, `--docker` meaning, output lines.
4. `documents/sbcli_ws.md`: `sb.flow.yaml`, inline `instances`, short and
   long form, `wires` grammar (validated in 0.6.0, honoured by codegen in a
   later release), `sb ws check` new rows with their texts, `up: false`.
5. `documents/sbcli_doctor.md`: the four rows.
6. `documents/sbcli.md`: layout tree, command table rows for init / up / run
   / stop, the FAQ row about stopscript, and a new "Deprecations" section
   holding the table from README §8 and the warning format.
7. `documents/sbcli_pubsub.md` and `documents/sbcli_pubsub_consuming.md`:
   `topic:` optional; file renamed everywhere; fix the invalid zenoh wildcard
   `app1-*` to `app1-$*` at consuming §3.3 (around line 189) and line 942;
   mark the "parse `--id` in main()" pattern as still valid but note
   `SB_INSTANCE_ID` is now exported by `sb up`/`sb run`; the cross-module
   subscription section points at `wires` as the upcoming mechanism, with
   `--key` still the working one in 0.6.0.
8. Delete `documents/sbcli_docker_runscript.md`; replace its one inbound link
   in `sbcli.md` with `sbcli_module.md`.
9. `requirements.md` (about line 645 and 894), `README.md`,
   `platforms/README.md`: rename files, remove runscript mentions, keep the
   tmux/psmux table.
10. `installguide_windows.md`: close §9.5 (native launch removes the bash
    pane command; psmux status "to be verified"), add an LLVM/libclang row to
    the build-machine prerequisites table (iceoryx2 runs bindgen on the
    windows target), and the same note in `documents/sbcli_pubsub_consuming.md`
    §7 beside the `iceoryx2 = "=0.9.3"` pin.
11. `examples/`: convert the five example projects: delete every
    `runscript.bash`, rename `sb.dev.yml` to `sb.module.yaml` and `flow.yaml`
    to `sb.flow.yaml` where present, add `scripts.start` only where the
    language default does not apply (python examples use `main.py` so
    defaults apply; check each), update each example README and
    `examples/level5_hello_world/run.sh` to the new file names and to
    `sb up` without scripts.

**Constraints:** write in the voice and structure of the existing docs; no
Claude attribution anywhere; keep every doc's existing section numbering
where possible so cross-links survive; quote output lines exactly as
contract §10; where the binary's final text is uncertain, quote the contract
and flag the line under `DRIFT` so the post-build pass can confirm it. Do not
document phase 3 wire codegen as shipped.

**Done when:** no shipped doc mentions `runscript.bash` or `stopscript.bash`
except in the Deprecations table and the Migrating section; no shipped doc
names `sb.dev.yml` or `flow.yaml` except as the deprecated old name; every
inbound link to the deleted doc is repointed; `grep -rn "sb.dev.yml\|runscript" documents/`
returns only those allowed lines.

**Leave alone:** every `.rs` file, `Cargo.*`, `versions.json`,
`platforms/*/version.json`, `plan/**`.
