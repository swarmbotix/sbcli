# `sb init` — Module Adoption Reference

Official reference for `sb init`: how a user's existing project (a
cargo crate, a Python project, a C++ tree, a Flutter app, a Unity
game) becomes a swarmbotix module that the rest of the toolchain
(`sb pub/sub *`, `sb up`, swarmctl) can manage.

This document is normative for behavior shipped at L2.

The canonical spec is `requirements.md` in the sbcli **source
repository** — a maintainer document that is not part of your install;
when it and this page disagree, it wins.

---

## 1. Conceptual model

`sb init` **adopts** an existing project directory. It is not a project
scaffolder — sb does not run `cargo new`, `python -m venv`, or
`flutter create`. The user already has a project that builds and runs
on its own. `sb init` just drops three sb-managed artifacts into the
project root and registers the module with the active workspace's
`flow.yaml` (see [sbcli_ws.md](sbcli_ws.md) §4).

After `sb init`, the project's build system, dependency manager, and
internal layout are unchanged. The only structural imposition is the
**IO directory** (`swarmbotix_io/` by default; `Assets/Scripts/swarmbotix_io/`
for Unity) which `sb pub/sub *` later populates with generated
publisher / subscriber files.

### 1.1 Three artifacts written into `<rootpath>`

| Artifact | Owner | What it holds |
|---|---|---|
| `sb.dev.yml` | sb-managed; user-readable, host-specific. **Gitignore.** | Module name, language, absolute root path, IO dir, publishers list, subscribers list. See §4. |
| `runscript.bash` | user-edited stub | An executable launcher; sb stubs a TODO-print body that exits non-zero. The user fills it in. `sb up` / `sb run` invoke this. See §5. |
| `swarmbotix_io/` | sb-managed | IO directory; empty after init. Populated by `sb pub add` / `sb sub add` at L3. See [sbcli_pubsub.md](sbcli_pubsub.md) §5. Unity puts this at `Assets/Scripts/swarmbotix_io/` instead. |

`sb init` never touches anything else in the project tree. User
source files, `Cargo.toml`, `pyproject.toml`, `pubspec.yaml`,
`Assets/`, etc. are all left alone (see §6 for the test matrix).

### 1.2 One side effect outside `<rootpath>`

A fourth thing happens, but outside the project: the module is
appended to the active workspace's `flow.yaml`:

```yaml
# <sb_home>/workspaces/<active>/flow.yaml
modules:
  <module_name>: <abs_path_to_rootpath>/sb.dev.yml
```

`<module_name>` is derived from `basename(rootpath)` — see §3.

---

## 2. Command surface

```bash
sb init [<rootpath>] [LANGUAGE_FLAG] [--force]
```

| Argument | Required? | Default | Effect |
|---|---|---|---|
| `<rootpath>` | optional | `.` (cwd) | Project root to adopt. Must already exist and be a directory. Tilde + relative paths accepted; sb canonicalizes via `Path::canonicalize`. |
| `--rust` / `--python` / `--cpp` / `--flutter` / `--unity` | required IF no `sb.dev.yml` exists at `<rootpath>` | (none) | Language tag written to `sb.dev.yml`. **Exactly one** allowed; clap enforces mutual exclusion via the `lang` group. |
| `--force` | optional | refuse-overwrite | Overwrite an existing `sb.dev.yml` / `runscript.bash`. Has no effect on `swarmbotix_io/` (always idempotent — created if missing, left alone otherwise). |

The CLI exit code is **zero** on full success, **non-zero** on any
validation or filesystem error. Either every step succeeds or none of
the state-changing steps run (see §8 for atomicity caveats).

---

## 3. Module-name derivation + validation

`<module_name>` is **always** `basename(canonical(rootpath))`. There
is no `--name` flag. If you need a different module name, rename or
symlink the directory before running `sb init`.

The basename must satisfy `sb_core::validate_identifier`:

- Non-empty.
- First character: ASCII letter or `_`.
- Remaining characters: ASCII letters, digits, or `_`.

Same rule as workspace names ([sbcli_ws.md](sbcli_ws.md) §1.2).
Common adopting-friction names that get rejected:

