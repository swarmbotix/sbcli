# `sb doctor` — Environment Validation Reference

Official reference for `sb doctor`: what it checks, what it considers
"OK" vs "FAIL" vs "SKIP", and how to read its output when something
about your host environment is wrong.

This document is normative for behavior shipped at L1.

The canonical spec is `requirements.md` in the sbcli **source
repository** — a maintainer document that is not part of your install;
when it and this page disagree, it wins.

---

## 1. Conceptual model

`sb doctor` is a **read-only diagnostic**. It loads the merged
`sb.config.yml` (see [sbcli_config.md](sbcli_config.md) §3) and walks
a hard-coded checklist:

1. **Executables** — each declared tool path must exist on disk and
   answer to its version flag.
2. **Transports** — the zenoh / iceoryx2 crate versions this `sb` was
   built against, reported so the libraries below have something to be
   compared with.
3. **Shared libraries** — each declared `.so` must exist and
   `dlopen` cleanly (the actual symbols aren't called), and its version
   is read off the install and checked against the linked crate.
4. **Message vault** — `<message_definitions>/std/` must contain the
   forge-bundled `.proto` files; offers to re-install on miss.

No state is written. No network calls. No build / compile happens.
Doctor is safe to run at any time, including during ongoing pub/sub
sessions.

The output is a fixed-format checklist; each line is one of:

```
[OK]   <name>       <details>
[FAIL] <name>       <reason>
[SKIP] <name>       <why skipped>
```

A FAIL line never aborts the run — doctor keeps going to surface
every problem in one pass (per L1 TDD test #10). The process exit
code is **0** only if every check is `[OK]` or `[SKIP]`; **non-zero**
on any `[FAIL]`. The non-zero exit prints a summary line:

```
N check(s) failed
```

---

## 2. The checklist

Order is stable; the L1 doctor implementation iterates a hard-coded
list. Each entry corresponds to one dependency.

| # | Check name | What's verified | What configures it |
|---|---|---|---|
| 1 | `protoc` | Executable exists; `<path> --version` exits 0 and prints a version. | `protoc:` in sb.config.yml |
| 2 | `flatc` | Same. | `flatc:` |
| 3 | `transports` | States the zenoh / iceoryx2 crate versions compiled into this `sb`. Reports a property of the binary, so it is **always `[OK]`** and reads nothing from the config. | nothing — fixed at build time |
| 4 | `libzenohc` | File exists at path; `libloading::Library::new(<path>)` succeeds (dlopen); the install's own version is read and compared with the linked zenoh crate (major.minor). | `libzenohc:` |
| 5 | `libiceoryx2` | Same shape as libzenohc, but compared on **every digit** — iceoryx2 opens shared-memory segments on exact version equality. | `libiceoryx2:` |
| 6 | `tmux` | Executable exists; `<path> -V` exits 0 (note: tmux uses `-V`, not `--version`). | `tmux:` |
| 7 | `message styles` | Lists **every** style under the messages root — one row each with its message count and whether its bindings are built. No style is marked "active": there is no such thing. Fails only when the root holds no styles at all. | `messages_root:` |
| 8 | `unity targets` | Scans each style's `message_targets/` that sits inside a Unity project and counts generated `.cs` files a current `sb` would not have written. **Warns, never fails** — the breakage is in the host project, not in `sb`. | `messages_root:` |
| 9 | `config keys` | Reports `message_style` / `message_definitions` / `message_targets` if still present. They parse but do nothing, which is the combination that silently misleads. | any stale `sb.config.yml` |
| 10 | `std vault` | Walks `<messages_root>/ros2/message_definitions/std/` and confirms the expected forge files are present. Always the ros2 style — that is where `std/` lives. | `messages_root:` |

Checks 1, 2, 4, 5 and 6 share the same structure (`CheckResult` from
`sb_doctor`). Checks 7, 8 and 9 have their own logic because they don't
run a subprocess and aren't a `dlopen` — they inspect the on-disk style
tree, the generated C# in it, and the config as parsed. Check 3 touches
neither the config nor the disk: its two numbers are baked into the
binary when it is built (§4.9).

**Check 10 always looks at ros2.** `std/` ships there and the embedded
bundle installs there, so the check is not affected by what anyone happens
to be working on — nothing is "currently selected" any more.

### 2.1 The "not configured" path

Each of the path-reading checks — `protoc`, `flatc`, `libzenohc`,
`libiceoryx2`, `tmux` — takes its path from the merged config. If the
field is unset (`None` after merging), the check is **`[SKIP]`**, not
`[FAIL]`:

```
[SKIP] flatc        not set in sb.config.yml
```

Rationale: not every host needs every tool. A pure-Rust user with no
FlatBuffers consumers can leave `flatc:` unset and doctor stays
green. The downstream `sb message compile --fb` would still error
loudly when actually invoked (see [sbcli_messages.md](sbcli_messages.md)
§9.3).

The exit code treats SKIP as success — a doctor run with three
configured tools and two skipped exits 0. To make a missing tool a
hard error, set the field to a non-existent path:

```yaml
flatc: /not/installed
```

Then doctor reports `[FAIL] flatc /not/installed (file not found)`
and the run exits non-zero.

---

## 3. Output format

```
$ sb doctor
sb <version>
[OK]   protoc       /opt/protobuffer/protoc-35.0-linux-x86_64/bin/protoc (libprotoc 35.0)
[OK]   flatc        /opt/flatbuffers/Linux.flatc.binary.g++-13/flatc (flatc version 25.12.19)
[OK]   transports   zenoh 1.9.0, iceoryx2 0.9.3 (linked into this sb)
[OK]   libzenohc    /opt/zenoh-c/current/lib/libzenohc.so (v1.9.0, dlopen OK)
[OK]   libiceoryx2  /opt/iceoryx2/current/lib/libiceoryx2_ffi_c.so (v0.9.3, dlopen OK)
[OK]   tmux         /usr/bin/tmux (tmux 3.4)
[OK]   message styles /home/el/.swarmbotix/messages
                    ros2         138 msg  targets built
                    swarmbotix    14 msg  targets built
[OK]   unity targets no stale generated C# in a Unity tree
[OK]   config keys  no deprecated keys
[OK]   std vault    /home/el/.swarmbotix/messages/ros2/message_definitions/std populated
```

Three rows carry versions, and they are three different facts. `transports`
is what this binary links; the two `lib*` rows are what the installs on this
box are. While all three agree, `sb` and generated C/C++ consumers speak the
same protocol; when they diverge, the `lib*` row says so (§4.3, §4.4).

Every style is listed and none is marked "active" — there is no such
state. A style whose bindings were never generated says so and names the
fix; that is otherwise invisible until someone references one of its
messages and codegen fails against a missing target tree:

```
                    swarmbotix    14 msg  NOT COMPILED — run `sb message compile --style swarmbotix`
```

A config still carrying the dead keys:

```
[FAIL] config keys  message_style, message_targets set but IGNORED — message
                    names are fully qualified now (<style>/<namespace>/<Leaf>),
                    so there is no active style to select. Delete the line(s)
                    from sb.config.yml.
```

Field widths:

- Line 1 is not a check: `sb doctor` prints `sb <version>` — the literal
  string `sb ` followed by the same `CARGO_PKG_VERSION` that
  `sb --version` reports — before the checklist. Skip it when parsing.
  (It is written as a placeholder here on purpose: this document does
  not mirror the current release number.)
- Column 1: tag in brackets (`[OK]`, `[FAIL]`, `[SKIP]`, `[WARN]`),
  6 chars wide.
- Column 2: check name, padded to 12 chars (`protoc`, `flatc`,
  `transports`, `libzenohc`, `libiceoryx2`, `tmux`, `message styles`,
  `unity targets`, `config keys`, `std vault`). Two names overflow that pad and push
  column 3 to the right — `message styles` (14) and `unity targets`
  (13). Do not align a parser on a fixed column offset.
- Column 3: free-form `<path> (<details>)` or `<reason>`. Continuation
  lines are indented 20 spaces to sit under column 3.

Each line after the version is one check. There is no header row, no
totals line on success, and no JSON / machine-readable mode.

Tags are **colored** (green / red / yellow, bold) when stdout is a TTY.
Redirecting to a file or a pipe, or setting a non-empty `NO_COLOR`,
emits plain ASCII — so the shapes quoted here are exactly what a script
sees.

### 3.1 Tag semantics

| Tag | Meaning | Exit-code impact |
|---|---|---|
| `[OK]` | Check ran, succeeded. | no |
| `[FAIL]` | Check ran, failed (file missing, dlopen error, subprocess non-zero). | yes — flips exit to non-zero |
| `[SKIP]` | Config field unset or check structurally inapplicable. | no |
| `[WARN]` | Something is wrong, but in the **host project** rather than in `sb`. | no |

Only `[FAIL]` moves the exit code: `Report::all_ok()` is "no check
failed", so a run of nothing but `[OK]`, `[SKIP]` and `[WARN]` exits 0.

`[WARN]` exists for two situations today: stale generated C# under a
Unity tree (§4.7), and a native transport library whose version differs
from the crate `sb` links (§4.3, §4.4). Both break a *host* project
rather than `sb`. Failing there would block every unrelated `sb doctor`
on the box until someone fixed a tree `sb` does not own; staying silent
is what let the breakage accumulate in the first place. If you want
warnings to be fatal in CI, grep for the tag:

```bash
sb doctor | grep -q '^\[WARN\]' && exit 1
```

---

## 4. Per-check details

### 4.1 `protoc`

```
[OK]   protoc       /opt/protobuffer/protoc-35.0-linux-x86_64/bin/protoc (libprotoc 35.0)
[FAIL] protoc       /usr/local/bin/protoc (file not found)
[FAIL] protoc       /tmp/dead/protoc (--version exited 127)
[SKIP] protoc       not set in sb.config.yml
```

Verification:

1. Read `cfg.protoc` (typed as `Option<PathBuf>`).
2. If `None` → `[SKIP]`.
3. If the file doesn't exist → `[FAIL]`.
4. Spawn `<path> --version`, capture stdout. If exit code != 0 →
   `[FAIL]` with the exit code.
5. On success, extract the first line of stdout (e.g. `libprotoc 35.0`)
   for the details column.

### 4.2 `flatc`

Same shape as `protoc`. The details column shows the first line of
`<path> --version`, typically `flatc version 25.12.19`.

### 4.3 `libzenohc`

```
[OK]   libzenohc    /opt/zenoh-c/current/lib/libzenohc.so (v1.9.0, dlopen OK)
[OK]   libzenohc    /opt/local/lib/libzenohc.so (version unknown, dlopen OK)
[WARN] libzenohc    /opt/zenoh-c/1.8.4/lib/libzenohc.so (v1.8.4, dlopen OK)
                    sb links zenoh 1.9.0, this library is 1.8.4.
                    Patch releases interoperate; a differing major.minor is not guaranteed to.
                    Fix: sb config set libzenohc <path to a 1.9.0 install>
[FAIL] libzenohc    /opt/zenoh-c/current/lib/libzenohc.so (dlopen error: <message>)
```

Verification:

1. Read `cfg.libzenohc`. If `None` → `[SKIP]`.
2. If the file doesn't exist → `[FAIL]`.
3. `libloading::Library::new(path)` — load + immediately drop.
   No symbols are looked up; this only catches "the dynamic linker
   can't load this file at all" failures (typically: missing
   transitive deps, wrong architecture, GLIBC mismatch).
4. Determine the install's version (below), print it, and compare it
   with the zenoh crate from §4.9. Zenoh keeps patch releases
   wire-compatible, so only a differing **major.minor** warns.

Possible failures dlopen catches:

- Missing transitive deps: `libssl.so.1.1: cannot open shared object
  file: No such file or directory`.
- Architecture mismatch: `wrong ELF class: ELFCLASS32` (32-bit lib
  against 64-bit host).
- GLIBC: `version GLIBC_2.34 not found`.

**Where the version comes from.** Neither transport library exports a
version symbol, so there is nothing to call. What the installs do carry
is metadata beside the binary and a version in the path, and doctor
reads them in this order, first hit winning:

1. `<libdir>/pkgconfig/*.pc` → the `Version:` field.
2. `<libdir>/cmake/<pkg>/*ConfigVersion.cmake` → `set(PACKAGE_VERSION "…")`.
3. A `vX.Y.Z` / `X.Y.Z` component of the **canonical** path — the config
   normally points through a `current` symlink, which is resolved first
   so the version it hides becomes visible.
4. A `libfoo.so.X.Y.Z` filename suffix.

An install that states its version nowhere prints `version unknown` and
is never warned about: a number that could only be guessed at should not
be reported as if it had been measured.

What this still does **not** catch:

- A library that loads and reports a matching version but was built with
  incompatible options. Only the runtime pub/sub path proves that.

### 4.4 `libiceoryx2`

Same shape as `libzenohc`, with one difference that matters: the
comparison is on **every digit**, not just major.minor.

```
[WARN] libiceoryx2  /opt/iceoryx2/v0.9.0/lib/libiceoryx2_ffi_c.so (v0.9.0, dlopen OK)
                    sb links iceoryx2 0.9.3, this library is 0.9.0.
                    Segments are opened on exact version equality, so C/C++ consumers built
                    against this library will get VersionMismatch and an empty topic list.
                    Fix: sb config set libiceoryx2 <path to a 0.9.3 install>
```

iceoryx2 writes its full `major.minor.patch` into every shared-memory
segment and compares for equality when opening one. A 0.9.0 process
cannot see a service created by a 0.9.3 process: it gets
`VersionMismatch`, and `sb topic list` prints `(no topics)` against a
publisher that is very much alive. That is why a patch difference is
worth a warning here and not on zenoh.

The warning is not fatal — `sb` itself links the crate and keeps
working. What breaks is the generated C/C++ code a host project builds
against this library. Align the two by pointing `libiceoryx2` at the
matching install (`sb config set libiceoryx2 <path>`), or by rebuilding
`sb` against the version the box has.

### 4.5 `tmux`

```
[OK]   tmux         /usr/bin/tmux (tmux 3.4)
[FAIL] tmux         /usr/bin/tmux (-V exited 1)
```

Verification: same as protoc/flatc, but invokes `<path> -V` (note the
short flag — tmux's `--version` triggers help on some builds).
First line of stdout becomes the details column.

L5's `sb up` / `sb run` will require tmux at runtime; doctor's tmux
check is what tells you ahead of time whether those will work.

### 4.6 `std vault`

```
[OK]   std vault    /home/el/.swarmbotix/messages/ros2/message_definitions/std populated
[FAIL] std vault    /home/el/.swarmbotix/messages/ros2/message_definitions/std missing X expected file(s) (run `sb message list` to install)
[FAIL] std vault    /home/el/.swarmbotix/messages/ros2/message_definitions/std not found (run `sb message list` to install the bundle)
[SKIP] std vault    /home/dev/proj/messages has no ros2 style — std bundle lives in the configured vault, not a discovered one
```

The `[SKIP]` variant fires only when the vault was **discovered from
cwd** (§4.8) and has no `ros2` style at all. A standalone message repo
legitimately has none, and the forge bundle installs into the configured
vault only, so failing there would flag every such repo for doing
nothing wrong. Against the configured vault a missing `std/` is still a
hard `[FAIL]`.

Verification:

1. Resolve `<message_definitions>` (see [sbcli_messages.md](sbcli_messages.md) §1).
2. Walk `<message_definitions>/std/`.
3. For each file in the forge-bundled `STD_BUNDLE`
   (`Header.proto`, `String.proto`, `StringStamped.proto`,
   `Vector3.proto`, `Twist.proto`, `TwistStamped.proto`,
   `Image.proto`, `ImageStamped.proto`), confirm the file exists in
   the vault.
4. If any are missing → `[FAIL]` with the count + hint at
   `sb message list` (which auto-installs).

The `[FAIL]` here is **soft** in the sense that it's trivially
recoverable: `sb message list` triggers `Vault::ensure_installed`
which copies missing files from the binary's embedded `STD_BUNDLE`.
After that, doctor goes green.

User-edited std files survive: `ensure_installed` only writes
**missing** files (per L1 TDD test #5 — `install_preserves_user_edits`).
Doctor only checks existence, not byte-equality with the bundle, so a
hand-modified `std/Header.proto` is still `[OK]`.

### 4.7 `unity targets`

```
[OK]   unity targets no stale generated C# in a Unity tree
```

```
[WARN] unity targets generated C# Unity cannot compile:
                    swarmbotix: 12 generated .cs file(s) predate the current emitter
                      /path/to/UnityProj/Assets/Scripts/messages/swarmbotix/message_targets
                    File-scoped namespaces (C# 10) / [InlineArray] (.NET 8); Unity caps at C# 9.
                    Or an inline `_Array<N>` wrapper, which collides once two messages share
                    an element type (CS0101). Wrappers now live in iox2/_arrays/.
                    Fix: re-run `sb message compile` — the emitter handles both now.
                    These trees are usually committed, so commit the regenerated files too.
```

**This check never fails the run** (§3.1). What it finds is broken code
in a *user's Unity project*, not a broken `sb` install; making it fatal
would block every unrelated `sb doctor` on the box until somebody
cleaned up a tree `sb` does not own.

Verification, per style:

1. Resolve that style's `message_targets/`.
2. Walk **up** from it looking for a directory containing both
   `Assets/` and `ProjectSettings/`. No such ancestor → the tree is not
   inside a Unity project, and the style is skipped entirely.
3. Recursively count `.cs` files under `message_targets/` that a current
   `sb` would not have written — any file with a **file-scoped namespace**
   (`namespace Foo;`, C# 10), a `[InlineArray]` attribute (.NET 8), or an
   inline `_Array<N>` wrapper struct declared outside `iox2/_arrays/`.
4. Any offenders → one `[WARN]` naming each style, its count, and the
   path.

Why these three shapes: the emitter targets **C# 9 / .NET Standard 2.1**
so its output compiles inside Unity, but fixing the emitter does not
rewrite files already on disk — only a recompile does. File-scoped
namespaces raise `error CS8773` in Unity; `[InlineArray]` needs .NET 8;
and the pre-0.1.39 inline array wrapper declared one struct per message
file, which becomes `error CS0101` the moment two messages share an
element type (wrappers now live once each in `iox2/_arrays/`).

Detection is by **file content**, not by directory name, so it stays
accurate no matter which backends a style emits. The fix is always the
same: re-run `sb message compile`, then commit the regenerated files —
these trees are normally checked in.

Note that `sb` emits **all** backends into every tree, Unity included;
it does not narrow the backend set to suit one consumer. See
[sbcli_messages.md](sbcli_messages.md) §4.5 (backend selection) and
§5.1.

### 4.8 `message styles` — and which vault doctor is describing

```
[OK]   message styles /home/el/.swarmbotix/messages
                    ros2         138 msg  targets built
                    swarmbotix    14 msg  targets built
```

One row per style under the resolved root: its name, its `.proto` count,
and whether its `message_targets/` holds anything. Three per-style
states:

| State | Means |
|---|---|
| `targets built` | `message_targets/` is non-empty |
| ``NOT COMPILED — run `sb message compile --style <name>` `` | definitions exist, nothing generated |
| `no definitions` | the style directory exists but holds no `.proto` |

The only failure is a root with **no styles at all** — the one state
that makes `sb message` and `sb pub` unusable:

```
[FAIL] message styles no styles found under /home/el/.swarmbotix/messages
```

**Doctor reports the vault the next command will actually use.** `sb`
walks up from the current directory looking for a `messages/` tree, and
a discovered tree beats `messages_root` from config
([sbcli_messages.md](sbcli_messages.md) §1). A doctor describing a
different tree than `sb message compile` is about to touch would be
worse than no doctor, so the check follows the same resolution and
**says so in the output**:

```
[OK]   message styles /home/dev/proj/messages (discovered from cwd — overrides `messages_root`)
```

And when cwd *is* itself a style directory, only that style is in scope:

```
[OK]   message styles /home/dev/proj/messages
                    cwd is the `swarmbotix` style — only it is in scope
                    swarmbotix    14 msg  targets built
```

Two consequences worth internalizing:

- **Run `sb doctor` from `$HOME`** (or with `$SB_CONFIG` set, which
  disables the walk) when you want a report about the configured
  install rather than about whatever checkout you happen to be sitting
  in.
- `std vault` (§4.6) **skips instead of failing** when a discovered root
  has no `ros2` style. A standalone message repo legitimately has no
  `std/`; the forge bundle installs into the configured vault only.

### 4.9 `transports`

```
[OK]   transports   zenoh 1.9.0, iceoryx2 0.9.3 (linked into this sb)
```

The two numbers are the zenoh and iceoryx2 **crates statically linked
into this binary**, fixed when `sb` was built. They are not read from
the config, cannot be changed by editing anything on the box, and are
not affected by which native libraries are installed.

This row exists so the `lib*` rows have something to be compared with.
On its own it answers "which transport versions does this `sb` speak",
a question that previously had no answer short of consulting the source
of the release it came from.

The row never fails. Its values are read out of the source repository's
lockfile when the binary is compiled, so they are exact by construction
rather than transcribed. A binary built outside that tree has nothing to
read and says so:

```
[OK]   transports   linked crate versions unavailable (built without Cargo.lock)
```

Official releases are always built in-tree, so this line should not
appear in an installed `sb`; if it does, the build did not come from the
release procedure and the two `lib*` rows will not be version-checked.

---

## 5. Exit code and error reporting

```bash
$ sb doctor
sb <version>
[OK]   protoc       ...
[FAIL] flatc        /opt/flatc/dead (file not found)
[OK]   transports   ...
[OK]   libzenohc    ...
[OK]   libiceoryx2  ...
[OK]   tmux         ...
[OK]   message styles ...
[WARN] unity targets generated C# Unity cannot compile:
                    ...
[OK]   config keys  no deprecated keys
[OK]   std vault    ...
error: 1 check(s) failed
$ echo $?
1
```

The error line goes to **stderr** (consistent with the rest of the
CLI). Stdout has the checklist; stderr has the summary. Pipe stdout
to a parser without losing the failure signal.

Multiple failures count separately:

```
error: 3 check(s) failed
```

There's no `--quiet` flag; doctor always prints the full checklist.
Wrap it for CI:

```bash
sb doctor > /dev/null || { echo "host needs setup"; exit 1; }
```

---

## 6. Examples

### 6.1 Fresh dev box, nothing configured

```
$ sb doctor
sb <version>
[SKIP] protoc       not set in sb.config.yml
[SKIP] flatc        not set in sb.config.yml
[OK]   transports   zenoh 1.9.0, iceoryx2 0.9.3 (linked into this sb)
[SKIP] libzenohc    not set in sb.config.yml
[SKIP] libiceoryx2  not set in sb.config.yml
[SKIP] tmux         not set in sb.config.yml
[OK]   message styles /home/user/.swarmbotix/messages
                    ros2         138 msg  NOT COMPILED — run `sb message compile --style ros2`
[OK]   unity targets no stale generated C# in a Unity tree
[OK]   config keys  no deprecated keys
[FAIL] std vault    /home/user/.swarmbotix/messages/ros2/message_definitions/std missing 8 expected file(s) (run `sb message list` to install)
error: 1 check(s) failed
```

Recovery: `sb message list` (auto-installs) + edit
`~/.swarmbotix/sb.config.yml` to point at the installed tools (see
[sbcli_config.md](sbcli_config.md) §6).

### 6.2 Fully-configured dev box

```
$ sb doctor
sb <version>
[OK]   protoc       /opt/protobuffer/protoc-35.0-linux-x86_64/bin/protoc (libprotoc 35.0)
[OK]   flatc        /opt/flatbuffers/Linux.flatc.binary.g++-13/flatc (flatc version 25.12.19)
[OK]   transports   zenoh 1.9.0, iceoryx2 0.9.3 (linked into this sb)
[OK]   libzenohc    /opt/zenoh-c/current/lib/libzenohc.so (v1.9.0, dlopen OK)
[OK]   libiceoryx2  /opt/iceoryx2/current/lib/libiceoryx2_ffi_c.so (v0.9.3, dlopen OK)
[OK]   tmux         /usr/bin/tmux (tmux 3.4)
[OK]   message styles /home/el/.swarmbotix/messages
                    ros2         138 msg  targets built
                    swarmbotix    14 msg  targets built
[OK]   unity targets no stale generated C# in a Unity tree
[OK]   config keys  no deprecated keys
[OK]   std vault    /home/el/.swarmbotix/messages/ros2/message_definitions/std populated
```

Exit code 0. This is the expected steady state.

### 6.3 Tool moved or upgraded out from under sb.config.yml

```
$ sb doctor
sb <version>
[OK]   protoc       /opt/protoc/bin/protoc (libprotoc 35.0)
[FAIL] flatc        /opt/flatc-old/bin/flatc (file not found)
...
error: 1 check(s) failed
```

The `flatc:` entry still points at a directory the user deleted.
Fix:

```bash
sb config set flatc /opt/flatc-new/bin/flatc
sb doctor
```

### 6.4 Loadable file, wrong version

```
$ sb doctor
sb <version>
...
[OK]   transports   zenoh 1.9.0, iceoryx2 0.9.3 (linked into this sb)
[WARN] libiceoryx2  /opt/iceoryx2/v0.9.0/lib/libiceoryx2_ffi_c.so (v0.9.0, dlopen OK)
                    sb links iceoryx2 0.9.3, this library is 0.9.0.
                    Segments are opened on exact version equality, so C/C++ consumers built
                    against this library will get VersionMismatch and an empty topic list.
                    Fix: sb config set libiceoryx2 <path to a 0.9.3 install>
```

The `.so` loads cleanly, so `dlopen` has nothing to say about it; what
disagrees is the version, which doctor reads off the install and
compares with §4.9. Exit code stays 0 — `sb` works, the C/C++ consumers
built against that library do not. Fix:

```bash
sb config set libiceoryx2 /opt/iceoryx2/current/lib/libiceoryx2_ffi_c.so
sb doctor
```

### 6.5 CI gating

```yaml
- name: Validate host
  run: sb doctor

- name: Build
  run: cargo build --release
```

Doctor's non-zero exit fails CI early with a clear list of what's
wrong, before the multi-minute Rust build kicks off.

---

## 7. Configuration interactions

Doctor reads the **merged** config (env override → workspace →
global → built-in defaults), same as every other verb. To diagnose
which file populated each field:

```bash
SB_CONFIG=/tmp/scratch.yml sb doctor      # uses /tmp/scratch.yml as the highest-priority layer
```

vs.

```bash
sb doctor                                  # only the global ~/.swarmbotix/sb.config.yml
```

There's currently no `sb doctor --explain` mode that prints the
source-of-each-field. The layering is documented in
[sbcli_config.md](sbcli_config.md) §3 and the
`sb_config::ConfigSources` type tracks it internally; that data just
isn't surfaced through doctor's output yet.

---

## 8. What doctor doesn't check

By design (the L1 spec) doctor only validates the tooling that L1+
sb itself shells out to. Not validated:

- **Rust toolchain.** Cargo's own checks run when the user's project
  builds. sb itself is a single binary; the user's Rust toolchain
  isn't on the critical path for `sb` to function.
- **Python interpreter / packages.** `sb message new` doesn't shell
  out to Python; doctor doesn't either. The L3 examples activate
  conda explicitly. A `sb doctor` flag to check
  `pip show eclipse-zenoh` is planned but not shipped.
- **Flutter / Dart toolchain.** Same.
- **Package-manager copies of the transports.** The version comparison
  in §4.3 / §4.4 covers the native libraries named in the config. It
  says nothing about what `pip show iceoryx2` reports in whichever
  environment happens to be active, which must match the same number
  for Python consumers to see `sb`'s topics.
- **Network connectivity.** Zenoh's runtime discovery needs the
  network; doctor never opens a Zenoh session.
- **Shared-memory permissions.** iceoryx2's runtime needs
  `/dev/shm` writability; doctor doesn't probe.
- **Generated code freshness.** Doctor doesn't compare
  `<targets>/` mtimes against `<defs>/` mtimes. Use
  `sb message compile` to refresh.
- **`flow.yaml` referential integrity.** Doctor doesn't check that
  every entry in every workspace's `flow.yaml` points at an
  existing `sb.dev.yml`. That's a swarmctl concern (L6).

---

## 9. Edge cases and gotchas

### 9.1 tmux's `-V` quirk

The first L1 implementation used `--version` for tmux; that triggered
the help text on older tmux builds and the version-extraction regex
failed. The L1 report notes this caught during the first end-to-end
run; the fix (using `-V`) is locked in by
`crates/sb-cli/tests`.

### 9.2 First-line stdout assumption

Doctor extracts the first line of `<tool> --version`'s stdout as the
details column. Tools that print a banner before the version (rare)
would show the banner instead. None of the L1-required tools do
this, but if you swap in an exotic protoc / flatc fork, the column
might look odd. No correctness issue — exit code still reflects
truth.

### 9.3 Tool path with spaces

`Command::new(<path>)` accepts paths with spaces, but the details
column's free-form formatting may look strange:

```
[OK]   protoc       /home/user/With Spaces/bin/protoc (libprotoc 35.0)
```

Cosmetic only; the check itself is fine.

### 9.4 `libloading` dlopen errors are sometimes terse

Some platforms return only "image not found" on dlopen failure, even
when the file does exist (e.g. when transitive deps are missing).
The details column reproduces what libloading gives us — debugging
might need `ldd <path>` to see the actual missing deps.

### 9.5 std vault check predates fresh-install

If `<sb_home>` is brand-new and you've never run any `sb` command,
`<message_definitions>/std/` doesn't exist yet. The std-vault check
reports the miss. Either run `sb message list` (which installs) or
just run `sb doctor` again — it has no install side effect on its
own, only the diagnostic.

### 9.6 No retry / sleep / network probe

Doctor runs each check **once**, synchronously, with no retries.
Transient errors (a tool whose binary is paged out mid-`--version`,
extremely rare) would surface as a one-off FAIL. Re-run if you
suspect noise.
