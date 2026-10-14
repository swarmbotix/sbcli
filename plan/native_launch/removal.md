# 0.7.0: removal of the surface deprecated in 0.6.0

Status: implemented 2026-10-13, cut as 0.7.0, not installed on the dev box (its own workspaces are still on the old names). Verification: `cargo test --workspace` green, clippy clean, `level5_e2e_hello_world` green, reinstall over a 0.6 installed copy with top-level `instances:` verified to replace it.

Rollback point: sbcli `a2934bf`, sbcli_forge `c34d02a`.

Same execution model as 0.6.0 (`README.md` §4): one write-only wave, one
build on the main thread, errors routed back by message. Three agents:
`cli-launch`, `core-ws`, `docs`. Tests minimal: convert or delete what the
removal breaks, add nothing beyond what §5 lists.

## 1. What is removed

| item | 0.6.0 | 0.7.0 |
|---|---|---|
| `sb.dev.yml` fallback | read with warning | not read; `sb.module.yaml` only |
| `flow.yaml` fallback | read with warning | not read; `sb.flow.yaml` only |
| `runscript.bash` launch path | honoured when no `scripts:` | ignored; every module launches natively |
| `stopscript.bash` | honoured on legacy modules | ignored |
| `sb pub`, `sb sub`, `sb list`, `sb service` | work with warning | removed from clap |
| top-level `instances:` in the flow | read with warning | rejected with a hint |
| `sb init --runscript` and the stub template | writes `runscript.bash` | removed; `runscript.rs` deleted |

Kept: `sb_core::Deprecation`, `deprecation::warn_deprecated`, `warn_all`
(for future deprecations; no items today), the doctor `deprecated` row (now
reports legacy files as unsupported), `sb init --docker`.

## 2. Error and message texts

All exact. `<hint>` is `see sbcli_module.md §Migrating`.

| situation | text |
|---|---|
| module dir has `sb.dev.yml` and no `sb.module.yaml` (install, init, io, doctor) | `module manifest sb.module.yaml not found at <dir>; sb.dev.yml is no longer read since 0.7.0, rename it (<hint>)` |
| workspace dir has `flow.yaml` and no `sb.flow.yaml` (`sb install`, `find_flow_upwards`, `read_flow`) | `sb.flow.yaml not found at <dir>; flow.yaml is no longer read since 0.7.0, rename it (<hint>)` |
| flow has top-level `instances:` | serde rejects it as an unknown field; `WorkspaceFlow` validation error text is `top-level instances: is no longer read since 0.7.0; move each list under modules.<name>.instances (<hint>)` produced by a pre-parse probe (see §3 core) |
| `sb pub ...` etc. | clap's own `unrecognized subcommand` error, plus a `tip:` line clap prints when the removed names are registered as hidden aliases is NOT used; instead `Cmd` keeps no trace and the error is clap's default |
| a module root still holds `runscript.bash` or `stopscript.bash` | no error; doctor `deprecated` row lists them: `runscript.bash at <path> is ignored since 0.7.0 (<hint>)` |

Doctor `deprecated` row: WARN with one line per legacy file found (`sb.dev.yml at`, `flow.yaml at`, `runscript.bash at`, `stopscript.bash at`), each ending `is ignored since 0.7.0 (<hint>)`. OK text unchanged: `nothing deprecated in use`.

## 3. Per crate

### sb-core (`core-ws`)

- Delete `LEGACY_MODULE_FILE`, `LEGACY_FLOW_FILE`, `is_legacy_module_file`,
  `is_legacy_flow_file`. `module_file_candidates()` and
  `flow_file_candidates()` become `[&'static str; 1]`.
- Delete `WorkspaceFlow.instances` (the legacy map) and
  `has_legacy_instances`. `instances_for` reads the inline list only.
  Delete `FlowValidationError::BothInstanceForms` and
  `InstancesForUnknownModule`; add `LegacyTopLevelInstances` with the §2 text.
- Add `pub fn probe_legacy_flow_keys(yaml: &str) -> Option<FlowValidationError>`:
  a cheap pre-parse (serde_yaml::Value) that returns
  `LegacyTopLevelInstances` when the document has a top-level `instances`
  key. Callers run it before the strict parse so the user gets the hint, not
  `unknown field`.
- Unit tests: delete or flip the legacy-form cases.

### sb-workspace, sb-pubsub, sb-gopro, sb-doctor (`core-ws`)

- `find_module_file` / `find_flow_file` probe the single name; when the
  legacy file exists instead, callers that need the file return the §2 error.
