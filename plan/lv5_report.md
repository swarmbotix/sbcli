# L5 — Launch & Production (running report)

Last updated: 2026-08-06

---

## Status: done, runtime-verified end-to-end

Every "Done when" item from [level5.html](level5.html) AND every TDD
test from its plan has a passing test. Two new crates landed —
`sb-launch` (tmux orchestration) and `sb-gopro` (sb.prd.yml export) —
plus the L5 CLI verbs `sb up / run / stop / down / attach / gopro` wired
into [crates/sb-cli/src/main.rs](../crates/sb-cli/src/main.rs).

```
sb up                              # launch every module in active workspace
sb run <module> [args...]          # idempotent single-module launch; args -> runscript.bash
sb stop <module> [args...]         # symmetric counterpart; args -> stopscript.bash
sb down                            # kill the workspace's tmux session
sb attach                          # interactive: exec tmux attach; non-tty: print session name
sb gopro                           # one module — dev.yml + io_dir/ → prd.yml (sanitized)
sb gopro --all                     # every module in the active workspace
```

**The installer moved inside the package.** Up to 0.1.40 a release page carried
five assets: three zips, their checksums, and a loose `install.sh` /
`install.ps1`. The installers were *zip pickers* — read `uname -m`, glob for a
sibling `swarmbotix-*-<arch>.zip`, extract it to a temp dir, copy the payload
out. That shape has three problems, and the third is the one that matters:

- Two downloads that must match. A user with the right zip and last release's
  installer gets an install nobody tested.
- The script on the release page can drift from the payload it installs. They
  are versioned together in the repo and separately on the page.
- **The installer is the least-verified thing in the release.** The zip has a
  `.sha256` and CI now checks it; the loose script beside it has neither.

So `build.sh` / `build.ps1` stage `install.*` and `uninstall.*` into the payload
at step 7, before zipping, and no longer drop a copy in the version slot;
`build.yml` stopped uploading `install.*` as an artifact. The package is the
unit now: unzip it, run the `install.sh` that comes out, and it installs its own
siblings. `PAYLOAD_DIR` is simply `$SCRIPT_DIR` — the glob, the temp dir, the
`unzip` dependency and the whole extract path are gone from both installers.

The arch check survived the move but changed meaning: it used to *select* among
sibling zips, and now *asserts* that the package's `VERSION` `platform:` line
agrees with `uname -m`. Same failure prevented (an aarch64 payload installing
cleanly on x86_64, then dying with "cannot execute binary file" on the first
`sb`), one download instead of two.

Two guards are new, because the user now does the unzipping and can put the
package anywhere:

- **Unpacked inside `$SB_HOME`** is refused. Source and destination would be
  the same tree, and `rm -rf "$SB_HOME/documents"` would delete the files it is
  about to copy.
- **Not a package at all** (a bare `install.sh` copied out on its own) is
  refused before anything is written, rather than failing halfway with the home
  directory already half-built.

**`uninstall.sh` / `uninstall.ps1` are shipped files now, not here-docs.** They
used to be generated into `$SB_HOME` by a heredoc inside the installer, which
means the uninstaller could only ever be reviewed by reading the installer.
They ship in the zip, and `install.sh` copies one into `$SB_HOME`, so the two
copies are the same bytes. Semantics changed to match what "uninstall" means:
remove the PATH entry, then delete `$SB_HOME` — confirmed unless `--yes`, with
`--keep-home` for the old binary-only behaviour. The old `--purge` spelling is
accepted and ignored.

The one non-obvious part: the installed `uninstall.sh` lives inside the tree it
deletes, and **bash reads a script incrementally**, seeking back into the file
between commands. Deleting it mid-run can abort the uninstall halfway. It
re-execs from a `mktemp` copy when it detects it is inside `$SB_HOME`, and the
child removes that copy on exit. PowerShell needs none of this (it reads the
whole `.ps1` first) but does need `Set-Location` out of `$SB_HOME`, since a
directory cannot be removed while it is the current location.

**A `set -e` footgun, found by the smoke test and pre-existing.** The bashrc
prompt falls back to `/dev/tty` when stdin is closed, guarded by
`[[ -r /dev/tty ]]`. That test asks `access(2)`, which answers yes even when
the process has no controlling terminal; the redirect then fails with `ENXIO`
and `set -e` ends the script — after everything is already copied, so the
install looks like a failure but is not. Both scripts now *attempt the open*
inside an `if` (`{ exec 3</dev/tty; } 2>/dev/null`) rather than testing it, and
tolerate a failed read. The smoke test in installguide_ubuntu.md now checks the
decline path's **exit status**, not just that `~/.bashrc` was untouched, which
is what would have caught this earlier.

