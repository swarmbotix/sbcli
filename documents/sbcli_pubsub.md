# `sb io` / `sb pub` / `sb sub` / `sb list`: Pub/Sub Reference

Official reference for the `sb pub`, `sb sub`, and `sb list`
subcommand surfaces and, `sb 0.5.0+`, for `sb io`, the surface that
replaces them: module resolution, validation, mutation semantics,
transport selection, encodings, conflict-policy flags, per-language
codegen, and the operational guarantees that govern when files are
written, regenerated, or removed.

This document is normative for behavior shipped at L3.

**sb 0.5.0+: use `sb io` (§14).** It manages publishers, subscribers,
and services in one name space per module, adds the `encoding:` choice
(`proto`, `fb`, `raw`), and regenerates a whole module with
`sb io update`. `sb pub`, `sb sub`, `sb list`, and `sb service` keep
working as documented here but are deprecated and receive no new
features; `sb 0.6.0+` prints a deprecation warning on stderr each time
one is used, and 0.7.0 removes them ([sbcli.md](sbcli.md) §10).
Sections 1 to 13 describe the model both surfaces share: resolution,
schema, codegen, and conflict policy.

`sb 0.6.0+`: the module file is `sb.module.yaml` (`sb.dev.yml` before;
still read, with a warning, until 0.7.0). Its launch keys (`scripts`,
`env`, `requires`, `platforms`) are documented in
[sbcli_module.md](sbcli_module.md); this document covers its
`publishers`, `subscribers`, and `services`.

The canonical spec is `requirements.md` in the sbcli **source
repository** — a maintainer document that is not part of your install;
when it and this page disagree, it wins.

---

## 1. Conceptual model

`sb pub` and `sb sub` are **mutating** verbs that operate on a single
module's `sb.module.yaml`. Each `add`, `edit`, or `rm` does two things in
lockstep:

1. **Mutates the YAML.** Reads `sb.module.yaml`, modifies the
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
sb pub add  -m <MsgType> (<topic> | --key <WIRE_KEY>)
                                   [-n <name>] [--iox2 | --zenoh]
                                   [--force | --skip-existing]
                                   [--module <name>]
sb pub edit <name>                 [-m <MsgType>] [-t <topic> | --key <WIRE_KEY>]
                                   [--module <name>]
sb pub rm   <name>                 [--module <name>]
sb sub list                        [--module <name>]
sb sub add  -m <MsgType> (<topic> | --key <WIRE_KEY>)
                                   [--iox2 | --zenoh]
                                   [--force | --skip-existing]
                                   [--module <name>]
sb sub edit <topic-or-key>         [--module <name>]
sb sub rm   <topic-or-key>         [--module <name>]
```

`-m` is **reserved for `MsgType`**. Module selection uses the
long-only flag `--module <name>` to avoid the clash.

`<topic>` is a bare name that `sb` prefixes at codegen; `--key` takes a
verbatim wire key instead (sb 0.5.0+). Each entry carries exactly one
of the two (§4).

---

## 2. The two on-disk artifacts a mutation touches

Per module:

```
<module_root>/
├─ sb.module.yaml                               # READ + WRITE
└─ <io_dir>/                                    # WRITE + DELETE
   ├─ publishers/
   │  └─ <name>.<ext>                           # one per publisher
   ├─ subscribers/
   │  └─ <name>.<ext>                           # one per subscriber
   └─ services/
      └─ <name>.<ext>                           # one per service (sb 0.5.0+; Rust and Python)
```

`<io_dir>` defaults to `swarmbotix_io` (or `Assets/Scripts/swarmbotix_io`
for Unity) — see [sbcli_init.md](sbcli_init.md) §4. `<ext>` is the
language's natural extension: `rs`, `py`, `cpp`, `dart`, `cs`.

The IO dir contents are **deterministic codegen output**. They are
byte-stable for a given `(language, transport, kind, name, topic,
msg_type, device, workspace, module)` tuple — see §6.6 for the golden
snapshot suite. You may either commit them or gitignore + regenerate
at build time; sb has no opinion.

`sb.module.yaml` is the **source of truth** — if it and the generated
files disagree, the YAML wins on the next mutation (the file is
regenerated to match).

---

## 3. Module resolution

Every `sb pub/sub *` verb takes an optional `--module <name>` flag.
The lookup chain (highest priority first):

1. **`--module <name>` given AND `./<name>/sb.module.yaml` exists** in
   the current working directory (or, deprecated, `./<name>/sb.dev.yml`).
2. **`--module <name>` given AND active workspace's `sb.flow.yaml`
   has `<name>`**: the value in the installed copy is the absolute
   path to its manifest ([sbcli_ws.md](sbcli_ws.md) §4.3).
3. **No `--module` flag → `./sb.module.yaml` in cwd** (or, deprecated,
   `./sb.dev.yml`). Errors if neither exists.

`sb 0.6.0+` looks for `sb.module.yaml` first and falls back to
`sb.module.yaml` with a warning; an installed copy written by 0.5.0 names
`sb.dev.yml` paths explicitly, and those keep resolving.

If `--module <name>` is given and neither rule 1 nor rule 2 hits, the
CLI errors with **every path it tried**, including the dangling
`sb.flow.yaml` entry if rule 2 missed because the registered path no
longer exists.

```
error: module "ghost" not found. Searched:
  - /current/dir/ghost/sb.module.yaml
  - /home/el/.swarmbotix/workspaces/demo/sb.flow.yaml
```

If no `--module` is given and cwd has no module manifest:

```
error: no module manifest (sb.module.yaml) in cwd (/wrong/place). Pass --module <name> or `cd` into a module dir.
```

The resolver is `sb_pubsub::resolve_module`. Both lookups (rule 1
sibling, rule 2 workspace) require the resolved file to exist; a
registered-but-missing entry in `sb.flow.yaml` is treated as a miss for
that rule, **and** the path is included in the error tail.

`sb io` (§14) and `sb service` resolve their module through the same
chain. The module root is then the directory holding `sb.module.yaml`
unless the file sets `root:` (§4.1).

---

## 4. Schema — `PubSpec` and `SubSpec`

The `publishers:` and `subscribers:` lists in `sb.module.yaml` hold these
two record types (defined in `sb_core::config`):

```yaml
publishers:
  - name: image_raw
    topic: image_raw
    type: ros2/std/ImageStamped
    transport: iceoryx2
subscribers:
  - name: cmd_vel
    key: dev01/nav/teleop/zenoh/cmd_vel
    type: ros2/std/TwistStamped
    transport: zenoh