| basename | Reason |
|---|---|
| `my-app` | `-` not allowed |
| `123-app` | starts with digit AND contains `-` |
| `app.v2` | `.` not allowed |
| `My App` | space not allowed |
| `` (empty) | reserved |

Workaround for the typical `dash-case-cargo-crate-name` problem: keep
the cargo `name = "my-app"` in `Cargo.toml` (cargo accepts it), but
rename the **directory** to `my_app` so the basename validates. The
sb module name and the cargo package name are independent.

---

## 4. `sb.dev.yml` schema

```yaml
module: pubber
language: rust
root: /home/el/projects/pubber
io_dir: swarmbotix_io
publishers:
  - name: hello
    topic: hello
    type: std/StringStamped
    transport: zenoh
subscribers: []
```

All fields are required unless noted. `deny_unknown_fields` is on, so
typos are rejected at parse time.

| Field | Type | Set by | Notes |
|---|---|---|---|
| `module` | string | `sb init` (from basename) | Must satisfy `validate_identifier`. Never changes after init. |
| `language` | enum (`rust` / `python` / `cpp` / `flutter` / `unity`) | `sb init` (from flag) | Drives codegen template selection at L3. Cannot be re-tagged without `--force` (see §7). |
| `root` | absolute path | `sb init` | Canonical absolute path. **Host-specific** — gitignore this file. |
| `io_dir` | relative path | `sb init` (language-dependent default) | `swarmbotix_io` for Rust / Python / C++ / Flutter; `Assets/Scripts/swarmbotix_io` for Unity. User-editable post-init (e.g. to nest under `src/generated/`). |
| `publishers` | list of `PubSpec` | `sb pub add/edit/rm` | Empty after `sb init`. See [sbcli_pubsub.md](sbcli_pubsub.md) §4. |
| `subscribers` | list of `SubSpec` | `sb sub add/edit/rm` | Same. |

Serialization quirk: `io_dir` writes back without a trailing slash
even though requirements.md prose shows `swarmbotix_io/`. That's
`PathBuf` normalization at the YAML boundary — functionally identical
because every consumer `Path::join`s onto it.

---

## 5. `runscript.bash` stub

Written executable (mode `0755` on Unix; no-op on Windows). The body
is identical across languages — only the **comment block** at the top
changes to show per-language launch examples. The body itself just
echoes a TODO and exits non-zero so a user who forgets to fill it in
sees a loud failure when `sb up` invokes it.

Skeleton (`crates/sb-workspace/src/runscript.rs`):

```bash
#!/usr/bin/env bash
# runscript.bash — sb-managed launch stub.
#
# `sb up` and `sb run <module>` invoke this script to start the
# module. Replace the body below with the real launch command for
# your project. Examples per language:
#
#   Rust:    cargo run --release
#   Python:  python main.py
#   C++:    ./build/my_app
#   Flutter: flutter run -d linux
#   Unity:  ./Builds/Linux/my_game.x86_64
set -euo pipefail

echo "TODO: edit runscript.bash to launch your module" >&2
exit 1
```

The L2 test `crates/sb-cli/tests` golden-snippets
the comment block + asserts the executable bit; if you change the
stub, that test breaks unless you re-bless.

`sb init` writes this file only if it doesn't already exist or if
`--force` is passed. Hand-edits to a user-filled `runscript.bash`
survive every subsequent `sb init` (even `--force` only overwrites if
you ask).

**No `stopscript.bash` stub is written.** `sb stop <module>` runs a
`stopscript.bash` at the module root, but `sb init` never creates one —
there is no generic teardown worth stubbing, and `sb stop` errors
loudly rather than run a placeholder. Author it yourself when you need
one; see [sbcli_launch.md](sbcli_launch.md) §4.

---

## 6. Behavior matrix

`sb init --<lang> <rootpath>` against five starting conditions:

