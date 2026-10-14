# Consuming `sb io` Codegen: Per-Language Integration

`sb io add` (sb 0.5.0+; the deprecated `sb pub add` / `sb sub add` generate the same files) produces files under `<module>/<io_dir>/{publishers,subscribers,services}/<name>.<ext>`. It does **not** produce the entry-point (`main.cpp`, `main.py`, `src/main.rs`) or the build glue (`CMakeLists.txt`, `pyproject.toml`, `Cargo.toml`). This doc covers the hand-off: how to write your entry-point + build files so they correctly consume the generated structure and, crucially, how to refer to topics so your code doesn't drift the moment the user renames a workspace, flips a transport, or runs multiple instances.

The worked examples in §5 to §7 use iceoryx2. For Zenoh entries, and the typed protobuf API they get (sb 0.5.0+), see §8.1.

This is the missing piece between [sbcli_init.md](sbcli_init.md) (which scaffolds the module shell) and [sbcli_pubsub.md](sbcli_pubsub.md) (which describes what gets generated).

---

## 1. Topic naming — the rules that protect you from silent drift

Topics are the most common place for hand-written code to silently desync from sb's generated code. Read this section before writing any code that references a topic string.

### 1.1 The canonical form (sb 0.1.5+)

Every wire-topic is fully qualified with **five** segments, and has no
leading `/` (sb 0.5.0+):

```
<device>/<workspace>/<module>/<transport>/<bare_topic>
```

| Segment | Source | Example |
|---|---|---|
| `<device>` | `sb config get device` | `dev01` |
| `<workspace>` | active workspace name (`cat ~/.swarmbotix/active`) | `ws1` |
| `<module>` | `module:` in this module's `sb.module.yaml` | `app1` |
| `<transport>` | literal `iox2` or `zenoh`, picked from the entry's `--iox2` / `--zenoh` flag | `iox2` |
| `<bare_topic>` | the positional `<topic>` of `sb io add pub` / `sb io add sub` | `hello` |

A topic added with `--iox2` and a topic added with `--zenoh` on the same module + same bare name are **two different wire paths** that never pair with each other. The `<transport>` segment exists exactly to prevent that collision.

### 1.2 Bare-topic convention

The bare topic you type on the CLI:

- **`snake_case`**, lowercase, ASCII letters / digits / `_`.
- Must satisfy `validate_identifier` for the derived `<name>` (first char letter/`_`, then alphanumerics/`_`).
- No `-`, `.`, spaces, or uppercase. Matches ROS2.

| Good | Bad |
|---|---|
| `hello`, `image_raw`, `cmd_vel`, `joint_state` | `chatMessage`, `image-raw`, `image.raw`, `My Topic` |

Note: the `<module>` and `<device>` segments **do** accept `-` (added in sb 0.1.11 so runtime overrides like `controller-2` parse). See §3 for the runtime-override API.

### 1.3 The wire form and the one rule

At codegen time, the bare topic gets folded into the canonical 5-segment key, and that key **is** the wire form (sb 0.5.0+): Zenoh KeyExpr forbids a leading `/`, so the canonical form has none, and iceoryx2 accepts internal `/`. Both transports produce the **same string shape**; only the `iox2`/`zenoh` segment differs:

```
topic_full   = <device>/<workspace>/<module>/<transport>/<bare_topic>      # canonical = wire
```

For the entry `sb io add pub hello -m ros2/std/StringStamped --iox2` on module `app1` in workspace `ws1` on device `dev01`, the key is `dev01/ws1/app1/iox2/hello`, used verbatim as the iceoryx2 `ServiceName`.

**The one rule, in capital letters:**

> **NEVER HARDCODE A WIRE-FORMAT TOPIC STRING IN YOUR CODE.** Always reference the generated `<Name>_SERVICE` / `<Name>_TOPIC` constant — or, when you want runtime overrides, the per-segment `<Name>_DEFAULT_*` constants — from `swarmbotix_io/{publishers,subscribers}/<name>.<ext>`.

If you write `"dev01/ws1/app1/iox2/hello"` literally anywhere in your own code, you have just broken the contract that lets `sb` rename workspaces, modules, devices, or transports without breaking your build. The generated file is the **single source of truth** for the wire format.

### 1.4 Cross-module pub/sub: the `--key` form

When a subscriber lives in a **different module** from the publisher, the subscriber must pass the **publisher's full key** with `--key`, **including the `<transport>` segment** (sb 0.5.0+):

```bash
# Publisher in module `camera`, with --iox2:
sb io add pub image_raw -m ros2/std/ImageStamped --iox2 --module camera
# → publishes on dev01/demo/camera/iox2/image_raw

# Subscriber in module `detector`; note the `iox2` segment:
sb io add sub --key dev01/demo/camera/iox2/image_raw -m ros2/std/ImageStamped --iox2 --module detector
```

If you forget the `<transport>` segment in the key, the subscriber listens on `dev01/demo/camera/image_raw` (4 segments) while the publisher publishes on `dev01/demo/camera/iox2/image_raw` (5 segments): different keys, no traffic, no error.

### 1.5 What `sb` does for you with a bare topic

A bare topic (`hello`, stored as `topic:`) gets the full prefix injected by codegen: `dev01/<ws>/<module>/<transport>/hello`. A `--key` value (`--key anything/else`, stored as `key:`) is used verbatim, and `sb` does **not** inject the `<transport>` segment. Use the bare form whenever possible; only reach for `--key` for cross-module or legacy ROS2 pairing. A positional topic may not contain `/` at all (sb 0.5.0+); the error names the `--key` spelling to use instead.

---

## 2. Cardinal rules for consuming generated code

1. **The generated file is meant to be included/imported, not modified.** `sb io edit` and `sb io update` regenerate it; hand-edits are lost. (Service routers under `services/` are the exception: you write your routes in them, and only `sb io update --services` overwrites them.)
2. **Reference `<Name>_SERVICE` / `<Name>_TOPIC` for the wire topic.** Or, for runtime overrides, reference `<Name>_DEFAULT_*` (see §3).
3. **For iceoryx2: the flat payload type is pulled in by sb-codegen automatically.** The generated module re-exports it as `<Leaf>` (Rust / Python) or aliases it as `<Name>_Payload` (C++).
   - **Rust / Python** bake an absolute path to `<message_targets>/iox2/<pkg>/<Leaf>/<Leaf>.<ext>` in at `sb io add` time; no `mod iox_types { include!(...) }` or `PYTHONPATH` wiring required.
   - **C++** emits a bare `#include "<Leaf>.h"` (the basename). You must add `-I<message_targets>/iox2/<pkg>/<Leaf>` to your build glue once per consumed message — see §5.2 for the canonical CMake snippet. The basename form was adopted in sb 0.1.23 so the generated file is portable across host and Docker builds (the same translation unit no longer hardcodes `/home/<dev>/.swarmbotix/...`).

   Re-running `sb message compile --iox2` or moving `message_targets` invalidates the generated file; `sb io update` regenerates it with the new path.
