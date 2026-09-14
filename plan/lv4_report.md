# L4 — Topic Introspection (running report)

Last updated: 2026-05-28

---

## Status: done, runtime-verified end-to-end

Every "Done when" item from [level4.html](level4.html) AND every TDD
test from its plan now has a passing test. All gaps from the first
pass closed in this round.

```
sb topic list    [-t <sec>] [-k <kw>] [--case-sensitive]
                 [--transport zenoh|iceoryx2] [--json]
sb topic listen  <topic> [--transport zenoh|iceoryx2] [--raw] [-n <count>]
sb topic pub     <topic> <bytes-hex> [--transport zenoh|iceoryx2]
sb topic prune   [--json]
```

| # | "Done when" / TDD item | Verification | Status |
|---|---|---|---|
| DW1 | `sb topic list` reports L3 publisher within 2 s with schema + rate | [level4_e2e_rust_stamped.rs](../crates/sb-cli/tests/level4_e2e_rust_stamped.rs) → [examples/rust_zenoh_stamped/run.sh](../examples/rust_zenoh_stamped/run.sh) — drives a real L3-scaffolded zenoh publisher emitting `std/StringStamped` (Header + payload), asserts `schema:"std/StringStamped"` + `rate_hz:10` in `sb topic list --json` | ✅ |
| DW2 | Output snapshots stable; keyword filter ported | [level4_output_format.rs](../crates/sb-cli/tests/level4_output_format.rs) (4) + [sb-discover unit tests](../crates/sb-discover/src/) | ✅ |
| DW3 | `sb topic listen` decodes `StringStamped` to JSON; `--raw` emits hex | [level4_listen_decode.rs](../crates/sb-cli/tests/level4_listen_decode.rs) (dynamic decode via vault FileDescriptorSet + prost-reflect; foreign bytes fall back to hex honestly) | ✅ |
| DW4 | `sb topic pub` round-trips on both transports | [level4_pub_roundtrip.rs](../crates/sb-cli/tests/level4_pub_roundtrip.rs) (Zenoh) + [level4_pub_roundtrip_iceoryx.rs](../crates/sb-cli/tests/level4_pub_roundtrip_iceoryx.rs) (iox2) | ✅ |
| TDD1 | Live Zenoh discovery | [level4_discover_zenoh.rs](../crates/sb-cli/tests/level4_discover_zenoh.rs) (2 tests) | ✅ |
| TDD2 | Live iceoryx2 discovery | [level4_discover_iceoryx.rs](../crates/sb-cli/tests/level4_discover_iceoryx.rs) (2 tests) | ✅ |
| TDD3 | E2E vs L3 publishers | DW1 above | ✅ |
| TDD4 | Keyword filter ported | unit tests + a live-Zenoh + live-iox2 case | ✅ |
| TDD5 | Output snapshot — table + JSON | [level4_output_format.rs](../crates/sb-cli/tests/level4_output_format.rs) | ✅ |
| TDD6 | Bare-topic resolution | [sb-discover resolve.rs](../crates/sb-discover/src/resolve.rs) (6 unit tests) | ✅ |
| TDD7 | listen decode against L3 publisher | DW3 above | ✅ |
| TDD8 | pub round-trip both transports | DW4 above | ✅ |
| TDD9 | Rate from single sample | [level4_rate_from_header.rs](../crates/sb-cli/tests/level4_rate_from_header.rs) (publisher pushes at ~20 Hz but declares 10 Hz; assertion catches the declared value, not the observed) | ✅ |

```bash
# Default suite (no extra runtime deps):
cargo test -p sb-discover -p sb-listen        # 24 unit (probe, render, resolve, decode, hex parser)
cargo test -p sb-cli --test 'level4_*'        # 4 default + 12 ignored gates

# Heavy gates (one binary at a time — cross-binary Zenoh state can flake
# when run all together; each is reliable on its own):
for t in level4_discover_zenoh level4_discover_iceoryx \
         level4_rate_from_header level4_cli_topic_list \
         level4_pub_roundtrip level4_pub_roundtrip_iceoryx \
         level4_listen_decode level4_e2e_rust_stamped; do
  cargo test -p sb-cli --test $t -- --ignored --test-threads=1
done
# Result this run: 12/12 pass.
```