| Starting state | sb.dev.yml | runscript.bash | swarmbotix_io/ | flow.yaml | Notes |
|---|---|---|---|---|---|
| Empty dir | written | written (exec) | created | appended | The default new-project case. |
| Existing `sb.dev.yml`, no flag | (preserved) | (preserved) | (preserved) | appended (idempotent) | Re-init. No artifacts written; just registers in the active workspace. Used by tests that want a deterministic re-init. |
| Existing `sb.dev.yml` + same `--<lang>` | (preserved) | (preserved) | (preserved) | appended (idempotent) | Same as above. The flag is checked against the file's `language:` tag; mismatch errors. |
| Existing `sb.dev.yml` + different `--<lang>` | **error** | (preserved) | (preserved) | (not appended) | `"language tag mismatch: <path> has language: <a>, but --<b> was passed"`. Use `--force` to overwrite. |
| Existing `sb.dev.yml`, `--force`, optional `--<lang>` | overwritten | overwritten | (preserved if exists; created if not) | appended | `--force` is destructive on the two sb files. Use sparingly. |
| Project files present (e.g. `Cargo.toml`, `main.py`) but no sb files | written | written | created | appended | User files untouched. The test `crates/sb-cli/tests` covers all five languages. |

Idempotency invariants:

- Re-running `sb init <same_rootpath>` with no `--force` and a matching
  `--<lang>` flag is a no-op for the three project artifacts and a
  no-op-or-overwrite for the `flow.yaml` entry (the value is rewritten
  only if the path differs).
- The `flow.yaml` append uses `BTreeMap::insert`, so duplicate keys
  collapse to one entry — there is no scenario where `flow.yaml`
  accumulates multiple lines for the same module name.

---

## 7. Subcommand semantics — `sb init [<rootpath>] [OPTIONS]`

**Effects (in order)**:

1. Loads the merged `SbCliConfig` (env / workspace / global / defaults).
2. Resolves `<sb_home>` (see [sbcli_ws.md](sbcli_ws.md) §2).
3. Reads `<sb_home>/active` — errors if missing/empty with:
   ```
   no active workspace — run `sb ws create <name> && sb ws set <name>` first
   ```
4. Resolves `<rootpath>` to its canonical absolute form via
   `Path::canonicalize`. Errors if the path doesn't exist or isn't a
   directory.
5. Derives `<module_name> = basename(canonical_rootpath)`, validates
   it against the identifier rule (§3). Errors with
   `"invalid module name <name> (from rootpath basename): <reason>"`.
6. Reads `<rootpath>/sb.dev.yml` if present.
7. Resolves the **language** by reconciling the file's existing
   `language:` tag (if any) and the `--<lang>` flag (if any):
   - File present + flag matches: use the matched language.
   - File present + flag differs: error (see §6 table).
   - File present + no flag: use the file's language.
   - File absent + flag present: use the flag.
   - File absent + no flag: error
     `"no sb.dev.yml at <path> and no --rust|--python|--cpp|--flutter|--unity flag"`.
8. Resolves `io_dir`: from the existing file if present, otherwise
   `Language::default_io_dir()`.
9. Writes `sb.dev.yml` if (a) it didn't exist OR (b) `--force` was
   passed. The body is regenerated from the resolved fields; existing
   `publishers` / `subscribers` lists are **preserved** when reading
   the prior file and re-emitted on overwrite. Hand-edited fields
   outside the schema would be dropped (but `deny_unknown_fields`
   would have rejected them on read anyway).
10. Writes `runscript.bash` if missing or `--force`. Sets mode `0755`
    on Unix.
11. Creates `<rootpath>/<io_dir>/` if missing. Never deletes or
    rewrites an existing IO dir.
12. Reads the active workspace's `flow.yaml`, inserts
    `<module_name> → <rootpath>/sb.dev.yml`, writes back. The write is
    skipped if the entry already exists with that exact path.

The CLI's stdout reflects which artifacts changed:

```
adopted module pubber (language: rust, root: /home/el/projects/pubber)
  wrote sb.dev.yml
  wrote runscript.bash (executable)
  created IO directory
  registered in flow.yaml
```

Re-running with no changes:

```
adopted module pubber (language: rust, root: /home/el/projects/pubber)
  (already registered in flow.yaml)
```

