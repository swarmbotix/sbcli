# L1 — Foundation, Messages & Doctor (running report)

Source of truth for L1 progress. Update on every state change.

Last updated: 2026-05-22

> **Reading this later:** the command transcripts below are verbatim from
> that date and use the **two-segment** message names (`std/Header`) that
> `sb` accepted at the time. Names became fully qualified
> (`<style>/<namespace>/<Leaf>`, e.g. `ros2/std/Header`) in **0.1.35** and
> the old form is now a hard error. Prefix with the style before pasting
> anything from here — see [documents/sbcli_messages.md](../documents/sbcli_messages.md) §2.

---

## Status: complete (initial pass)

All ten L1 TDD tests are covered. The "Done when" checklist in
[level1.html](level1.html) is satisfied on this dev box.

Run the full suite:

```bash
cargo test
```

70 tests pass (53 unit + 17 integration).

---

## What landed

### Workspace layout

```
swarmbotix/
  Cargo.toml                          ← workspace, 6 members
  rust-toolchain.toml                 ← stable
  message_definitions/std/            ← forge-bundled .proto source-of-truth
    Header.proto                      ← has metadata.msg_freq_desired (L4 hook)
    String.proto                      ← bare payload (mirrors std_msgs/String)
    StringStamped.proto               ← Header + data (ROS2 *Stamped convention)
    Vector3.proto
    Twist.proto
    TwistStamped.proto
    Image.proto
    ImageStamped.proto
  crates/
    sb-core/                          ← pure logic, zero IO
    sb-config/                        ← layered sb.config.yml loader
    sb-vault/                         ← embedded bundle + CRUD + protoc
    sb-iox2-typegen/                  ← .proto → flat iox2-compatible Rust/C++/Python
    sb-doctor/                        ← env validation
    sb-cli/                           ← clap binary (L1 verbs only)
```

### sb-core — pure logic

- [topic.rs](../crates/sb-core/src/topic.rs) — `TopicName` parser/validator with `TopicNameError` enum covering every failure mode (missing slash, wrong segment count, empty segment, illegal char). Serde-aware.
- [transport.rs](../crates/sb-core/src/transport.rs) — `Transport::{Zenoh, Iceoryx2}` with per-transport `wire_name()` (Zenoh keeps slashes; iceoryx2 transliterates `/` → `__`) and `tag()` for `sb topic list` (`[z]`/`[i]`).
- [config.rs](../crates/sb-core/src/config.rs) — serde types `SbCliConfig`, `ModuleDevConfig`, `ModulePrdConfig`, `WorkspaceFlow`, `PubSpec`, `SubSpec`, with `SbCliConfig::merge(env, workspace, global)` resolving the layering as a pure function. `deny_unknown_fields` everywhere so schema drift fails loudly.

### sb-config — IO + provenance

- [lib.rs](../crates/sb-config/src/lib.rs) — `LoadInputs::from_env()` reads `$SB_CONFIG` and the global `~/.swarmbotix/sb.config.yml`; `load()` returns the merged `SbCliConfig` + `ConfigSources` showing which path produced each layer. `set_key()` writes a single key with full parent-dir creation, used by `sb config set`.

### sb-vault — embedded bundle + CRUD

- [lib.rs](../crates/sb-vault/src/lib.rs) — `STD_BUNDLE` via `include_dir!` embeds `message_definitions/std/` into the binary. `Vault::ensure_installed()` copies missing files; existing files (including user-edited std/Header.proto) are never clobbered. `Vault::list/new_message/remove` provide CRUD; `MessageName::parse` enforces `<ns>/<PascalCase>` with strict validation. `compile_to_descriptor_set` shells out to protoc with `--include_imports --include_source_info`.

### sb-doctor — environment validation

