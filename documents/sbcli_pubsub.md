# `sb pub` / `sb sub` / `sb list` — Pub/Sub Reference

Official reference for the `sb pub`, `sb sub`, and `sb list`
subcommand surfaces: module resolution, validation, mutation
semantics, transport selection, conflict-policy flags, per-language
codegen, and the operational guarantees that govern when files are
written, regenerated, or removed.

This document is normative for behavior shipped at L3.

The canonical spec is `requirements.md` in the sbcli **source
repository** — a maintainer document that is not part of your install;
when it and this page disagree, it wins.

---

## 1. Conceptual model

`sb pub` and `sb sub` are **mutating** verbs that operate on a single
module's `sb.dev.yml`. Each `add`, `edit`, or `rm` does two things in
lockstep:

1. **Mutates the YAML.** Reads `sb.dev.yml`, modifies the
   `publishers:` or `subscribers:` list, writes back.
2. **Mirrors the change on disk.** Calls `sb-codegen` to render (or
   delete) the matching language file under
   `<module_root>/<io_dir>/{publishers,subscribers}/<name>.<ext>`.

If either half fails, the command exits non-zero. The two halves are
**not transactional** — a codegen failure after a YAML write leaves
the disk reflecting the post-mutation YAML but missing the generated
file (see §11 for partial-failure semantics).

`sb list`, `sb pub list`, and `sb sub list` are **read-only**.

The verb surface is:

```
sb list                            [--module <name>]
sb pub list                        [--module <name>]
sb pub add  -m <MsgType> <topic>   [-n <name>] [--iox2 | --zenoh]
                                   [--force | --skip-existing]
                                   [--module <name>]
sb pub edit <name>                 [-m <MsgType>] [-t <topic>]
                                   [--module <name>]
sb pub rm   <name>                 [--module <name>]
sb sub list                        [--module <name>]
sb sub add  -m <MsgType> <topic>   [--iox2 | --zenoh]
                                   [--force | --skip-existing]
                                   [--module <name>]
sb sub edit <topic>                [--module <name>]
sb sub rm   <topic>                [--module <name>]
```

`-m` is **reserved for `MsgType`**. Module selection uses the
long-only flag `--module <name>` to avoid the clash.

---

## 2. The two on-disk artifacts a mutation touches

Per module:

```
<module_root>/
├─ sb.dev.yml                                   # READ + WRITE
└─ <io_dir>/                                    # WRITE + DELETE
   ├─ publishers/
   │  └─ <name>.<ext>                           # one per publisher
   └─ subscribers/
      └─ <name>.<ext>                           # one per subscriber
```

`<io_dir>` defaults to `swarmbotix_io` (or `Assets/Scripts/swarmbotix_io`
for Unity) — see [sbcli_init.md](sbcli_init.md) §4. `<ext>` is the
language's natural extension: `rs`, `py`, `cpp`, `dart`, `cs`.

The IO dir contents are **deterministic codegen output**. They are
byte-stable for a given `(language, transport, kind, name, topic,
msg_type, device, workspace, module)` tuple — see §6.6 for the golden
snapshot suite. You may either commit them or gitignore + regenerate
at build time; sb has no opinion.

`sb.dev.yml` is the **source of truth** — if it and the generated
files disagree, the YAML wins on the next mutation (the file is
regenerated to match).

---

## 3. Module resolution

Every `sb pub/sub *` verb takes an optional `--module <name>` flag.
The lookup chain (highest priority first):

1. **`--module <name>` given AND `./<name>/sb.dev.yml` exists** in
   the current working directory.
2. **`--module <name>` given AND active workspace's `flow.yaml`
   has `<name>`** — the value is the absolute path to its
   `sb.dev.yml`.
3. **No `--module` flag → `./sb.dev.yml` in cwd**. Errors if absent.

If `--module <name>` is given and neither rule 1 nor rule 2 hits, the
CLI errors with **every path it tried**, including the dangling
`flow.yaml` entry if rule 2 missed because the registered path no
longer exists.

```
error: module "ghost" not found. Searched:
  - /current/dir/ghost/sb.dev.yml
  - /home/el/.swarmbotix/workspaces/demo/flow.yaml
```

If no `--module` is given and cwd has no `sb.dev.yml`:

```
error: no sb.dev.yml in cwd (/wrong/place). Pass --module <name> or `cd` into a module dir.
```

The resolver is `sb_pubsub::resolve_module`. Both lookups (rule 1
sibling, rule 2 workspace) require the resolved file to exist; a
registered-but-missing entry in `flow.yaml` is treated as a miss for
that rule, **and** the path is included in the error tail.

---

## 4. Schema — `PubSpec` and `SubSpec`

The `publishers:` and `subscribers:` lists in `sb.dev.yml` hold these
two record types (defined in `sb_core::config`):

```yaml
publishers:
  - name: image_raw
    topic: image_raw
    type: ros2/std/ImageStamped
    transport: iceoryx2
subscribers:
  - name: cmd_vel
    topic: /dev01/nav/teleop/zenoh/cmd_vel
    type: ros2/std/TwistStamped
    transport: zenoh
```