4. **Cross-language pub/sub requires matching iceoryx2 payload-type names** on both sides. The sb codegen pins all three languages to the proto leaf (see §4) — no user wrapper needed.
5. **iceoryx2 `try_recv_latest()` returns a zero-copy `Sample` handle, not a payload copy** (sb 0.1.22+). The returned object aliases the publisher's shared-memory slot — drop it as soon as you've read what you need so iceoryx2 can reclaim the slot. See §7.4 (Rust), §5.4 (C++), §6.3 (Python) for the per-language access pattern. Zenoh subscribers are unaffected — the payload is bytes off the wire, not shared memory, so it is returned by value.
6. **A Zenoh entry's API follows its `encoding:`** (sb 0.5.0+). A `proto` entry (the default) gets a typed `publish(msg)` and `try_recv_latest_msg` in Rust, Python, and C++ next to the bytes API; `fb` and `raw` entries are bytes-only. See §8.1.

---

## 3. `TopicOverrides` — runtime per-segment override (sb 0.1.11+)

The generated publisher/subscriber class has **two** constructors:

- A no-arg `open()` / `__init__(node)` that uses the codegen-baked defaults.
- An override-aware form (`open_with` / `module=...` kwargs / `TopicOverrides` struct) that lets you replace any of `<device>`, `<workspace>`, `<module>`, `<bare_topic>` **at construction time**.

This is the supported way to run **N instances of the same module** (swarm), point a publisher at a different device for a remote replay, or rebind a topic without re-running `sb io edit`.

> **Instance id form.** The `id` is just a string: anything matching `[A-Za-z0-9_-]+` works. Numeric (`--id 2`) and named (`--id left`) forms are equivalent; the latter is conventional when each instance has a distinct role (`left`/`right`, `front`/`rear`) rather than just an index. `sb 0.6.0+`: the canonical ids are the `instances` under the module's entry in `sb.flow.yaml` ([sbcli_ws.md](sbcli_ws.md) §4.6); `sb up` and `sb run` pass each one to its process as `--id <id>` and as the environment variable `SB_INSTANCE_ID` (§10).

### 3.1 What gets emitted alongside the class

For a publisher named `hello` (bare-topic input), the generated file exposes these:

| Symbol | Value | Notes |
|---|---|---|
| `<Name>_DEFAULT_DEVICE` | `"dev01"` | from `sb.config.yml::device` |
| `<Name>_DEFAULT_WORKSPACE` | `"ws1"` | active workspace |
| `<Name>_DEFAULT_MODULE` | `"app1"` | from `sb.module.yaml::module` |
| `<Name>_DEFAULT_TOPIC` | `"hello"` | the bare topic |
| `<Name>_TRANSPORT_SEGMENT` | `"iox2"` or `"zenoh"` | not overridable — transport is fixed by template choice |

Reference these constants instead of hardcoding. They update on every `sb io edit` / `sb io update`.

### 3.2 The override API per language

The iceoryx2 publisher/subscriber classes are **non-template**: the codegen bakes in the specific payload type. The Zenoh classes carry no payload type in their constructor; a `proto` entry's typed methods are generic over the message instead (§8.1). Either way, every constructor takes only transport handles and (optionally) `TopicOverrides`:

```rust
// Rust — TopicOverrides with Default impl. No `<T>` generic.
HelloPublisher::open(&session)?;                                         // defaults
HelloPublisher::open_with(&session, TopicOverrides {
    module: Some(format!("{}-{}", DEFAULT_MODULE, id)),
    ..Default::default()
})?;
// Full-name override — bypass segment assembly entirely:
HelloPublisher::open_with(&session, TopicOverrides {
    full: Some("legacy/path/from/elsewhere".into()),
    ..Default::default()
})?;
```

```cpp
// C++ — designated initializers on the override struct. No template arg.
swarmbotix_io::HelloPublisher pub(node);                                 // defaults
swarmbotix_io::HelloTopicOverrides ov;
std::string mod = std::string(swarmbotix_io::Hello_DEFAULT_MODULE) + "-" + id;
ov.module = mod;                                                          // string_view — keep `mod` alive!
swarmbotix_io::HelloPublisher pub(node, ov);
// Full-name override — bypass segment assembly entirely. The backing
// string must outlive `pub` because `full` is a `string_view`.
std::string wire = "legacy/path/from/elsewhere";
ov = {}; ov.full = wire;
swarmbotix_io::HelloPublisher pub2(node, ov);
```

```python
# Python — kwargs. No payload_type argument (codegen pins it).
HelloPublisher(node)                                                     # defaults
HelloPublisher(node, module=f"{DEFAULT_MODULE}-{id}")
HelloPublisher(node, full="legacy/path/from/elsewhere")                  # bypass assembly
```

```dart
// Flutter — named args (Zenoh only; raw bytes payload).
await HelloPublisher.open(session);                                      // defaults
await HelloPublisher.open(session, module: '$HelloDefaultModule-$id');
await HelloPublisher.open(session, full: 'legacy/path/from/elsewhere');  // bypass assembly
```

```csharp
// Unity — optional parameters (Zenoh only; raw bytes payload).
new HelloPublisher(session);                                             // defaults
new HelloPublisher(session, module: $"{DefaultModule}-{id}");
new HelloPublisher(session, full: "legacy/path/from/elsewhere");          // bypass assembly
```

**`full` semantics (added in sb 0.1.21):** When set, the string is used **verbatim** as the iceoryx2 `ServiceName` / Zenoh key expression: no `<transport>` injection, no segment defaults. Per-segment fields are ignored. Use it when you need to pair with a runtime-computed name or a foreign / legacy path from your `main()` without re-running `sb io add`.

### 3.3 `is_owned`: when overrides DON'T apply (foreign topics, `key:` entries)

The override machinery is only emitted for a `topic:` entry, that is, when the user passed a **bare** topic to `sb io add`. For a `key:` entry (e.g. `sb io add sub --key dev01/demo/cam/iox2/image_raw -m ros2/std/ImageStamped --iox2`), the generated file omits `<Name>_DEFAULT_*` + `<Name>TopicOverrides` and emits a single `SERVICE = "..."` const verbatim. The reasoning: a key belongs to whoever owns that path; the subscriber is just listening, and runtime overrides on the subscriber's side don't change which wire key the publisher writes to.

**Practical implication for swarms with cross-module subscribers:** if app1 publishes on `dev01/ws/app1-2/iox2/state` (instance 2 via override), an app2 subscriber registered with `sb io add sub --key dev01/ws/app1/iox2/state ...` (no `-2`) will not pair. Since sb 0.1.21 the bare-topic path is the cleanest fix; a `key:` entry still loses overrides at codegen time. Options:

1. **Switch to a bare-topic subscriber and use the `full` override at runtime.** Re-add via `sb io add sub state -m ... --iox2 --module remote-app1` (or any bare topic), then in your subscriber's `main()` set `TopicOverrides { full: Some(format!("dev01/ws/app1-{id}/iox2/state")), ..Default::default() }`. The generated wrapper does the rest. New since 0.1.21 (`full` field).
2. **Re-add the subscriber per instance** (`sb io add sub --key dev01/ws/app1-2/iox2/state ... --name state_2`). N instances → N entries; doesn't scale.
3. **Manually build the wire path in your subscriber's `main()`** and call the underlying transport API directly (bypass the generated `HelloSubscriber`). See §10.4 for a worked sketch.
4. **Use Zenoh, not iceoryx2.** Zenoh KeyExpr supports wildcards: `dev01/ws/app1-$*/zenoh/state` matches every instance. (`*` matches only a whole chunk between two `/`, so `app1-*` is not a valid key expression; `$*` is the wildcard that matches inside a chunk.) iceoryx2 services don't: each subscriber connects to exactly one ServiceName.