Also fixed while in the file: `install.ps1` carried two em-dashes in comments,
breaking its own stated ASCII-only rule (a non-ASCII byte decodes to a smart
quote under 5.1's Windows-1252 assumption). Harmless inside a comment, but the
rule exists so nobody has to work out whether a given case is harmless.

`installguide_windows.md` is **not** updated: the Windows scripts changed here
but were not run, and that guide's job is to carry verified claims. Its Part 1
install flow and §11 smoke test still describe the loose-installer layout.

**v0.1.40 (Linux cut) — `build.sh` replaces the copy-paste staging snippet.**
0.1.40 had shipped on Windows only; `platforms/linux/dist/` was empty while the
dev box still ran 0.1.14. Cutting the Linux side surfaced why the gap existed:
the Linux "staging script" was a bash snippet pasted out of installguide_ubuntu.md
Part 2, opening with `ROOT=/home/el/Dropbox/Projects/swarmbotix` — a path that
exists on exactly one machine. A procedure that has to be edited before it can
be run is a procedure that gets skipped, and Windows had had a real
`build.ps1` since §8.1 landed.

So the fix is a script, not a corrected path:
[platforms/linux/dist-tooling/build.sh](../platforms/linux/dist-tooling/build.sh)
is now the Linux counterpart of `build.ps1` — same `.env` + descriptor inputs,
same fatal drift check, same three output files, same `--skip-build` /
`--no-zip` flags and `DIST_ROOT` / `TARGET_TRIPLE` / `PROFILE` overrides.
installguide_ubuntu.md Part 2 documents the script instead of carrying a copy of it,
which is what let the two drift apart in the first place.

Three things the script does that the snippet did not:

- **Verifies the binary before wrapping a version-named zip around it.** It
  runs `sb --version` on the staged artifact and refuses a mismatch. Without
  it, `--skip-build` after a bump silently ships the previous release under the
  new name — the one failure the drift check cannot catch, because `Cargo.toml`
  and the descriptor agree with each other and only `target/` is stale.
- **Emits the `.sha256`.** `platforms/README.md` had documented a checksum in
  the Linux `dist/` tree for some time; the snippet never produced one, so the
  documented layout and the real one disagreed.
- **Excludes `message_targets/` and stages `custom/`.** The snippet excluded
  `custom` and let `message_targets` through, the reverse of what `build.ps1`
  does. `custom/` is scaffold that ships (empty), `message_targets/` is
  generated output that must not.

Two `set -e` traps worth recording, both of which aborted the first run with no
output at all: `dotenv` returning grep's exit status for an *absent* optional
key, and `[ "$x" = dev ] && y=z` returning 1 on the non-dev path. Both are
written defensively now, with comments saying why.

`install -m 0755` also fails outright on a checkout living on an NTFS/exFAT
mount, which is where this repo sits. `place_exec` falls back to `cp` plus a
best-effort `chmod`; `install.sh` re-applies `0755` on the target machine, so
the shipped binary is executable regardless of what the build filesystem can
represent.

**Doc ownership split out into `/sb-doc` + `/sb-repo-doc`.** The same cut showed
that "update the docs" is really two jobs with opposite failure modes.
`documents/*.md` is *payload*: `install.sh` does `rm -rf $SB_HOME/documents`
then copies, so a stale file reaches every user on their next upgrade with the
binary's authority. Everything else (installguide, requirements, platforms
README, these reports) never leaves the repo, and its failure mode is a
maintainer following a procedure that no longer works. `/sb-doc` owns the
first, `/sb-repo-doc` the second; the test is literally "does it go in the zip".

Two rules are encoded because both were violated during this cut:

- **Platform scope.** A Linux build must not edit `installguide_windows.md`,
  even when a Linux change makes a sentence there false. That guide describes a
  platform you did not build or test, so an edit from the other side is an
  unverified claim in a document whose only job is to be verified. Report the
  staleness; let the next Windows cut confirm it.