```

| Field | Type | Set by | Notes |
|---|---|---|---|
| `name` | identifier | `-n` flag or topic's last segment | Drives the generated filename + the struct name (PascalCase). Must satisfy `validate_identifier`. |
| `topic` | string | `<topic>` positional, or `-t` on `sb pub edit` | An owned, bare topic (`image_raw`). Codegen prepends `<device>/<workspace>/<module>/<transport>/`. May not contain `/`. An entry carries at most one of `topic` and `key` (sb 0.5.0+). `sb 0.6.0+`: optional; an entry with neither uses its `name` as the topic, and `sb` writes `topic:` back on the next write. |
| `key` | string | `--key` flag | A verbatim wire key (`dev01/nav/teleop/zenoh/cmd_vel`), used as is: no prefix is prepended and no `<transport>` segment is injected. A leading `/` given to `--key` is dropped. An entry carries exactly one of `topic` and `key` (sb 0.5.0+). |
| `type` | string | `-m` / `--msg` flag | Must parse as `<style>/<namespace>/<Leaf>` and exist in the vault (see §5.1). Stored verbatim, e.g. `ros2/std/ImageStamped`. Two-segment names are rejected; see [sbcli_messages.md](sbcli_messages.md) §2. `sb 0.5.0+`: required unless `encoding: raw`, which takes none. |
| `transport` | enum (`zenoh` / `iceoryx2`) | `--zenoh` / `--iox2` or sb.config.yml default | See §7 for default resolution. |
| `encoding` | enum (`proto` / `fb` / `raw`) | `--proto` / `--fb` / `--raw` on `sb io` (§14.3) | `sb 0.5.0+`. Payload framing, default `proto` and omitted on disk when `proto`. Zenoh only: an iceoryx2 payload is always the fixed-layout struct generated from the `.proto`, so an iceoryx2 entry omits the key (an explicit `encoding: proto` is accepted). `fb` carries FlatBuffers bytes of `type`; `raw` carries opaque bytes with no vault type. |
| `mime` | string | the value after `--raw` | `sb 0.5.0+`. Raw only, optional: the payload's content type, e.g. `text/json`. It becomes the wire tag (§6.4). |

`deny_unknown_fields` is on for both records; hand-edited fields outside
the schema are rejected on read. So is an entry that carries both
`topic` and `key`, a `topic` containing `/`, and an empty `key` (error
texts in §12). The one exception is a `topic: /...` entry written
before 0.5.0: it is read as `key:` (§13.9).

**`topic:` is optional** (sb 0.6.0+). A hand-written entry such as
`{ name: markers, type: ros2/custom/MarkerDetections, transport: zenoh }`
reads as `topic: markers`, so the name must also be a valid topic
segment. `sb` still writes `topic:` out whenever it writes the file, so
a file written by 0.6.0 parses on 0.5.0 as long as it uses no other new
key. Before 0.6.0 an entry with neither `topic` nor `key` was an error.

`sb 0.5.0+` also enforces the encoding rules on read: `raw` takes no
`type`, every other encoding needs one, `mime` needs `raw`, and an
encoding other than `proto` needs `transport: zenoh` (error texts in
§12). When an entry breaks several rules, the topic error is reported
first, then the transport one.

`SubSpec` is identical in shape to `PubSpec`, but `sb sub *` keys
subscribers by **topic or key** rather than name (because subscribers
are fan-out and a single name doesn't uniquely identify one). Internally
the `name` is still derived (it's the generated filename) but the
user-facing API only ever takes the topic or key: `sb sub edit` and
`sb sub rm` match their argument against both, ignoring a leading `/`
(sb 0.5.0+). `sb io` (§14) addresses every entry by name instead, which
the shared name space below makes unambiguous.

### 4.1 Module-level fields (sb 0.5.0+)

Besides `publishers:` and `subscribers:`, `sb.module.yaml` carries the
fields below, and (sb 0.6.0+) the launch keys `scripts`, `env`,
`requires`, `platforms`, plus `version`, `sb`, `description`, all
documented in [sbcli_module.md](sbcli_module.md) §2:

| Field | Type | Notes |
|---|---|---|
| `module` | identifier | Written by `sb init`, see [sbcli_init.md](sbcli_init.md) §4. |
| `language` | enum | Same. Selects the templates. |
| `root` | path, optional | The module root. Absent (what `sb init` writes since 0.5.0): the directory holding `sb.module.yaml`. Relative: resolved against that directory. Absolute: used as is, which is how files written before 0.5.0 keep working. |
| `io_dir` | relative path | Default `swarmbotix_io` (`Assets/Scripts/swarmbotix_io` for Unity), resolved against the module root. |
| `services` | list of `{ name }` | One request/response router per entry, rendered to `<io_dir>/services/<name>.<ext>` (Rust and Python). See [sbcli_reqres.md](sbcli_reqres.md) §2. |

```yaml
module: ips_track
language: python
io_dir: swarmbotix_io
publishers:
  - name: pose
    topic: pose
    type: ros2/std/StringStamped
    transport: zenoh
  - name: status
    topic: status
    transport: zenoh
    encoding: raw
    mime: text/json
subscribers:
  - name: cmd
    key: dev01/ws_ips/planner/zenoh/cmd
    type: ros2/std/TwistStamped
    transport: zenoh
services:
  - name: calib