A future sb release may parse `key:` entries into segments and emit `TopicOverrides` for them too. Until then, the rule above holds.

The `is_owned` semantics are **unchanged** by the string-form id introduction (sb 0.1.13+). Whether the id is `2` or `left`, a foreign subscriber registered with a `key:` will still fail to pair with an override-shifted publisher.

---

## 4. Cross-language type identity (iceoryx2)

iceoryx2 enforces payload-type identity via a UTF-8 string name registered on the service. Mismatch → `PublishSubscribeOpenError(IncompatibleTypes)`.

**Convention for the sb ecosystem:** the iceoryx2 type name is always the **proto leaf name** (`StringStamped`, `ImageStamped`, `Twist`). sb-iox2-typegen pins it on every emission, so all three languages agree without any user wrapper:

| Language | How sb pins the name |
|---|---|
| Rust   | `#[derive(ZeroCopySend)]` plus `#[type_name("<Leaf>")]` — iceoryx2 reads the attribute, not the Rust type path. Necessary because each consumer crate compiles its own copy of the iox2-flat module, so `type_name::<T>()` would otherwise differ between projects. |
| C++   | `static constexpr const char* IOX2_TYPE_NAME = "<Leaf>";` on the generated POD. iceoryx2-cxx looks for this constant before falling back to the mangled C++ symbol. |
| Python | The class name is the leaf (`Image1280x1024`, `StringStamped`). iceoryx2's Python binding uses `payload_type.__name__`. |

If you see `PublishSubscribeOpenError(IncompatibleTypes)` between a pub and a sub that both use sb-generated code, the most common cause is **stale `~/.swarmbotix/messages/ros2/message_targets` from a pre-`<T>`-removal build**: re-run `sb message compile --iox2 <pkg>/<Leaf>` to overwrite, then `sb io update` to regenerate the wrappers.

---

## 5. C++ worked example

### 5.1 `main.cpp` — defaults + `--id` swarm override

```cpp
#include <chrono>
#include <cstring>
#include <iostream>
#include <string>
#include <thread>

#include "iox2/iceoryx2.hpp"
#include "swarmbotix_io/publishers/hello.cpp"    // generated by `sb io add pub`
// NOTE: the generated file `#include`s the payload header by basename
// (`#include "StringStamped.h"`). Resolution depends on CMake adding
// `-I<message_targets>/iox2/std/StringStamped` (see §5.2). The class
// is exposed as `swarmbotix_io::Hello_Payload`. No user `#include` and
// no IOX2_TYPE_NAME wrapper are needed.