| Field | Type | Set by | Notes |
|---|---|---|---|
| `name` | identifier | `-n` flag or topic's last segment | Drives the generated filename + the struct name (PascalCase). Must satisfy `validate_identifier`. |
| `topic` | string | `<topic>` positional | Bare (`image_raw`) or fully-qualified (`/dev01/ws/cam/iox2/image_raw`). Codegen prepends the `/<device>/<workspace>/<module>/<transport>/` prefix only if no leading `/`. When fully-qualified, the `<transport>` segment must match the entry's `transport` field. |
| `type` | string | `-m` / `--msg` flag | Must parse as `<style>/<namespace>/<Leaf>` and exist in the vault (see §5.1). Stored verbatim, e.g. `ros2/std/ImageStamped`. Two-segment names are rejected — see [sbcli_messages.md](sbcli_messages.md) §2. |
| `transport` | enum (`zenoh` / `iceoryx2`) | `--zenoh` / `--iox2` or sb.config.yml default | See §7 for default resolution. |

`deny_unknown_fields` is on for both records; hand-edited fields outside
the schema are rejected on read.

`SubSpec` is identical in shape to `PubSpec`, but `sb sub *` keys
subscribers by **topic** rather than name (because subscribers are
fan-out and a single name doesn't uniquely identify one). Internally
the `name` is still derived (it's the generated filename) but the
user-facing API only ever takes the topic.

---

## 5. Validation

### 5.1 Message-type lookup

Every `add` / `edit` resolves `-m <MsgType>` through `sb_vault`:

1. Parses `<MsgType>` via `sb_vault::MessageName::parse`. Rejects
   anything that isn't `<style>/<namespace>/<PascalCase>` — three
   segments, no fewer (see [sbcli_messages.md](sbcli_messages.md) §2).
2. Calls `vault.list()` and checks the parsed name is present.

On miss:

```
error: message "ros2/std/Imagew" not in vault. Run `sb message list` to see available types.
```

The vault spans every style under `messages_root` (see
[sbcli_messages.md](sbcli_messages.md) §1). `-m` takes a **fully-qualified**
name, `<style>/<namespace>/<Leaf>`, and the generated file's iox2 payload
path resolves from that message's own style — which is what lets one module
publish `swarmbotix/images/Image720pRgb8` and subscribe to
`ros2/sensor_msgs/Image`. The list is the union of:

- Forge-bundled `std/*` and ROS2-mirror packages.
- Any user-created namespaces under the resolved
  `<message_definitions>/`.

No protoc / compile step runs — `sb pub add` is a metadata operation,
not a codegen one. It does **not** require the message to have been
`sb message compile`'d. The generated pub/sub file references the
type by name only; whether the user has Rust / Python / C++ bindings
ready for it at compile/runtime is independent.

### 5.2 Name + topic conflicts (collision check)

Each add scans the module's existing publishers + subscribers and
reports every entry that would collide on:

- **Same name** (publisher only — sub names are derived from topics).
- **Same topic** (across publishers AND subscribers — a topic can have
  at most one role per module unless the user explicitly opts out via
  `--force`; see §8).

If any collisions are found, the verb's behaviour depends on the
conflict policy resolved in §8. The default is to error.

### 5.3 Default-name derivation

When the user omits `-n` on `sb pub add`, the name is derived as the
**last `/`-separated segment** of the topic:

| `<topic>` | derived name |
|---|---|
| `hello` | `hello` |
| `/dev01/ws/cam/image_raw` | `image_raw` |
| `nested/path/leaf` | `leaf` |
| `/` | `/` (then fails identifier validation) |

`sb sub add` always derives the name this way (no `-n` flag).

### 5.4 Identifier validation on name

The derived (or `-n`-supplied) name must satisfy
`validate_identifier`. Mirror of the workspace/module rule —
non-empty, ASCII letter/`_` first, then alphanumerics/`_`.

---

## 6. Codegen

After `sb.dev.yml` mutation, the matching template renders into
`<io_dir>/{publishers,subscribers}/<name>.<ext>`.

### 6.1 Cell matrix

|          | Zenoh                  | iceoryx2               |
|----------|------------------------|------------------------|
| Rust     | pub ✓ · sub ✓          | pub ✓ · sub ✓          |
| Python   | pub ✓ · sub ✓          | pub ✓ · sub ✓          |
| C++      | pub ✓ · sub ✓          | pub ✓ · sub ✓          |
| Flutter  | pub ✓ · sub ✓          | — (mobile sandbox)     |
| Unity    | pub ✓ · sub ✓          | — (game sandbox)       |

The two `—` cells are structurally impossible. The CLI errors with a
different message for "not yet implemented" vs "impossible" (§11.3).

### 6.2 Template engine + layout

`sb-codegen` embeds `templates/` via `include_dir!`. Naming:

```
templates/<lang>/<{publisher,subscriber}>_<{zenoh,iceoryx}>.<ext>.j2
```

minijinja 2.x renders each template against the `TemplateCtx` struct
defined in `crates/sb-codegen/src/lib.rs`. Variables exposed:

| Variable | Example | Notes |
|---|---|---|
| `name` | `image_raw` | The spec's `name` field. |
| `struct_name_base` | `ImageRaw` | PascalCase of `name`. |
| `struct_name` | `ImageRawPublisher` / `ImageRawSubscriber` | `struct_name_base` + role suffix. |
| `module` | `pubber` | Module name from `sb.dev.yml`. |
| `workspace` | `demo` | Active workspace name. |
| `device` | `dev01` | From merged `sb.config.yml`. |
| `topic_full` | `/dev01/demo/pubber/iox2/image_raw` | Fully-qualified path including `<transport>` segment. If the user passed `--topic` with a leading `/`, used verbatim (and must already include the `<transport>` segment); otherwise prepended with `/<device>/<workspace>/<module>/<transport>/`. |
| `topic_zenoh` | `dev01/demo/pubber/zenoh/image_raw` | `topic_full` without the leading `/`. Zenoh KeyExpr forbids leading slashes. |
| `topic_iox` | `dev01/demo/pubber/iox2/image_raw` | Identical to `topic_zenoh`. iceoryx2 0.9 `ServiceName::new` accepts `/` (per upstream's own example `ServiceName::new("My/Funk/ServiceName")`), so no transliteration. Kept as a distinct name for template-side clarity. |
| `topic_bare` | `image_raw` | Last segment of the input topic (the transport segment is **not** the bare topic). |
| `transport_segment` | `iox2` / `zenoh` | The literal string used in the path. `iox2` for iceoryx2, `zenoh` for zenoh. |
| `msg_namespace` | `std` | The MsgType's namespace. |
| `msg_leaf` | `ImageStamped` | The MsgType's leaf. |
| `msg_type` | `ros2/std/ImageStamped` | Full vault name, all three segments. |
| `transport` | `zenoh` / `iceoryx2` | Transport tag. |
| `is_zenoh` / `is_iceoryx` | bool | For Jinja branching. |
| `is_publisher` / `is_subscriber` | bool | Mirrored from the kind. |
| `role` | `publisher` / `subscriber` | String form. |
| `is_owned` | bool | True when the user passed a bare topic, so the full path is built from device/workspace/module/transport/bare here at codegen. False when the user passed a leading-slash topic verbatim — in that case the path "belongs" to whoever owns it and per-segment overrides don't apply. Templates gate the `TopicOverrides` emission on this flag (§6.5). |

### 6.3 Topic naming: one canonical identity, one wire form per leading-slash rule

A pub/sub entry has **one canonical topic identity**, derived once
from `(device, workspace, module, transport, <topic positional>)` at
codegen time. The wire form for both transports is **identical** —
the canonical form with the leading `/` stripped. Only the
`<transport>` segment (`iox2` vs `zenoh`) differentiates two entries
with the same bare name.

**Canonical form** (what `sb list`, `sb topic *`, docs, log output,
and comments inside generated files all use):

```
/<device>/<workspace>/<module>/<transport>/<bare_topic>
```

`<transport>` is the literal `iox2` or `zenoh`, taken from the
entry's transport. This is `topic_full` in the template context. The
leading `/` is part of the canonical form. The one exception: if the
user passes a topic that already starts with `/` (e.g.
`/flat/path/hello`), the positional is used verbatim and the
`<device>/<workspace>/<module>/<transport>` prefix is **not**
prepended (see §13.7).

**The conversion rule** — applied deterministically at template
render time, no user input:

```
transport_segment = "iox2" if transport == iceoryx2 else "zenoh"
topic_full   = /<device>/<workspace>/<module>/<transport_segment>/<bare_topic>   # canonical
topic_zenoh  = topic_full[1:]                                                    # strip leading /
topic_iox    = topic_zenoh                                                       # same wire form
topic_bare   = topic_full.rsplit('/', 1)[-1]                                     # last segment
```

**Worked example** — `sb pub add -m ros2/std/StringStamped hello --iox2
--module app2_cpp` in workspace `ws1` on device `dev01`:

| Template var       | Value                                     | Where it appears                                                  |
|--------------------|-------------------------------------------|-------------------------------------------------------------------|
| `topic_full`       | `/dev01/ws1/app2_cpp/iox2/hello`          | `sb list`, `sb topic *`, comments in generated code, docs         |
| `topic_zenoh`      | `dev01/ws1/app2_cpp/iox2/hello`           | (unused for this entry — transport is iceoryx2)                   |
| `topic_iox`        | `dev01/ws1/app2_cpp/iox2/hello`           | iceoryx2 `ServiceName::new(...)` argument, generated `*_SERVICE`  |
| `topic_bare`       | `hello`                                   | Generated filename + `struct_name_base` (PascalCase: `Hello`)     |
| `transport_segment`| `iox2`                                    | Embedded in `topic_full`                                          |

So the C++ iceoryx2 publisher template emits:

```cpp
inline constexpr const char* Hello_SERVICE = "dev01/ws1/app2_cpp/iox2/hello";
```

while a C++ zenoh publisher for the **same bare name** (a separate
entry added with `--zenoh`) emits:

```cpp
auto pub = session.declare_publisher(KeyExpr("dev01/ws1/app2_cpp/zenoh/hello"));
```

The two refer to **different logical topics** that happen to share a
bare name — exactly the collision-avoidance the transport segment
buys us. `sb list` shows them as
`/dev01/ws1/app2_cpp/iox2/hello` and
`/dev01/ws1/app2_cpp/zenoh/hello`.

**Why the leading-`/` trim:**

- **Slash form (`topic_full`)** is the human-facing canonical
  identity. Always what users type and what `sb` shows.
- **Zenoh key expressions** forbid leading slashes (zenoh-keyexpr
  rejects them at parse time). So `topic_zenoh` strips exactly that
  one character.
- **iceoryx2 service names** accept `/` freely — the upstream docs
  literally use `ServiceName::new("My/Funk/ServiceName")` as the
  canonical example. So `topic_iox` is the same string as
  `topic_zenoh`. (Earlier versions of `sb` encoded `/` as `__`
  thinking iceoryx2 forbade slashes; that was wrong, fixed in
  0.1.7.)

**Pairing publisher ↔ subscriber across modules.** Two endpoints
land on the same wire iff they render the **same** `topic_zenoh`
(for zenoh) or `topic_iox` (for iceoryx2). Because both derive
deterministically from `topic_full`, this reduces to: *both ends
must produce the same canonical topic.* When the subscriber lives
in a different module from the publisher, the subscriber must use
the publisher's **fully-qualified** form (with leading `/`) as its
`<topic>` positional — otherwise its `<module>` segment will be its
own module name, not the publisher's, and the rendered service name
/ key won't match. See §10.1 and §10.4.

### 6.4 Per-language output shape

Each template emits a self-contained file. None of them import L1's
`sb message compile` output directly — they reference types by name
in comments and take raw bytes / typed payloads at the API boundary.

| Language | Zenoh payload | iceoryx2 payload |
|---|---|---|
| Rust | `&[u8]` | typed `T: ZeroCopySend + Copy + 'static` (generic) |
| Python | `bytes` | typed `ctypes.Structure` subclass (parameter to `__init__`) |
| C++ | `std::vector<uint8_t>` + raw-pointer overload | `template <typename T>` |
| Flutter | `Uint8List` | n/a |
| Unity | `byte[]` (P/Invoke into `zenoh.dll`) | n/a |

**iceoryx2 payloads are always typed.** Zero-copy shared memory
requires fixed-size layout, which is exactly what L1's
`sb message compile --iox2` produces (see [sbcli_messages.md](sbcli_messages.md)
§5). Users import that flat type and pass it into the iceoryx2
constructors. Protobuf serialization has no place in the iceoryx2
publish path — that's the zenoh route. (This was a Python iceoryx2
template bug in slice C; it's fixed and re-verified — see
plan/lv3_report.md "Bug fix" section.)

Each generated file leads with an **Example usage** block showing how
to import the class from the user's `main` and how to populate the
payload. The example doubles as documentation; users can copy-paste
it as a starting point.

### 6.5 Per-segment runtime overrides (`TopicOverrides`, sb 0.1.11+)

The generated pub/sub class has **two** ways to construct:

1. **All-default form** — `open(&node)`, `HelloPublisher(node)`, etc. Uses the segments baked into the file at codegen time. For iceoryx2 the payload type is pinned by codegen (no `<T>` generic in Rust, no template arg in C++, no `payload_type` kwarg in Python).
2. **Override form** — `open_with(&node, TopicOverrides { module: Some(...), ..Default::default() })` in Rust, designated init on `HelloTopicOverrides` in C++, kwargs in Python/Dart/C#. Lets the caller replace any of `device` / `workspace` / `module` / `bare_topic` at process startup, OR set `full` (sb 0.1.21+) to bypass segment assembly entirely and use a verbatim wire name (useful for pairing with a runtime-computed or foreign path from your `main()`).

This is the supported way to run N instances of the same module
(swarm semantics) — see [sbcli_pubsub_consuming.md](sbcli_pubsub_consuming.md)
§3, §10. The user's `main()` parses `--id N` (or any other flag),
constructs a `TopicOverrides { module: format!("{}-{}", DEFAULT_MODULE, id) }`,
and the publisher opens on `/dev/ws/<module>-<id>/<transport>/<topic>`.

**Emitted alongside the class** (when `is_owned == true`):

| Symbol (Rust shape; per-language equivalents in 6.4) | Source field |
|---|---|
| `pub const DEFAULT_DEVICE: &str` | `device` |
| `pub const DEFAULT_WORKSPACE: &str` | `workspace` |
| `pub const DEFAULT_MODULE: &str` | `module` |
| `pub const DEFAULT_TOPIC: &str` | `topic_bare` |
| `pub const TRANSPORT_SEGMENT: &str` | `transport_segment` |
| `pub struct TopicOverrides { device, workspace, module, topic, full }` | `full` short-circuits the 5-segment assembly when set (sb 0.1.21+) |
| `TopicOverrides::service_name()` (iox2) / `::key_expr()` (zenoh) | renders the final wire string — returns `full` verbatim when set, else assembles from segments |

**iceoryx2 subscribers also emit** (sb 0.1.22+; all transports / `is_owned` states):

| Symbol | Type | Notes |
|---|---|---|
| `<Name>Sample` (Rust / C++) | `iceoryx2::sample::Sample<…>` / `iox2::Sample<…>` | Zero-copy handle. Move-only; drops release the slot. |
| `<Name>Sample` (Python) | Generated wrapper class | `__slots__ = (_sample, _view)`. `__getattr__` forwards field reads to the ctypes Structure aliasing shmem. |
| `<Name>Subscriber::try_recv_latest` | `Result<Option<<Name>Sample>, _>` (Rust) / `std::optional<<Name>Sample>` (C++) / `Optional[<Name>Sample]` (Python) | Drains queue, returns newest sample as a zero-copy handle. The payload is NOT copied. |

Per-language access pattern (field reads only — see [sbcli_pubsub_consuming.md](sbcli_pubsub_consuming.md) §5.4 / §6.3 / §7.4 for full worked examples and lifetime rules):

- **Rust** — `msg.<field>` auto-derefs through `Sample`'s `Deref<Target = <Payload>>`.
- **C++** — `msg->payload().<field>` (Sample has no `operator*`; use the `.payload()` accessor).
- **Python** — `msg.<field>` via `__getattr__` forwarding to the ctypes Structure; raw view at `msg._view`.

Any change to this contract (return type, access pattern, lifetime rules) **must** also update the consuming-doc sections above — under-documenting it ships a breaking change users can't anticipate.

**C++ binding shape (sb 0.1.23+).** The C++ template targets the pinned iceoryx2 install at `/opt/iceoryx2/current` (0.9.3 as of sb 0.1.40), not upstream `iceoryx2-cxx`. Three call-site differences are baked into the publisher/subscriber templates: `bb::Expected` is unwrapped via an inline `detail::expect_or_throw` helper (no `.expect()` member exists in v0.9.x); `SampleMut` is sent via the free function `iox2::send(std::move(s))`, not a member; `write_payload(T&&)` takes an rvalue, so the const-ref `publish(const Payload&)` API copies into an rvalue (`write_payload(Payload(msg))`). The `#include` for the payload header is also a **basename** (`#include "<Leaf>.h"`) so the file is portable across host and Docker builds — the user's build system must add `-I<message_targets>/iox2/<pkg>/<Leaf>`. See [sbcli_pubsub_consuming.md](sbcli_pubsub_consuming.md) §5.2 (CMake snippet) and §9.1 (migration / diagnose).

**`is_owned == false` (foreign topic):** the user passed a leading-slash
path verbatim (e.g. `sb sub add /dev01/demo/cam/iox2/image_raw`). The
template skips the override machinery and emits a single
`SERVICE = "<verbatim>"` const. Rationale: that path belongs to whoever
owns it; the subscriber is just listening, and per-segment overrides
on the subscriber side don't change which key the publisher writes to.

This is a deliberate limitation — and the most common gotcha for swarm
patterns with cross-module subscribers. The consuming doc §10.4
documents the manual workaround (subscribers construct the wire path
in `main()` from raw segments and call the iceoryx2 / zenoh API
directly). A future sb release may parse leading-slash inputs into
segments and emit `TopicOverrides` for those too; until then, plan
swarm subscribers around this rule.

**Templates branch via Jinja**:

```jinja
{% if is_owned %}
pub const DEFAULT_MODULE: &str = "{{ module }}";
// ... + TopicOverrides struct + open_with(...) ...
{% else %}
pub const SERVICE: &str = "{{ topic_iox }}";
// ... single-constructor variant ...
{% endif %}
```

When editing a template, both branches must compile against the
corresponding language toolchain. The L3 codegen snapshots cover the
owned case; foreign-topic cases are exercised by integration tests in
`crates/sb-cli/tests/level3_pub_add_mutation.rs` and the L4 e2e tests.

### 6.6 Golden snapshot tests

Every cell has a byte-exact golden under
`crates/sb-cli/tests/level3_goldens/<lang>/`. Tests under
`crates/sb-cli/tests/level3_codegen_*_snapshots.rs` re-render the
template in-process and diff against the golden. Re-bless with:

```bash
SB_BLESS=1 cargo test -p sb-cli --test 'level3_codegen_*'
```

If you change a template, every reachable cell snapshot fails — the
CI workflow is: edit template, `SB_BLESS=1`, review the diff, commit.

### 6.7 What about edits + transports?

`sb pub edit <name> -m <NewType>` does **not** re-derive a different
template path; the transport in the in-memory spec stays the same.
To change the transport, `sb pub rm <name>` + `sb pub add ...
--<new-transport>`. There's intentionally no `sb pub edit
--transport` — flipping transports is a bigger change (different file
contents, different service-name format) and the rm + add pair makes
the diff visible.

---

## 7. Transport selection

`--iox2` and `--zenoh` are mutually exclusive (clap-level group
`transport`). If neither is given, the transport defaults from the
merged `sb.config.yml`:

| Config key | What it controls |
|---|---|
| `transport_on_device` | Default for same-device communication. Built-in default: `iceoryx2`. |
| `transport_cross_device` | Default for cross-device communication. Built-in default: `zenoh`. |

The current implementation uses `transport_on_device` as the bare-
default (no flag). The CLI does **not** automatically distinguish
on-vs-cross-device at `pub add` time — it just falls back to the
on-device default. The cross-device knob exists for future routing
decisions (e.g. L6's swarmctl placement engine).

Override per-invocation with the explicit flag:

```bash
sb pub add -m ros2/std/StringStamped /chatter --zenoh        # explicit
sb pub add -m ros2/std/ImageStamped /image_raw --iox2        # explicit
sb pub add -m ros2/std/StringStamped /chatter                # uses config default (iceoryx2)
```

`$SB_CONFIG` and per-workspace overrides flow through the standard
merge chain (see [sbcli_config.md](sbcli_config.md) §3).

---

## 8. Conflict policy — `--force`, `--skip-existing`, interactive prompt

When `sb pub add` or `sb sub add` finds existing entries that collide
on name or topic (§5.2), the verb consults a `ConflictPolicy`:

| Source | Resolves to |
|---|---|
| `--force` flag | `Force` — remove the colliding entries (and their generated files), then add the new one. |
| `--skip-existing` flag | `Skip` — exit 0, leave existing state alone. |
| Neither flag, **stdin is a TTY** | Prompt `[o]verwrite, [s]kip, [c]ancel?` and map to the above. |
| Neither flag, **stdin is not a TTY** | `Error` — print the colliders and hint at the flags. |

`--force` and `--skip-existing` are clap-grouped (`conflict`) and
mutually exclusive.

Force semantics:

- Removes **every** colliding entry — both publishers and
  subscribers, both same-name and same-topic — in one pass.
- Deletes the corresponding generated file before adding the new one
  (so a stale `publishers/image.rs` doesn't survive a
  `pub add --force` that re-targets the slot).
- Reports each removed entry on stdout:
  ```
    overwrote pub hello
    overwrote sub hello
  added publisher on module pubber
    wrote .../publishers/hello.rs
    updated .../sb.dev.yml
  ```

Skip semantics:

- No `sb.dev.yml` write. No file write. No file delete.
- Prints `<role> skipped on module <name> (already exists)`.
- Exit code: 0 (success).

Error semantics (default in non-TTY):

```
error: sb pub add blocked by 1 existing entry:
  - pub "hello" on topic "hello" (same topic)
Re-run with --force to overwrite, --skip-existing to no-op, or `cd`
to a TTY and re-run for an interactive prompt.
```

The collider description includes a `reason`: `same name`, `same
topic`, or `same name + topic`.

Cross-role collision: `sb sub add hello` against an existing
publisher `hello` (same topic) is a same-topic collision. `--force`
removes the publisher. Documented in
`crates/sb-cli/tests/level3_conflict_policy.rs`.

---

## 9. Subcommands

### 9.1 `sb pub add -m <MsgType> <topic> [OPTIONS]`

**Effects (in order)**:

1. Resolve the module (§3). Errors with `"no sb.dev.yml ..."` or
   `"module not found ..."` on miss.
2. Validate `<MsgType>` against the vault (§5.1).
3. Derive `<name>` (§5.3) or use `-n` value. Validate against the
   identifier rule (§5.4).
4. Find colliders (§5.2). Apply policy (§8) — may error, skip, or
   queue Force-removals.
5. Resolve transport (§7).
6. Render the template (§6) and write
   `<io_dir>/publishers/<name>.<ext>` (creating subdirs as needed).
7. Append the new `PubSpec` to `sb.dev.yml`. Write back.
8. Print the outcome.

Output (Force path, with overwrite):

```
  overwrote pub hello
added publisher on module pubber
  wrote /path/to/pubber/swarmbotix_io/publishers/hello.rs
  updated /path/to/pubber/sb.dev.yml
```

### 9.2 `sb pub edit <name> [OPTIONS]`

```bash
sb pub edit hello -m ros2/std/StringStamped
sb pub edit hello -t /new/topic
sb pub edit hello -m ros2/std/Header -t /other/topic
```

**Effects (in order)**:

1. Resolve module (§3).
2. Error if at least one of `-m`, `-t` is not given:
   `"at least one of -m/--msg or -t/--topic is required"`.
3. Find the publisher by `<name>`. Error if missing:
   `"publisher <name> not found on module <module>"`.
4. Clone the spec. Apply requested mutations:
   - `-m`: validate new MsgType. Update spec's `type`.
   - `-t`: check the new topic doesn't collide with **other**
     publishers/subscribers (the edited publisher's own current
     topic is excluded). Update spec's `topic`.
5. Re-render the file (same path — `<name>` doesn't change on edit).
6. Replace the spec in the list. Write `sb.dev.yml`.

`edit` does **not** support changing the transport — see §6.7. To
change transport: `rm` + `add`.

Output:

```
edited publisher on module pubber
  wrote /path/to/pubber/swarmbotix_io/publishers/hello.rs
  updated /path/to/pubber/sb.dev.yml
```

### 9.3 `sb pub rm <name> [--module <name>]`

**Effects (in order)**:

1. Resolve module (§3).
2. Find the publisher by `<name>`. Error if missing.
3. Remove it from the list.
4. Delete `<io_dir>/publishers/<name>.<ext>` if it exists.
5. Write back `sb.dev.yml`.

Removing a publisher does **not** remove a subscriber on the same
topic — they're independent entries. Use `sb sub rm <topic>` for the
subscriber.

Output:

```
removed publisher on module pubber
  removed /path/to/pubber/swarmbotix_io/publishers/hello.rs
  updated /path/to/pubber/sb.dev.yml
```

If the generated file was already absent (e.g. you deleted it
manually before running `rm`), the `removed` line is omitted but the
YAML write still happens.

### 9.4 `sb sub add -m <MsgType> <topic> [OPTIONS]`

Same shape as `sb pub add` (§9.1) except:

- `-n` flag is not exposed; the name is always derived from the
  topic's last segment.
- The collider check considers same-topic conflicts against both
  publishers and subscribers.

### 9.5 `sb sub edit <topic> [--module <name>]`

```bash
sb sub edit /dev01/demo/cam/iox2/image_raw
```

`sub edit` is **regenerate-from-current-state**, not "change the
fields." Useful when `sb.config.yml`'s `device:` or
`transport_*_device:` defaults change and you want to refresh the
generated file without removing-and-readding the subscriber.

**Effects (in order)**:

1. Resolve module.
2. Find the subscriber by `<topic>`. Error if missing.
3. Validate the spec's stored MsgType (in case the vault has
   changed under the subscriber).
4. Re-render the file from current spec values.

Output:

```
regenerated subscriber on module subber
  wrote /path/to/subber/swarmbotix_io/subscribers/image_raw.py
```

`sb sub edit` does **not** write `sb.dev.yml` (the spec is unchanged
— only the file is). That's by design.

### 9.6 `sb sub rm <topic> [--module <name>]`

Same shape as `sb pub rm` (§9.3) but keyed by topic.

### 9.7 `sb list [--module <name>]`

Renders both publishers and subscribers in a single table:

```
role  name       type               topic                            transport
-------------------------------------------------------------------------------
pub   hello      std/StringStamped  hello                            zenoh
sub   image_raw  std/ImageStamped   /dev01/demo/cam/iox2/image_raw   iceoryx2
```

Columns: role, name, type, topic, transport. Widths auto-fit per
invocation. An empty module renders:

```
(no publishers or subscribers)
```

### 9.8 `sb pub list [--module <name>]` / `sb sub list [--module <name>]`

Same as `sb list` filtered to one scope:

```bash
$ sb pub list
role  name   type               topic   transport
--------------------------------------------------
pub   hello  std/StringStamped  hello   zenoh
```

The role column is preserved even in the filtered views (consistent
output that can be piped through the same parsers).

---

## 10. Examples

### 10.1 Bootstrap a Rust publisher and Python subscriber

```bash
sb ws create demo && sb ws set demo

sb init --rust /path/to/pubber
( cd /path/to/pubber && sb pub add -m ros2/std/StringStamped hello --zenoh )

sb init --python /path/to/subber
( cd /path/to/subber && sb sub add -m ros2/std/StringStamped /dev01/demo/pubber/zenoh/hello --zenoh )
```

The subscriber uses the **publisher's fully-qualified topic** so both
ends land on the same wire. If both used the bare `hello`, the
publisher would publish on `/dev01/demo/pubber/zenoh/hello` and the
subscriber would listen on `/dev01/demo/subber/zenoh/hello` —
different keys; no traffic. See `examples/rust_python_zenoh/run.sh`
for a working end-to-end script.

### 10.2 Iterate on a publisher with `--force`

```bash
$ sb pub add -m ros2/std/StringStamped hello --zenoh
added publisher on module pubber
  wrote .../publishers/hello.rs
  updated .../sb.dev.yml

# Realize the type was wrong — change it without manual cleanup:
$ sb pub add -m ros2/std/Header hello --zenoh --force
  overwrote pub hello
added publisher on module pubber
  wrote .../publishers/hello.rs
  updated .../sb.dev.yml
```

`--force` is also useful in scripts that regenerate things from
scratch:

```bash
for pub in hello world; do
  sb pub add -m ros2/std/StringStamped "/dev01/demo/api/zenoh/$pub" --zenoh --force
done
```

### 10.3 Idempotent CI: `--skip-existing`

CI re-runs an init script that always calls `sb pub add`. Without a
flag, the second run errors. With `--skip-existing`:

```bash
$ sb pub add -m ros2/std/StringStamped hello --zenoh --skip-existing
publisher skipped on module pubber (already exists)
$ echo $?
0
```

The script can be re-run any number of times safely.

### 10.4 Cross-module pub/sub via fully-qualified topic

Two modules in the same workspace, one publishes a sensor reading the
other reads. The subscriber needs the publisher's fully-qualified
topic:

```bash
sb pub add -m ros2/std/ImageStamped image_raw --iox2 --module camera
# camera publishes on /dev01/demo/camera/iox2/image_raw

sb sub add -m ros2/std/ImageStamped /dev01/demo/camera/iox2/image_raw --iox2 --module detector
# detector listens on the same key
```

### 10.5 Regenerate after a config change

Bumped `device:` from `dev01` to `dev02` in `sb.config.yml`. Old
generated files still reference `dev01`. Sweep:

```bash
for module in /paths/to/each/module/; do
  ( cd "$module" && sb sub edit /dev01/.../iox2/image_raw )   # picks up new device
done
```

Or just `rm` + `add` each entry. `sub edit` only re-renders the file
without changing the YAML.

### 10.6 List everything across a workspace

There's no built-in `sb list --all-modules` yet (planned for L6). For
now, iterate `flow.yaml`:

```bash
yq '.modules | to_entries[] | .key' ~/.swarmbotix/workspaces/demo/flow.yaml | \
  while read -r mod; do
    echo "=== $mod ==="
    sb list --module "$mod"
  done
```

---

## 11. Atomicity, partial failures, and the "impossible" error

### 11.1 Mutation order

Each `add` / `edit` / `rm` runs §9 in **strict order**: YAML write
**after** the codegen succeeds. So if the template renders but the
YAML write fails (disk full, permissions), you end up with a file on
disk that the YAML doesn't know about. The recovery is to re-run the
command (idempotent if the file is byte-identical — which the
template guarantees per §6.6).

### 11.2 `--force` overwrite atomicity

Force performs **N removals + 1 add** sequentially. If a removal step
fails (file gone, permission), the partial removals already happened
and the new entry hasn't been added yet. Re-running with `--force`
finishes the job (any prior removed entries are already gone from
the YAML; the new add proceeds).

### 11.3 Two structurally-different "no template" errors

`sb pub add --iox2` on a Flutter or Unity module errors with:

```
error: flutter has no iceoryx2 path — Flutter and Unity run on mobile / sandboxed game runtimes where shared memory is unavailable. Use --zenoh.
```

This is **not** "not yet implemented" — it's structural. The two
sandbox runtimes can't access POSIX shared memory across processes,
so iceoryx2 fundamentally can't work there. The error wording reflects
that to avoid wasted user time looking for a roadmap.

Any other missing cell falls through to:

```
error: <language> <transport> codegen is not yet implemented at this point in L3 (this build only ships the templates for languages that have landed). See plan/lv3_report.md for the slice schedule.
```

At L3-as-shipped, every non-structurally-impossible cell is wired —
that error is reachable only if a future language is added to
`Language` but no template lands with it.

### 11.4 Vault drift mid-edit

If you `sb message rm` a type while a publisher references it, then
re-run `sb pub edit <pub> -t /new/topic`, the validation step in
`pub_edit` re-checks the stored type. If it's gone from the vault,
edit fails:

```
error: invalid stored msg_type "ros2/robot_msgs/Gone": message "ros2/robot_msgs/Gone" not in vault. Run `sb message list` to see available types.
```

Recovery: either restore the type (`sb message new ros2/robot_msgs/Gone`
and fill in the schema) or `sb pub rm` and re-add with a still-valid
type.

---

## 12. Error catalogue

Stable wordings (substring-asserted by integration tests in
`crates/sb-cli/tests`):

| Code path | Message |
|---|---|
| Module not in cwd | `no sb.dev.yml in cwd (<dir>). Pass --module <name> or \`cd\` into a module dir.` |
| `--module <n>` miss | `module "<n>" not found. Searched:\n  - <path1>\n  - <path2>` |
| MsgType parse | `invalid message name "<m>": <reason from MessageNameError>` |
| MsgType missing | `message "<m>" not in vault. Run \`sb message list\` to see available types.` |
| Identifier | `invalid publisher name "<n>": <reason>` / `invalid subscriber name "<n>" (derived from topic): <reason>` |
| Publisher missing on edit/rm | `publisher "<n>" not found on module "<m>"` |
| Subscriber missing on edit/rm | `subscriber for topic "<t>" not found on module "<m>"` |
| Already exists (default policy) | `sb pub add blocked by N existing entr<y|ies>:\n  - <role> "<n>" on topic "<t>" (<reason>)\n  ...\nRe-run with --force to overwrite, --skip-existing to no-op, or \`cd\` to a TTY and re-run for an interactive prompt.` |
| `pub edit` with no mutation flag | `at least one of -m/--msg or -t/--topic is required` |
| Structural-impossible (Flutter/Unity iceoryx) | `<lang> has no iceoryx2 path — Flutter and Unity run on mobile / sandboxed game runtimes where shared memory is unavailable. Use --zenoh.` |
| Not-yet-implemented (future languages) | `<lang> <transport> codegen is not yet implemented at this point in L3 ...` |

---

## 13. Edge cases and gotchas

### 13.1 Topic = pure leading slash

`sb pub add -m ... /` would derive name `""` (empty) and fail
identifier validation. Don't do that; topic must have at least one
non-slash character.

### 13.2 Editing the topic to one that exists as a subscriber

```bash
sb sub add -m ros2/std/StringStamped chat --zenoh    # subscriber on `chat`
sb pub add -m ros2/std/StringStamped foo --zenoh     # publisher on `foo`
sb pub edit foo -t chat                         # → collision detected
```

`pub edit` rejects with the same collider message `pub add` uses. If
you want to claim the topic for a publisher, `sb sub rm chat` first,
or use `sb pub add chat --force` and remove the publisher you no
longer want.

### 13.3 IO dir lives under a build-system out-of-tree dir

`sb.dev.yml`'s `io_dir:` is relative to the module root. You can point
it at a sibling dir if your build system prefers:

```yaml
io_dir: ../build/swarmbotix_io
```

Codegen joins `<module_root>/<io_dir>` and `Path::join` collapses
`..` naturally. The build system can then include / link / package
that dir however it wants.

### 13.4 Re-running `add` with no flag in a non-TTY context

Common in CI:

```yaml
- run: sb pub add -m ros2/std/StringStamped hello --zenoh
```

If a prior pipeline step (or a previous build of the same step) left
`hello` registered, this command fails because non-TTY default is
`Error`. Fix: add `--skip-existing` to the script. The cost is that a
genuinely-wrong type wouldn't get caught either — pick the trade-off
your CI tolerates. Some pipelines run `sb pub rm` first as a hard
reset.

### 13.5 `sb pub add` doesn't compile messages

You're free to `sb pub add -m ros2/custom/NotCompiledYet ...` even if
`custom/NotCompiledYet.proto` is just a `// TODO` skeleton. The
generated pub/sub file references the type by name (in comments + in
the example block) but doesn't import it at codegen time. When the
user fills in the proto and runs `sb message compile`, the generated
bindings appear under `<message_targets>/`, ready for the user to wire
into their `main.rs` / `main.py` / etc.

### 13.6 `--name` with a topic that has no slash

```bash
sb pub add -m ros2/std/StringStamped hello -n other --zenoh
```

Stores `name: other, topic: hello`. The generated filename is
`publishers/other.rs`; the topic-namespace prefix is
`/dev01/<ws>/<module>/zenoh/hello`. Useful when the natural name
collides with something else but the topic should keep its bare form.

### 13.7 Topic with a leading `/` overrides namespace prefixing

```bash
sb pub add -m ros2/std/StringStamped /flat/path/hello --zenoh
```

`topic_full` is `/flat/path/hello` verbatim, **not**
`/dev01/<ws>/<module>/zenoh/flat/path/hello`. Useful for legacy ROS2
topics or for cross-workspace traffic. The user is responsible for
including any `<transport>` segment they want; `sb` does not inject
it when the topic is leading-slash-prefixed. The derived name is
still `hello` (last segment).

### 13.8 `sb sub list` with no module errors with a more useful message

When in a directory without `sb.dev.yml`, `sb pub list` and
`sb sub list` error the same way as `sb pub add` — same module
resolution chain. To list a specific module from anywhere with an
active workspace:

```bash
sb pub list --module pubber
```