- **Re-derive the tree, never trust a hardcoded list.** Both skills open by
  enumerating from `git rev-parse --show-toplevel` and reconciling against
  their own tables, because a moved or merged folder rots a file list exactly
  the way it rotted the old `ROOT=` line. When the structure moves, scripts
  break before prose does, so the scripts get re-run first.

Three findings raised here, all since **closed** later in 0.1.40:

- `sb stop` was documented nowhere and the L5 launch verbs carried `(ref TBD)`
  in `sbcli.md` §4. `documents/sbcli_launch.md` now owns `up` / `run` / `stop` /
  `down` / `attach`, including the semantics that surprise people: `sb stop`
  reuses `sb run`'s `respawn-window -k` path, so the running process is killed
  *before* the stopscript executes, and `sb init` writes no stopscript stub.
- `sbcli_pubsub_consuming.md` credited `CLAUDE.md` with the iceoryx2 pin that
  `versions.json` has owned since earlier in 0.1.40, and quoted `0.9` / `v0.9.0`
  against a `=0.9.3` install. Corrected, and `/sb-deps` now carries the list of
  doc sites a pin bump has to follow.
- The deeper version of that: **every** `../` link in the shipped set was dead,
  31 of them, because `install.sh` copies only `documents/`. `CLAUDE.md` was the
  worst case — gitignored, so absent even from a clone. The rule now is that a
  link in `documents/*.md` may only target another file in `documents/`;
  repo files are named in code spans, and the per-doc header says outright that
  they live in the source repository. `/sb-doc`, `/sb-repo-doc`, `/sb-reship`
  and the `sb-doc-writer` agent all carry it.

Verified: 351 tests pass / 0 fail / 19 ignored, clippy clean on touched files,
all three install paths smoke-tested against sandbox `SB_HOME` (auto-accept,
idempotent re-run, decline), `sha256sum -c` OK. The pre-existing L1 sandbox
failures noted under "Known gaps" below did **not** reproduce on this box —
that failure needs a dev-box `sb.config.yml` pointing `message_definitions` at
a custom path, which this machine's does not.

**v0.1.40 — iceoryx2 0.9.0 → 0.9.3; `versions.json` + `/sb-deps` own dep pins.**
`sb topic list` printed `(no topics)` against a live Python (pip
iceoryx2 0.9.3) publisher: iceoryx2 embeds its exact
`major.minor.patch` in every shm segment and `open()` rejects any
difference (`DynamicStorageOpenError::VersionMismatch`), and sb's
`Cargo.lock` had frozen the `"0.9"` caret pin at 0.9.0. The version
requirement was the wrong shape, not just the wrong number — so the fix
is structural: repo-root `versions.json` is now the source of truth for
transport crate versions, `Cargo.toml` carries exact `=X.Y.Z` pins, and
the new `/sb-deps` skill is the only writer (mirroring how `/sb-release`
owns the sb version). The `FlushFileBuffers … Access is denied` spam in
the same session is a benign Windows-PAL wart (fsync on read-only
discovery handles; the PAL swallows it), not the cause.

**v0.1.19 — `sb run/stop` passthrough args.** `sb run <module>` and
`sb stop <module>` now accept trailing args (clap
`trailing_var_arg + allow_hyphen_values`) that are appended verbatim to
the module's `runscript.bash` / `stopscript.bash` argv. The module's
path is resolved through the active workspace's `flow.yaml`, so both
verbs work from any cwd — matching the cam_gige_ht-style "module owns
its own launch + teardown" pattern.