int main(int argc, char** argv) {
  // Parse `--id N` (optional). Empty = single-instance.
  std::string id;
  for (int i = 1; i + 1 < argc; ++i) {
    if (std::string(argv[i]) == "--id") { id = argv[i + 1]; break; }
  }

  using namespace iox2;
  // The pinned iox2 C++ binding's `bb::Expected` has no `.expect()` — use
  // `.has_value()` / `.value()`. The generated publisher class wraps this
  // internally via `detail::expect_or_throw`; you only see it at the call
  // sites you write yourself.
  auto node_exp = NodeBuilder().create<ServiceType::Ipc>();
  if (!node_exp.has_value()) {
    std::cerr << "create node failed\n";
    return 1;
  }
  auto node = std::move(node_exp).value();

  // Build the override. `string_view` fields reference `mod_storage`,
  // so it MUST outlive `pub` — declare it before `pub` and don't reassign.
  std::string mod_storage;
  swarmbotix_io::HelloTopicOverrides ov;
  if (!id.empty()) {
    mod_storage = std::string(swarmbotix_io::Hello_DEFAULT_MODULE) + "-" + id;
    ov.module = mod_storage;
  }
  swarmbotix_io::HelloPublisher pub(node, ov);

  std::cout << "publishing on " << pub.service_name() << " at 30 Hz\n";

  auto next = std::chrono::steady_clock::now();
  const auto period = std::chrono::nanoseconds(1'000'000'000 / 30);
  uint64_t seq = 0;
  while (true) {
    swarmbotix_io::Hello_Payload payload{};
    payload.header_timestamp_ns =
        std::chrono::duration_cast<std::chrono::nanoseconds>(
            std::chrono::system_clock::now().time_since_epoch()).count();
    std::string text = "hello #" + std::to_string(seq++);
    std::strncpy(payload.data, text.c_str(), sizeof(payload.data) - 1);
    pub.publish(payload);
    next += period;
    std::this_thread::sleep_until(next);
  }
}
```

**Key points:**

- `int main(int argc, char** argv)` — so `--id 2` reaches the binary. For a module with instances, `sb up` appends `--id <id>` to the start command itself (see §5.3), and also sets `SB_INSTANCE_ID`, which `std::getenv` reads without any argument parsing.
- `swarmbotix_io::Hello_Payload` — type alias the codegen emits for `<iox2_cpp_namespace>::<Leaf>` (e.g., `swarmbotix_std::StringStamped`). Use it directly; no `IOX2_TYPE_NAME` wrapper.
- `mod_storage` lifetime — `HelloTopicOverrides::module` is `std::string_view`. The backing string must outlive `pub`. Declaring `mod_storage` before the `if` and never reassigning it after `pub` is constructed keeps this honest.
- `pub.service_name()` — accessor on the publisher returns the actual wire string in use (defaults or override-resolved). Useful for logging.
- `swarmbotix_io::HelloPublisher` — no template argument; the payload type is baked in. Pre-0.1.13 code used `HelloPublisher<HelloPayload>` — drop the `<...>` when migrating.
- `#include "swarmbotix_io/publishers/hello.cpp"` — yes, including a `.cpp`. The publisher class definition must live in a header so `Hello_Payload` is available to callers; the `.cpp` extension is a sb-codegen convention. **Don't add it to `add_executable(... main.cpp hello.cpp)`** — that produces ODR violations.

### 5.2 `CMakeLists.txt`

```cmake
cmake_minimum_required(VERSION 3.20)
project(<module_name> CXX)
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

# iceoryx2-cxx install root — strip `/lib/libiceoryx2_ffi_c.so` from `sb config get libiceoryx2`.
list(PREPEND CMAKE_PREFIX_PATH "/opt/iceoryx2/current")
find_package(iceoryx2-cxx CONFIG REQUIRED)

add_executable(<module_name> main.cpp)

# The generated `swarmbotix_io/publishers/<name>.cpp` does `#include "<Leaf>.h"`
# by basename (sb 0.1.23+). Resolve it via `-I<message_targets>/iox2/<pkg>/<Leaf>`.
# `SB_MESSAGE_TARGETS` overrides (set it in a container build, or in the
# manifest's env:); otherwise ask sb. Repeat the `target_include_directories`
# line per consumed message.
if(DEFINED ENV{SB_MESSAGE_TARGETS})
  set(SB_MSG_TARGETS "$ENV{SB_MESSAGE_TARGETS}")
else()
  execute_process(COMMAND sb config get message_targets
                  OUTPUT_VARIABLE SB_MSG_TARGETS
                  OUTPUT_STRIP_TRAILING_WHITESPACE
                  COMMAND_ERROR_IS_FATAL ANY)
endif()

target_include_directories(<module_name> PRIVATE
  ${CMAKE_SOURCE_DIR}
  ${SB_MSG_TARGETS}/iox2/std/StringStamped       # one -I per consumed message
)

target_link_libraries(<module_name> PRIVATE iceoryx2-cxx::shared-lib-cxx)
```

Available CMake targets in iceoryx2-cxx v0.9.x (unchanged across 0.9.0 → 0.9.3): `iceoryx2-cxx::includes-only-cxx`, `iceoryx2-cxx::static-lib-cxx`, `iceoryx2-cxx::shared-lib-cxx` (recommended).

### 5.3 `sb.module.yaml`

No launch script (sb 0.6.0+). The C++ defaults configure and build with
CMake (`cmake -S . -B build -DCMAKE_BUILD_TYPE=Release`, then
`cmake --build build --config Release`) and start `build/<module>` with
`{args}`, plus `--id <id>` for each instance: exactly what the
`main(argc, argv)` in §5.1 parses. Declare only what differs:

```yaml
module: app1                     # the CMake target above is named after the module
language: cpp
platforms:
  linux:
    env: { LD_LIBRARY_PATH: "/opt/iceoryx2/current/lib:${LD_LIBRARY_PATH}" }
```

Listing only `linux` under `platforms` also marks the module Linux-only;
add `windows: {}` (with its own `env`) when it builds there too. A CMake
target with another name needs a `start` override,
`"build/<target> {args}"`, and on Windows, where the multi-config
generator writes `build/Release/<target>.exe`, a
`platforms.windows.scripts.start` of its own. Everything else is in
[sbcli_module.md](sbcli_module.md) §3.2 and §6.

### 5.4 C++ subscriber — zero-copy receive (iceoryx2)

For an iceoryx2 subscriber, the generated header emits:

```cpp
// `<Name>_Payload` aliases the flat iox2 type (e.g. swarmbotix_std::ImageStamped).
using Hello_Payload = swarmbotix_std::StringStamped;

// `<Name>Sample` aliases the iceoryx2 Sample handle. Move-only.
using HelloSample =
    iox2::Sample<iox2::ServiceType::Ipc, Hello_Payload, void>;

class HelloSubscriber {
 public:
  // Drains the queue, returns the newest sample as a zero-copy handle.
  std::optional<HelloSample> try_recv_latest();
};
```

Consume it like this:

```cpp
swarmbotix_io::HelloSubscriber sub(node);

while (true) {
  if (auto msg = sub.try_recv_latest()) {
    // `msg` is std::optional<HelloSample>. Use `msg->payload()` (NOT
    // `*msg` — Sample has no operator*) to get a `const Hello_Payload&`
    // that aliases the publisher's shared-memory slot. Read fields off
    // that reference; do not copy unless you genuinely need ownership.
    const auto& payload = msg->payload();
    std::cout << payload.header_timestamp_ns << '\n';
    // `msg` (and thus the slot reservation) ends with this scope.
  }
  std::this_thread::sleep_for(std::chrono::milliseconds(10));
}
```

**Lifetime rules:**

- `HelloSample` is **move-only** (deleted copy constructor) — it owns the
  reservation on one slot. Move it (or its containing `std::optional`)
  freely; do not try to copy.
- Holding the optional keeps the slot reserved on the publisher side.
  Long-lived storage of `HelloSample` will eventually exhaust the
  publisher's slot count. Drain → read → drop.
- The `Hello_Payload&` returned by `msg->payload()` is invalidated the
  moment the optional is destroyed or reassigned. Don't outlive `msg`
  with a saved reference.

**If you need an owned copy** of the payload (e.g. to hand off to another
thread that may outlive the read loop), copy explicitly: `Hello_Payload
owned = msg->payload();`. That's a `memcpy` of the POD out of shmem —
opt in only when ownership is genuinely required.

---

## 6. Python worked example

### 6.1 `main.py` — defaults + `--id` swarm override

```python
import argparse
import time

# The generated module re-exports `StringStamped` directly — it's loaded
# via importlib from the absolute path baked at `sb io add` time. No
# PYTHONPATH wiring for the payload type.
from swarmbotix_io.publishers.hello import (
    HelloPublisher,
    StringStamped,
    DEFAULT_MODULE,
    open_node,
)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--id", default="", help="swarm instance discriminator")
    args = ap.parse_args()

    module = f"{DEFAULT_MODULE}-{args.id}" if args.id else None
    node = open_node()
    pub = HelloPublisher(node, module=module)
    print(f"publishing on {pub.service_name}", flush=True)

    seq = 0
    while True:
        msg = StringStamped()
        msg.header_timestamp_ns = time.time_ns()
        msg.data = f"hello #{seq}".encode()[: len(msg.data) - 1] + b"\0"
        pub.publish(msg)
        seq += 1
        time.sleep(0.02)


if __name__ == "__main__":
    main()
```

**Key points:**

- `HelloPublisher(node, module=...)` — no `payload_type` argument; the codegen pins it. `module=None` falls back to `DEFAULT_MODULE`.
- `StringStamped` is re-exported from the generated module; the importlib-from-absolute-path machinery is in the generated file (no user setup).
- `pub.service_name` — instance attribute set in the constructor; reflects defaults or overrides.
- Subscriber variant: see §6.3 — `try_recv_latest()` returns a zero-copy `Sample` wrapper, not an owned struct.

### 6.2 `sb.module.yaml`

```yaml
module: app1
language: python
```

Nothing else to declare when the entry point is `main.py` at the module
root (sb 0.6.0+). The default start is `{python} main.py {args}`, where
`{python}` is the module's `.venv/bin/python` (`.venv/Scripts/python.exe`
on Windows) when it exists, else `python3` (`python` on Windows); an
instance also gets `--id <id>`, which the `argparse` in §6.1 reads.

No `PYTHONPATH` is needed: the generated module loads the payload type
from the absolute `message_targets` path, and Python puts `main.py`'s
directory, the module root, first on `sys.path`, so
`from swarmbotix_io.publishers...` resolves.

To pin a particular interpreter, such as a conda environment, override
`start` with an environment reference rather than a machine-specific
path:

```yaml
scripts:
  start: "${CONDA_PREFIX:-/usr}/bin/python3 main.py {args}"
```

`${CONDA_PREFIX}` is read from the shell you run `sb up` in
([sbcli_module.md](sbcli_module.md) §5).

The `iceoryx2` Python module ships separately (`pip install iceoryx2`). `sb doctor` does not check it — verify yourself with `<your-python> -c "import iceoryx2"`.

### 6.3 Python subscriber — zero-copy receive (iceoryx2)

The generated subscriber module emits **two** classes: `HelloSubscriber`
(the polling subscriber) and `HelloSample` (a zero-copy view wrapper).
`try_recv_latest()` returns `Optional[HelloSample]`:

```python
from swarmbotix_io.subscribers.hello import HelloSubscriber, open_node

node = open_node()
sub = HelloSubscriber(node)

while True:
    msg = sub.try_recv_latest()
    if msg is not None:
        # `msg.<field>` forwards via __getattr__ to a ctypes Structure
        # that aliases the publisher's shared-memory slot — no copy.
        print(msg.header_timestamp_ns)
        # If you need the raw ctypes Structure (e.g. for `ctypes.addressof`,
        # `memoryview`, slicing): use `msg._view`. There is intentionally no
        # public `.payload` accessor — that name would shadow a proto
        # field literally called `payload`.
    time.sleep(0.01)
```

**Lifetime rules:**

- The `HelloSample` wrapper holds two references: the iceoryx2 `Sample`
  (keeps the slot reserved) and a `ctypes.Structure` view (aliases the
  shmem). Both must outlive any field read.
- Field reads (`msg.header_timestamp_ns`, etc.) are forwarded to the
  ctypes Structure via `__getattr__`. They do **not** copy.
- When `msg` is GC'd or reassigned, the Sample handle is released → the
  shmem slot returns to the publisher's free list. Don't pin
  long-lived references to `msg._view` past the lifetime of `msg`.

**If you need an owned copy:**

```python
import ctypes
owned = type(msg._view).from_buffer_copy(msg._view)
# Now `owned` is independent of the iceoryx2 slot.
```

This is the only path that allocates — opt in only when the payload
needs to outlive the read loop.

---

## 7. Rust worked example

### 7.1 `src/main.rs` — defaults + `--id` swarm override

```rust
#[path = "swarmbotix_io/publishers/hello.rs"]
mod hello;

// The generated module re-exports the iox2-flat type. No `mod
// iox_types { include!(...) }`, no shared payload crate.
use hello::StringStamped;

use iceoryx2::prelude::*;
use std::env;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn parse_id() -> Option<String> {
    let mut args = env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--id" { return args.next(); }
    }
    None
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let node = NodeBuilder::new().create::<ipc::Service>()?;

    let id = parse_id();
    let ov = hello::TopicOverrides {
        module: id.map(|i| format!("{}-{}", hello::DEFAULT_MODULE, i)),
        ..Default::default()
    };
    let publisher = hello::HelloPublisher::open_with(&node, ov)?;

    let mut seq: u64 = 0;
    loop {
        let mut msg = StringStamped::default();
        msg.header_timestamp_ns = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos() as u64;
        // ... populate msg.data, msg.header_frame_id, ... per the iox2-flat layout ...
        publisher.publish(msg)?;
        seq += 1;
        std::thread::sleep(Duration::from_millis(20));
    }
}
```

**Key points:**

- `hello::HelloPublisher` — no `<T>` generic; the payload type is baked in. Pre-0.1.13 code used `HelloPublisher<StringStamped>` — drop the generic when migrating.
- `use hello::StringStamped;` — the codegen re-exports it. Internally it loads the iox2-flat file via `#[path = "<abs>"] mod __payload;`, which means cross-project pub/sub agrees on iceoryx2's wire type-name because sb-iox2-typegen pins `#[type_name("StringStamped")]` on the struct (see §4).