---

## What landed this round

### 1. iceoryx2 holds — discovery + round-trip both work

[`level4_discover_iceoryx.rs`](../crates/sb-cli/tests/level4_discover_iceoryx.rs)
spawns a `[u8]`-slab iox2 service in-process and verifies
`discover_iceoryx` surfaces it with `schema` populated from the
`Service::list` type-name registry. Keyword filter case covered too.

[`level4_pub_roundtrip_iceoryx.rs`](../crates/sb-cli/tests/level4_pub_roundtrip_iceoryx.rs)
pre-arms an iox2 subscriber in a thread, runs the CLI subprocess to
publish 4 bytes, asserts the subscriber sees the exact bytes.

The fix that unblocked these: **`IceoryxFrames` must hold the
PortFactory alive**. Previously the factory dropped at the end of
`listen_iceoryx`, which silently invalidated the subscriber port's
access to the service config — every `send()` reported "delivered to
0 subscribers". Captured in source comments at [iceoryx.rs](../crates/sb-listen/src/iceoryx.rs)
field-ordering remark. Diagnosed via a temporary smoke test that
compared sb-listen helpers against raw iox2 docs examples; smoke
test removed once root cause was fixed.

Also tightened `listen_iceoryx` to `open_or_create()` so a
subscriber can race ahead of any publisher (matches Zenoh's
`declare_subscriber` semantics). The publisher path adds a 200 ms
post-send settle window so the publisher port doesn't drop before
the subscriber polls — needed for the one-shot CLI invocation case.

### 2. True L3 → discovery end-to-end example

New [`examples/rust_zenoh_stamped/`](../examples/rust_zenoh_stamped/):

```
examples/rust_zenoh_stamped/
├── run.sh                      — L4 e2e gate; mints SB_HOME, scaffolds
│                                  pubber via `sb pub add`, runs the
│                                  pubber, runs `sb topic list --json`,
│                                  asserts schema + rate appear.
└── pubber/
    ├── Cargo.toml              — standalone crate (zenoh 1.9 + prost 0.13)
    ├── build.rs                — prost-build over message_definitions/std/{Header,StringStamped}.proto
    └── main.rs                 — uses the L3-scaffolded `chatter.rs` publisher
                                   AND a real prost-generated `StringStamped`
                                   with `header.metadata.{msg_type, msg_freq_desired}`
                                   filled in — exactly what an end user would write.
```

[`level4_e2e_rust_stamped.rs`](../crates/sb-cli/tests/level4_e2e_rust_stamped.rs)
delegates to `run.sh` (single source of truth — example breaks ⇒
test catches it; test passes ⇒ example is guaranteed to work).

This closes the previously documented "synthetic publisher only" gap.
The discovery half of L4 is now exercised against the **actual L3
codegen path** end-to-end.

### 3. Dynamic protobuf decode in `sb topic listen`

[`sb_listen::decode_frame_dynamic`](../crates/sb-listen/src/decode.rs)
consumes a `prost_reflect::DescriptorPool` and emits a
`FramePayload::Json` envelope for any Header-leading `*Stamped`
payload whose schema resolves in the pool. The CLI builds the pool
on each `sb topic listen` invocation from the vault's
FileDescriptorSet (compiled fresh via `sb_vault::compile_to_descriptor_set`).
If `protoc` is missing or the vault is empty, the CLI surfaces a
`warning:` on stderr and continues with hex output — never silently
lies about what's on the wire.

Falls back to hex (with `schema` still populated when known) for:
- raw mode (`--raw` flag),
- foreign / non-Stamped payloads,
- schemas the consumer's pool doesn't know.

Schema → protobuf full-name mapping is mechanical:
`std/StringStamped` → `swarmbotix.std.StringStamped` (every vault
`.proto` declares `package swarmbotix.<namespace>;`).

