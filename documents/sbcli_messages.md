# `sb message` — Message Management Reference

Official reference for the `sb message` subcommand surface: lifecycle,
codegen, capacity configuration, and the operational semantics that
govern when files are read, written, overwritten, or removed.

This document is normative for behavior shipped at L1.

The canonical spec is `requirements.md` in the sbcli **source
repository** — a maintainer document that is not part of your install;
when it and this page disagree, it wins.

---

## 1. Conceptual model

Messages are grouped into **styles**. A style is one directory under
`messages_root`, and it owns **two distinct on-disk roots** so source
files and generated bindings never share a directory.

```text
<messages_root>/            ← default <sb_home>/messages
  ros2/                     ← std/ + the ROS2 mirror, 138 .proto
    message_definitions/
    message_targets/
  swarmbotix/               ← the main style — header/, primitives/,
    message_definitions/      images/, sensors/
    message_targets/
  <yours>/                  ← styles are an OPEN SET
    message_definitions/
```

| Role | Resolved path | Holds |
|---|---|---|
| Source-of-truth | `<messages_root>/<style>/message_definitions` | `.proto` files (your edits), the forge-bundled `std/*` and ROS2-mirror namespaces, `.cache/descriptor.bin` |
| Codegen output | `<messages_root>/<style>/message_targets` | `iox2/`, `proto/`, `fb/` backend trees produced by `sb message compile` |

**There is no "active" style.** A message is named in full wherever it
appears — CLI argument, `sb.dev.yml`, or `Header.metadata.msg_type` on the
wire:

```
<style>/<namespace>/<Leaf>
```

`ros2/std/Header`, `swarmbotix/images/Image480pMono8`. The style segment is
part of the identity, not a lookup against somebody's configuration.

Why it has to be that way: a single module routinely publishes one style
and subscribes to another, so no one setting could describe it. And
`msg_type` travels to consumers that share no config with the publisher —
`images/Image480pMono8` alone would not say which schema produced the
bytes, since two styles may each define an `images/Image`.

A 2-segment name is therefore an **error**, not a lookup:

```
$ sb pub add -m images/Image480pMono8 cam --iox2
error: invalid message name "images/Image480pMono8": message names are
fully qualified as <style>/<namespace>/<Leaf> (e.g. ros2/std/Header,
swarmbotix/images/Image480pMono8). Run `sb message list` to see every
message with its style.
```