### 7.2 `Cargo.toml`

```toml
[package]
name = "<module>"
version = "0.1.0"
edition = "2021"

[dependencies]
iceoryx2 = "=0.9.3"  # exact pin — must equal what `sb` links (sb 0.1.40+)
```

The user's `Cargo.toml` must depend on `iceoryx2` (not just import it) — the generated `hello.rs` includes the iox2-flat type whose `#[derive(ZeroCopySend)]` resolves through the `iceoryx2` crate path.

**Pin the patch digit, not just the minor (sb 0.1.40+).** iceoryx2 stamps its
full `major.minor.patch` into every shared-memory segment and `open()` compares
it for *exact* equality — a 0.9.0 process cannot see services created by a
0.9.3 process. It fails with `VersionMismatch`, and `sb topic list` reports
`(no topics)` against a perfectly live publisher. A caret-style requirement
(`iceoryx2 = "0.9"`) lets your `Cargo.lock` freeze on whatever patch existed
the day it was written, which is exactly how that mismatch happens.

The version to pin is whatever your `sb` links, and everything on the host must
agree with it to the patch digit: the Rust crate above, the Python wheel
(`pip show iceoryx2`), and the C/C++ shared library at the `libiceoryx2` path in
`sb.config.yml` (`sb doctor` prints it). On the reference Linux host that path
is `/opt/iceoryx2/current/lib/libiceoryx2_ffi_c.so`, where `current` is a
symlink to the installed `v<x.y.z>` tree — read the link to see which version
you are on. **sb 0.1.40 ships against 0.9.3.**

**Windows builds need LLVM.** The `iceoryx2` crate runs `bindgen` when it
builds for a Windows target, and `bindgen` loads libclang. Install LLVM
(for example `winget install LLVM.LLVM`) and point `LIBCLANG_PATH` at
its `bin` directory, which a module can do portably in its manifest
([sbcli_module.md](sbcli_module.md) §6.3):

```yaml
platforms:
  linux: {}
  windows: { env: { LIBCLANG_PATH: "${LIBCLANG_PATH:-C:/Program Files/LLVM/bin}" } }
```

### 7.3 `sb.module.yaml`

```yaml
module: app1
language: rust
```

Nothing else to declare when the binary carries the module's name, as
the package name or a `[[bin]] name` (sb 0.6.0+): the defaults run
`cargo build --release` and start `target/release/<module>` with
`{args}`, plus `--id <id>` for an instance, which `parse_id()` in §7.1
reads. A binary named otherwise needs one line:

```yaml
scripts:
  start: "target/release/pub_hello{exe} {args}"
```

`{exe}` adds `.exe` on Windows, so the same line works on both
platforms ([sbcli_module.md](sbcli_module.md) §3.2).

### 7.4 Rust subscriber — zero-copy receive (iceoryx2)

The generated subscriber module emits both the subscriber type and a
`<Name>Sample` type alias for the iceoryx2 `Sample` handle. `try_recv_latest`
returns `Result<Option<<Name>Sample>, _>` — the `Sample` aliases the
publisher's shared-memory slot.

```rust
#[path = "swarmbotix_io/subscribers/hello.rs"]
mod hello;

use iceoryx2::prelude::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let node = NodeBuilder::new().create::<ipc::Service>()?;
    let subscriber = hello::HelloSubscriber::open(&node)?;

    loop {
        if let Some(msg) = subscriber.try_recv_latest()? {
            // `msg` is `hello::HelloSample`
            //   = `iceoryx2::sample::Sample<ipc::Service, StringStamped, ()>`.
            // Sample implements `Deref<Target = StringStamped>`, so
            // field access auto-derefs through to the shmem-backed payload
            // — no copy.
            println!("ts_ns={}", msg.header_timestamp_ns);
            // If you want to format the whole payload: `println!("{:?}", *msg)`
            // (one explicit deref to get `&StringStamped`).
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
```

**Lifetime rules:**

- `HelloSample` is move-only (no `Clone`) — it owns the slot
  reservation. Move it freely; `Drop` releases the slot.
- Auto-deref makes field reads transparent (`msg.field`), but a
  literal `*msg` only yields a `StringStamped` reference if you bind
  it (`let payload: &StringStamped = &*msg;`). You can't move
  the payload out of `*msg` because `Sample`'s deref target isn't
  owned by you.
- The `Sample` is `Send` when the iceoryx2 service is built with a
  thread-safe sync policy (the default for `ipc::Service`). You can
  hand the sample to another thread; the receiving thread is now on
  the hook to drop it promptly.

**If you need an owned copy:**

```rust
let owned: StringStamped = *msg;   // requires `StringStamped: Copy`
// or
let owned: StringStamped = (*msg).clone();   // requires `StringStamped: Clone`
```

The iox2-flat types emitted by `sb message compile --iox2` are POD
(`#[derive(Copy)]` is added where the layout permits), so the `*msg`
form usually works. Opt in only when ownership is genuinely required.

---

## 8. Zenoh payloads, Flutter, and Unity