```

**Names are unique within a module.** Publishers, subscribers, and
services share one name space: a name is the generated file's stem and
the handle `sb io edit` / `sb io rm` take, so it must identify exactly
one entry. Adding an entry whose name is already used by any of the
three is refused (§5.2). The check runs when an entry is added, not
when the file is read, so a hand-edited duplicate loads but the next
add on that name is refused.

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
`sb message compile`'d. `sb 0.5.0+`: an `encoding: raw` entry has no
`type` and skips this lookup; `sb io add --fb` is the one case that
does need compiled output (§14.3). The generated pub/sub file references the
type by name only; whether the user has Rust / Python / C++ bindings
ready for it at compile/runtime is independent.

### 5.2 Name + topic conflicts (collision check)

Each add scans the module's existing publishers + subscribers and
reports every entry that would collide on:

- **Same name**: any publisher, subscriber, or (sb 0.5.0+) service on
  the module, since they share one name space (§4.1). A service
  collider is listed as `service "<name>" (same name)`.
- **Same topic/key** (across publishers AND subscribers: a topic or
  key can have at most one role per module unless the user explicitly
  opts out via `--force`; see §8).

If any collisions are found, the verb's behaviour depends on the
conflict policy resolved in §8. The default is to error.

### 5.3 Default-name derivation

When the user omits `-n` on `sb pub add`, the name is derived from
the topic or key: a bare topic is its own name, and a key gives its
**last `/`-separated segment**:

| `<topic>` / `--key` | derived name |
|---|---|
| `hello` | `hello` |
| `--key dev01/ws/cam/iox2/image_raw` | `image_raw` |

`sb sub add` always derives the name this way (no `-n` flag).

### 5.4 Identifier validation on name

The derived (or `-n`-supplied) name must satisfy
`validate_identifier`. Mirror of the workspace/module rule —
non-empty, ASCII letter/`_` first, then alphanumerics/`_`.

---

## 6. Codegen

After `sb.module.yaml` mutation, the matching template renders into
`<io_dir>/{publishers,subscribers}/<name>.<ext>`, or, for a service
(sb 0.5.0+), `<io_dir>/services/<name>.<ext>`.

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
| `module` | `pubber` | Module name from `sb.module.yaml`. |
| `workspace` | `demo` | Active workspace name. |
| `device` | `dev01` | From merged `sb.config.yml`. |
| `topic_full` | `dev01/demo/pubber/iox2/image_raw` | The wire key, including the `<transport>` segment, with no leading `/` (sb 0.5.0+). A `topic:` entry builds it as `<device>/<workspace>/<module>/<transport>/<topic>`; a `key:` entry uses its key as is (and must already include any `<transport>` segment it wants). |
| `topic_zenoh` | `dev01/demo/pubber/iox2/image_raw` | Identical to `topic_full` (sb 0.5.0+). Zenoh KeyExpr forbids a leading `/`, and the canonical form has none. |
| `topic_iox` | `dev01/demo/pubber/iox2/image_raw` | Identical to `topic_full`. iceoryx2 0.9 `ServiceName::new` accepts `/` (per upstream's own example `ServiceName::new("My/Funk/ServiceName")`), so no transliteration. Kept as a distinct name for template-side clarity. |
| `topic_bare` | `image_raw` | The bare topic of a `topic:` entry, or the last segment of a `key:` (the transport segment is **not** the bare topic). |
| `transport_segment` | `iox2` / `zenoh` | The literal string used in the path. `iox2` for iceoryx2, `zenoh` for zenoh. |
| `msg_namespace` | `std` | The MsgType's namespace. |
| `msg_leaf` | `ImageStamped` | The MsgType's leaf. |
| `msg_type` | `ros2/std/ImageStamped` | Full vault name, all three segments. |
| `transport` | `zenoh` / `iceoryx2` | Transport tag. |
| `is_zenoh` / `is_iceoryx` | bool | For Jinja branching. |
| `is_publisher` / `is_subscriber` | bool | Mirrored from the kind. |
| `role` | `publisher` / `subscriber` | String form. |
| `is_owned` | bool | True for a `topic:` entry: the full key is built from device/workspace/module/transport/topic here at codegen. False for a `key:` entry: the key is used verbatim, so the path "belongs" to whoever owns it and per-segment overrides don't apply. Templates gate the `TopicOverrides` emission on this flag (§6.5). |
| `encoding` | `proto` / `fb` / `raw` | sb 0.5.0+. The entry's `encoding:`; `is_proto` / `is_fb` / `is_raw` are the matching bools. |
| `mime` | `text/json` | sb 0.5.0+. The entry's `mime:`, raw only. |
| `wire_tag` | `application/protobuf;ros2/std/ImageStamped` | sb 0.5.0+. The Zenoh encoding string the entry stamps (§6.4). |
| `service_name` / `is_default_service` | `calib` / bool | sb 0.5.0+, services only. A service named `service` is the module's default and serves on the bare service base ([sbcli_reqres.md](sbcli_reqres.md) §3). Services render `<lang>/service_app.<ext>.j2`. |

### 6.3 Topic naming: one canonical identity, one wire form

A pub/sub entry has **one canonical topic identity**. For a `topic:`
entry it is derived once from `(device, workspace, module, transport,
topic)` at codegen time; for a `key:` entry it is the key itself. The
canonical form **is** the wire form, byte for byte, on both transports
(sb 0.5.0+). Only the `<transport>` segment (`iox2` vs `zenoh`)
differentiates two entries with the same bare name.

**Canonical form** (what `sb topic *`, docs, log output, comments
inside generated files, and the wire all use):

```
<device>/<workspace>/<module>/<transport>/<bare_topic>
```

`<transport>` is the literal `iox2` or `zenoh`, taken from the
entry's transport. This is `topic_full` in the template context. It
has no leading `/`. A `key:` entry is not assembled: its key is used
verbatim and the `<device>/<workspace>/<module>/<transport>` prefix is
**not** prepended (see §13.7).

**The conversion rule**, applied deterministically at template
render time, no user input:

```
transport_segment = "iox2" if transport == iceoryx2 else "zenoh"
topic_full   = <device>/<workspace>/<module>/<transport_segment>/<topic>     # topic: entry
topic_full   = <key>                                                         # key: entry, verbatim
topic_zenoh  = topic_full                                                    # same string
topic_iox    = topic_full                                                    # same string
topic_bare   = <topic>, or the last segment of <key>
```

**Worked example**: `sb pub add -m ros2/std/StringStamped hello --iox2
--module app2_cpp` in workspace `ws1` on device `dev01`:

| Template var       | Value                                     | Where it appears                                                  |
|--------------------|-------------------------------------------|-------------------------------------------------------------------|
| `topic_full`       | `dev01/ws1/app2_cpp/iox2/hello`           | `sb topic *`, comments in generated code, docs                    |
| `topic_zenoh`      | `dev01/ws1/app2_cpp/iox2/hello`           | (unused for this entry: transport is iceoryx2)                    |
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
bare name, which is exactly the collision avoidance the transport
segment buys us. `sb topic list` shows them as
`dev01/ws1/app2_cpp/iox2/hello` and
`dev01/ws1/app2_cpp/zenoh/hello`; `sb list` shows each entry's stored
`topic`, `hello`, with its transport (§9.7).

Because the canonical form carries the transport, `sb topic listen` and
`sb topic pub` read it from the `<transport>` segment (sb 0.4.1+):
`sb topic listen dev01/ws1/app2_cpp/iox2/hello` subscribes on iceoryx2
with no flag, and `-t zenoh` / `-t iox2` is needed only for a path that is
not in this form (§13.7).

**Canonical and wire form are the same string (sb 0.5.0+).** Zenoh key
expressions forbid a leading `/` (zenoh-keyexpr rejects it at parse
time), and iceoryx2 service names accept `/` freely (the upstream docs
use `ServiceName::new("My/Funk/ServiceName")` as the canonical example),
so one slash-less string serves both transports, the CLI, and the docs.
Before 0.5.0 the canonical form carried a leading `/` that codegen
stripped for the wire; there is no longer a second spelling to
translate between.

**Pairing publisher and subscriber across modules.** Two endpoints
land on the same wire iff they render the **same** `topic_full`. Two
`topic:` entries in different modules never do: each end's `<module>`
segment is its own module name. So when the subscriber lives in a
different module from the publisher, the subscriber uses `--key` with
the publisher's full key (here `--key dev01/ws1/app2_cpp/iox2/hello`).
See §10.1 and §10.4.

### 6.4 Per-language output shape

Each template emits a self-contained file. None of them import L1's
`sb message compile` output directly — they reference types by name
in comments and take raw bytes / typed payloads at the API boundary.

| Language | Zenoh payload | iceoryx2 payload |
|---|---|---|
| Rust | `&[u8]`; sb 0.5.0+ `proto` entries also take a prost message | typed `T: ZeroCopySend + Copy + 'static` (generic) |
| Python | `bytes`; sb 0.5.0+ `proto` entries also take a protobuf message | typed `ctypes.Structure` subclass (parameter to `__init__`) |
| C++ | `std::vector<uint8_t>` + raw-pointer overload; sb 0.5.0+ `proto` entries also take a protobuf message | `template <typename T>` |
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