Tests in [`level4_listen_decode.rs`](../crates/sb-cli/tests/level4_listen_decode.rs):
- decoded JSON contains `data: "hello-from-test"` (the payload field)
  AND nested `header.metadata.{msgType, msgFreqDesired}` (proving
  recursive prost-reflect decode works through nested messages),
- foreign bytes fall back to hex with `schema: null`.

### 4. Tests added this round

| File | Tests | Notes |
|---|---|---|
| [level4_discover_iceoryx.rs](../crates/sb-cli/tests/level4_discover_iceoryx.rs) | 2 (`#[ignore]`'d) | `[u8]`-slab service discovery + keyword filter |
| [level4_pub_roundtrip_iceoryx.rs](../crates/sb-cli/tests/level4_pub_roundtrip_iceoryx.rs) | 1 (`#[ignore]`'d) | CLI subprocess publishes; in-process subscriber sees the bytes |
| [examples/rust_zenoh_stamped/run.sh](../examples/rust_zenoh_stamped/run.sh) + [level4_e2e_rust_stamped.rs](../crates/sb-cli/tests/level4_e2e_rust_stamped.rs) | 1 (`#[ignore]`'d) | true L3 publisher → discovery; schema + rate assertions |
| [level4_listen_decode.rs](../crates/sb-cli/tests/level4_listen_decode.rs) | 2 (`#[ignore]`'d) | dynamic JSON decode via vault FileDescriptorSet; foreign bytes fall back to hex |
| [sb-listen decode unit](../crates/sb-listen/src/decode.rs) | +1 | `schema_to_protobuf_full_name` mapping |

---

## Test totals after this round

- **24 unit tests** across sb-discover + sb-listen (header probe,
  render, resolve, decode + new dynamic-decode mapping, hex parser).
- **4 default integration tests** in sb-cli (level4_output_format).
- **12 ignored integration tests** — every TDD item from
  level4.html has a live runtime gate.
- **L3 regression check**: 64 of 64 default L3 tests still pass.

---

## Design decisions worth documenting

### Why `IceoryxFrames` keeps the PortFactory

iox2's subscriber port internally references the service factory's
shared state. When the factory drops, the port can lose access to
the service config — observed as "delivered to 0 subscribers" with
zero error noise. Field order in `IceoryxFrames` (sub, _service,
_node) is load-bearing for Drop sequencing and documented inline at
[iceoryx.rs](../crates/sb-listen/src/iceoryx.rs).

### Why the dynamic-decode pool is rebuilt per `sb topic listen` invocation

`sb topic listen` is a long-running command (until SIGINT). Building
the pool once on invocation is essentially free and guarantees the
schema view matches what's in the vault NOW — no risk of a stale
cached pool decoding new types as opaque. If startup latency
matters later (e.g. swarmctl), caching becomes a pool-builder
service rather than an attribute of `sb-listen`.

### Why we map `std/StringStamped` → `swarmbotix.std.StringStamped` instead of storing the proto full-name in the Header

Vault names (`<ns>/<Leaf>`) are the user-facing identity — they
match `sb pub add -m <type>` flags and `sb message list`. Protobuf
package names (`swarmbotix.<ns>.<Leaf>`) are an implementation
detail of the wire format. Keeping the Header field user-facing
means the on-wire data stays stable if we ever change the proto
package convention. The mapping function lives in one place
([decode.rs::schema_to_protobuf_full_name](../crates/sb-listen/src/decode.rs))
with a unit test pinning it.

### Why `examples/rust_zenoh_stamped/pubber/` is a standalone crate (its own `[workspace]`)

Same pattern as the L3 examples — keeps the build hermetic, lets
each example pin its own dependency versions, and means
`cargo build` from the forge root doesn't try to compile the example.

---

## Known gaps / follow-ups

- **JSON-via-schema `sb topic pub`** (e.g. `sb topic pub /chatter
  '{"data":"hi"}'`) is still deferred. The dynamic decoder reads
  bytes → JSON cleanly; the encoder direction needs the same pool
  + DynamicMessage::parse_text/serde to encode user JSON → wire
  bytes. Mechanical from here, but unscoped for L4. Lands at L5
  alongside `sb gopro`.

## Follow-up landed 2026-05-28

- **`sb topic prune`** — closes the loop on the `(dead)` marker that
  L4's discovery already surfaces. iceoryx2 service registrations are
  on-disk and outlive their owning process when a publisher crashes
  or gets `kill -9`'d; until now the only recovery was deleting the
  iox2 state directory by hand. `prune_iceoryx()` in `sb-discover`
  walks `Node::list(Config::global_config(), …)` and, for every
  `NodeState::Dead`, calls `try_remove_stale_resources()`. iceoryx2's
  `ResourcesAlreadyCleanedUp` is treated as success (another instance
  beat us to it; same net effect). `--json` emits the `PruneReport`
  shape `{cleaned_pids, failed}`. Zenoh is session-based and has no
  on-disk state to prune, so the verb is iceoryx2-only by design.
- **Cross-test contamination on live gates.** Same caveat carried
  over from L3: each L4 ignored gate is reliable when run alone,
  but running all 12 in one `cargo test` can intermittently fail
  from cross-binary Zenoh / iox2 state. Run them per-binary as
  shown above for clean signal.
- **iceoryx2 0.8.1 vs 0.9 system mismatch** carried over from L1 —
  doctor flags it; runtime tests bundle their own internals so this
  doesn't bite the L4 gates. To be fixed when the dev box updates
  `libiceoryx2_ffi_c.so`.

## Follow-up landed 2026-07-25 — Windows iceoryx2 startup noise

`sb topic list` on Windows printed two iceoryx2 warnings ahead of its
output:

```
0 [W] "User::from_uid(4294967295)"
| Unable to acquire user entry details ... `/etc/passwd` (2147483647). ...
1 [W] "Config::global_config()"
| No config file was loaded, a config with default values will be used.
```

**Diagnosis — not a bug in `sb` or in iceoryx2.** iceoryx2 resolves its
config file exactly once, lazily, on the first `Config::global_config()`
(reached by `Service::list`, `Node::list`, and every `NodeBuilder::new()`).
That lookup tries three paths; on Windows the per-user one needs the home
dir via `getpwuid_r`, which the Windows PAL stubs to `-1` wholesale
(`iceoryx2-pal-posix/src/windows/pwd.rs` — all four `pw*`/`gr*` fns return
`-1`, since Windows has no `/etc/passwd`). `getuid()` likewise returns
`uid_t::MAX` = 4294967295. iceoryx2 warns and continues by design — the
`Err(UnknownError) => warn + Ok(User { details: None })` branch in
`user.rs` exists for exactly this case. `User::from_self()` has **one**
call site in all of `iceoryx2` + `iceoryx2-cal`: `config.rs:574`, the
config-dir lookup. Nothing in the data path touches user identity, and
Windows shared memory is a real Win32 implementation
(`CreateFileMappingA` / `MapViewOfFile` / `OpenFileMappingA`). The
`(no topics)` result was correct — `C:\Temp\iceoryx2\services\` was empty.

**Fix.** `warm_iceoryx_global_config()` in `sb-cli/src/main.rs`, called
once at the top of `real_main()` after `Cli::parse()`. It runs the
config lookup with the iceoryx2 log level at `Error`, populating the
once-cell before any command can trigger it; every later call hits the
`is_initialized()` early-return and stays silent. No `#[cfg]` — the Linux
case (global-config warning only; `getpwuid_r` succeeds there) is a
strict subset of the Windows case. The level is restored via
`set_log_level_from_env_or_default()` so genuine post-startup iceoryx2
warnings still print. `IOX2_LOG_LEVEL` opts out entirely — set it and
nothing is suppressed. `iceoryx2` moved from dev-deps to deps on
`sb-cli` for this (already in the build graph; no new compile cost).

Verified on Windows 11: default run is clean, `IOX2_LOG_LEVEL=warn`
brings both warnings back, `--json` stdout stays empty of log output
(the iceoryx2 console logger writes to stderr via `cerrln`).

**Still open — `sb topic prune` floods stderr on Windows.** Separate
mechanism, *not* fixed by the above: `win32_call!` in
`iceoryx2-pal-posix/src/windows/win32_call.rs:124` calls `std::eprintln!`
directly, bypassing the iceoryx2 logger entirely, so no log level can
suppress it. On this box `sb topic prune` emits many
`< Win32 API error > ... FlushFileBuffers ... [ 5 ] Access is denied`
and `FindNextFileA` lines while walking ~21 stale node registrations in
`C:\Temp\iceoryx2\nodes\`. Needs its own decision (filter prune's stderr,
or take it upstream) — scoped out of this fix.

Also note `installguide_windows.md` §9.10 ("iceoryx2 runtime on Windows —
unverified") is now partly answered: iox2 demonstrably creates and
monitors runtime state on Windows (node dirs with `.node_monitor`,
`.node_monitor_owner_lock`, `.port_tag`). The shared-memory *transport*
still needs a live publisher/subscriber pair to confirm.

**Shipped in 0.1.30** (cut 2026-07-25 via `/sb-release patch`). Staged at
`platforms/windows/dist/0.1.30/`, smoke-tested against a sandbox `SB_HOME`,
and installed to `C:\Users\hylee\.swarmbotix`. Verified from `C:\Users\hylee`:
`sb topic list` → `(no topics)`, no `[W]` lines. Existing `sb.config.yml` and
all 138 message definitions preserved.

---

## `sb topic listen` never decoded to JSON — protobuf name mapping

**Symptom.** `decode_frame_dynamic` resolved the schema name from the
`Header` probe, then failed to find it in the `DescriptorPool` and fell
through to the hex branch. Every sb-generated frame printed as hex with a
populated `schema` column, which reads as "your consumer is missing the
schema" rather than "the lookup is broken".

**Two independent faults in `schema_to_protobuf_full_name`:**

1. **The style segment was not dropped.** `split_once('/')` on the
   now-canonical 3-segment name `ros2/std/StringStamped` yielded
   `ns="ros2"`, `leaf="std/StringStamped"` → `swarmbotix.ros2.std/StringStamped`.
   A protobuf full name containing `/` resolves in no pool, ever. The
   function's doc comment still described the pre-0.1.35 2-segment name.
2. **The `swarmbotix.` prefix was hard-coded.** It is correct for `std/` and
   the whole `swarmbotix` style, but the ROS2 mirror declares **bare**
   packages — `sensor_msgs`, `geometry_msgs`, `nav_msgs`, … — which is
   **130 of the 138 `ros2` protos**. Even with fault 1 fixed, the prefix
   alone would have kept ~94% of the ros2 style undecodable.

**Fix.** Replaced with `protobuf_name_candidates`, which takes the **last
two** segments (handling the 2- and 3-segment forms identically) and offers
both package conventions, most-qualified first:

```
ros2/std/StringStamped → [swarmbotix.std.StringStamped, std.StringStamped]
ros2/nav_msgs/Path     → [swarmbotix.nav_msgs.Path,     nav_msgs.Path]
```

`decode_frame_dynamic` takes the first that resolves in the pool. No
guessing, and adding a third convention later is one more entry.

**Known limit, now documented in-code:** the protobuf package encodes no
style, so two styles defining the same `<ns>/<Leaf>` are indistinguishable
at this layer and whichever the pool holds wins. Fixing that properly means
putting the style into the package, which is a wire-format change.

**Why the suite was green through all of it.** The one test that exercises
the real path, `level4_listen_decode`, is `#[ignore]`'d behind live
zenoh + protoc and does not run in a normal `cargo test`. The unit test that
did run, `schema_name_maps_to_protobuf_full_name`, asserted
`nav_msgs/Path → swarmbotix.nav_msgs.Path` — it encoded fault 2 as expected
behavior. A test can only protect an invariant it states correctly.

**Verified.** Four replacement unit tests cover style-dropping, 2-vs-3
segment equivalence, both package conventions, and degenerate input. The
live test was run explicitly on this box and passes:
`PATH=<protoc> cargo test --test level4_listen_decode -- --ignored` →
**2 passed**, decoding a real zenoh `StringStamped` frame to
`{"data":"hello-from-test"}`.