### 8.1 Encodings and the typed protobuf API (sb 0.5.0+)

A Zenoh entry's payload is set by its `encoding:`, chosen with `sb io add ... --proto` (the default), `--fb`, or `--raw [<mime>]` ([sbcli_pubsub.md](sbcli_pubsub.md) §14.3). The generated wrapper owns the key expression and stamps every sample with the entry's wire tag, exported as `WIRE_TAG` (Rust, Python) or `<Name>_WIRE_TAG` (C++). What you hand it, and get back, depends on the encoding:

| `encoding` | Publisher (Rust / Python / C++) | Subscriber (Rust / Python / C++) |
|---|---|---|
| `proto` | `publish(msg)` serializes a protobuf message; `publish_bytes(...)` sends bytes you serialized yourself | `try_recv_latest()` returns the bytes; `try_recv_latest_msg` parses them as your protobuf type |
| `fb` | **bytes only**: `publish(bytes)`, the same as `publish_bytes(bytes)`; build the buffer with the bindings `sb message compile --fb` emits | **bytes only**: `try_recv_latest()`; read it with the same bindings |
| `raw` | **bytes only**, as for `fb`; the content is whatever the entry's `mime:` says | **bytes only** |

The Flutter (Dart) and Unity (C#) wrappers are bytes-only for every encoding and set no wire tag (§8.2).

**Where the protobuf types come from.** Python and C++ use what `sb message compile --proto` emits under `<message_targets>/proto/<pkg>/<Leaf>/` (`<Leaf>_pb2.py`; `<Leaf>.pb.h` + `<Leaf>.pb.cc`, linked against libprotobuf). `sb` emits no Rust protobuf code ([sbcli_messages.md](sbcli_messages.md) §5.2): generate prost types in your crate (for example with `prost-build`) from the vault `.proto` and depend on the `prost` crate, since the typed methods are generic over `prost::Message`.

Rust, for a `proto` entry `pose` of type `ros2/std/StringStamped` (`StringStamped` being your prost type):

```rust
#[path = "swarmbotix_io/publishers/pose.rs"]
mod pose;
#[path = "swarmbotix_io/subscribers/cmd.rs"]
mod cmd;

let publisher = pose::PosePublisher::open(&session)?;
publisher.publish(&StringStamped { data: "hello".into(), ..Default::default() })?;  // typed
publisher.publish_bytes(&already_serialized)?;                                       // bytes

let subscriber = cmd::CmdSubscriber::open(&session)?;
if let Some(msg) = subscriber.try_recv_latest_msg::<StringStamped>()? { /* typed */ }
if let Some(bytes) = subscriber.try_recv_latest()? { /* Vec<u8> */ }
```

`publish` refuses a `Vec<u8>`, `String`, or `Bytes` argument with an error (prost would wrap them a second time); send those with `publish_bytes`. A payload that does not decode as `M` makes `try_recv_latest_msg` return an error.

Python (the classes come from `<Leaf>_pb2`):

```python
from swarmbotix_io.publishers.pose import PosePublisher
from swarmbotix_io.subscribers.cmd import CmdSubscriber
from StringStamped_pb2 import StringStamped

pub = PosePublisher(session)
pub.publish(StringStamped(data="hello"))      # typed: SerializeToString() for you
pub.publish_bytes(already_serialized)         # bytes (publish() also passes bytes through)

sub = CmdSubscriber(session)
msg = sub.try_recv_latest_msg(StringStamped)  # StringStamped or None
raw = sub.try_recv_latest()                   # bytes or None
```

C++ (`StringStamped` from `StringStamped.pb.h`):

```cpp
#include "swarmbotix_io/publishers/pose.cpp"
#include "swarmbotix_io/subscribers/cmd.cpp"

swarmbotix_io::PosePublisher pub(session);
StringStamped msg;
msg.set_data("hello");
pub.publish(msg);                                     // typed: SerializeAsString()
pub.publish(std::vector<uint8_t>{/* bytes */});       // bytes overload, same as publish_bytes

swarmbotix_io::CmdSubscriber sub(session);
if (auto m = sub.try_recv_latest_msg<StringStamped>()) { /* typed */ }
if (auto bytes = sub.try_recv_latest()) { /* std::vector<uint8_t> */ }
```

The C++ `try_recv_latest_msg` returns `std::nullopt` both when nothing arrived and when the payload does not parse.

For an `fb` or `raw` entry the same files expose only the bytes calls (`publish`, `publish_bytes`, `try_recv_latest`). Both ends of a wire key must use the same encoding; `sb ws check` reports a key whose entries disagree.

### 8.2 Flutter / Unity

iceoryx2 is unavailable in these runtimes (mobile / game sandbox — no POSIX shared memory). Use `--zenoh` instead. The consumption pattern follows the same shape (include generated file, reference `<Name>_DEFAULT_*` for overrides, call `open(...)` with named args) but:

- The payload over Zenoh is **raw bytes**, whatever the entry's `encoding:`; you serialize/deserialize yourself.
- Typical serialization choice: `proto` (protobuf) bindings from `sb message compile --proto`, or `fb` (FlatBuffers) from `--fb`.
- Cross-language type-identity (§4) is **not** a concern with Zenoh: the wire carries bytes plus an encoding tag, not an iceoryx2 type name.
- Swarm overrides work identically: `await HelloPublisher.open(session, module: 'controller-$id')` in Flutter, `new HelloPublisher(session, module: $"controller-{id}")` in Unity.

---

## 9. Common pitfalls

### 9.1 C++ binding API: the pinned v0.9.x install vs upstream (sb 0.1.23+)

The C++ codegen targets the **iceoryx2 install at `/opt/iceoryx2/current`** — the same version `sb` itself links, **0.9.3 as of sb 0.1.40** (see §7.2 on pinning the patch digit). Its C++ binding differs from upstream `iceoryx2-cxx` in three places:

| Operation | Upstream `iceoryx2-cxx` | v0.9.x (this install) | What the codegen emits |
|---|---|---|---|
| Unwrap `bb::Expected` | `.expect("…")` | `.has_value()` / `.value()` | `detail::expect_or_throw(expr, "…")` — inline templated helper that throws `std::runtime_error` on `!has_value()`. Inline so it is ODR-safe across translation units. |
| Send a `SampleMut` | `sample.send()` | free function `iox2::send(std::move(sample))` | `detail::expect_or_throw(iox2::send(std::move(initialized)), "send")` |
| `SampleMutUninit::write_payload` | accepts `const T&` | takes `T&&` (rvalue ref) | `sample.write_payload(<Name>_Payload(msg))` — explicit rvalue copy so the user-facing `publish(const Payload&)` signature stays unchanged |

These shapes are baked into `crates/sb-codegen/templates/cpp/publisher_iceoryx.cpp.j2` and `crates/sb-codegen/templates/cpp/subscriber_iceoryx.cpp.j2`. Pre-0.1.23 generated files emit `.expect("…")` and `.send()` directly and **will not compile against this install**; run `sb io update` to regenerate.

**Diagnose a stale generated file:**

```bash
grep -nE '\.expect\(|initialized\.send\(' swarmbotix_io/publishers/*.cpp swarmbotix_io/subscribers/*.cpp
```

Any hit means the file predates 0.1.23. `sb io update` (or `sb io edit <name>` for one entry) regenerates it with the v0.9.x-compatible shape.

**Subscriber receive shape.** `subscriber_.receive()` in v0.9.x returns `bb::Expected<bb::Optional<Sample>, ReceiveError>` — the outer `Expected` is unwrapped via `detail::expect_or_throw`; the inner `bb::Optional` empty state breaks the drain loop inside `try_recv_latest()`. Same `std::optional<<Name>Sample>` return shape as before; the internal machinery is what changed.

### 9.2 `TopicOverrides` `string_view` lifetime (C++)

The C++ override struct holds `std::string_view` fields to avoid forcing every default into a heap allocation. If you build the override module name as a temporary:

```cpp
ov.module = std::string("controller") + "-" + id;   // ← BUG: temporary destroyed
```

— the `string_view` dangles by the time the publisher constructor runs. Always store the `std::string` in a variable that outlives the constructor:

```cpp
std::string mod_storage = std::string("controller") + "-" + id;
ov.module = mod_storage;   // mod_storage outlives the constructor — OK
HelloPublisher pub(node, ov);
```

If this trips up too many users, file a request to switch the struct fields to owning `std::string`s.

### 9.3 `PublishSubscribeOpenError(IncompatibleTypes)` (cross-language / cross-project)

Publisher and subscriber registered different payload type-name strings on the same iox2 service. Under the post-`<T>` API sb-iox2-typegen pins the type name to the proto leaf in all three languages (Rust: `#[type_name("<Leaf>")]`; C++: `IOX2_TYPE_NAME = "<Leaf>"`; Python: class name), so this should not happen with freshly compiled types. If you see it, the most common cause is **stale `~/.swarmbotix/messages/ros2/message_targets`** holding a pre-`<T>`-removal version of the iox2-flat type: re-run `sb message compile --iox2 <pkg>/<Leaf>` to overwrite, then `sb io update` to regenerate.

If the mismatch persists after recompile, the secondary cause is a layout drift between projects: one project's `message_targets` has an older version of the proto than the other's. Bring both `message_targets` trees to the same revision and recompile both ends.

### 9.4 Stale iox2 state across schema changes

iceoryx2 persists service metadata under `/tmp/iceoryx2/` and `/dev/shm/iox2_*`. Re-running with a changed payload schema or type-name fails with `PublishSubscribeOpenOrCreateError`. Clean (when no iceoryx2 processes are live):

```bash
rm -rf /tmp/iceoryx2 /dev/shm/iox2_*
```

### 9.5 Subscriber `--key` omits the `<transport>` segment

The most common topic-mismatch trap:

```bash
# WRONG: 4-segment key, but publisher emits 5 segments:
sb io add sub --key dev01/demo/cam/image_raw -m ros2/std/ImageStamped --iox2 --module detector

# RIGHT: include the `iox2` segment:
sb io add sub --key dev01/demo/cam/iox2/image_raw -m ros2/std/ImageStamped --iox2 --module detector
```

Both succeed at codegen time (a key is not validated); the subscriber just never receives traffic. `sb io list` on the subscriber's module shows the stored key, and `sb topic list` shows the publisher's live key: the two must be identical, character for character.

### 9.6 Foreign-topic (`key:`) subscribers don't get `TopicOverrides`

If you registered the subscriber with `--key`, the generated file omits `<Name>_DEFAULT_*` + `<Name>TopicOverrides`. See §3.3 for why and what to do if you need per-instance subscription targeting.

### 9.7 Hardcoded wire-format topics drift silently

If you write `dev01/ws1/app1/iox2/hello` (the wire key, which is also the canonical form since sb 0.5.0) anywhere except in code generated by sb, every workspace/module/device/transport rename silently breaks that callsite while sb-managed callsites update on the next `sb io update`. **Always go through the generated `<Name>_SERVICE` / `<Name>_TOPIC` / `<Name>_DEFAULT_*` constants.**

### 9.8 Forgetting `{args}` in a custom start command

Every language default ends in `{args}` (`{bin} {args}`, `{python} main.py {args}`). A custom `scripts.start` without it drops the instance's `args` and `sb run`'s passthrough args; the `--id <id>` of an instance still arrives, because `sb` appends it after expansion (sb 0.6.0+). Symptom: `sb run app1 -- --verbose` changes nothing. Keep `{args}` in any `start` you write ([sbcli_module.md](sbcli_module.md) §13.2).

### 9.9 `main()` without `argc, argv`

C-style `int main()` is legal but ignores all CLI args. Use `int main(int argc, char** argv)` (or `if __name__ == "__main__": main()` with `argparse` in Python; or `env::args()` in Rust) so `--id 2` is actually visible to the override logic.

### 9.10 Generated file has `.cpp` extension but acts as a header

`swarmbotix_io/publishers/<name>.cpp` defines a class template — must be header-includable. The extension is a sb-codegen convention. **Don't add it to `add_executable(... main.cpp <name>.cpp)`** — that produces ODR violations. Treat it like a `.hpp`.

### 9.11 Python `iceoryx2` module not found

`sb doctor` checks the C library (`libiceoryx2_ffi_c.so`) but not the Python bindings. Verify with `<your-python> -c "import iceoryx2"`. If missing: `pip install iceoryx2`. Make sure the module starts with the **same** interpreter that has the package installed: the default `{python}` is the module's `.venv` interpreter if it has one, else `python3` as found in the tmux environment, which may not be your activated conda environment. Give the module a `.venv`, or pin the interpreter in `scripts.start` (§6.2).

### 9.12 `sb init` does not scaffold `main.{cpp,py,rs}` or build files

Intentional — `sb init` **adopts** an existing project, it doesn't scaffold a new one (`sb` never runs `cargo new`, `python -m venv`, or writes `cmake_minimum_required`). The user brings the entry-point and build system; this doc shows the canonical shape for each.

### 9.13 Same bare topic on both transports — they DON'T collide

In sb 0.1.5+ you can have

```bash
sb io add pub hello -m ros2/std/StringStamped --iox2  --name hello_shm
sb io add pub hello -m ros2/std/StringStamped --zenoh --name hello_net
```

on the same module. They publish to `dev01/ws/mod/iox2/hello` and `dev01/ws/mod/zenoh/hello` respectively: different wire paths, no overlap. Useful when you want the same logical message available both on local shared memory and over the network. Entry names are unique within a module (sb 0.5.0+), hence the two `--name`s; `sb io list` shows them as two distinct entries.

---

## 10. Running multiple instances (swarm semantics)

Same module code, N processes, each publishing on a distinct wire path. This is the swarm-controller / multi-drone-sim pattern.

### 10.1 The pattern in one paragraph

`sb.flow.yaml` lists the module's instances → `sb up` starts one tmux window per instance, named `<module>-<N>`, and appends `--id N` to each start command, also exporting `SB_INSTANCE_ID=N` → the binary reads its id → it constructs `TopicOverrides { module: <DEFAULT_MODULE>-<N> }` → the publisher opens on `<device>/<ws>/<module>-<N>/<transport>/<topic>`. The window name and the wire segment are the same string (sb 0.6.0+).

### 10.2 Declare and launch N instances

```yaml
# sb.flow.yaml
modules:
  app1: { path: modules/app1, instances: [1, 2, 3] }
```

```bash
sb install .          # the installed copy picks up the instances
sb up                 # windows app1-1, app1-2, app1-3
sb run app1:2         # rebuild, then restart only instance 2
sb stop app1          # close all three
```

Each instance:

- Runs in its own tmux window, `app1-<i>`, in the `sb-<workspace>` session.
- Runs the start command with `--id <i>` appended and `SB_INSTANCE_ID=<i>` in its environment.
- The binary's `main()` reads the id, sets `ov.module = "app1-" + id`, and opens a publisher on `dev01/ws/app1-<i>/iox2/state`.

The instance list is data in `sb.flow.yaml`: a hundred instances are a hundred ids, not a shell loop. An instance can also carry its own `args` and `env` ([sbcli_ws.md](sbcli_ws.md) §4.6).

### 10.3 Per-instance windows (built into `sb`)

`sb` owns the windows: one session per workspace, one window per instance, each addressed by an exact target, so `app1-1` and `app1-10` never collide and a second `sb run app1:1` respawns the existing window instead of opening a duplicate ([sbcli_launch.md](sbcli_launch.md) §1.1, §3). Before 0.6.0 the generated launch script had to derive a per-instance tmux session name from `--id` itself, because `tmux new -A` attaches to an existing session and silently discards the new command; that logic is gone.

To run one instance in the current terminal instead, with its output and exit code there: `sb run --fg app1:2`.

### 10.4 Subscribing to a specific instance (the foreign-topic case)

If your subscriber lives in a **different module** and was registered with `--key` (a `key:` entry), you don't get `TopicOverrides` (§3.3). To target instance `N` of the publisher, you must build the wire path yourself in `main()`:

```python
# Python subscriber tracking app1's instance with the same id as this one
import argparse, os
from iceoryx2 import NodeBuilder, ServiceName, ServiceType
from StringStamped import StringStamped

ap = argparse.ArgumentParser()
ap.add_argument("--id", default=os.environ.get("SB_INSTANCE_ID", ""),
                help="track this app1 instance")
args = ap.parse_args()

device, workspace, pub_module, transport, topic = "dev01", "ws", "app1", "iox2", "state"
if args.id:
    pub_module = f"{pub_module}-{args.id}"
service = f"{device}/{workspace}/{pub_module}/{transport}/{topic}"

node = NodeBuilder.new().create(ServiceType.Ipc)
svc = node.service_builder(ServiceName.new(service)).publish_subscribe(StringStamped).open_or_create()
sub = svc.subscriber_builder().create()
# ... poll loop ...
```

The hardcoded segments here are a known wart. `sb 0.6.0+` lets you declare this edge in `sb.flow.yaml` instead, as `app2.state: "app1[{id}].state"` (the publisher instance with the subscriber's own id) or `"app1[*].state"` (every instance, Zenoh only); `sb ws check` validates it today ([sbcli_ws.md](sbcli_ws.md) §4.7). Code generation does not read `wires:` yet, so in 0.6.0 the workaround above is still required; a later release generates the subscriber's key from the wire and retires it.

### 10.5 Discovery — finding running instances

`sb topic list --transport iceoryx2` enumerates all live iox2 services on the host. Each instance shows as its own entry, with the **owning publisher's PID** and an explicit **`(dead)`** suffix if the process that registered the service is no longer alive (this is how `sb topic list` distinguishes a live publisher from a stale on-disk service registration left behind by a crashed process):

```
  - [i]      - Hz  PID 12345           StringStamped           dev01/ws/app1-1/iox2/state
  - [i]      - Hz  PID 12347           StringStamped           dev01/ws/app1-2/iox2/state
  - [i]      - Hz  PID 12340 (dead)    StringStamped           dev01/ws/app1-3/iox2/state   ← orphan registration
```

The topic prints as the wire key as is, in the table and in the `--json` `topic` field alike (sb 0.5.0+; from 0.4.2 until then a `/` was prepended). `sb topic listen` / `sb topic pub` take it unchanged, and a leading `/` is also accepted, so a row pastes into either.

PID + liveness come straight from iceoryx2's service registry (`Service::list` → `dynamic_details.nodes` → `UniqueNodeId::pid()`); you don't need to put anything on the wire for it to work. The `(dead)` flag is the answer to *"why does `sb topic list` keep showing a topic that nobody is publishing to anymore?"* — iox2 leaves the on-disk service file under `/tmp/iceoryx2/services/` when a publisher exits abruptly (Ctrl+C in tmux, kill -9, etc.) and the file persists until pruned. **`sb topic prune` is the fix** — it walks the iceoryx2 node registry and calls `try_remove_stale_resources()` on every node whose owning process is gone, printing the PIDs it cleaned (`--json` for `{cleaned_pids, failed}`). It is iceoryx2-only: Zenoh is session-based and has no on-disk state to prune. `rm -rf /tmp/iceoryx2 /dev/shm/iox2_*` remains the brute-force fallback if a registration is too damaged to remove cleanly.

Zenoh entries render `-` in the PID column today — Zenoh has no notion of publisher identity on the wire, and the codegen doesn't yet stamp `getpid()` into `Header.metadata`. If you need cross-transport PID visibility, that's a separate schema bump (`Header.metadata.publisher_pid`) and is not landed yet.

Filter by substring: `sb topic list -k app1-`. iceoryx2 has no wildcard subscribe — discovery is enumerate-only.

For Zenoh, the wildcard `dev01/ws/app1-$*/zenoh/state` matches all instances at subscribe time (`$*` matches inside a chunk; a bare `*` must be a whole chunk, so `app1-*` is not a valid key expression). Use Zenoh when you want a single ground-station subscriber that follows every drone.

---

## 11. Checklist

Before running a new pub/sub pair end-to-end:

- [ ] `sb doctor` is all-OK on this host.
- [ ] `sb message list` shows the message type; `sb message compile` has run.
- [ ] `sb ws list` shows the right active workspace.
- [ ] `sb io add` succeeded: `<io_dir>/{publishers,subscribers}/<name>.<ext>` exists (`sb ws check` lists any that are missing).
- [ ] For a Zenoh entry: both ends use the same `encoding:`, and a `proto` entry's protobuf types are available to your build (§8.1).
- [ ] Bare topic is `snake_case`, lowercase, ROS2-style.
- [ ] If a subscriber uses `--key` (cross-module), **the key includes the `<transport>` segment** matching the publisher's flag.
- [ ] Entry-point (`main.cpp` / `main.py` / `src/main.rs`) **uses `argc/argv` (or equivalent)** and constructs the publisher via the no-arg form OR `TopicOverrides` — never with a hardcoded wire string.
- [ ] The module's `sb.module.yaml` resolves to the binary you built: the language default fits, or `scripts.start` names it and keeps `{args}` ([sbcli_module.md](sbcli_module.md) §3.2).
- [ ] Cross-language pair: payload type names match (C++ has `IOX2_TYPE_NAME = "<Leaf>"`).
- [ ] Build glue (`CMakeLists.txt` / `PYTHONPATH`) points at `<message_targets>/iox2/<pkg>/<Leaf>/`.
- [ ] On clean re-runs after schema changes: `rm -rf /tmp/iceoryx2 /dev/shm/iox2_*`.
- [ ] The subscriber's `key:` (as `sb io list` shows it) is identical to the publisher's key as `sb topic list` prints it, or, if running swarm instances, both ends use the same `<module>-<id>` suffix.
- [ ] If you're running multiple instances: they are listed under the module's entry in `sb.flow.yaml`, your `main()` reads `--id` (or `SB_INSTANCE_ID`), and `sb topic list` shows the per-instance wire paths.