**Zenoh publishers tag the encoding (sb 0.4.0+).** The Rust, Python, and
C++ Zenoh publisher templates declare the publisher with the encoding
`application/protobuf;<style>/<ns>/<Leaf>` (the entry's `msg_type`), so
every sample names its vault message. The payload bytes are unchanged:
`publish()` still takes the serialized message. `sb topic listen` reads
this tag before the Header probe ([sbcli_messages.md](sbcli_messages.md)
§1.1). The Flutter and Unity Zenoh templates are unchanged and set no
encoding. A publisher file generated before 0.4.0 carries no tag until it
is re-rendered, for example with `sb pub edit <name> -m <its current type>`
(§9.2), or (sb 0.5.0+) with `sb io update`; `sb message compile` does not
rewrite publisher files. Until then,
`sb topic listen` types its frames by the Header probe, `--type`, or
(sb 0.4.1+) the entry's `type` in the module's `sb.module.yaml`
([sbcli_messages.md](sbcli_messages.md) §1.1).

**Encodings and typed wrappers (sb 0.5.0+).** The wire tag follows the
entry's `encoding:` (§4, §14.3), and so does the API of the generated
Rust, Python, and C++ Zenoh wrappers:

| `encoding` | Wire tag | Publisher | Subscriber |
|---|---|---|---|
| `proto` (default) | `application/protobuf;<style>/<ns>/<Leaf>` | `publish(msg)` serializes a protobuf message; `publish_bytes(...)` sends serialized bytes | `try_recv_latest()` (bytes) plus a typed receive |
| `fb` | `application/flatbuffers;<style>/<ns>/<Leaf>` | bytes only: build the buffer with the bindings `sb message compile --fb` emits | bytes only |
| `raw` | the entry's `mime:`, else `application/octet-stream` | bytes only | bytes only |

The typed receive is `try_recv_latest_msg::<M>()` in Rust (prost; the
module needs the `prost` crate), `try_recv_latest_msg(MsgClass)` in
Python (the `_pb2` class), and `try_recv_latest_msg<Message>()` in C++,
which returns `std::nullopt` when the payload does not parse. Each
generated file exports the tag it stamps or expects as `WIRE_TAG`
(Rust, Python) or `<Name>_WIRE_TAG` (C++). The
Flutter (Dart) and Unity (C#) Zenoh wrappers stay bytes-only and set no
tag, whatever the encoding. iceoryx2 wrappers are unaffected: their
payload is always the typed struct.

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
and the publisher opens on `dev/ws/<module>-<id>/<transport>/<topic>`.
That pattern is still the one to use in 0.6.0. What changed
(sb 0.6.0+) is the launch side: for a module with instances in
`sb.flow.yaml`, `sb up` and `sb run` append `--id <id>` to the start
command themselves and also export `SB_INSTANCE_ID=<id>`, so `main()`
can take the id from either ([sbcli_module.md](sbcli_module.md) §9).

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

**`is_owned == false` (foreign topic, a `key:` entry):** the user
passed a verbatim wire key (e.g.
`sb sub add -m ros2/std/ImageStamped --key dev01/demo/cam/iox2/image_raw --iox2`).
The template skips the override machinery and emits a single
`SERVICE = "<verbatim>"` const. Rationale: that key belongs to whoever
owns it; the subscriber is just listening, and per-segment overrides
on the subscriber side don't change which key the publisher writes to.

This is a deliberate limitation, and the most common gotcha for swarm
patterns with cross-module subscribers. The consuming doc §10.4
documents the manual workaround (subscribers construct the wire path
in `main()` from raw segments and call the iceoryx2 / zenoh API
directly). A future sb release may parse `key:` entries into
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
owned case; foreign-topic (`key:`) cases are exercised by integration tests in
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
sb pub add -m ros2/std/StringStamped chatter --zenoh         # explicit
sb pub add -m ros2/std/ImageStamped image_raw --iox2         # explicit
sb pub add -m ros2/std/StringStamped chatter                 # uses config default (iceoryx2)
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
    updated .../sb.module.yaml
  ```

Skip semantics:

- No `sb.module.yaml` write. No file write. No file delete.
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

`sb pub`, `sb sub`, and `sb list` are deprecated in favour of `sb io` (§14) from sb 0.5.0: they keep working as described in this section but receive no new features, so `encoding:`, `mime:`, and services are reachable only through `sb io`.

### 9.1 `sb pub add -m <MsgType> (<topic> | --key <WIRE_KEY>) [OPTIONS]`

`<topic>` is a bare name (sb 0.5.0+). A positional containing `/`,
leading or inner, is refused, and the error spells out the `--key`
form:

```
error: topic "/dev01/ws/mod/iox2/hello" may not contain '/'; for a verbatim wire key use --key dev01/ws/mod/iox2/hello
```

`--key <WIRE_KEY>` stores a `key:` entry instead of a `topic:` one; a
leading `/` on its value is dropped. The two conflict.

**Effects (in order)**:

1. Resolve the module (§3). Errors with `"no module manifest ..."` or
   `"module not found ..."` on miss.
2. Validate `<MsgType>` against the vault (§5.1).
3. Derive `<name>` (§5.3) or use `-n` value. Validate against the
   identifier rule (§5.4).
4. Find colliders (§5.2). Apply policy (§8) — may error, skip, or
   queue Force-removals.
5. Resolve transport (§7).
6. Render the template (§6) and write
   `<io_dir>/publishers/<name>.<ext>` (creating subdirs as needed).
7. Append the new `PubSpec` to `sb.module.yaml`. Write back, rewriting
   any legacy `topic: /...` entries in the file as `key:` (§13.9).
8. Print the outcome.

Output (Force path, with overwrite):

```
  overwrote pub hello
added publisher on module pubber
  wrote /path/to/pubber/swarmbotix_io/publishers/hello.rs
  updated /path/to/pubber/sb.module.yaml
```

### 9.2 `sb pub edit <name> [OPTIONS]`

```bash
sb pub edit hello -m ros2/std/StringStamped
sb pub edit hello -t new_topic
sb pub edit hello --key new/topic
sb pub edit hello -m ros2/std/Header -t other_topic
```

**Effects (in order)**:

1. Resolve module (§3).
2. Error if none of `-m`, `-t`, `--key` is given:
   `"at least one of -m/--msg, -t/--topic, or --key is required"`.
3. Find the publisher by `<name>`. Error if missing:
   `"publisher <name> not found on module <module>"`.
4. Clone the spec. Apply requested mutations:
   - `-m`: validate new MsgType. Update spec's `type`.
   - `-t`: a bare topic only; a value containing `/` is refused, as
     for `sb pub add` (§9.1). Check the new topic doesn't collide with
     **other** publishers/subscribers (the edited publisher's own
     current topic is excluded). The entry becomes `topic: <new>`,
     replacing its `topic` or `key`.
   - `--key` (sb 0.5.0+): a verbatim wire key, leading `/` dropped.
     Same collision check. The entry becomes `key: <new>`, replacing
     its `topic` or `key`. Conflicts with `-t`.
5. Re-render the file (same path: `<name>` doesn't change on edit).
6. Replace the spec in the list. Write `sb.module.yaml`, rewriting any
   legacy `topic: /...` entries as `key:` (§13.9).

`edit` does **not** support changing the transport — see §6.7. To
change transport: `rm` + `add`.

Output:

```
edited publisher on module pubber
  wrote /path/to/pubber/swarmbotix_io/publishers/hello.rs
  updated /path/to/pubber/sb.module.yaml
```

### 9.3 `sb pub rm <name> [--module <name>]`

**Effects (in order)**:

1. Resolve module (§3).
2. Find the publisher by `<name>`. Error if missing.
3. Remove it from the list.
4. Delete `<io_dir>/publishers/<name>.<ext>` if it exists.
5. Write back `sb.module.yaml`.

Removing a publisher does **not** remove a subscriber on the same
topic; they're independent entries. Use `sb sub rm <topic-or-key>` for
the subscriber.

Output:

```
removed publisher on module pubber
  removed /path/to/pubber/swarmbotix_io/publishers/hello.rs
  updated /path/to/pubber/sb.module.yaml
```

If the generated file was already absent (e.g. you deleted it
manually before running `rm`), the `removed` line is omitted but the
YAML write still happens.

### 9.4 `sb sub add -m <MsgType> (<topic> | --key <WIRE_KEY>) [OPTIONS]`

Same shape as `sb pub add` (§9.1), including the `/`-in-topic error
and `--key`, except:

- `-n` flag is not exposed; the name is always derived from the bare
  topic, or from the key's last segment.
- The collider check considers same-topic conflicts against both
  publishers and subscribers.

### 9.5 `sb sub edit <topic-or-key> [--module <name>]`

```bash
sb sub edit dev01/demo/cam/iox2/image_raw
```

The argument matches an entry's `topic` or its `key`. A leading `/` is
ignored, so `/dev01/demo/cam/iox2/image_raw` finds the same entry
(sb 0.5.0+).

`sub edit` is **regenerate-from-current-state**, not "change the
fields." Useful when `sb.config.yml`'s `device:` or
`transport_*_device:` defaults change and you want to refresh the
generated file without removing-and-readding the subscriber.

**Effects (in order)**:

1. Resolve module.
2. Find the subscriber by `<topic-or-key>`. Error if missing:
   `subscriber for topic/key "<x>" not found on module "<m>"`.
3. Validate the spec's stored MsgType (in case the vault has
   changed under the subscriber).
4. Re-render the file from current spec values.

Output:

```
regenerated subscriber on module subber
  wrote /path/to/subber/swarmbotix_io/subscribers/image_raw.py
```

`sb sub edit` does **not** write `sb.module.yaml` for the entry it
regenerates (the spec is unchanged; only the file is). That's by
design. The exception (sb 0.5.0+) is a file that still holds legacy
`topic: /...` entries: those are rewritten as `key:` (§13.9).

### 9.6 `sb sub rm <topic-or-key> [--module <name>]`

Same shape as `sb pub rm` (§9.3) but keyed by topic or key, matched as
in §9.5 (a leading `/` is ignored).

### 9.7 `sb list [--module <name>]`

Renders both publishers and subscribers in a single table:

```
role  name       type               topic                          transport
----------------------------------------------------------------------------
pub   hello      std/StringStamped  hello                          zenoh
sub   image_raw  std/ImageStamped   dev01/demo/cam/iox2/image_raw  iceoryx2
```

Columns: role, name, type, topic, transport. The topic column shows the
entry's `topic`, or its `key` as stored, never with a leading `/`
(sb 0.5.0+). Widths auto-fit per invocation. An empty module renders:

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
( cd /path/to/subber && sb sub add -m ros2/std/StringStamped --key dev01/demo/pubber/zenoh/hello --zenoh )
```

The subscriber uses `--key` with the **publisher's full key** so both
ends land on the same wire. If both used the bare `hello`, the
publisher would publish on `dev01/demo/pubber/zenoh/hello` and the
subscriber would listen on `dev01/demo/subber/zenoh/hello`:
different keys; no traffic. See `examples/rust_python_zenoh/run.sh`
for a working end-to-end script.

### 10.2 Iterate on a publisher with `--force`

```bash
$ sb pub add -m ros2/std/StringStamped hello --zenoh
added publisher on module pubber
  wrote .../publishers/hello.rs
  updated .../sb.module.yaml

# Realize the type was wrong — change it without manual cleanup:
$ sb pub add -m ros2/std/Header hello --zenoh --force
  overwrote pub hello
added publisher on module pubber
  wrote .../publishers/hello.rs
  updated .../sb.module.yaml
```

`--force` is also useful in scripts that regenerate things from
scratch:

```bash
for pub in hello world; do
  sb pub add -m ros2/std/StringStamped --key "dev01/demo/api/zenoh/$pub" --zenoh --force
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

### 10.4 Cross-module pub/sub via `--key`

Two modules in the same workspace, one publishes a sensor reading the
other reads. The subscriber passes the publisher's full key with
`--key`:

```bash
sb pub add -m ros2/std/ImageStamped image_raw --iox2 --module camera
# camera publishes on dev01/demo/camera/iox2/image_raw

sb sub add -m ros2/std/ImageStamped --key dev01/demo/camera/iox2/image_raw --iox2 --module detector
# detector listens on the same key
```

`--key` is the working mechanism in 0.6.0. `sb 0.6.0+` also accepts the
same edge declared once in the workspace's `sb.flow.yaml`,

```yaml
wires:
  detector.image_raw: camera.image_raw
```

which `sb ws check` validates (both entries exist, transports and types
agree; [sbcli_ws.md](sbcli_ws.md) §4.7). Code generation does not read
`wires:` yet; a later release will derive the subscriber's key from it,
including the per-instance (`[{id}]`) and fan-in (`[*]`) forms that
`--key` cannot express. Until then, keep the `--key` entry.

### 10.5 Regenerate after a config change

Bumped `device:` from `dev01` to `dev02` in `sb.config.yml`. Old
generated files still reference `dev01`. `sb 0.5.0+`: regenerate in one
command (§14.6):

```bash
sb io update --all            # every module in the active workspace
sb io update camera           # one module
```

`sb io update` re-renders every publisher and subscriber from
`sb.module.yaml` without changing the YAML; services only with
`--services`. Only `topic:` entries pick up the new device; a `key:`
entry is verbatim, so its key keeps naming `dev01` until you change it
(`sb io edit <name> --key <new key>`).

Before 0.5.0 the sweep was `sb sub edit <topic>` per subscriber (which
re-renders without changing the YAML) and `rm` + `add` per publisher.

### 10.6 List everything across a workspace

There's no built-in `sb io list --all-modules` yet (planned for L6). For
now, iterate the installed `sb.flow.yaml`:

```bash
yq '.modules | to_entries[] | .key' ~/.swarmbotix/workspaces/demo/sb.flow.yaml | \
  while read -r mod; do
    echo "=== $mod ==="
    sb io list --module "$mod"
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
re-run `sb pub edit <pub> -t new_topic`, the validation step in
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
| Module not in cwd | `no module manifest (sb.module.yaml) in cwd (<dir>). Pass --module <name> or \`cd\` into a module dir.` |
| `--module <n>` miss | `module "<n>" not found. Searched:\n  - <path1>\n  - <path2>` |
| MsgType parse | `invalid message name "<m>": <reason from MessageNameError>` |
| MsgType missing | `message "<m>" not in vault. Run \`sb message list\` to see available types.` |
| Identifier | `invalid publisher name "<n>": <reason>` / `invalid subscriber name "<n>" (derived from topic): <reason>` |
| Publisher missing on edit/rm | `publisher "<n>" not found on module "<m>"` |
| Subscriber missing on edit/rm | `subscriber for topic/key "<x>" not found on module "<m>"` (sb 0.5.0+) |
| Already exists (default policy) | `sb pub add blocked by N existing entr<y|ies>:\n  - <role> "<n>" on topic "<t>" (<reason>)\n  ...\nRe-run with --force to overwrite, --skip-existing to no-op, or \`cd\` to a TTY and re-run for an interactive prompt.` |
| `pub edit` with no mutation flag | `at least one of -m/--msg, -t/--topic, or --key is required` (sb 0.5.0+) |
| `<topic>` positional containing `/` | `topic "/dev01/ws/mod/iox2/hello" may not contain '/'; for a verbatim wire key use --key dev01/ws/mod/iox2/hello` (sb 0.5.0+) |
| `sb.module.yaml` load: entry has both `topic` and `key` | `publishers[0]: exactly one of topic: or key: is required (both given)` (sb 0.5.0+) |
| manifest load: entry has neither | before 0.6.0 only: `publishers[0]: exactly one of topic: or key: is required (neither given)`. `sb 0.6.0+` reads such an entry as `topic: <name>` (§4) |
| `sb.module.yaml` load: `topic` contains `/` (not a leading one, §13.9) | `publishers[0]: topic "a/b" may not contain '/'; use key: for a verbatim wire key`, followed by a parenthetical naming the equivalent `key: <device>/<ws>/<module>/<transport>/a/b` (sb 0.5.0+) |
| `sb.module.yaml` load: empty `key` | `publishers[0]: key: is empty` (sb 0.5.0+) |
| `sb.module.yaml` load: raw entry with a `type` | `publishers[0]: encoding: raw takes no type:` (sb 0.5.0+) |
| `sb.module.yaml` load: no `type`, encoding not raw | `publishers[0]: type: is required unless encoding: raw` (sb 0.5.0+) |
| `sb.module.yaml` load: `mime` without raw | `publishers[0]: mime: is only valid with encoding: raw` (sb 0.5.0+) |
| `sb.module.yaml` load: `fb` / `raw` on iceoryx2 | `publishers[0]: encoding: is only valid for transport: zenoh` (sb 0.5.0+) |
| Name already used on the module | `name "<n>" is already used by <publisher\|subscriber\|service> "<n>" on module "<m>"; publishers, subscribers, and services share one name space` (sb 0.5.0+) |
| `sb io add --raw --iox2` | `iceoryx2 payloads are typed; use a message with a bytes field and --iox2` (sb 0.5.0+) |
| `sb io add` / `edit --fb` without compiled FlatBuffers | `flatbuffers backend not compiled for style <style>; run sb message compile <type> --fb` (sb 0.5.0+) |
| `sb io edit` / `rm` unknown name | `no publisher, subscriber, or service named "<n>" on module "<m>"` (sb 0.5.0+) |
| Transport not inferable (`sb topic listen` / `pub`, no `-t`) | `cannot infer transport from not/canonical: expected <device>/<ws>/<module>/<zenoh\|iox2>/<topic>, or pass -t zenoh\|iox2` |
| Structural-impossible (Flutter/Unity iceoryx) | `<lang> has no iceoryx2 path — Flutter and Unity run on mobile / sandboxed game runtimes where shared memory is unavailable. Use --zenoh.` |
| Not-yet-implemented (future languages) | `<lang> <transport> codegen is not yet implemented at this point in L3 ...` |

The load errors name the offending list and index: `publishers[<i>]`
or `subscribers[<i>]`.

---

## 13. Edge cases and gotchas

### 13.1 Topic containing `/`

`<topic>` (and `-t` on `sb pub edit`) is a bare name. Any `/` in it,
leading or inner, is refused (sb 0.5.0+):

```
error: topic "/dev01/ws/mod/iox2/hello" may not contain '/'; for a verbatim wire key use --key dev01/ws/mod/iox2/hello
```

To address a full wire key, pass it with `--key` (§13.7). Before 0.5.0
a leading `/` marked a verbatim path and an inner `/`
(`nested/path/leaf`) was prefixed like any bare name; neither spelling
is accepted on the command line any more.

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

`sb.module.yaml`'s `io_dir:` is relative to the module root. You can point
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

### 13.6 `--name` with a bare topic

```bash
sb pub add -m ros2/std/StringStamped hello -n other --zenoh
```

Stores `name: other, topic: hello`. The generated filename is
`publishers/other.rs`; the wire key is
`dev01/<ws>/<module>/zenoh/hello`. Useful when the natural name
collides with something else but the topic should keep its bare form.

### 13.7 `--key` bypasses namespace prefixing (sb 0.5.0+)

```bash
sb pub add -m ros2/std/StringStamped --key flat/path/hello --zenoh
```

`topic_full` is `flat/path/hello` verbatim, **not**
`dev01/<ws>/<module>/zenoh/flat/path/hello`. Useful for legacy ROS2
topics, for cross-workspace traffic, and for subscribing to another
module's publisher (§10.4). The user is responsible for including any
`<transport>` segment they want; `sb` does not inject it into a key.
The derived name is still `hello` (last segment). A leading `/` on the
value (`--key /flat/path/hello`) is dropped. Before 0.5.0 the same
effect came from a positional topic with a leading `/`; that spelling
is now refused (§13.1).

`sb topic listen` and `sb topic pub` read the transport only from the
five-segment form, so for a key like this one pass `-t zenoh` or
`-t iox2` (sb 0.4.1+); without it they exit 1 with
`cannot infer transport from flat/path/hello: expected <device>/<ws>/<module>/<zenoh|iox2>/<topic>, or pass -t zenoh|iox2`.

### 13.8 `sb sub list` with no module errors with a more useful message

When in a directory without a module manifest, `sb pub list` and
`sb sub list` error the same way as `sb pub add` — same module
resolution chain. To list a specific module from anywhere with an
active workspace:

```bash
sb pub list --module pubber
```

### 13.9 Legacy `topic: /...` entries (configs written before 0.5.0)

A module manifest (then `sb.dev.yml`) or `sb.prd.yml` written before 0.5.0 may hold entries
whose `topic` starts with `/`:

```yaml
subscribers:
  - name: image_raw
    topic: /dev01/demo/cam/iox2/image_raw
    type: ros2/std/ImageStamped
    transport: iceoryx2
```

sb 0.5.0+ reads such an entry as `key: dev01/demo/cam/iox2/image_raw`:
the leading `/` is dropped and the rest is used verbatim, which is the
same wire key the entry produced before. Generated code, `is_owned`
(false), and pairing are unchanged, and reading the file never
rewrites it.

- `sb doctor` reports each module of the active workspace that holds
  legacy entries on a `[WARN] topics` row naming the module, each
  entry with its old and new spelling, and the command that rewrites
  the file ([sbcli_doctor.md](sbcli_doctor.md) §4.10). The warning
  never fails doctor.
- Any `sb pub` / `sb sub` command that writes the file (`add`, `edit`,
  `rm`) rewrites every legacy entry in it as `key:` and says so:

  ```
    rewrote 1 legacy topic entry as key: in /path/to/subber/sb.module.yaml
  ```

  (`entries` when the count is not 1.)

A legacy entry with an inner `/` but no leading one (`topic: a/b`) is
**not** carried over. Before 0.5.0 it was prefixed like a bare name;
now the file fails to load with
`subscribers[0]: topic "a/b" may not contain '/'; use key: for a verbatim wire key`,
followed by a parenthetical naming the key it used to produce. Replace
the entry by hand with that key,
`key: <device>/<ws>/<module>/<transport>/a/b` (your own device,
workspace, module, and transport), to keep the same wire key.

---

## 14. `sb io` (sb 0.5.0+)

`sb io` is the module-interface surface that replaces `sb pub`,
`sb sub`, `sb list`, and `sb service init`. It edits the same
`sb.module.yaml` and renders the same files (§2, §6), with three additions:
one name space for publishers, subscribers, and services (§4.1); the
`encoding:` choice (§14.3); and `sb io update`, which regenerates a
module's `swarmbotix_io/` without changing its YAML (§14.6).

```
sb io add pub (<topic> | --key <WIRE_KEY>) -m <Type> [--zenoh | --iox2] [--proto | --fb] [-n <name>]
                                   [--force | --skip-existing] [--module <name>]
sb io add pub (<topic> | --key <WIRE_KEY>) --raw [<mime>] [--zenoh] [-n <name>] ...
sb io add sub ...                                   the same forms as `add pub`
sb io add service <name>                            [--module <name>]
sb io edit <name> [-m <Type>] [-t <topic> | --key <WIRE_KEY>] [--proto | --fb | --raw [<mime>]]
                                   [--module <name>]
sb io rm   <name>                                   [--module <name>]
sb io list                                          [--module <name>]   (alias: sb io ls)
sb io update [<module> | --all] [--services]
```

Every verb except `update` acts on one module, resolved as in §3
(`--module <name>`, else the `sb.module.yaml` in the current directory).
Mutations print the same lines as `sb pub` / `sb sub` (§9.1), for
example `added service on module api`.

### 14.1 `sb io add pub` / `sb io add sub`

| Argument | Meaning |
|---|---|
| `<topic>` | A bare name, stored as `topic:` and prefixed at codegen. May not contain `/` (§9.1, §13.1). |
| `--key <WIRE_KEY>` | A verbatim wire key instead, stored as `key:` (§13.7). The way to subscribe to another module's publisher. Conflicts with `<topic>`. |
| `-m <Type>` | Vault message, `<style>/<namespace>/<Leaf>` (§5.1). Required unless `--raw`; refused with `--raw`. |
| `--zenoh` / `--iox2` | Transport. Without a flag, `--fb` and `--raw` entries use Zenoh (the only transport they allow), and a `proto` entry uses the config default (§7: `transport_on_device`, iceoryx2 unless set). |
| `--proto` / `--fb` / `--raw [<mime>]` | Encoding, default `--proto` (§14.3). Because the mime is optional, put `--raw [<mime>]` after the topic, or the topic is read as the mime. |
| `-n`, `--name <n>` | Entry name. Default: the bare topic, or the key's last segment (§5.3). Needed when that name is already used by another entry of any kind on the module (§4.1). |
| `--force` / `--skip-existing` | Conflict policy for a name or topic collision, with the interactive prompt when neither is given on a terminal (§8). |

**Effects (in order)**: resolve the module (§3); validate the type
against the vault (§5.1) and the transport / encoding / type / mime
combination (§14.3); check the name is free across publishers,
subscribers, and services and find topic collisions (§5.2); render
`<io_dir>/publishers/<name>.<ext>` or `subscribers/<name>.<ext>` (§6);
write `sb.module.yaml`, rewriting any legacy `topic: /...` entries as
`key:` (§13.9).

```bash
sb io add pub pose -m ros2/std/StringStamped --zenoh               # proto, the default
sb io add pub image_raw -m ros2/std/ImageStamped --iox2            # iceoryx2: always the typed struct
sb io add pub imu -m swarmbotix/sensors/Imu --zenoh --fb           # FlatBuffers bytes
sb io add pub status --zenoh --raw text/json                       # raw bytes tagged text/json
sb io add pub blob --zenoh --raw                                   # raw bytes tagged application/octet-stream
sb io add sub --key dev01/ws_ips/ips_track/zenoh/pose -m ros2/std/StringStamped --zenoh --name track_pose
```

### 14.2 `sb io add service <name>`

Appends `{ name: <name> }` to `services:` and renders
`<io_dir>/services/<name>.<ext>`, a request/response router over Zenoh
queryables. Rust and Python modules only. The name must satisfy the
identifier rule (§5.4) and be unused by any entry on the module. The
router, its routes, and its key layout are described in
[sbcli_reqres.md](sbcli_reqres.md).

### 14.3 Encodings

| Flag | Stored as | `type:` | Transport | Wire tag | Generated Rust / Python / C++ API |
|---|---|---|---|---|---|
| `--proto` (default) | nothing (`proto` is the default) | required | `zenoh` or `iox2` | `application/protobuf;<type>` (zenoh) | typed and bytes (§6.4) |
| `--fb` | `encoding: fb` | required | `zenoh` | `application/flatbuffers;<type>` | bytes |
| `--raw [<mime>]` | `encoding: raw`, plus `mime: <mime>` when given | none | `zenoh` | `<mime>`, else `application/octet-stream` | bytes |

Rules, checked when an entry is added or edited and again whenever
`sb.module.yaml` is read (§12):

- **Zenoh only.** `--raw` or `--fb` with `--iox2` is an error: an
  iceoryx2 payload is always the fixed-layout struct generated from the
  `.proto` (`--raw --iox2` answers
  `iceoryx2 payloads are typed; use a message with a bytes field and --iox2`).
- **`--fb` needs compiled FlatBuffers.** The type's FlatBuffers output
  (`<message_targets>/fb/<ns>/<Leaf>/`) must exist, or the add or edit
  is refused with
  `flatbuffers backend not compiled for style <style>; run sb message compile <type> --fb`
  ([sbcli_messages.md](sbcli_messages.md) §4.5, §5.3).
- **`<mime>` is `type/subtype`**, optionally followed by `;`
  parameters, with no whitespace. `application/protobuf` and
  `application/flatbuffers` are refused for a raw entry, because
  listeners read those tags as a typed payload.
- **Both ends agree.** A subscriber must use the encoding of the
  publisher it listens to. `sb ws check` reports a wire key whose
  entries disagree ([sbcli_ws.md](sbcli_ws.md) §5.3).

How `sb topic listen` and `sb topic list` read each tag:
[sbcli_messages.md](sbcli_messages.md) §1.1.

### 14.4 `sb io edit <name>`

Changes the publisher or subscriber called `<name>` in place, or, with
no flag at all, only regenerates its file (what `sb sub edit` did).
A service has no editable fields
(``services have no editable fields; edit the file in <io_dir>/services``);
an unknown name answers
`no publisher, subscriber, or service named "<name>" on module "<module>"`.
The flags:

| Flag | Change |
|---|---|
| `-m <Type>` | New message type. |
| `-t <topic>` | New bare topic (`topic:`); no `/` (§9.1). |
| `--key <WIRE_KEY>` | New verbatim key (`key:`). Conflicts with `-t`. |
| `--proto` / `--fb` / `--raw [<mime>]` | New encoding. Switching to `--raw` drops the stored `type`; switching away from raw drops the stored `mime` and needs `-m`. |

The new topic or key is checked against the module's other entries
(§5.2), the result against §14.3, and then the file is re-rendered (its
name does not change) and `sb.module.yaml` is written. The transport cannot
be edited (§6.7): `sb io rm` then `sb io add`.

### 14.5 `sb io rm <name>` and `sb io list`

`sb io rm <name>` removes the publisher, subscriber, or service called
`<name>` from `sb.module.yaml` and deletes its generated file. Deleting a
service file discards any routes written in it.

`sb io list` (alias `sb io ls`) prints the `sb list` table (§9.7) for
every publisher, subscriber, and service on the module. It never
writes.

### 14.6 `sb io update [<module> | --all] [--services]`

Regenerates `swarmbotix_io/` from `sb.module.yaml` without changing the
YAML.

- Targets `<module>` (resolved as `--module` is, §3); else the module in
  the current directory when it holds `sb.module.yaml`; else, or with
  `--all`, every module registered in the active workspace, including
  `up: false` ones. That last form is the step `sb install` runs
  ([sbcli_ws.md](sbcli_ws.md) §5.2). An active workspace is required in
  every form, since its name is the `<workspace>` segment of each owned
  key.
- Every publisher and subscriber is re-rendered. A file whose rendered
  bytes already match the file on disk is reported unchanged and not
  touched. An entry that cannot render (its type is no longer in the
  vault, or there is no template for its language and transport) is
  reported as skipped with the reason, and the other entries still run.
- Services are regenerated only with `--services`. Their files are
  routers you edit, and regenerating one overwrites your routes.
- The one write to `sb.module.yaml`: legacy `topic: /...` entries are
  rewritten as `key:` (§13.9).

One line per module:

```
ips_track: 2 written, 1 unchanged, 0 skipped
calib: 0 written, 1 unchanged, 1 skipped (status: <why it did not render>)
```

A skipped entry or a module that fails to load makes the command exit
non-zero (`<n> entr(y/ies) or module(s) could not be updated`) after
every target has run.

Run it after changing `device:`, after a workspace is renamed or
reinstalled under another name, after pulling an `sb.module.yaml` edited
elsewhere, and after upgrading `sb` (so the files pick up new
templates).