Both roots derive from `messages_root` plus the style **in the name**.
Style names must be plain directory names; `/`, `\`, `:`, `.` and `..`
are rejected so a style can never resolve outside `messages_root`.

`sb` also walks up from the current directory looking for a `messages/`
tree (a directory with at least one `<style>/message_definitions/` child),
so running it inside a checkout uses that checkout's messages.
`$SB_CONFIG` being set disables the walk.

`message_style`, `message_definitions` and `message_targets` are **dead
config keys** as of 0.1.35 — see [sbcli_config.md](sbcli_config.md) §2.1.

**Cross-style imports are not supported.** `protoc` runs with a single
style's `message_definitions/` as its include root, so a `.proto` in one
style cannot `import` from another. The shipped split is verified clean:
nothing in the ROS2 namespaces imports `std/`, and `std/` imports only
`std/`.

`std/` — including the load-bearing `std/Header` — ships in the **ros2**
style. The embedded first-run bundle installs into that style only; a
vault opened for any other style neither installs nor reports on it.

The **swarmbotix** style carries swarmbotix's own definitions:

| Namespace | Holds |
|---|---|
| `header/` | `Header` — UNIX timestamp in nanoseconds, `frame_id`, `seq`, and the `metadata` block L4 reads for per-topic rate |
| `primitives/` | `Vector3`, `Vector4`, `Quaternion`, `Mat33`, `Mat44`, `Pose`, `Twist`, `Transform` |
| `images/` | `Image` — one schema, eight fixed-size iox2 types (§5.1) |
| `sensors/` | `Imu`, `LaserScan`, `PointCloud`, `GnssFix` |

`header/Header.timestamp_ns` is nanoseconds since the UNIX epoch, chosen
as the base unit precisely so every coarser unit is an **exact integer
divide** — µs = `/1_000`, ms = `/1_000_000`, s = `/1_000_000_000`. No
floating point, no rounding drift. `uint64` ns covers year 2554.

Throughout this doc, the placeholder `<defs>` refers to a style's
`message_definitions/` directory and `<targets>` to its
`message_targets/` sibling. Both are **derived** from `messages_root`
plus the style segment of the message name — neither is configurable on
its own (the keys that once did that are dead; see §1).

Where prose says "the vault" without
qualification, it means `<defs>` — the term is a colloquial holdover
from the pre-split implementation and is kept because it reads
naturally for the source-of-truth tree. Every `sb message *` subcommand
reads from `<defs>` and writes generated artifacts to `<targets>`
(with the one exception called out in §3: `.cache/descriptor.bin`
lives under `<defs>`).

---

## 2. Naming convention

A message is addressed by a **three-part path** (sb 0.1.35+):
`<style>/<namespace>/<Leaf>`.

- `<style>` is the style directory under `messages_root` — see §1. It is
  part of the message's identity, not a lookup against configuration.
  Allowed characters are ASCII alphanumerics and `_`; `/`, `\`, `:`, `.`
  and `..` are rejected so a name can never resolve outside
  `messages_root`.
- `<namespace>` is both the on-disk directory under that style's
  `message_definitions/` **and** the proto `package` declaration
  (formatted as `swarmbotix.<namespace>` for the forge-bundled `std/*`,
  otherwise as `<namespace>` verbatim for ROS2-mirror packages like
  `sensor_msgs`). ASCII alphanumerics and `_`.
- `<Leaf>` is the proto `message` name. It also drives the basename of
  every generated file across every backend, so it must be a valid
  identifier in Rust, C++, Python, and C# simultaneously. **Enforced**:
  it must start with an ASCII uppercase letter and contain only ASCII
  alphanumerics — PascalCase, no `_`, `.`, `-`, or spaces.

Valid: `ros2/sensor_msgs/JointState`, `ros2/std/Header`,
`swarmbotix/images/Image`. A name that resolves to an
[`iox2_variants`](#64-iox2_variants--one-schema-many-fixed-size-types)
type is equally valid wherever a message name is accepted —
`swarmbotix/images/Image480pMono8`.

Invalid (rejected at parse time): fewer than three segments, more than
three, any empty segment, a lowercase or non-alphanumeric leaf.

**Two-segment names are a hard error, not a fallback.** The pre-0.1.35
form (`std/Header`) does not resolve against a default style — there is
no default style. The error names the fix:

```
$ sb message compile std/Header
error: message names are fully qualified as <style>/<namespace>/<Leaf>
(e.g. ros2/std/Header, swarmbotix/images/Image480pMono8) — got
"std/Header". Run `sb message list` to see every message with its style.
```

`sb message list` prints every message in exactly this form, so it is
the fastest way to get a name you can paste.

Parsing happens in `sb_vault::MessageName::parse`; the error surface is
attached to every command that takes a name.

---

## 3. Vault layout

### 3.1 Definitions tree — `<defs>/`

```
<defs>/
├─ <pkg>/<Leaf>.proto         # source — you edit these
├─ std/                       # forge-shipped bundle (immutable for new/rm)
│  └─ Header.proto, ...
├─ sensor_msgs/               # ROS2-mirror packages, also forge-shipped
│  └─ Image.proto, JointState.proto, ...
│
└─ .cache/
   └─ descriptor.bin          # serialized FileDescriptorSet IR
```

Only `.proto` files and the `.cache/` directory live here. The `.cache/`
subdir holds the `prost_types::FileDescriptorSet` regenerated on every
`sb message compile` — keeping it next to the sources avoids stale-IR
hazards across split CI/dev workflows that send generated output
elsewhere with `--out`. Deleting `.cache/descriptor.bin` is harmless; the next
compile rebuilds it. `--out` does not affect its location.

### 3.2 Targets tree — `<targets>/`

```
<targets>/
├─ iox2/                          # iceoryx2 flat-POD bindings
│  ├─ <pkg>/<Leaf>/               # per-message dir, one file per language
│  │  ├─ <Leaf>.rs                #   Rust   (#[repr(C)])
│  │  ├─ <Leaf>.h                 #   C++    (struct in <pkg> namespace)
│  │  ├─ <Leaf>.py                #   Python (ctypes.Structure, _pack_ = 1)
│  │  └─ <Leaf>.cs                #   C#     ([StructLayout Sequential Pack=1])
│  │
│  └─ _arrays/                    # C# only — shared `repeated <Message>` wrappers
│     └─ <pkg>_<Leaf>_Array<N>.cs #   namespace sb_iox2_arrays, one per (type, cap)
│
├─ proto/                         # native protobuf bindings
│  └─ <pkg>/<Leaf>/
│     ├─ <Leaf>.pb.cc + .pb.h     #   C++    (protoc --cpp_out)
│     ├─ <Leaf>_pb2.py            #   Python (protoc --python_out)
│     └─ <Leaf>.cs                #   C#     (protoc --csharp_out)
│
└─ fb/                            # FlatBuffers bindings
   └─ <pkg>/<Leaf>/
      ├─ <Leaf>_generated.h       #   C++    (flatc --cpp)
      ├─ <Leaf>_generated.rs      #   Rust   (flatc --rust)
      ├─ <Leaf>.py                #   Python (flatc --python)
      └─ <Leaf>.cs                #   C#     (flatc --csharp)
```

Notes:
- The three backend roots (`iox2/`, `proto/`, `fb/`) sit at the top of
  `<targets>/`. Every backend's tree mirrors `<pkg>/<Leaf>/` so a given
  message has one folder per backend, with all language files for that
  message inside.
- `iox2/_arrays/` is the one exception, and it exists only for C#. See
  §5.1.2 — a `repeated <Message>` field needs a named wrapper type there,
  and a type may be declared exactly once per assembly, so the wrapper
  cannot live in the message dir of every message that uses it. The leading
  underscore keeps it out of the `<pkg>/` namespace space.
- `<targets>` is always `<messages_root>/<style>/message_targets/` — a
  top-level peer of `<defs>`, not a subtree of it. For the `ros2` style on
  a default install that is
  `~/.swarmbotix/messages/ros2/message_targets/`. There is no config key
  for it: the old `message_targets:` key is dead (§1), and the only way to
  send output elsewhere is `--out` per invocation.
- `sb message compile --out <dir>` overrides `<targets>` for a single
  invocation without touching the config file.
- Source `.proto` files are never read from or written to anything
  under `<targets>/`.

---

## 4. Subcommands

All `sb message *` subcommands resolve `<defs>` and `<targets>` once at
startup (see §1). None of them mutate `sb.config.yml`.

### 4.1 `sb message list`

```bash
sb message list
```

Walks the vault, ensures the `std/*` bundle is installed (a one-shot
copy from the forge binary's embedded resources), and prints every
message grouped by namespace:

```
builtin_interfaces/
  Duration
  Time
sensor_msgs/
  CompressedImage
  Image
  Image1080p
  ...
std/
  Header
  ...
```

The list contains **only** `.proto` files — generated artifacts under
`iox2/`, `proto/`, `fb/` are ignored. If you've manually deleted a
`.proto` from a forge-shipped namespace, `ensure_installed()` restores
it before listing.

### 4.2 `sb message new <style>/<namespace>/<Leaf>`

```bash
sb message new ros2/robot_msgs/JointTarget
```

**Effects (in order)**:

1. Parses `<style>/<namespace>/<Leaf>`. Rejects malformed names.
2. Refuses if `<pkg>` is `std` — that namespace is reserved for the
   forge bundle (`Vault::new_message` returns
   `"cannot create messages under 'std/' (reserved for the forge bundle)"`).
3. Refuses if `<defs>/<pkg>/<Leaf>.proto` already exists.
4. Creates `<defs>/<pkg>/` if missing.
5. Writes a skeleton:
   ```protobuf
   // <pkg>/<Leaf>.proto — generated skeleton.
   syntax = "proto3";

   package swarmbotix.<pkg>;

   message <Leaf> {
     // TODO: add fields
   }
   ```
6. **Auto-compiles the new message across every available backend**
   (iox2 always; proto if `protoc` is set in `sb.config.yml`; fb if
   `flatc` is set). The skeleton is empty, so each backend emits an
   empty struct/class. This keeps the invariant that *every message in
   the vault has up-to-date generated bindings*.

Output:

```
created /home/el/.swarmbotix/messages/swarmbotix/message_definitions/robot_msgs/JointTarget.proto
wrote   /home/el/.swarmbotix/messages/swarmbotix/message_definitions/.cache/descriptor.bin
iox2:   wrote 4 file(s) under ...
proto:  wrote C++/Python/C# bindings (1 source file(s)) into ...
fb:     wrote Rust/C++/Python/C# FlatBuffers bindings (1 source file(s)) into ...
```

If any backend fails (e.g., `protoc` configured but its path is dead),
the entire `new` operation returns non-zero; the `.proto` file is left
in place so you can fix the tooling and re-run.

### 4.3 `sb message edit <style>/<namespace>/<Leaf>`

```bash
sb message edit ros2/sensor_msgs/JointState
```

**Effects (in order)**:

1. Parses the name; resolves the `.proto` path via `vault.path_of`.
2. Errors if the path does not exist (use `sb message new` first).
3. Launches `$EDITOR` on the file. Falls back to `vi` when `$EDITOR` is
   unset.
4. Waits for the editor process to exit.
5. **If the editor exited with status 0**, recompiles that one message
   across every available backend (same logic as `new`'s post-compile
   step). Any non-zero editor exit skips the recompile, prints the
   editor status as part of the error, and returns non-zero.

Edits work on **any** message file, including the forge-bundled
`std/*` ones. Local std edits survive `ensure_installed()` (the install
check only repopulates *missing* files), but the forge bundle itself is
not modified — a `sb` reinstall on another host gets the upstream
version, not your local fork.

### 4.4 `sb message rm <style>/<namespace>/<Leaf>`

```bash
sb message rm ros2/robot_msgs/JointTarget
```

**Effects (in order)**:

1. Parses the name.
2. Refuses if `<pkg>` is `std` (`"refusing to delete std/<Leaf> — std
   messages ship with the forge"`).
3. Errors if `<defs>/<pkg>/<Leaf>.proto` does not exist.
4. Removes the `.proto`.
5. **Removes every per-message generated directory** under the resolved
   `<targets>` root:
   - `<targets>/iox2/<pkg>/<Leaf>/` (recursively)
   - `<targets>/proto/<pkg>/<Leaf>/` (recursively)
   - `<targets>/fb/<pkg>/<Leaf>/` (recursively)
6. Prunes the now-empty parent package dirs
   (`<targets>/iox2/<pkg>/`, `<targets>/proto/<pkg>/`,
   `<targets>/fb/<pkg>/`) — but only when they're actually empty, so
   other messages in the same namespace are untouched.

If you previously compiled with `--out <dir>` into a custom location
that's not the configured `<targets>`, `rm` will **not** clean that
custom dir (it has no way to know where you put the outputs). Clean it
yourself or re-run `compile --out <dir>` after the `rm` to refresh.

Output:

```
removed robot_msgs/JointTarget
cleaned 3 generated artifact dir(s)
```

The number after `cleaned` is how many of the three backend leaf-dirs
existed and were removed (so it's `0` if you never compiled the
message, `1` if only iox2 was ever emitted, etc.).

### 4.5 `sb message compile [flags] [<style>/<namespace>/<Leaf>]`

```bash
sb message compile                              # everything, all available backends
sb message compile --iox2                       # everything, iox2 only
sb message compile ros2/std/Header              # one message, all available backends
sb message compile --iox2 --proto ros2/sensor_msgs/Image1080p
```

The most flag-heavy subcommand. See §5 for what each backend produces;
this section covers the flag surface and operational semantics.

#### Positional argument

| Form | Effect |
|---|---|
| _(omitted)_ | Compiles every `.proto` in the vault. |
| `<style>/<namespace>/<Leaf>` | Compiles only that one message. Errors if absent. The style comes from the name. |

When a name is given, only that single proto is fed to `protoc` when
generating `descriptor.bin`, and codegen iterates only over the matching
`FlatStruct` for the iox2 backend. Other messages already present in
older `descriptor.bin` data are not touched by this run.

#### Backend selection

Resolved per style, highest priority first. The first rule that matches
decides; nothing lower is consulted.

| # | Rule | Wins when |
|---|---|---|
| 1 | **Explicit `--iox2` / `--proto` / `--fb`** | any flag is passed |
| 2 | **`backends:` in `<style>/sb.style.yml`** | the style carries a pin |
| 3 | **Tool availability** | nothing above matched |

**1 — explicit flags.** Only the flags you set run. Setting `--proto` or
`--fb` while the corresponding tool is missing **errors** with an actionable
message. An explicit flag is a direct instruction and is never
second-guessed, including by rule 3.

**2 — the persisted pin.** Written by
[`sb message backends`](#46-sb-message-backends-style-backend--clear) to
`<style>/sb.style.yml`, a sibling of `message_definitions/`. It lives in the
style tree rather than in `sb.config.yml` because a style is often a
standalone repo cloned onto machines that share no configuration — a pin that
did not travel would have to be re-made on every box. A pin naming a backend
whose tool is missing **errors** rather than silently dropping it. An empty
list (`backends: []`) is a real answer meaning "emit nothing automatically",
distinct from having no key at all.

**3 — tool availability.** The default: `iox2` always (the emitter is
in-tree, no external tool needed), `proto` iff `protoc` resolves to an
existing path, `fb` iff `flatc` does.

This is deliberately **not** narrowed by what the host project can consume.
An earlier build detected a Unity project around the output directory and
dropped to `fb` only; that was wrong. A style's generated tree feeds C++,
Python and Rust clients as well as whatever project it happens to sit
inside, so suppressing a backend to suit one consumer silently deprives all
the others. Host compatibility is a property of the **emitter**, not of the
selection — see §5.1.1.

Whenever rule 2 narrows the selection, `sb` prints why. A narrowed selection
is never silent.

#### Output directory

| Flag | Effect |
|---|---|
| `-o <dir>` / `--out <dir>` | Generated trees land under `<dir>/` instead of `<targets>` (`<messages_root>/<style>/message_targets/`, the sibling of `<defs>` — **not** a `targets/` subtree of it, whatever `--help` says; see §9.8). |

Source `.proto` files and `.cache/descriptor.bin` are always written
under `<defs>`, regardless of `--out`. Only the
`iox2/`, `proto/`, `fb/` trees relocate.

#### Style filter — `--style <name>` (sb 0.1.35+)

| Flag | Effect |
|---|---|
| `--style <name>` | Compile only that style. Omit it and **every** style under `messages_root` is compiled. |

```bash
sb message compile                        # every style
sb message compile --style swarmbotix     # just the swarmbotix style
sb message compile --style ros2 --iox2    # composes with backend flags
```

It is a **filter, not a mode**: it selects which styles this one run
touches and persists nothing. Nothing is left "selected" afterwards, and
the next bare `sb message compile` is back to compiling everything. To
change what a style emits *persistently*, pin its backends with
[`sb message backends`](#46-sb-message-backends-style-backend--clear).

`--style` and a positional message name are **mutually exclusive** —
clap rejects the combination outright, because the name already carries
its own style:

```
$ sb message compile --style ros2 ros2/std/Header
error: the argument '--style <NAME>' cannot be used with '[NAME]'
```

An unknown style is refused with the list of real ones:

```
$ sb message compile --style nosuch
error: no style "nosuch" under /home/el/.swarmbotix/messages (present: ros2, swarmbotix)
```

`sb doctor` reports which styles have their targets built;
``NOT COMPILED — run `sb message compile --style <name>` `` is the line
that sends you here.

#### iox2 capacity overrides

These flags affect only the iox2 emitter (other backends don't need
fixed-size storage). Each is `Option<u32>`; absence means "use config /
built-in default."

| Flag | What it sizes | Built-in default |
|---|---|---|
| `--string-cap N` | per-`string` field byte cap | 256 |
| `--bytes-cap N` | per-`bytes` field byte cap | 4096 |
| `--vec-cap N` | per-`repeated T` (scalar / struct) element count | 256 |
| `--string-array-cap N` | per-`repeated string` element count (each string still consumes `--string-cap` bytes) | 10 |

CLI flags override **every** config-derived default for the duration of
that single invocation. See §6 for the full precedence model.

#### Operational semantics

**What gets written**:

1. `<defs>/.cache/descriptor.bin` — rewritten every run, even with
   `--out` pointing elsewhere. Only the protos matched by the name
   filter (or all, when no filter) are in the descriptor.
2. For each enabled backend, the per-message dirs under
   `<out>/<backend>/<pkg>/<Leaf>/` get their files written.

**Overwrite policy**: files are written via `std::fs::write` —
**in-place, per-file overwrite**. The compile step does **not** wipe
the target dir before writing. As a consequence:

- Rerunning `compile --iox2 <name>` after a schema change overwrites
  the four iox2 files for that message but **does not touch** the
  `<targets>/proto/<pkg>/<Leaf>/` and `<targets>/fb/<pkg>/<Leaf>/` dirs.
  Those proto/fb bindings stay frozen on whatever schema they were last
  compiled against — they become stale silently.
- Removing a field from a schema and recompiling that single backend
  removes references to the field from that backend's output, but any
  language file that's no longer emitted at all (currently none in the
  standard pipeline, but a future plugin could) would survive as cruft.

If you want a clean rebuild, the safe pattern today is:

```bash
sb message rm <style>/<ns>/<Leaf>     # cleans .proto + all 3 backend dirs
# (or just manually rm the per-message dirs)
rm -rf <targets>/{iox2,proto,fb}/<pkg>/<Leaf>
sb message compile <style>/<ns>/<Leaf> # regenerates from current source
```

(There's no `--clean` flag at L1. It would be additive to add.)

---

### 4.6 `sb message backends <style> [<backend>...] [--clear]`

Show or pin which backends `sb message compile` emits for a style. The pin is
rule 2 of [backend selection](#backend-selection).

```bash
sb message backends vrobots_msgs             # show current setting
sb message backends vrobots_msgs fb          # pin to fb only
sb message backends vrobots_msgs iox2 fb     # pin to two
sb message backends vrobots_msgs none        # pin an empty list
sb message backends vrobots_msgs --clear     # drop the pin
```

Writes `<style>/sb.style.yml`:

```yaml
backends: [fb]
```

**Why the file lives in the style tree.** A style is frequently a standalone
git repo (see the cwd-discovery rules in requirements.md §Message Vault),
cloned onto machines that share no `sb.config.yml`. "Which backends are safe
to emit here" is a property of the style and the project consuming it, not of
the box — so the answer has to survive a `git clone`. Commit `sb.style.yml`.

`none` is the spelling for an empty list because bare
`sb message backends <style>` already means "show". An empty list emits
nothing automatically; `descriptor.bin` is still written, and explicit flags
still work.

**Typical use.** A style whose consumers only ever read one wire format —
pinning stops the other backends being regenerated on every compile:

```bash
sb message backends vrobots_msgs fb
sb message compile                    # fb only, on this box and every clone
```

A Unity host is *not* a reason to pin: the emitters target what Unity can
compile (§5.1.1), so the full set is safe there.

---

## 5. Backends

### 5.1 `iox2` (in-tree emitter, `sb-iox2-typegen`)

**Purpose**: produce fixed-layout POD structs suitable for iceoryx2
zero-copy IPC over shared memory.

**Pipeline**:
1. `protoc` produces a `FileDescriptorSet` (this step always runs and
   populates `.cache/descriptor.bin`, even when only `--iox2` is set).
2. `sb_iox2_typegen::flatten` walks the descriptor and produces a
   `Vec<FlatStruct>` — one struct per top-level message, with all
   variable-length fields replaced by fixed-size storage + companion
   length/count fields, and all nested message fields recursively
   inlined (with `<parent>_<child>` field naming).
3. Each `FlatStruct` is rendered by four emitters (`rust.rs`, `cpp.rs`,
   `python.rs`, `csharp.rs`) into per-language source.

#### 5.1.1 Language level of the generated code

The iox2 emitters target the **oldest** level any consumer uses, because the
generated tree is frequently checked out inside a host project that compiles
everything in it automatically.

| Language | Target | Why |
|---|---|---|
| C# | **C# 9 / .NET Standard 2.1** | Unity 6 caps here and auto-compiles every `.cs` under `Assets/` |
| C++ | C++17 | |
| Rust | 2021 edition | |
| Python | 3.8+ | |

Two C# constructs are specifically avoided, both of which used to break Unity
with one error per generated file:

- **File-scoped namespaces** (`namespace X;`) are C# 10 → `error CS8773`.
  Block-scoped namespaces cost one indent level and compile everywhere.
- **`[InlineArray(N)]`** is .NET 8 / C# 12. A repeated-message field instead
  emits a wrapper struct with `N` explicitly-named fields under
  `LayoutKind.Sequential, Pack = 1`. That is the **same memory layout** —
  `[InlineArray]` is compact syntax, not a different representation — plus a
  `ref` indexer, so `arr[i]` reads identically at the call site.

Nothing emitted is newer than C# 9, so the output stays valid on modern .NET
as well. Targeting down costs nothing; targeting up breaks a host.

`proto/` C# is a separate matter: it needs a `Google.Protobuf` assembly,
which a Unity project must vendor itself. `sb` cannot supply that through
codegen.

#### 5.1.2 C# array wrappers — `iox2/_arrays/`

C# `fixed` buffers accept primitives only, so a `repeated <Message>` field
cannot be expressed inline the way Rust's `[T; N]` and C++'s `T[N]` can. It
needs a **named wrapper type** — the one construct in the iox2 output that
exists in C# and nowhere else.

A named type may be declared exactly once per assembly, and the generated
tree is one assembly (Unity compiles everything under `Assets/` together).
So each wrapper is emitted **once**, into its own file:

```
<targets>/iox2/_arrays/swarmbotix_primitives_Vec3Msg_Array256.cs
```

| | |
|---|---|
| Namespace | `sb_iox2_arrays` — never a message namespace |
| Type name | `<element pkg>_<Leaf>_Array<capacity>` |
| One file per | `(element type, capacity)` pair |
| Referenced as | `global::sb_iox2_arrays.<Name>` from the consuming struct |

```csharp
namespace swarmbotix_services {
    public unsafe struct SrvOMRWallMsg {
        public global::sb_iox2_arrays.swarmbotix_primitives_Vec3Msg_Array256 points;
        public uint points_count;
    }
}
```

Element access is unchanged — `msg.points[i]` — via a `ref` indexer on the
wrapper. Only code that *names* the type needs the `sb_iox2_arrays`
qualification.

Dedup is by output path, not bookkeeping: two messages sharing an element
type produce the same path with byte-identical contents, so the second write
is a no-op. That is what keeps `sb message compile <one-message>` correct —
it re-emits the wrappers that message needs without disturbing any other.

> **Before 0.1.39** the wrapper was emitted inline in every message that
> referenced it. Two messages in one package holding the same
> `repeated <Message>` therefore declared the same struct twice in that
> package's namespace: `error CS0101`, plus `CS0111` and `CS0579` on the
> duplicated members and `[StructLayout]`. Regenerate with `sb message
> compile` to clear it; `sb doctor` flags trees still holding the old shape.

**Flattening rules**:

| Proto field | iox2 storage |
|---|---|
| Primitive scalar (`int32`, `double`, etc.) | mapped to native scalar (`i32`, `f64`, …) |
| `string` | `[u8; STRING_CAP]` (null-terminated) |
| `bytes` | `[u8; BYTES_CAP]` + `<name>_len: u32` |
| `repeated <scalar>` | `[T; VEC_CAP]` + `<name>_count: u32` |
| `repeated <message>` | `[FlatChild; VEC_CAP]` + `<name>_count: u32`; child also flattened as its own struct |
| `repeated string` | `[[u8; STRING_CAP]; STRING_ARRAY_CAP]` + `<name>_count: u32` |
| Singular `<message>` | inlined into parent (each child field gains `<field>_` prefix) |
| `enum` | mapped to `i32` |
| `oneof` | not supported (compile errors) |
| `map<K,V>` | not supported (compile errors) |

**Language output**:

- **Rust** (`<Leaf>.rs`): `#[repr(C)]`, derives `Debug + Clone + Copy
  + PartialEq`, `Default` via `unsafe { mem::zeroed() }`. Comment hint
  to add `#[derive(iceoryx2::prelude::ZeroCopySend)]` when compiling
  against iceoryx2 itself.
- **C++** (`<Leaf>.h`): plain struct in a `namespace <pkg>` (sanitized
  — `swarmbotix.std` → `swarmbotix_std` to avoid colliding with C++'s
  `std`). Header-guard form: `SB_GEN_<PKG>_<LEAF>_H`.
- **Python** (`<Leaf>.py`): `ctypes.Structure` subclass with
  `_pack_ = 1` and `_fields_` listing every field by name + ctype.
- **C#** (`<Leaf>.cs`): `[StructLayout(LayoutKind.Sequential, Pack=1)]
  public unsafe struct`. Uses `fixed byte`/`fixed <prim>` buffers for
  primitive arrays; repeated POD-struct fields reference a shared wrapper
  from `iox2/_arrays/` (§5.1.2). Requires
  `<AllowUnsafeBlocks>true</AllowUnsafeBlocks>`.

### 5.2 `proto` (protoc native plugins)

**Pipeline**: per source, invokes
```
protoc --proto_path=<defs>
       --cpp_out=<staging>
       --python_out=<staging>
       --csharp_out=<staging>/<pkg>
       <pkg>/<Leaf>.proto
```

C# output is directed into a per-package staging subdir because
`protoc`'s C# generator otherwise dumps files flat by leaf name —
which would clobber same-leaf protos like `std/Header` vs
`std_msgs/Header` against each other. After all sources are processed,
files are relocated to `<out>/proto/<pkg>/<Leaf>/<filename>`.

**Files emitted** per message:

| File | From |
|---|---|
| `<Leaf>.pb.cc` + `<Leaf>.pb.h` | `--cpp_out` |
| `<Leaf>_pb2.py` | `--python_out` |
| `<Leaf>.cs` | `--csharp_out` |

No Rust output — there's no first-party `protoc-gen-rust`. Use the
iox2 backend or `prost-build` in a user crate for Rust protobuf.

### 5.3 `fb` (FlatBuffers via `flatc`)

**Pipeline** (three phases):

1. `flatc --proto` converts every source `.proto` into a `.fbs` in a
   per-package staging subdir. Per-package staging is required because
   `flatc --proto -o` dumps files flat by leaf name and would otherwise
   clobber duplicate leaves the same way protoc's C# generator does.
2. The proto-import graph is reconstructed by parsing each `.proto`'s
   `import` statements transitively. This builds the exact set of
   package dirs each `.fbs` needs as `-I` paths for the language step
   (skipping unrelated packages avoids the namespace ambiguity where
   two packages declare a `Header.fbs` and the wrong one gets resolved).
3. For each `.fbs`, `flatc --rust --cpp --python --csharp -o <dest>` is
   invoked with the precise `-I` set. flatc's namespace-driven output
   subdirs are flattened so every file for one message ends up directly
   under `<out>/fb/<pkg>/<Leaf>/` (the `__init__.py` namespace markers
   are dropped during the flatten pass).

**Files emitted** per message:

| File | From |
|---|---|
| `<Leaf>_generated.h` | `--cpp` |
| `<Leaf>_generated.rs` | `--rust` |
| `<Leaf>.py` | `--python` |
| `<Leaf>.cs` | `--csharp` |

---

## 6. Capacity configuration

### 6.1 The four caps

| Cap | Applies to | Built-in default |
|---|---|---|
| `string_capacity` | per `string` field (incl. each element of `repeated string`) | 256 bytes |
| `bytes_capacity` | per `bytes` field | 4096 bytes |
| `vec_capacity` | per `repeated <scalar/message>` field (element count) | 256 |
| `string_array_capacity` | per `repeated string` field (element count) | 10 |

All caps are u32. There's no special meaning for zero — treat them as
strictly positive.

### 6.2 Sources, by precedence

For every cap except per-field `string_array_caps` entries (covered
separately below), the value used at compile time is taken from the
**first** source that supplies a non-`None` value:

1. CLI flag (`--string-cap`, `--bytes-cap`, `--vec-cap`,
   `--string-array-cap`) — applies to the entire `sb message compile`
   invocation.
2. Workspace `sb.config.yml` (`string_array_cap` only at L1; other caps
   are CLI-only — they'd be additive to add).
3. Global `~/.swarmbotix/sb.config.yml` (same key set as workspace).
4. Built-in default from `CodegenConfig::default()`.

### 6.3 Per-field overrides — `bytes_caps`, `vec_caps`, `string_array_caps`

Three maps, one key shape: `<proto_pkg>.<Msg>.<field>`. Each wins over its
global fallback.

| Key | Overrides | Units |
|---|---|---|
| `bytes_caps` | `bytes_capacity` | bytes |
| `vec_caps` | `vec_capacity` | elements |
| `string_array_caps` | `string_array_capacity` | elements |

A single global number cannot serve a whole vault: `vec_capacity: 256`
gives a fixed 3×3 matrix a 256-element array **and** truncates a lidar
sweep. Shipped defaults pin the exact sizes:

```yaml
vec_caps:
  swarmbotix.primitives.Mat33.data:            9
  swarmbotix.primitives.Mat44.data:           16
  swarmbotix.sensors.LaserScan.ranges:      1080
  swarmbotix.sensors.LaserScan.intensities: 1080
bytes_caps:
  swarmbotix.sensors.PointCloud.data:    2097152
```

### 6.4 `iox2_variants` — one schema, many fixed-size types

A variable-length payload has **no single correct iceoryx2 size**. An image
is 230 KB at 360p mono8 and 6 MB at 1080p rgb8; sizing one struct for the
worst case wastes shared memory on every small frame, and sizing it for the
common case makes large frames impossible.

`iox2_variants` resolves this without duplicating the schema. One `.proto`
fans out into several iox2 types at build time:

```yaml
iox2_variants:
  <proto_pkg>.<Msg>:
    <OutputTypeName>:
      <field>: <capacity>
```

Shipped for `images/Image` — capacity is `width × height × bytes_per_pixel`:

| iox2 type | w × h × bpp | `data` |
|---|---|---|
| `Image360pMono8` | 640 × 360 × 1 | 230,400 |
| `Image360pRgb8` | 640 × 360 × 3 | 691,200 |
| `Image480pMono8` | 640 × 480 × 1 | 307,200 |
| `Image480pRgb8` | 640 × 480 × 3 | 921,600 |
| `Image720pMono8` | 1280 × 720 × 1 | 921,600 |
| `Image720pRgb8` | 1280 × 720 × 3 | 2,764,800 |
| `Image1080pMono8` | 1920 × 1080 × 1 | 2,073,600 |
| `Image1080pRgb8` | 1920 × 1080 × 3 | 6,220,800 |

Rules:

- **The base type is not emitted for iox2.** `images/Image` produces eight
  structs and no `Image`. The base is exactly the type with no correct
  size, so referencing it is always a mistake — the codegen makes it
  impossible rather than documenting it.
- **`proto/` and `fb/` are unaffected.** They carry dynamic lengths, so
  `Image` exists there normally. Variants are purely an iox2 concern.
- **Shared dependencies are emitted once.** All eight variants embed
  `header/Header`; it appears in the output a single time.
- **`sb pub add -m swarmbotix/images/Image480pMono8` works.** Variants have no
  `.proto`, so vault lookup would miss them; `validate_msg_type` resolves
  a name against `iox2_variants` before failing.
- Add a resolution by adding an entry here — **never** another `.proto`.

### 6.5 Per-field `string_array_caps`

`repeated string` is the one cap where per-field tuning is most useful
(e.g. `sensor_msgs.JointState.name` realistically needs 30+ slots for a
humanoid, while `MultiDOFJointState.joint_names` rarely needs more
than a handful). The config supports a map keyed by
`<pkg>.<MsgName>.<field>`:

```yaml
# ~/.swarmbotix/sb.config.yml

string_array_caps:
  sensor_msgs.JointState.name:                          32
  sensor_msgs.MultiDOFJointState.joint_names:           16
  trajectory_msgs.JointTrajectory.joint_names:          32
  trajectory_msgs.MultiDOFJointTrajectory.joint_names:  16
  visualization_msgs.InteractiveMarkerUpdate.erases:    32
```

The five entries above are also shipped as `SbCliConfig::builtin_defaults`
so a host with no `sb.config.yml` still gets sensible numbers.

Resolution order for a single `repeated string` field at compile time:

1. `--string-array-cap N` CLI flag (uniform override for the run, applies
   to every `repeated string` field encountered).
2. Per-field entry in `string_array_caps`.
3. Global `string_array_cap` (the single-value fallback).
4. Built-in default (10).

The key format intentionally matches the legacy skip-warning's
`{pkg}.{msg}.{field}` shape so values copied from old logs map 1:1.

### 6.6 Worked example

A field declared as
```protobuf
repeated string joint_names = 1;
```
inside `trajectory_msgs.JointTrajectory`, compiled with the default
config, emits (across the four iox2 languages):

```rust
// Rust
pub joint_names: [[u8; 256]; 32],
pub joint_names_count: u32,
```
```cpp
// C++
char joint_names[32][256];
uint32_t joint_names_count;
```
```python
# Python
("joint_names", (ctypes.c_char * 256) * 32),
("joint_names_count", ctypes.c_uint32),
```
```csharp
// C# — flat 1D buffer; byte layout matches [[u8;256];32]
public fixed byte joint_names[8192]; // [32][256]
public uint joint_names_count;
```

Bumping the per-field cap to 64 in `sb.config.yml` regenerates with
`[64][256]` storage everywhere.

---

## 7. Standard messages (`std/*` and the forge bundle)

The `std/*` namespace, plus the ROS2-mirror packages (`builtin_interfaces/`,
`geometry_msgs/`, `nav_msgs/`, `sensor_msgs/`, `shape_msgs/`,
`std_msgs/`, `stereo_msgs/`, `trajectory_msgs/`, `visualization_msgs/`,
`diagnostic_msgs/`), are shipped inside the `sb` binary as an embedded
bundle.

**On first run** (any `sb message *` command), `ensure_installed()`:
- Creates the vault root if missing.
- Copies every bundled `.proto` into its package dir under the vault,
  preserving the on-disk hierarchy.
- Skips files that already exist (so local edits to bundled messages
  survive subsequent runs).

**Protection rules**:
- `sb message new std/<X>` → refused. The `std/` directory is reserved.
- `sb message rm std/<X>` → refused. Removing a bundled message would
  break codegen for everything that imports it.
- `sb message edit std/<X>` → **allowed**. Edits persist locally; they
  do not propagate back into the forge bundle.

### 7.1 Fixed-resolution Image variants

Six pre-defined variants of `sensor_msgs/Image` ship for camera use
cases that need a known byte capacity baked into the type contract:

| Message | Resolution | RGB8 byte count |
|---|---|---|
| `sensor_msgs/Image480p` | 640 × 480 | 921600 |
| `sensor_msgs/Image640p` | 640 × 360 (nHD) | 691200 |
| `sensor_msgs/Image720p` | 1280 × 720 | 2764800 |
| `sensor_msgs/Image1080p` | 1920 × 1080 | 6220800 |
| `sensor_msgs/Image1440p` | 2560 × 1440 (QHD) | 11059200 |
| `sensor_msgs/Image2160p` | 3840 × 2160 (UHD) | 24883200 |

The schema is identical to `sensor_msgs/Image` (resolution lives only
in the type name + comments + suggested `--bytes-cap` invocation).
Compile each with its target byte count for iox2 use:

```bash
sb message compile --iox2 --bytes-cap 6220800 sensor_msgs/Image1080p
```

For non-RGB encodings (YUV420 = 1.5 B/px, NV12, 16-bit, etc.), adjust
the cap accordingly — the schema doesn't enforce an encoding.

---

## 8. Use cases

### 8.1 Add a brand-new custom message

```bash
sb message new ros2/robot_msgs/ArmGoal
```

A skeleton `.proto` is created and immediately compiled (empty struct in
every backend). You then edit fields:

```bash
sb message edit ros2/robot_msgs/ArmGoal     # opens $EDITOR, recompiles on save
```

Your downstream Rust crate / C++ project / Python script can already
import the bindings — they'll just be empty until you re-edit and let
the auto-recompile run.

### 8.2 Iterate on an existing schema

```bash
sb message edit ros2/sensor_msgs/CompressedImage
```

Same as 8.1 for an existing file. The editor opens, you edit, save,
exit; the auto-recompile updates **all three backends** for that
message. If `$EDITOR` is unset, defaults to `vi`; set it explicitly for
non-Unix workflows:

```bash
EDITOR=code-insiders sb message edit ros2/robot_msgs/ArmGoal
```

### 8.3 Tune one field's iox2 storage permanently

Edit `~/.swarmbotix/sb.config.yml`:

```yaml
string_array_caps:
  robot_msgs.ArmGoal.tags: 8
```

Then rebuild:

```bash
sb message compile ros2/robot_msgs/ArmGoal
```

The next compile of `ArmGoal` will use `[[u8; 256]; 8]` for the `tags`
field, regardless of what `string_array_cap` says.

### 8.4 One-off cap override without touching config

```bash
sb message compile --iox2 --string-array-cap 128 robot_msgs/ArmGoal
```

The 128 applies for this invocation only; the next compile reverts to
config-driven values.

### 8.5 Compile only iox2 because protoc/flatc are slow / unavailable

```bash
sb message compile --iox2
```

iox2 is always available (in-tree emitter). proto and fb are skipped
entirely. If you later need them:

```bash
sb message compile --proto --fb
```

(Use `sb doctor` first to confirm `protoc` and `flatc` are configured.)

### 8.6 Remove a no-longer-needed custom message

```bash
sb message rm ros2/robot_msgs/OldDraft
```

Deletes the `.proto`, plus `<targets>/iox2/robot_msgs/OldDraft/`,
`<targets>/proto/robot_msgs/OldDraft/`, and
`<targets>/fb/robot_msgs/OldDraft/`. If `robot_msgs/` becomes empty
under any of those three backend roots, the empty package dir is
pruned too.

### 8.7 Recover from a stale partial build

You've been iterating with `--iox2` only, but now you ship to a
consumer that needs Python protobuf bindings. The `proto/` dir for the
edited messages is frozen on the old schema. Force a fresh full build:

```bash
sb message compile ros2/robot_msgs/ArmGoal           # drop --iox2 → all three run
```

Or, for a from-scratch rebuild that purges old generated content first:

```bash
rm -rf <targets>/{iox2,proto,fb}/robot_msgs/ArmGoal
sb message compile ros2/robot_msgs/ArmGoal
```

### 8.8 Redirect the codegen output for a CI artifact

```bash
sb message compile --out ./build/sb-bindings
```

Source `.proto` files stay under `<defs>`; `descriptor.bin` still
lands at `<defs>/.cache/descriptor.bin`. Only the three generated
trees relocate to `./build/sb-bindings/{iox2,proto,fb}/`.

---

## 9. Edge cases and gotchas

### 9.1 In-place overwrite, not clean wipe

`sb message compile` rewrites the files a backend would normally
produce, but never deletes "stranger" files that happen to live in the
same dir. In practice the standard pipeline only emits the four/three
files documented in §5, so the dir ends up consistent — but if a future
plugin or a manual experiment dropped extra files into one of the
per-message dirs, those will survive a recompile. Use `sb message rm`
to wipe deterministically.

### 9.2 Stale backends after `--iox2`-only rebuilds

Covered in §4.5 and §8.7. The CLI does **not** warn about stale `proto/`
or `fb/` bindings after an iox2-only rebuild. If your downstream
consumers might pick up the older bindings, prefer a full
`sb message compile <name>` (no backend flag) once you're done
iterating.

### 9.3 Tool gating

`protoc` is required for *any* `sb message compile`, even
`--iox2`-only, because the descriptor IR is what the iox2 emitter
consumes. If `protoc` is not set or its path is dead, compile fails
with:

```
error: protoc path not set in sb.config.yml — set it or run `sb doctor`
```

`flatc` is only required for `--fb`. Setting `--fb` without `flatc`
configured errors with the analogous flatc message.

### 9.4 Duplicate leaf names across packages

The vault legitimately contains messages like `std/Header` and
`std_msgs/Header`. Both compile cleanly; the codegen pipeline handles
the collision in two places:

- **protoc C#** output is staged into per-package subdirs (`<staging>/<pkg>/<Leaf>.cs`)
  so the two `Header.cs` files don't clobber each other.
- **flatc** is invoked with a transitive-import-aware `-I` list so the
  *right* `Header.fbs` is loaded per source — the wrong one's
  namespace would cause a `type referenced but not defined` error.

You generally don't need to do anything; just be aware that messages
are addressed by full `<style>/<namespace>/<Leaf>` everywhere, never by
bare leaf.

### 9.5 `repeated string` storage cost

Each `repeated string` field consumes
`string_capacity × string_array_capacity` bytes — for the defaults
(256 × 10 = 2560 B per field, more if overridden). Stacked on a struct
with several such fields, total size grows quickly. If you need to fit
into a tight shared-memory budget, tune the per-field map down for
fields you control, or split the message into smaller variants.

### 9.6 `repeated message` fields with deeply-nested children

When `flatten` inlines a singular nested message, every child field is
prefixed with the parent field name (`header.frame_id` becomes
`header_frame_id`). When the nested message is *repeated*, the child is
emitted as a sibling top-level struct in the same file and the repeated
field becomes a fixed-size array of that struct. This means a message
graph with deep nesting can produce a single file containing several
struct definitions; this is intentional and documented as part of the
flat-IR design.

### 9.7 `oneof` and `map<K,V>`

Both are explicitly rejected by the flatten step with
`FlattenError::OneofUnsupported` / `FlattenError::MapUnsupported`. The
proto and fb backends would technically accept them (the upstream tools
support both), but the compile step happens after flattening fails, so
you can't get partial output. Refactor the schema or skip iox2 for
those messages until L2+ adds support.

### 9.8 Two `--help` strings are stale (as of sb 0.1.40)

The binary's own help text lags the layout in two places. This document
is right and `--help` is wrong:

| Where | `--help` says | Reality |
|---|---|---|
| `sb message --help` | the vault is `~/.swarmbotix/message_definitions/` | the flat layout is pre-0.1.31. It is `~/.swarmbotix/messages/<style>/message_definitions/` (§1) |
| `sb message compile --help` | `--out` defaults to `<vault>/targets/` | it defaults to `<messages_root>/<style>/message_targets/` (§3.2) |

Both are cosmetic — no behavior depends on them — but a reader who
trusts `--help` will look in a directory that does not exist. `sb doctor`
and `sb config get messages_root` print the real paths.