- `resolve_module_yaml`, `read_flow`, `find_flow_upwards`, `install_flow`,
  `init_module`, `register_in_repo_flow`: no legacy branches;
  `install_flow` and `read_flow` run `probe_legacy_flow_keys` first.
- `InitOptions.runscript`, `InitOutcome.legacy_manifest`,
  `InitOutcome.runscript_written` deleted; `runscript.rs` deleted; the
  `--runscript` flag and its print line in `main.rs` deleted (`core-ws` edits
  `InitArgs` and `cmd_init` only, with the Edit tool).
- `write_flow` no longer deletes a 0.5 `flow.yaml` in the workspace dir.
- sb-pubsub `resolve_module`: single name, §2 error when only `sb.dev.yml` exists.
- sb-doctor: `manifest_path` single name; `check_ws_deprecated` lists legacy
  files with the §2 texts; `ws modules` FAIL text for a dir with only
  `sb.dev.yml` uses the §2 manifest error.
- Tests: `level2_legacy_manifest.rs` becomes `legacy_names_are_refused`
  (install fails, stderr contains `no longer read since 0.7.0`); remove the
  `--runscript` case from `level2_init_empty_dir.rs`; `level2_refuse_overwrite.rs`
  fixture renamed to the new names (it tests "existing files preserved",
  which still holds).

### sb-launch and sb-cli (`cli-launch`)

- Delete `PaneCmd::Legacy`, `StopPlan.legacy`, every runscript/stopscript
  probe and the mode rule; every module is native. `Pane.cmd` becomes
  `PaneCmd::Native` fields inlined (`argv`, `env`) or keep the enum with one
  variant, agent's choice, documented in the report.
- `sb-launch` unit tests updated.
- `main.rs`: delete `Cmd::List/Pub/Sub/Service`, `ListArgs`, `PubCmd`,
  `SubCmd`, `ServiceCmd`, `PubAddCli`, `PubEditCli`, `SubAddCli`, the
  `cmd_pub_*`, `cmd_sub_*`, `cmd_service_init` handlers and their dispatch
  arms. Keep `cmd_list` and every function the `sb io` handlers still call.
  `deprecation.rs`: delete `items::*`; keep the two functions and the
  constants (`SINCE`/`UNTIL` become `"0.7.0"` / `"0.8.0"` placeholders with
  a comment that no item uses them).
- Tests: every invocation of the removed verbs becomes its `sb io` form:
  `pub add <args>` -> `io add pub <args>`; `sub add` -> `io add sub`;
  `pub edit` -> `io edit`; `sub edit <topic>` -> `io edit <name> --key <key>`
  or whatever the `sb io edit` handler needs to reach the same end state
  (read the handler); `list` -> `io list`; `service init` ->
  `io add service <name>`. Files: `level3_list_rendering`,
  `level3_pub_add_mutation`, `level3_transport_default`,
  `level5_gopro_round_trip`, plus any multi-line invocation grep misses in
  `level3_cli_per_language`, `level3_conflict_policy`,
  `level3_rust_cargo_check`, `level4_*`. `level5_run_stop_args.rs` and
  `level5_launch_two_modules.rs` are rewritten on native modules
  (`scripts.start` writing the same sentinels) so the passthrough-args and
  two-module assertions survive; `l5_sandbox.rs` loses the runscript helpers.

### docs (`docs`)

- Delete `sbcli_launch.md` §10 (legacy) and every "fallback" sentence;
  `sbcli_init.md` loses `--runscript`; `sbcli_ws.md` §4.8 (legacy names)
  becomes a two-line "not read since 0.7.0" note; `sbcli.md` §10 table
  gets a "removed in" column filled with 0.7.0 and the warning-format
  paragraph says there are no active deprecations; `sbcli_module.md` §11
  Migrating stays, retitled "Migrating from 0.5 (required since 0.7.0)";
  `sbcli_doctor.md` `deprecated` row texts per §2; `sbcli_pubsub.md` §3
  module resolution single name; `README.md`, `requirements.md` deprecation
  sections updated; `examples/rust_python_zenoh/run.sh` comment lines 51 to
  54 reworded to `sb io add`.
- Marker for new text: `sb 0.7.0+`.

## 4. Main thread

Merge, `cargo check`, `cargo build`, `cargo test --workspace`, run the ignored
`level5_e2e_hello_world`, bump to 0.7.0 (`/sb-release`), bundle. Do not
install and do not push; report first.

## 5. Tests

No new tests. Converted: the ones named above. Deleted: none beyond what the
agents replace in place.