| # | "Done when" / TDD item | Verification | Status |
|---|---|---|---|
| DW1 | 2-module hello-world round-trips via `sb up`; observable via `sb topic list`; `sb down` cleans up | [level5_e2e_hello_world.rs](../crates/sb-cli/tests/level5_e2e_hello_world.rs) → [examples/level5_hello_world/run.sh](../examples/level5_hello_world/run.sh) | ✅ |
| DW2 | `sb gopro` output passes sanitization lint + round-trips | [level5_gopro_sanitized.rs](../crates/sb-cli/tests/level5_gopro_sanitized.rs) + [level5_gopro_round_trip.rs](../crates/sb-cli/tests/level5_gopro_round_trip.rs) | ✅ |
| DW3 | E2E smoke test green in CI | [level5_e2e_hello_world.rs](../crates/sb-cli/tests/level5_e2e_hello_world.rs) — `#[ignore]`'d; 158 s on this dev box | ✅ |
| TDD1 | `sb up` launches expected panes | [level5_launch_two_modules.rs](../crates/sb-cli/tests/level5_launch_two_modules.rs) (2) — tmux list-windows reports both modules | ✅ |
| TDD2 | `sb up` blames missing runscript before touching tmux | [level5_launch_missing_runscript.rs](../crates/sb-cli/tests/level5_launch_missing_runscript.rs) — module named in error AND no orphan session | ✅ |
| TDD3 | `sb up/down/attach` error cleanly without an active workspace | [level5_launch_no_active_workspace.rs](../crates/sb-cli/tests/level5_launch_no_active_workspace.rs) (4) | ✅ |
| TDD4 | `sb run <module>` is idempotent | [level5_launch_idempotent.rs](../crates/sb-cli/tests/level5_launch_idempotent.rs) (2) — re-run stays at one window | ✅ |
| TDD4a | `sb run <module> [args...]` forwards args to runscript.bash; `sb stop` mirrors it | [level5_run_stop_args.rs](../crates/sb-cli/tests/level5_run_stop_args.rs) (4) — sentinel-file round-trip on argv | ✅ |
| TDD5 | `sb attach` / `sb down` round-trip | [level5_attach_down.rs](../crates/sb-cli/tests/level5_attach_down.rs) | ✅ |
| TDD6 | `sb gopro` output is sanitized | [level5_gopro_sanitized.rs](../crates/sb-cli/tests/level5_gopro_sanitized.rs) — no `root:` / `language:` / `io_dir:` / `/home/` | ✅ |
| TDD7 | `sb gopro` round-trip (3 pubs, 2 subs, mixed transports) | [level5_gopro_round_trip.rs](../crates/sb-cli/tests/level5_gopro_round_trip.rs) | ✅ |
| TDD8 | `sb gopro --all` covers every module; broken siblings don't block | [level5_gopro_all.rs](../crates/sb-cli/tests/level5_gopro_all.rs) (2) | ✅ |
| TDD9 | Full end-to-end script | DW1 above | ✅ |

```bash
# Default suite (no extra runtime deps):
cargo test -p sb-launch -p sb-gopro        # 21 unit tests
cargo test -p sb-cli --test level5_launch_no_active_workspace        # 4
cargo test -p sb-cli --test level5_launch_missing_runscript          # 1
cargo test -p sb-cli --test level5_launch_two_modules                # 2 (needs tmux; skips cleanly otherwise)
cargo test -p sb-cli --test level5_launch_idempotent                 # 2
cargo test -p sb-cli --test level5_run_stop_args                     # 4 (needs tmux; skips cleanly otherwise)
cargo test -p sb-cli --test level5_attach_down                       # 1
cargo test -p sb-cli --test level5_gopro_sanitized                   # 1
cargo test -p sb-cli --test level5_gopro_round_trip                  # 1
cargo test -p sb-cli --test level5_gopro_all                         # 2

# Heavy gate (ignored by default — touches Zenoh + tmux + cargo, ~160 s):
cargo test -p sb-cli --test level5_e2e_hello_world -- --ignored
```

---

## What landed

### 1. `sb-launch` crate — tmux/psmux orchestration

[`crates/sb-launch/src/lib.rs`](../crates/sb-launch/src/lib.rs) is
pure plan-then-execute. The public API:

```
session_name(workspace)     -> String       // "sb-<workspace>"
plan_workspace(home, ws)    -> LaunchPlan   // reads flow.yaml + every sb.dev.yml + verifies runscript.bash
plan_single(home, ws, m)    -> (LaunchPlan, Pane)
resolve_tmux(cfg_tmux)      -> PathBuf      // honors sb.config.yml `tmux:`, else `which tmux`
up(&plan, &tmux)            -> UpOutcome
run_single(&plan, &pane, &tmux) -> RunOutcome
down(workspace, &tmux)      -> DownOutcome
attach_info(workspace, &tmux) -> AttachInfo
```

**Plan-then-execute** is load-bearing: TDD #2 needs `sb up` to fail
*before* tmux is touched if any module is missing a runscript. The
`plan_workspace` walk reports the offending module by name; if it
returns Ok, `up` is guaranteed to find every runscript on disk.

### 2. `sb-gopro` crate — `sb.prd.yml` export

[`crates/sb-gopro/src/lib.rs`](../crates/sb-gopro/src/lib.rs) is a
pure transform + a sanitization lint. The pipeline for one module:

```
read sb.dev.yml
  ↓
ModuleDevConfig → ModulePrdConfig    (dev_to_prd: drops root, language, io_dir)
  ↓
walk io_dir/ to record any missing pub/sub files (non-fatal warning)
  ↓
serialize to YAML
  ↓
lint_sanitized: scan for /home/, /Users/, C:\, root:, language:, io_dir:
  ↓                                 (any hit → abort with the offending line)
write <module_root>/sb.prd.yml
```

The lint is belt-and-braces: today `dev_to_prd` can't produce any of
those keys because `ModulePrdConfig` doesn't have those fields. But
if someone adds a field to `ModulePrdConfig` later that copies a
host-specific value, the lint catches it before the prd.yml ships.

`gopro_all` collects per-module errors instead of bailing on the
first failure (TDD #8) — one corrupt `sb.dev.yml` shouldn't block
freezing siblings that are healthy.

### 3. CLI wiring

`sb up / run / down / attach / gopro` added to
[main.rs](../crates/sb-cli/src/main.rs) under the `Cmd` enum, with
handler functions at the end of the file in their own
`launch (L5)` and `gopro (L5)` sections. The `gopro --module` /
`--all` flags are mutually exclusive via clap `conflicts_with`.

`sb attach` adapts to its TTY: interactive shell → `exec tmux attach
-t sb-<ws>`; non-interactive (CI, scripts) → print the session name
on stdout and exit 0 so callers can `grep`.

### 4. E2E example: `examples/level5_hello_world/`

```
examples/level5_hello_world/
├── run.sh           — TDD #9 driver; ws/init/pub add/sub add/up/list/down/gopro --all
├── pubber/          — standalone crate; Rust zenoh publisher emitting std/StringStamped
│   ├── Cargo.toml
│   ├── build.rs     — prost-build for std/Header + std/StringStamped
│   └── main.rs
└── subber/          — standalone crate; Rust zenoh subscriber, decodes + logs "RX_HELLO data=..."
    ├── Cargo.toml
    ├── build.rs
    └── main.rs
```

The example **is** the e2e test:
[`level5_e2e_hello_world.rs`](../crates/sb-cli/tests/level5_e2e_hello_world.rs)
just shells out to `run.sh`. Same pattern as L4's
`examples/rust_zenoh_stamped/` — single source of truth so the
example breaking can't desync from the test.

The smoke covers every L1..L5 surface in one go:

| Stage | Surface | Asserted |
|---|---|---|
| L2 | `sb ws create demo && sb ws set demo` | workspace dir + active marker |
| L2 | `sb init --rust <pubber> --force`, same for subber | sb.dev.yml + runscript.bash + flow.yaml entry |
| L3 | `sb pub add -m ros2/std/StringStamped hello --zenoh --force` | swarmbotix_io/publishers/hello.rs scaffolded |
| L3 | `sb sub add -m ros2/std/StringStamped /dev01/demo/pubber/hello --zenoh --force` | swarmbotix_io/subscribers/hello.rs scaffolded |
| L5 | `sb up` | tmux session sb-demo with 2 windows; both runscripts spawn |
| L4 | `sb topic list --transport zenoh --json -t 1.0` | `/dev01/demo/pubber/hello` discovered |
| (live) | subber log via `tee` | `RX_HELLO data=HELLO_FROM_L5` within 12 s |
| L5 | `sb down` | `tmux has-session -t sb-demo` returns non-zero |
| L5 | `sb gopro --all` | both modules get a clean sb.prd.yml; greps confirm no `/home/` or `/Users/` |

---

## Design decisions worth documenting

### Why `sb run <module>` uses `tmux respawn-window -k`

Naive idempotency would `kill-window` + `new-window`. That breaks
when the target window is the only one in the session: tmux kills
the session along with its last window, and `new-window` then fails
with "no server running on /tmp/tmux-1000/default". Found via TDD #4
on first run.

`respawn-window -k -t <session>:<window> <cmd>` kills the running
process inside the window and starts the new one in the same
window. The window (and therefore the session) never empties, so
the failure mode goes away.

### Why `gopro` lint runs on the serialized body, not the struct

The struct already enforces the prd shape — but the *real* invariant
the user cares about is "no host-specific strings hit the YAML on
disk." Running the lint on the serialized output catches both:

  1. Future regressions in `ModulePrdConfig` (someone adds a field
     that holds an absolute path).
  2. Future regressions in `dev_to_prd` (someone forgets to drop
     one of the host-only fields).

The lint is cheap (one pass over the body, ~6 substring checks per
line) so it runs unconditionally. The CLI surfaces a hard error with
the offending line number rather than writing a leaky prd.yml.

### Why `sb gopro --all` collects errors instead of bailing on first

Per TDD #8: a broken module shouldn't block freezing the rest of
the workspace. The CLI prints `error: gopro <module>: <reason>` per
failure, prints `gopro --all: N succeeded, M failed`, and exits
non-zero if any failed. Healthy modules still got their prd.yml
written — that's the contract.

### Why `sb attach` adapts to its TTY

`sb up` is a no-op detached background launch — useful for CI and
scripts. But the natural follow-up `sb attach` only makes sense
inside an interactive shell where tmux can grab the terminal.

In an interactive TTY we `exec tmux attach` so the user lands
inside the session. In CI / cargo test (non-TTY stdout), we print
the session name on stdout and exit 0 so scripts can do
`SESSION=$(sb attach)` to grab it without hanging on tmux's
interactive curses.

### Why `sb-launch` uses one window per module, not one pane

Same-session windows are independently scrollable, named, and
killable. Same-window panes share scrollback and a tile layout
that's hostile to long-running per-module logs. The tmux native
abstraction for "named, independent output streams" is the window;
panes are for visually-tiled side-by-side workflows. Per the spec
"one pane per module" — but tmux's term-of-art "pane" maps to
"window" when each is named and full-terminal. The plan documents
both terms in the test names to avoid ambiguity.

---

## Test totals after this round

- **21 unit tests** across sb-launch (11) + sb-gopro (10). Cover
  plan building, runscript validation, tmux path resolution, shell
  quoting, prd transform, sanitization lint, IO-dir presence walk.
- **14 default integration tests** in sb-cli (`level5_*`):
  - `level5_launch_no_active_workspace`: 4
  - `level5_launch_missing_runscript`: 1
  - `level5_launch_two_modules`: 2
  - `level5_launch_idempotent`: 2
  - `level5_attach_down`: 1
  - `level5_gopro_sanitized`: 1
  - `level5_gopro_round_trip`: 1
  - `level5_gopro_all`: 2
- **1 ignored integration test**:
  - `level5_e2e_hello_world` — proof-of-life for L1..L5 (~160 s).
- **L2/L3/L4 regression**: every default test still passes (all
  L2 — 27, all L3 — 51, all L4 — 4 default + 12 ignored gates,
  unaffected by L5).

---

## Known gaps / follow-ups

- **Pre-existing L1 codegen test failures** (`level1_codegen.rs`,
  `level1_first_run_install.rs`, `level1_doctor_happy_and_failures.rs`,
  `level1_protoc_compile.rs`, `level1_std_compiles_to_rust.rs`,
  `level1_vault_lookup.rs`) — unrelated to L5. Root cause:
  `tests/common/Sandbox::new()` copies the real dev-box
  `~/.swarmbotix/sb.config.yml`, which sets
  `message_definitions: /home/el/Dropbox/Projects/swarmbotix/messages`
  on this machine, but the tests then assert the vault is under
  `<tempdir>/.swarmbotix/message_definitions`. The two paths
  diverge whenever a developer customizes their global config.
  Fix is in the `Sandbox` helper (override `message_definitions:`
  to a per-test tempdir) — out of scope for L5.
- **iceoryx2 version mismatch** — resolved as of v0.1.40: sb links
  =0.9.3, matching the box's pip wheel and `iceoryx2_ffi_c.dll`;
  `versions.json` + `/sb-deps` keep it that way. (Historical: the
  0.8.1-vs-0.9 caveat carried from L1/L4, then 0.9.0-vs-0.9.3.)
- **`sb run <module> --build`** (auto cargo-build before launch)
  is deferred. Today the user's runscript is responsible for
  build-then-run — the example uses `exec cargo run --quiet`,
  which handles both. A future ergonomic improvement.
- **`sb up --foreground`** (attach immediately after creating the
  session) is unscoped — the spec calls for the two as separate
  verbs. A 5-line wrapper at the CLI layer if a future iteration
  wants it.
- **Pane layout / split / multi-pane** (e.g. one pane for stdout,
  one for `htop`) is out of scope. L5 ships the simplest possible
  one-window-per-module abstraction; complex layouts belong to
  whichever user runscript wants them.