---

## 8. Atomicity and failure modes

`sb init` is **not transactional**. The steps in §7 run sequentially
and write to disk one at a time. If step N fails, steps 1..N-1 have
already left their marks on the filesystem.

Common partial-failure scenarios:

| What fails | What's left on disk |
|---|---|
| Identifier validation (step 5) | Nothing — fails before any write. |
| `sb.dev.yml` write (step 9) | The file may exist as a 0-byte or partial-write. |
| `runscript.bash` write (step 10) | `sb.dev.yml` already written; runscript missing. Re-run `sb init --force` to write the runscript. |
| `io_dir` mkdir (step 11) | sb.dev.yml + runscript already written; IO dir missing. Re-run; step 11 is idempotent. |
| `flow.yaml` write (step 12) | Module fully adopted in the project tree; not registered in the workspace. Re-run `sb init <same_path>` — it'll skip the project artifacts (they exist) and finish the registration. |

The most common cause of step-12 failure is a stale active marker
(workspace deleted out from under the CLI; see [sbcli_ws.md](sbcli_ws.md) §9.1).

---

## 9. Examples

### 9.1 Adopt a cargo crate

```bash
$ sb ws create demo && sb ws set demo
$ cd /path/to/my_rust_crate
$ sb init --rust
adopted module my_rust_crate (language: rust, root: /path/to/my_rust_crate)
  wrote sb.dev.yml
  wrote runscript.bash (executable)
  created IO directory
  registered in flow.yaml
```

Now `/path/to/my_rust_crate/` has:

```
my_rust_crate/
├─ Cargo.toml             # untouched
├─ src/                   # untouched
├─ sb.dev.yml             # NEW (add to .gitignore — has absolute path)
├─ runscript.bash         # NEW (edit me before `sb up`)
└─ swarmbotix_io/         # NEW (empty until `sb pub add`)
```

### 9.2 Adopt a Unity project

```bash
$ cd /path/to/MyGame                # contains Assets/, ProjectSettings/
$ sb init --unity
adopted module MyGame (language: unity, root: /path/to/MyGame)
  wrote sb.dev.yml
  wrote runscript.bash (executable)
  created IO directory
  registered in flow.yaml
```

Unity's IO dir is special-cased:

```
MyGame/
├─ Assets/
│  ├─ Scenes/
│  └─ Scripts/
│     └─ swarmbotix_io/   # NEW (under Assets/Scripts/, not at project root)
├─ ProjectSettings/
├─ sb.dev.yml             # io_dir: Assets/Scripts/swarmbotix_io
└─ runscript.bash
```

The reason: Unity only compiles C# scripts located under
`Assets/`. Putting `swarmbotix_io/` at the project root would mean
Unity ignores everything `sb pub/sub add` emits.

### 9.3 Re-init to add a forgotten artifact

You manually deleted `runscript.bash` during a cleanup. Get it back:

```bash
$ cd /path/to/my_rust_crate
$ sb init --rust
adopted module my_rust_crate (language: rust, root: /path/to/my_rust_crate)
  wrote runscript.bash (executable)
  (already registered in flow.yaml)
```

`sb.dev.yml` and `swarmbotix_io/` were already present, so they're
left alone. Only the missing artifact is rewritten.

### 9.4 Change a module's language

```bash
$ sb init --python /path/to/my_rust_crate --force
# Overwrites the existing sb.dev.yml's `language: rust` with `language: python`.
# Runscript is also rewritten (its comment block changes per language).
```