- [lib.rs](../crates/sb-doctor/src/lib.rs) — one `CheckResult` per declared dependency (protoc, flatc, libzenohc, libiceoryx2, tmux, std vault). `[OK]/[FAIL]/[SKIP]` per line. Continues past failures (per L1 TDD #10). tmux uses `-V` rather than `--version` — that quirk caught during the first end-to-end run.

### sb-iox2-typegen — `.proto` → iceoryx2-compatible data definitions

- [crates/sb-iox2-typegen/](../crates/sb-iox2-typegen/) — new crate added after the initial L1 pass.
  Takes a `FileDescriptorSet`, flattens nested messages, and emits per-language
  data definitions where every struct is FLAT (no nested message fields) and every
  variable-length field has fixed-size storage. This is the contract iceoryx2 needs
  for zero-copy shared-memory IPC.
- **Flattening rules** ([crates/sb-iox2-typegen/src/flatten.rs](../crates/sb-iox2-typegen/src/flatten.rs)):
  - Primitives → mapped to per-language types.
  - `string` → `[u8; STRING_CAP]` (null-terminated, default 256).
  - `bytes` → `[u8; BYTES_CAP]` + companion `<field>_len: u32` (default 4096).
  - `repeated <scalar>` → `[T; VEC_CAP]` + `<field>_count: u32` (default 256).
  - Singular message field → fields inlined into parent with `<field>_<child>` naming, recursively.
  - `repeated <message>` → target flattened into its own struct, then `[FlatT; VEC_CAP]` + `_count`.
  - Enums → `i32`.
  - `oneof`, `map<K,V>` → flattening errors (incompatible with iceoryx2 fixed layout).
  - Recursion-depth limit 16; well-known guard for cycles.
- **Emitters**:
  - [rust.rs](../crates/sb-iox2-typegen/src/rust.rs) — `#[repr(C)]` + `Debug/Clone/Copy/PartialEq` derives, `impl Default` via `mem::zeroed()` (safe — all fields POD). Comment hint to add `#[derive(iceoryx2::prelude::ZeroCopySend)]` when compiling against iceoryx2.
  - [cpp.rs](../crates/sb-iox2-typegen/src/cpp.rs) — `<cstdint>` + plain POD struct, header guard, namespace = sanitized proto package (so the forge `std/` doesn't collide with C++ `::std`).
  - [python.rs](../crates/sb-iox2-typegen/src/python.rs) — `ctypes.Structure` with `_pack_ = 1`, `c_uint32 / c_char * N / ...` field mapping.
- **CLI surface** ([main.rs](../crates/sb-cli/src/main.rs)) — folded into the
  existing `sb message compile` verb. `compile` is the single entry point for
  format-emitting work (modeled as `protoc` / `flatc` / `sb-iox2-typegen` all
  hanging off the same .proto source):
  ```
  sb message compile [<vault-name>] [--out <dir>] \
      [--iox2 | --proto | --fb] \           # pick one, multiple, or none
      [--string-cap N --bytes-cap N --vec-cap N]
  ```
  - Always writes the FileDescriptorSet IR to `<vault>/.cache/descriptor.bin`.
  - **Codegen output defaults to the message vault** (e.g. `~/.swarmbotix/message_definitions/`)
    so generated files sit next to their source `.proto`. Pass `--out <dir>` to
    redirect them elsewhere (e.g. into a project's `src/generated/` for build systems).
  - **No flag = run every available format.** iox2 is always emitted; `proto` runs
    if `protoc` is set in sb.config.yml; `fb` runs if `flatc` is set. An explicit
    `--proto` / `--fb` errors with an actionable message when its tool isn't configured.
  - Single positional `<name>` (e.g. `std/Header`) filters to one message; omit for the whole vault.
  - Per-format outputs (all alongside the source `.proto` in `<out>/<vault-dir>/`):

    | Flag | Tool | Files emitted (per message) |
    |---|---|---|
    | `--iox2` | (built-in via sb-iox2-typegen) | `<Type>.rs`, `<Type>.h`, `<Type>.py` — flat zero-copy types |
    | `--proto` | configured `protoc` | `<Type>.pb.h`, `<Type>.pb.cc`, `<Type>_pb2.py` — standard protobuf bindings |
    | `--fb` | configured `flatc` | `<Type>_generated.rs`, `<Type>_generated.h`, plus a `swarmbotix/<pkg>/` Python package — FlatBuffers bindings (via the `.proto → .fbs` intermediate) |

### sb-cli — `sb` binary

- [main.rs](../crates/sb-cli/src/main.rs) — clap wrapper. L1 verbs:
  - `sb doctor`
  - `sb message list / new <Name> / rm <Name> / compile [<Name>] [--out <dir>] [--iox2|--proto|--fb] [cap overrides]`
  - `sb config open` (alias for the requirements.md "open in $EDITOR") and `sb config set <key> <value> [--workspace]`
- `~/` in `message_definitions` is expanded against `dirs::home_dir`; respects `$SB_CONFIG` and `$HOME` for hermetic tests.

---

## TDD test → location map

| #  | Spec                                  | Where                                                                                                                                |
| -- | ------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------ |
| 1  | TopicName table test (20+ fixtures)   | [crates/sb-core/src/topic.rs#L130](../crates/sb-core/src/topic.rs) — `mod tests`                                                        |
| 2  | YAML round-trip per config type       | [crates/sb-core/src/config.rs#L184](../crates/sb-core/src/config.rs) — `yaml_fixed_point`                                               |
| 3  | `SbCliConfig::merge` + env override   | [crates/sb-core/src/config.rs#L243](../crates/sb-core/src/config.rs)                                                                    |
| 4  | Vault CRUD                            | [crates/sb-vault/src/lib.rs](../crates/sb-vault/src/lib.rs) + [tests/level1_first_run_install.rs](../crates/sb-cli/tests/level1_first_run_install.rs) |
| 5  | Install-on-first-run                  | [tests/level1_first_run_install.rs](../crates/sb-cli/tests/level1_first_run_install.rs)                                                 |
| 6  | protoc invocation                     | [tests/level1_protoc_compile.rs](../crates/sb-cli/tests/level1_protoc_compile.rs)                                                       |
| 7  | Header / String split fixture         | [crates/sb-vault/src/lib.rs](../crates/sb-vault/src/lib.rs) — `header_contains_msg_freq_desired`, `string_has_no_header_field`          |
| 8  | Every std/* compiles to Rust          | [tests/level1_std_compiles_to_rust.rs](../crates/sb-cli/tests/level1_std_compiles_to_rust.rs) (prost-build)                             |
| 9  | Vault lookup API                      | [tests/level1_vault_lookup.rs](../crates/sb-cli/tests/level1_vault_lookup.rs) (FileDescriptorSet round-trip)                            |
| 10 | `sb doctor` happy + failure modes     | [tests/level1_doctor_happy_and_failures.rs](../crates/sb-cli/tests/level1_doctor_happy_and_failures.rs)                                 |
| +  | iceoryx2 codegen (Rust/C++/Python)     | [crates/sb-iox2-typegen/src/{flatten,rust,cpp,python}.rs](../crates/sb-iox2-typegen/src/) + [tests/level1_codegen.rs](../crates/sb-cli/tests/level1_codegen.rs) |

---

## End-to-end verification (this dev box)

```text
$ sb doctor
[OK]   protoc       /opt/protobuffer/protoc-35.0-linux-x86_64/bin/protoc (libprotoc 35.0)
[OK]   flatc        /opt/flatbuffers/Linux.flatc.binary.g++-13/flatc (flatc version 25.12.19)
[OK]   libzenohc    /opt/zenoh-c/current/lib/libzenohc.so (dlopen OK)
[OK]   libiceoryx2  /opt/iceoryx2/current/lib/libiceoryx2_ffi_c.so (dlopen OK)
[OK]   tmux         /usr/bin/tmux (tmux 3.4)
[OK]   std vault    /home/el/.swarmbotix/message_definitions/std populated
```

```text
$ sb message list
std/
  Header
  Image
  ImageStamped
  String
  StringStamped
  Twist
  TwistStamped
  Vector3

$ sb message compile std/Header
wrote /home/el/.swarmbotix/message_definitions/.cache/descriptor.bin

$ sb message new custom/Foo
created /home/el/.swarmbotix/message_definitions/custom/Foo.proto

$ sb message rm std/Header
error: refusing to delete std/Header — std messages ship with the forge
```

---

## Known gaps / follow-ups (not blocking L1)

- **iceoryx2 0.8.1 vs 0.9 plan mismatch.** `~/.swarmbotix/sb.config.yml` already flags the system `libiceoryx2_ffi_c.so` is v0.8.1, but the L3+ Rust crate plan pins `iceoryx2 = "0.9"`. Reconcile before L3 codegen starts shelling out to iceoryx2 APIs — either upgrade the system library or relax the crate pin.
- **`sb config open` UX.** Falls through to `$EDITOR` ↦ `vi`. Adequate for L1 but no per-workspace override path (requirement: "Edits land in `~/.swarmbotix/sb.config.yml` unless invoked inside an active workspace"). Workspace concept lands at L5.
- **`--workspace` on `sb config set`.** Stubbed out with a clear "not yet implemented" error. Wire up at L5 alongside the workspace concept.
- **Coverage instrumentation.** `cargo tarpaulin` / `cargo-llvm-cov` not yet wired up. The plan asks for ≥ 90% on parser/serde/vault. Spot-checking: every public surface has at least one unit test and most have failure-mode tests; formal coverage measurement is a follow-up.
- **CI.** No `.github/workflows/` yet. First-run install test currently runs as part of `cargo test`; needs a fresh-container job to satisfy the "First-run install verified in CI" bullet.

---

## Side-quest: ROS2 `common_interfaces` mirror (2026-05-22)

User dropped `common_interfaces/` (ROS2 messages source; a scratch drop, never tracked — the link here was always dead) at the
repo root as a temporary scratch dir and asked for every `.msg` to be converted
to `.proto`, keeping the ROS2 folder layout. Outcome:

- **Converter:** a one-off Python script parsed .msg, mapped primitive types to
  proto3, handled fixed/bounded/dynamic arrays, collapsed `uint8[]` → `bytes`, and
  preserved constants/comments as `//` annotations. Deleted after the conversion
  shipped — output is the source of truth, retrievable from git history if needed.
- **Output:** 120 `.proto` files under `message_definitions/<pkg>/<Type>.proto` across
  9 packages: `std_msgs`, `geometry_msgs`, `sensor_msgs`, `nav_msgs`,
  `diagnostic_msgs`, `shape_msgs`, `stereo_msgs`, `trajectory_msgs`, `visualization_msgs`.
  The intermediate `msg/` directory was dropped — package directories hold the
  `.proto` files directly. Each file declares `package <pkg>;` (no namespace
  prefix). No collision with the forge-bundled `swarmbotix.std` namespace because
  the package names differ (`std_msgs` vs `swarmbotix.std`).
- **protoc validation:** Bulk-compiled all 120 files with `protoc --descriptor_set_out`:
  - 60 compile cleanly.
  - 60 fail with the *single* error `Import "builtin_interfaces/msg/Time.proto" was not found`.
    `builtin_interfaces` is a separate ROS2 package (not part of `common_interfaces`);
    the dangling imports are documented behavior in the converter docstring.
  - 0 other errors.

### Layout implications for the vault

The flattened files live at depth 2 (`message_definitions/<pkg>/<Type>.proto`) — the same
depth as the forge-bundled `message_definitions/std/<Type>.proto`. `sb-vault::Vault::list`
walks at `min_depth(2).max_depth(2)`, so when `message_definitions` points at the
repo's `message_definitions/`, `sb message list` enumerates all 128 messages (120 converted
+ 8 forge `std/*`) as `<pkg>/<Type>` (e.g. `std_msgs/Header`,
`geometry_msgs/Twist`, `sensor_msgs/Image`). `MessageName::parse` already
accepts this form.

The default user vault at `~/.swarmbotix/message_definitions/` is populated by
`include_dir!` which embeds only `message_definitions/std/` today; the ROS2-converted
packages live on disk in the repo but are NOT yet embedded in the `sb` binary.

### Follow-ups

1. Drop `builtin_interfaces/Time.proto` + `Duration.proto` into the tree so the
   remaining 60 files compile. (Both are trivial: `int32 sec` + `uint32 nanosec`.)
2. Decide whether the ROS2-converted messages should be `include_dir!`-embedded
   in the `sb` binary alongside the forge `std/*`. Right now only `std/*` is
   embedded; the converted set lives on disk only.

---

## What L2 inherits from L1

These primitives are now stable and L2 should consume them as-is:

- `sb_core::TopicName`, `Transport`, `SbCliConfig`, `ModuleDevConfig`, `WorkspaceFlow`.
- `sb_config::load()` for any subcommand needing tool paths or transport defaults.
- `sb_vault::Vault` for any subcommand touching `.proto` files (scaffolding, codegen).
- The `std/Header.proto` `metadata.msg_freq_desired` field — locked in. L4 reads it from a single sample without a wire-format wrapper.

## Follow-up landed 2026-07-25 — message styles (0.1.31)

The vault was ROS2-shaped by assumption: one flat
`<sb_home>/message_definitions` + `<sb_home>/message_targets` holding
`std/` and 10 ROS2-mirror namespaces with no way to keep a second,
non-ROS2 message family beside it. Messages are now grouped into
**styles**.

**Layout.** `<messages_root>/<style>/{message_definitions,message_targets}`,
default `messages_root = <sb_home>/messages`. Two ship:

| Style | Namespaces | `.proto` |
|---|---|---|
| `ros2` | `std` + `builtin_interfaces`, `diagnostic_msgs`, `geometry_msgs`, `nav_msgs`, `sensor_msgs`, `shape_msgs`, `std_msgs`, `stereo_msgs`, `trajectory_msgs`, `visualization_msgs` | 138 |
| `swarmbotix` | `custom`, `robot_msgs` | 0 — scaffolding until its own Header defs land |

`std/` sits in **ros2** by design decision, not by accident: the split was
verified clean in both directions before the move (no ROS2 namespace
imports `std/`; `std/` imports only `std/`), so each style compiles
standalone. Styles are an **open set** — any subdirectory of
`messages_root` with a `message_definitions/` child is a style.

**Config.** Two new keys, `messages_root` + `message_style`; the paths
derive from them. `message_definitions` / `message_targets` survive as
explicit overrides that still win — the escape hatch tests and CI use.
`sb_config::resolve_message_{definitions,targets}` are now the single
source of truth; this deleted a **duplicated** `resolve_message_targets`
that had drifted between `sb-cli` and `sb-pubsub`. `--style <name>` is a
global flag sitting above every config layer. Style names reject `/`,
`\`, `:`, `.`, `..` so one can never resolve outside `messages_root`.

**Built-in default is `swarmbotix`; the installer seeds `ros2`.**
swarmbotix ships empty, and an empty active style means nothing to
compile or reference — the seeded value is an ordinary config layer
beating a built-in default, and is a one-line flip later.

**`std/*` bundle is style-scoped.** `Vault::for_style` gates
`ensure_installed` / `missing_std_files` on `owns_std_bundle()`, so the
embedded bundle only ever lands in ros2. `Vault::at` keeps the old
always-owns behavior for `$SB_CONFIG`-pinned vaults. `sb doctor` gained a
`message style` check and its `std vault` check reports `n/a` rather than
a permanent false failure on non-ros2 styles.

**Migration** (both installers): moves the flat tree into
`messages/ros2/`, drops `.cache/` and the stale `message_targets/`
(regenerated), deletes the old dirs, **and rewrites `sb.config.yml`** —
dropping only the legacy self-referential vault pins (an external pin is
left alone) and appending the two style keys, with a `.bak`. Without the
config half, the surviving explicit pins would have beaten the derived
path and pointed `sb` at just-deleted directories.

**Verified.** `cargo test --workspace` → **293 passed, 0 failed** (12 new
unit tests: 9 in `sb-config` for derivation/override/traversal/listing, 3
in `sb-vault` for bundle ownership). Test sandboxes now pin
`messages_root` + `message_style` and let the paths derive, so they
exercise the real resolution instead of hardcoded literals. On this
Windows box: fresh install, legacy-tree migration, and legacy-config
migration each smoke-tested in a sandbox `SB_HOME`, then installed for
real — 138 `.proto` migrated, 1658 bindings regenerated, `sb doctor` all
green, user's `protoc`/`device`/`bytes_caps` settings preserved.

Known cosmetic wart: the migrated `sb.config.yml` keeps the old
`# --- Vault ---` comment block, which now describes keys that are no
longer there. Harmless; the `.bak` and the shipped template both show the
intended shape.

## Follow-up landed 2026-07-25 — swarmbotix style content (0.1.32)

`swarmbotix/` went from empty scaffolding (`custom/`, `robot_msgs/`) to
four real namespaces. 14 `.proto`, all compiling on protoc 35.0.

| Namespace | Contents |
|---|---|
| `header/` | `Header` |
| `primitives/` | `Vector3`, `Vector4`, `Quaternion`, `Mat33`, `Mat44`, `Pose`, `Twist`, `Transform` |
| `images/` | `Image` — one schema, eight iox2 types |
| `sensors/` | `Imu`, `LaserScan`, `PointCloud`, `GnssFix` |

**Timestamp unit is load-bearing.** `Header.timestamp_ns` is UNIX
**nanoseconds** so every coarser unit is an exact integer divide (µs
`/1e3`, ms `/1e6`, s `/1e9`) — no float, no rounding drift. `uint64` ns
reaches year 2554. A publisher with only ms resolution multiplies up; it
does not change the unit.

### `iox2_variants` — one schema, N fixed-size types

The headline mechanism. A variable-length payload has **no single correct
iceoryx2 size**: an image is 230 KB at 360p mono8 and 6 MB at 1080p rgb8.
Sizing one struct for the worst case wastes shmem on every small frame;
sizing it for the common case makes large frames impossible. Previously
the only answer was one `.proto` per resolution (`sensor_msgs/Image1280x1024`).

Now `iox2_variants` in `sb.config.yml` maps `<pkg>.<Msg>` → output type
name → field → capacity, and `flatten()` re-runs the normal flattening
once per variant with those caps injected as per-field overrides:

| iox2 type | w × h × bpp | `data` |
|---|---|---|
| `Image360pMono8` / `Rgb8` | 640×360×1 / ×3 | 230,400 / 691,200 |
| `Image480pMono8` / `Rgb8` | 640×480×1 / ×3 | 307,200 / 921,600 |
| `Image720pMono8` / `Rgb8` | 1280×720×1 / ×3 | 921,600 / 2,764,800 |
| `Image1080pMono8` / `Rgb8` | 1920×1080×1 / ×3 | 2,073,600 / 6,220,800 |

Design decisions worth keeping:

- **The base type is not emitted for iox2.** `images/Image` yields eight
  structs and no `Image`. The base is precisely the size that is always
  wrong, so referencing it is made impossible rather than documented.
- **`proto/` and `fb/` are untouched** — they carry dynamic lengths, so
  `Image` exists there normally. Variants are purely an iox2 concern.
- **Shared dependencies emit once.** All eight variants embed
  `header/Header`; the `emitted` set keeps it single. Covered by a test.
- **`sb pub add -m images/Image480pMono8` resolves.** Variants have no
  `.proto`, so vault lookup misses them; `validate_msg_type` now consults
  `iox2_variants` (bridging proto FQN ↔ `<dir>/<Leaf>` on the last package
  segment) before failing.

### `vec_caps` — a real gap, not just new surface

`repeated <scalar>` had **one global capacity (256)** and no per-field
override, unlike `bytes` and `repeated string` which both had one. That
single number has to serve a fixed 3×3 matrix and a lidar sweep
simultaneously — it gave `Mat33.data` a 256-element array holding 9 values
and truncated any scan past 256 samples. `vec_caps` closes it with the
same `<pkg>.<Msg>.<field>` key shape. Pinned: `Mat33` 9, `Mat44` 16,
`LaserScan.ranges`/`intensities` 1080.

### Installer gap found during the release

The installer compiled only the **active** style, so `swarmbotix` shipped
with definitions and no bindings — broken the moment anyone switched to
it. Both installers now enumerate every style under `messages/` and
compile each, reporting per-style failures. Caught by watching the real
install output, not by a test.

**Verified.** `cargo test --workspace` → **301 passed, 0 failed** (8 new:
5 in `sb-iox2-typegen` for fan-out / base-suppression / dependency
de-duplication / scalar-field safety, 3 in `sb-config` for the shipped
capacity tables). End-to-end on this box: both styles compile, 8 image
variants generated with byte-exact capacities, `Mat33` is `[f64; 9]`,
`LaserScan.ranges` is `[f32; 1080]`.

**Deliberately not done — flag for a later decision.** "swarmbotix uses
full FlatBuffers" is satisfied at the *binding* level: the `fb` backend
emits FlatBuffers for every message and iceoryx2 needs no serialization at
all (flat POD). What is NOT done is changing the **L3 pub/sub templates**
to serialize FlatBuffers instead of protobuf on the zenoh path — that is
a published-API change across five languages and was out of scope here.
Authoring `.fbs` directly is also still unsupported; `sb-iox2-typegen`
is built on `prost_types::FileDescriptorSet` and would need a second
front-end (`flatc --bfbs` + a reflection reader).

## Follow-up landed 2026-07-25 — doctor lists every style (0.1.33)

`sb doctor` reported only the active style, mentioning the others in a
parenthetical `(also present: swarmbotix)` that was easy to miss. Inactive
styles are installed, compiled, and one `sb config set` away from being
live — but nothing in the report said anything about their state.

The `message style` check now emits one row per style under the messages
root, aligned beneath the message column:

```
[OK]   message style ros2 @ C:/Users/hylee/.swarmbotix/messages
                    * ros2         138 msg  targets built
                      swarmbotix    14 msg  targets built
                      (* = active)
```

The row that earns its keep is the failure one. A style with definitions
but no generated bindings previously looked identical to a healthy one:

```
                      swarmbotix    14 msg  NOT COMPILED — run `sb --style swarmbotix message compile`
```

That state was invisible until someone switched styles and codegen failed
against a missing target tree — exactly the gap the 0.1.32 installer fix
addressed from the other direction.

Implementation note: `CheckResult::name` is `&'static str` and the renderer
is one line per check, so the rows are a multi-line message with a 20-char
indent (`[OK]` + 3 + a 12-wide name + 1) rather than new check entries.

**Verified.** 304 passed, 0 failed (3 new `sb-doctor` tests: every style
listed, active marked exactly once, uncompiled style called out by name
with its fix command). Installed and confirmed on this box.

## Follow-up landed 2026-07-25 — swarmbotix is the default (0.1.34)

The 0.1.31 templates seeded `message_style: ros2` with the stated reason
"swarmbotix ships empty". 0.1.32 gave it 14 messages across four
namespaces, so that reason expired. Both shipped `sb.config.yml` templates
now seed `swarmbotix`, matching `DEFAULT_MESSAGE_STYLE` in `sb-core` —
one default, not two.

**The pre-0.1.31 migration branch deliberately still seeds `ros2`.** That
branch only fires for a flat-layout upgrade, whose entire vault just moved
into `messages/ros2/`; activating `swarmbotix` there would hide every
message the user already had. Preserving a working install beats matching
the new default, and `sb config set message_style swarmbotix` is one line.
Both installers carry that reasoning inline so it does not read as an
inconsistency.

This dev box was already pinned at `ros2` by its own 0.1.31 migration —
the installer never overwrites an existing `message_style` — so it was
flipped explicitly. Active vault is now the 14 swarmbotix messages, and
`std vault` correctly reports `n/a for style swarmbotix … std/ ships in
the ros2 style`.

Docs realigned: `documents/sbcli_config.md` §2.1, `sbcli_messages.md` §1
resolution order, `CLAUDE.md`, `requirements.md`. While editing CLAUDE.md
the four message-style sub-bullets had been orphaned from their parent by
earlier additions wedged between them — re-nested.

304 passed, 0 failed. Fresh-install seeding verified in a sandbox
(`message_style: swarmbotix`), then installed and flipped here.

## Follow-up landed 2026-07-25 — the "active style" mode is gone (0.1.35)

The style split shipped in 0.1.31 with a modal `message_style` config key:
one global setting every 2-segment message name resolved against. That was
wrong, and the counter-example is decisive — `Header.metadata.msg_type`
travels **on the wire** to consumers that share no configuration with the
publisher. `images/Image480pMono8` arriving in a header says nothing about
which schema produced the bytes; two styles may each define an
`images/Image`. A name whose meaning depends on the reader's config is not
an identity. Nor could one global mode describe a module that publishes
`swarmbotix/images/Image720pRgb8` while subscribing to
`ros2/sensor_msgs/Image` — an ordinary thing to do.

**Message names are now fully qualified: `<style>/<namespace>/<Leaf>.**
`MessageName` grew a `style` field; a 2-segment name is a hard error
carrying a teaching message rather than a lookup. There is no fallback
resolution, deliberately — a fallback is the ambiguity in another shape.

Consequences, each of which fell out of the rule rather than being bolted on:

- **`Vault` is rooted at `messages_root` and spans every style.** It gained
  `styles()`, `list_style()`, `defs_dir()`, `targets_dir()`; `Vault::at`
  no longer means "one style's directory" and `for_style` / `owns_std_bundle`
  are gone. `path_of` places a message inside its own style.
- **`sb-config` lost `resolve_message_style` / `resolve_style_dir` /
  `resolve_message_{definitions,targets}`.** In their place
  `message_definitions_for(cfg, style)` / `message_targets_for(cfg, style)`
  take the style **explicitly**, so a caller cannot accidentally depend on
  ambient state.
- **`sb-pubsub` resolves codegen targets from the message's own style**
  (`targets_for(cfg, msg)`), which is what makes the mixed-style module
  above actually work.
- **`compile_all` was deleted, not stubbed.** protoc takes exactly one
  include root, so styles must compile separately or one style's
  `import "std/Header.proto"` could bind to a same-named file in another.
  `compile_style(vault, style, …)` replaced it and `sb message compile`
  loops. A function that always errors would have been worse than none.
- **`FlatStruct` gained `style` + `qualified_name()`** while `vault_name` /
  `vault_dir()` stayed 2-segment: identity and output layout are different
  questions, and the targets root is already per-style.
- **The global `--style` flag is gone.** `--style` survives only on
  `sb message compile` as an explicit *filter*, and conflicts with a name
  argument (a name already says its style).
- **`sb message list` prints one fully-qualified name per line**, no
  `style/` + `namespace/` headers. Grouping under headers would make a line
  meaningful only relative to the header above it — the same ambiguity.
  Every line is pasteable into `sb pub add -m` and greppable.
- **`sb doctor`**: `message style` (singular, with an active marker) became
  `message styles` (all, no marker); `std vault` always checks ros2; a new
  `config keys` check fails while any dead key remains.

**Dead config keys.** `message_style`, `message_definitions`,
`message_targets` are still *parsed* — `deny_unknown_fields` would reject
an entire pre-0.1.35 file otherwise — but ignored, absent from
`KNOWN_KEYS`, stripped by both installers with a `.bak`, and flagged by
`sb doctor`. Parsed-but-silent is the combination that misleads; parsed-
and-loudly-reported is not.

**Caught by the smoke test, not by the suite:** both installers still
invoked the old global form `sb --style <s> message compile`, which now
fails to parse. The unit tests could not see it — it lives in a shell
script. Fixed to `sb message compile --style <s>`.

**Verified.** `cargo test --workspace` → **310 passed, 0 failed**. All 12
codegen goldens regenerated, each diff mechanically checked to be the style
prefix and nothing else before acceptance. On this box: upgrade stripped
the stale `message_style: swarmbotix`, both styles compiled, `sb doctor`
all green, `sb message list` shows 138 + 14 fully-qualified names.

---

## Standalone message repos — vault discovery by cwd shape

**Symptom.** Running `sb message compile` inside a standalone message repo
(`vrobots_msgs/`, with `message_definitions/` at its root) ignored the repo
entirely and rebuilt the global `~/.swarmbotix/messages` vault. There was no
way, short of `$SB_CONFIG` gymnastics, to say "build the tree I'm standing
in" — which is the whole point of a message repo that ships independently of
any workspace.

**Two independent bugs, either one sufficient to cause it:**

1. `discover_local_messages_root()` matched only a directory *literally
   named* `messages`, despite its own doc comment claiming it matched on the
   `<style>/message_definitions/` **shape**. A repo whose root *is* a style
   was invisible at every level of the walk.
2. `compile_style_pass` took its default output dir from
   `sb_config::message_targets_for(cfg, style)` — i.e. from config's
   `messages_root`, not from the vault actually in use. So even a correct
   discovery would have compiled local sources and written the bindings into
   the global tree.

**Fix.** `vault_for` now returns a `VaultCtx` carrying the vault plus its
provenance, and discovery matches three shapes (see requirements.md §Message
Vault). Provenance is load-bearing for two decisions:

- **`pinned_style`** — when cwd *is* a style, a bare `compile`/`list` touches
  only it. Without the pin the vault roots at cwd's parent and every sibling
  directory with a `message_definitions/` child would be swept in.
- **`local`** — the embedded `std/` bundle is installed into the configured
  global vault **only**. `ensure_installed()` on a discovered root is a no-op;
  previously it would have written an unasked-for
  `ros2/message_definitions/std/` tree into the user's repo (or its parent)
  on the first `sb message` call. This was latent before the fix too — the
  old `messages/`-named discovery had the same hole.

Output dirs now derive from `vault.targets_dir(style)` in both
`compile_style_pass` and `cmd_message_rm`, so sources and generated artifacts
can never straddle two roots.

`$SB_CONFIG` still short-circuits discovery — unchanged, and the reason the
whole integration suite was unaffected.

**Verified.** `cargo test` → full workspace green. 5 new unit tests in
`discovery_tests` cover each shape, the walk-up case, the no-std-leak
guarantee, and shape-vs-name matching. End-to-end on this box: in
`vrobots_msgs/`, `sb message list` shows 19 fully-qualified
`vrobots_msgs/...` names and `sb message compile` wrote 108 iox2 + 19 proto +
19 fb files into `vrobots_msgs/message_targets/`, leaving both the global
vault and the repo's parent untouched. In the forge, `sb message list` still
resolves `ros2` + `swarmbotix` from `messages/`.

### `sb doctor` reported a vault it was not going to use

Follow-on from the discovery fix above. `check_message_styles` and
`check_std_vault` both derived their root from `sb_config::resolve_messages_root(cfg)`,
so inside a standalone message repo `sb doctor` described
`~/.swarmbotix/messages` while every `sb message *` command operated on the
repo. A doctor that describes a different tree than the next command touches
is worse than no doctor.

`sb_doctor::run_with_vault(cfg, &VaultView { root, pinned_style, local })`
now takes the resolved vault from the caller; `run(cfg)` keeps the old
cfg-derived behavior for non-CLI callers. Two behavior changes follow:

- The `message styles` line is labeled `(discovered from cwd — overrides
  messages_root)` and, when cwd is itself a style, narrows to that style with
  an explicit `only it is in scope` note.
- `std vault` **skips** rather than fails on a discovered root with no `ros2`
  style. A standalone repo legitimately has no `std/`; the forge bundle is
  only ever installed into the configured vault.

Removed `run_owned(cfg, _vault_root)` — a compat shim that accepted a vault
root and silently ignored it, which is the same parsed-but-ignored trap the
dead config keys were cleaned up to avoid. It had no callers.

---

## Unity-incompatible codegen (ISSUE_unity_incompatible_csharp.md) — 0.1.37

**Reported:** `sb message compile` inside a Unity project emitted 27 iox2 C#
files that fail the host build with `error CS8773` ×27, plus 19 `proto/` files
needing a vendored `Google.Protobuf`. The workaround — remember `--fb` on every
invocation — silently re-breaks the project the one time you forget.

**Root cause is a policy gap, not a codegen bug.** `crates/sb-iox2-typegen/src/csharp.rs`
targets modern .NET on purpose: file-scoped namespaces (C# 10) and
`[InlineArray]` (.NET 8 / C# 12). Unity 6 caps at C# 9 / .NET Standard 2.1 and
`[InlineArray]` has no C# 9 equivalent, so the emitter can never target Unity as
written — and it does not need to: `sb pub add --iox2` **already refuses** Unity
because shared memory is unavailable on sandboxed game runtimes. `sb` was
blocking iceoryx2 at the pub/sub layer while shipping iceoryx2 bindings into the
same project at the message layer.

### Backend selection is now resolved, not assumed

Four rules, first match wins (requirements.md §Backend selection order):

1. Explicit `--iox2` / `--proto` / `--fb` — never second-guessed.
2. `backends:` in `<style>/sb.style.yml` — the persisted pin.
3. Host-project constraint — output inside a Unity project ⇒ `fb` only.
4. Tool availability — the historical default.

Rules 2 and 3 **print why**. A narrowed selection is never silent; silence is
what made the `--fb` workaround so easy to forget.

**The pin lives in the style tree, not `sb.config.yml`.** New file type
`sb.style.yml` (`sb_core::StyleConfig`), a sibling of `message_definitions/`,
written by `sb message backends <style> <backend>...`. A style is routinely a
standalone repo cloned onto machines that share no config — a pin that did not
travel would have to be re-made per box, which is the same "remember the flag"
failure one level up. `backends: []` is a real answer meaning "emit nothing
automatically", distinct from an absent key. A pin naming a backend whose tool
is missing errors rather than silently dropping it.

**Unity detection requires BOTH `Assets/` and `ProjectSettings/`** on some
ancestor of the *output* directory (`sb_vault::unity_project_root`). `Assets/`
alone is far too generic. Keying on the output dir rather than the style dir is
what makes `--out` outside the tree correctly lift the constraint.

**`sb doctor` gained `unity targets`** — warns, with the `rm -r` commands, when
a style's `message_targets/` sits under a Unity project and still holds `iox2/`
or `proto/`. Needed because fixing the emitter does not clean up a style
compiled by an older `sb`. It is a new `CheckStatus::Warn` (`[WARN]`, yellow)
that does **not** affect the exit code: the breakage is in the host project, and
failing would block every unrelated `sb doctor` until someone cleaned up.

### Second bug, found while verifying the first

`sb message compile --fb` was **not idempotent**. `flatten_namespace_subdirs`
moves flatc's namespace subdirs up into the per-message dir, but skipped the
move whenever the flat copy already existed — which is always true from the
second compile onward. The nested files stayed put, the subdir never emptied,
and the tree accumulated a complete duplicate set. In Unity that is a second
build failure (duplicate type definitions), arriving only on the *second*
compile, which is exactly why the generator looked fine in isolation.

Now overwrites instead of skipping: clear the destination first (`rename`
refuses an existing target on Windows) and remove the source after a `copy`
fallback, or the subdir stays non-empty. Verified stable at 94 files / 0 nested
dirs across three consecutive runs against the real `vrobots_msgs` style.

**Verified.** `cargo test --workspace` green. 14 new tests: backend resolution
(default, missing tools, Unity narrowing, explicit-flag override, `--out`
escape, pin precedence, empty pin, unavailable-tool error, marker strictness),
doctor `unity targets` (warn content, exit-code neutrality, fb-only clean,
non-Unity clean), and flattening idempotency across three runs. End-to-end in
`vrobots_msgs`: compile emits `fb` only with the reason printed,
`sb message backends vrobots_msgs fb` writes the pin, and `sb doctor` goes from
`[WARN]` to `[OK]` after cleanup.

### Correction to the above: narrowing was the wrong lever (0.1.38)

The 0.1.37 entry describes a host-project rule that dropped a Unity-hosted
style to `fb` only. That shipped and was **wrong**, and 0.1.38 removes it.

A style's `message_targets/` is consumed by more than the project it happens
to sit inside — `vrobots_msgs` lives under `Assets/` but feeds C++, Python and
Rust clients too. Suppressing `iox2/` and `proto/` to please the host silently
deprived every other consumer. The issue's suggestion 2 ("skip it rather than
emit it") is what that followed; it does not survive contact with a shared
message repo, which is the normal case, not the exception.

**Host compatibility belongs in the emitter, not in the selection.**
`csharp.rs` now targets C# 9 / .NET Standard 2.1 — the oldest level any
consumer uses:

- Block-scoped namespaces instead of file-scoped (C# 10 → `CS8773`).
- A repeated-message field emits a wrapper of `N` explicitly-named fields
  under `Sequential`+`Pack=1` instead of `[InlineArray(N)]` (.NET 8 / C# 12),
  plus a `ref` indexer. Layout is **identical** — `[InlineArray]` is compact
  syntax, not a different representation — so the wire format is unchanged and
  `arr[i]` reads the same at the call site.

Nothing emitted is newer than C# 9, so the output stays valid on modern .NET.
Targeting down costs nothing; targeting up breaks a host. A regression test
asserts the absence of every post-C#9 construct that has bitten this, in one
place, so a future emitter change cannot quietly reintroduce one.

Selection is back to: explicit flags → `sb.style.yml` pin → tool availability.
The pin survives as an opt-in for styles whose consumers genuinely read one
format; it is no longer the recommended answer for a Unity host.

Also removed: the automatic pruning of unselected backend dirs that 0.1.37
added. It existed only to clean up after the narrowing, and deleting a user's
committed generated code on a plain `compile` is not something to do on
inference.

`sb doctor`'s `unity targets` check was repurposed rather than dropped. Fixing
the emitter does not rewrite files already on disk, so it now detects
generated `.cs` under a Unity project that still carries a file-scoped
namespace or `[InlineArray]` — by **file content**, not directory name, so it
stays accurate whichever backends a style emits. The fix it prints is
`sb message compile`.

**Verified.** `cargo test --workspace` → 342 passed, 0 failed. End-to-end in
`vrobots_msgs`: all three backends emitted (108 iox2 + 19 proto + 19 fb), and
`grep` finds zero file-scoped namespaces and zero `InlineArray` across the
generated C#.

### Third bug in the same emitter: duplicate array wrappers (0.1.39)

Reported from `vrobots_msgs` after 0.1.38 landed, and unrelated to the
language level that 0.1.37/0.1.38 chased:

```
SrvOMRWallMsg.cs(29,26): error CS0101: The namespace 'swarmbotix_services'
                         already contains a definition for
                         'swarmbotix_primitives_Vec3Msg_Array256'
SrvOMRWallMsg.cs(28,6):  error CS0579: Duplicate 'StructLayout' attribute
```

Both diagnostics are **one** defect. `csharp.rs` collected the
`repeated <Message>` wrapper structs per *file* and appended them to that
file's body, inside the **consuming** message's namespace. Two messages in one
package holding the same element type therefore declared the same struct
twice. `CS0579` is Roslyn's follow-on report on the second declaration's
attribute, not a separate problem — reproduced exactly, in isolation, with a
two-file `dotnet build` at `LangVersion 9`.

Not specific to the reporter's style: the shipped **`ros2`** style had 7
duplicated wrapper names across 40 declarations, `std_msgs` alone declaring
`std_msgs_MultiArrayDimension_Array256` **12 times** (every `*MultiArray`
inlines `MultiArrayLayout`, whose `dim` field is the repeated one). Compiling
that tree as one assembly: 38 × `CS0101`, 38 × `CS0111`, 38 × `CS0579`.

Only C# is affected. Rust (`[T; N]`) and C++ (`T name[N]`) have native fixed
arrays; C#'s `fixed` buffers accept primitives only, so the wrapper is a real
named type — and a named type may be declared exactly once per assembly.

**The fix: one wrapper, one file.** `csharp::emit_array_wrappers` returns a
`GeneratedModule` per unique `(element type, capacity)`, written to
`iox2/_arrays/<pkg>_<Leaf>_Array<N>.cs` under namespace `sb_iox2_arrays`;
consumers reference `global::sb_iox2_arrays.<Name>`. The struct name is
unchanged, and `msg.points[i]` still reads the same via the `ref` indexer.

Dedup is **by output path**, not by bookkeeping — two consumers produce the
same path with byte-identical contents, so the second write is a no-op. That
property is what keeps `sb message compile <one-message>` correct: a
single-message run re-emits the wrappers it needs and cannot drop the ones
other messages depend on. A shared "collect everything, then write once" pass
would have broken exactly there.

`sb doctor`'s `unity targets` check gained a third marker alongside
file-scoped namespaces and `[InlineArray]`: a `_Array<N>` struct declared
*outside* `iox2/_arrays/`, i.e. a tree compiled before this fix. Recompiling
is the only thing that moves it.

**Verified.** `cargo test --workspace` green, including 4 new emitter unit
tests, 3 new integration tests (`level1_csharp_unique_types.rs`), and 2 new
doctor tests. The integration test asserts the general property the host
compiler actually enforces — no `(namespace, type)` pair declared in two files
— rather than checking for this one wrapper, so it holds against the next way
of breaking it. End-to-end on the `ros2` style: 602 files emitted, 25 shared
wrappers in `_arrays/`, zero duplicate declarations, and `dotnet build` at
`netstandard2.1` / `LangVersion 9` succeeds on the whole tree — the same tree
that fails with 114 errors when generated by 0.1.38.