⚠ This destroys any `publishers:` / `subscribers:` entries by
overwriting `sb.dev.yml`. The corresponding generated files under
`swarmbotix_io/` are **not** deleted — but they're now orphaned
(reference a `.rs` template the new language doesn't want). Re-add
them with `sb pub/sub add --force` after the language flip.

### 9.5 Adopt under a renamed dir to satisfy the identifier rule

```bash
$ # Original: /path/to/my-app/  (dash → invalid module name)
$ mv /path/to/my-app /path/to/my_app
$ sb init --rust /path/to/my_app
adopted module my_app (...)
```

The cargo crate name in `Cargo.toml` stays `my-app`; only the
directory name changes. The two are independent.

---

## 10. Error catalogue

| Code path | Message |
|---|---|
| no active workspace | `no active workspace — run \`sb ws create <name> && sb ws set <name>\` first` |
| missing rootpath | `rootpath <path> does not exist` |
| not a directory | `rootpath <path> is not a directory` |
| bad basename | `rootpath <path> has no usable basename` |
| identifier | `invalid module name "<n>" (from rootpath basename): <reason>` |
| language mismatch | `language tag mismatch: <path> has language: <a>, but --<b> was passed` |
| no flag, no file | `no sb.dev.yml at <path> and no --rust\|--python\|--cpp\|--flutter\|--unity flag` |
| multiple flags | `pick at most one of --rust / --python / --cpp / --flutter / --unity` (clap-level) |
| existing sb.dev.yml + no `--force` (rare — happens only when the existing file is partially parseable but conflicts) | passed through from `serde_yaml::from_str` |

Stable wordings — integration tests in `crates/sb-cli/tests`
depend on substrings of these messages.

---

## 11. Edge cases and gotchas

### 11.1 Symlinks in `<rootpath>`

`Path::canonicalize` resolves the entire path. If `<rootpath>` is a
symlink (e.g. `~/projects` is a symlink to `~/Dropbox/projects`), the
canonical form goes through and the **module name** comes from the
target's basename, not the symlink's. Usually equivalent, but worth
knowing if you have a symlink whose name differs from its target.

### 11.2 `<rootpath>` directly under `/`

`basename("/")` is the empty string and gets rejected by step 5 of
§7. There's no realistic reason to `sb init /` but the error message
(`"<path> has no usable basename"`) is the diagnostic if you somehow
end up there (e.g. via a broken script).

### 11.3 Network filesystems

NFS / SMB are supported but the `sb.dev.yml`'s `root:` field will
record the mount path. On another host that mounts the same volume at
a different point, `flow.yaml` entries become invalid. Either keep
the absolute path consistent across hosts or re-`sb init` on each.

### 11.4 What if the user gitignores `sb.dev.yml` but commits the IO dir?

That's the recommended pattern, actually. `sb.dev.yml` is
host-specific (absolute path in `root:`), so it should be
gitignored. The IO dir's contents are deterministic codegen output
(byte-stable per the L3 golden tests) and can be committed if the
project wants to vendor them — or gitignored and regenerated at build
time. Either works; sb has no opinion.

### 11.5 Re-running `sb init` after moving the project

```bash
$ mv /old/path/my_app /new/path/my_app
$ cd /new/path/my_app
$ sb init --rust       # re-registers with the new abs path; rewrites sb.dev.yml's `root:`
```

The previous `flow.yaml` entry (pointing at `/old/path/my_app/sb.dev.yml`)
gets overwritten with the new path because the module name is the
same. The old `sb.dev.yml` (if you kept it on disk anywhere) becomes
stale; nothing references it anymore.

### 11.6 Two projects with the same basename

```bash
$ sb init --rust /path/A/camera           # registers `camera → /path/A/camera/sb.dev.yml`
$ sb init --rust /path/B/camera           # **silently overwrites** that entry
```

`flow.yaml` is a `Map<String, PathBuf>` — same key, second insert
wins. Rename one of the directories to disambiguate.

### 11.7 Windows + `runscript.bash`

Windows hosts get the file with no executable bit (no-op on Windows).
Future L5 work will need a `runscript.cmd` or a `bash` (e.g. via WSL)
indirection. For now, Linux/macOS are the supported targets;
`set_executable` is `#[cfg(unix)]` only.

### 11.8 IO dir under a build-system "out of tree" build dir

If your build system insists on putting outputs in a parallel
directory (e.g. CMake's `build/`), set `io_dir: ../build/swarmbotix_io`
in `sb.dev.yml` after the initial `sb init`. sb resolves
`<rootpath>/<io_dir>` for codegen and `Path::join` collapses the `..`
naturally. The build system can then `add_subdirectory` it without
clashes.
