# Consuming the docker runscript

`sb init --docker` emits a `runscript.bash` that dispatches on `--id <name>`
to one pre-resolved `docker run` invocation per *instance* of your module.
This doc covers the **flow.yaml shape** you need, the **docker variant of
runscript.bash** that `sb init --docker` produces, and the **per-language
in-container entry-point** you write to consume it.

Sibling doc: [sbcli_pubsub_consuming.md](sbcli_pubsub_consuming.md). That one
covers the `sb pub add` / `sb sub add` generated files and the runtime
`TopicOverrides` API. *This* doc is everything else you need once your
module ships in a container.

---

## 1. What `--docker` is for

A single module (`cam_gige_ht`) often deploys as **multiple parallel
processes** — one per camera, robot arm, or sensor pair — each with its
own:

- Container image tag (`cam_gige_ht:v0.3` vs `cam_gige_ht:v0.4`)
- Environment variables (`CAM_SERIAL=A123`)
- `--device` mounts (`/dev/video0` vs `/dev/video1`)
- GPU access (`--gpus all` or a specific GPU index)
- Host volume mounts, network mode, etc.

These per-instance settings are *deployment* data, not source. They live in
`~/.swarmbotix/workspaces/<ws>/flow.yaml::instances`, get baked into the
generated `runscript.bash` at `sb init --docker` time, and disappear if you
rebuild for a non-docker target.

The same `--id <name>` mechanism is documented in
[sbcli_pubsub_consuming.md §3](sbcli_pubsub_consuming.md) for the
*topic-segment override*. With `--docker`, `--id` *also* picks which
container to launch. The two layers compose: the host runscript picks the
container; the container's `main()` reads `--id` and rewrites its wire
topic segment to match.

---

## 2. `flow.yaml::instances` schema

A single workspace's `flow.yaml` after `sb ws create demo` + `sb init`:

```yaml
modules:
  cam_gige_ht: /home/el/Github/vision_workspace/camera_publishers/cam_gige_ht/sb.dev.yml
```

To add two camera instances, hand-edit the file:

```yaml
modules:
  cam_gige_ht: /home/el/Github/vision_workspace/camera_publishers/cam_gige_ht/sb.dev.yml

instances:
  cam_gige_ht:
    - id: left
      label: "Left Camera"
      docker:
        image: cam_gige_ht:latest
        env:
          CAM_SERIAL: A123
          LOG_LEVEL: info
        devices:
          - /dev/video0
        gpus: all
    - id: right
      label: "Right Camera"
      docker:
        image: cam_gige_ht:latest
        env:
          CAM_SERIAL: B456
        devices:
          - /dev/video1
```

Field reference:

| Field | Required | Notes |
|---|---|---|
| `id` | yes | Wire-form string. Becomes the suffix on `<module>-<id>` in topic paths. Must match `[A-Za-z0-9_-]+` (same character class as wire segments). |
| `label` | no | Free-form display string for the L6 graph editor. If absent, the UI falls back to `id`. Not used at runtime. |
| `args` | no | Extra args appended to the `docker run … <image> <args>` invocation, before the script's own `"$@"`. Useful for `--config production.yaml` and similar in-container flags that vary per instance. |
| `docker.image` | yes (with `--docker`) | Image tag. Without an image, `sb init --docker` fails the preflight. |
| `docker.env` | no | Map of `KEY: value`. Emitted as `-e KEY=VALUE`. |
| `docker.devices` | no | Each entry becomes `--device <path>`. |
| `docker.mounts` | no | Each entry becomes `-v <host>:<container>[:ro]`. |
| `docker.gpus` | no | `"all"`, `"0"`, etc. — passed verbatim to `--gpus`. |
| `docker.network` | no | `"host"`, `"bridge"`, custom — passed verbatim to `--network`. |

> Duplicate `id` values within the same module's list are rejected at load
> time by `WorkspaceFlow::validate()`. Invalid IDs (spaces, slashes,
> empty) are rejected the same way. You'll see the error the next time
> `sb` reads `flow.yaml` — not waiting for `sb up`.

No management subcommand exists yet (`sb instance add/edit/rm` is on the
backlog). Edit `flow.yaml` directly and re-run
`sb init --docker --force` to regenerate the runscript.

---

## 3. `sb init --docker` walkthrough

```bash
cd ~/Github/vision_workspace/camera_publishers/cam_gige_ht
sb init --cpp --docker          # first time: also creates sb.dev.yml etc.
# …edit flow.yaml::instances…
sb init --docker --force        # subsequent: regenerate runscript only
```

Preflight checks (each emits an actionable error and exits non-zero):

1. **Active workspace** — same rule as plain `sb init`.
2. **`flow.yaml::instances.<module>` exists and is non-empty.** Without an
   instances list, the generated runscript would have no branches.
3. **Every instance has a `docker:` block with an `image:`.** Skipping
   `image` is the most common omission; the preflight names the offending
   instance.

A successful run:

- Writes `sb.dev.yml`, the IO directory, registers in `flow.yaml::modules`
  (same as plain `sb init`).
- Writes a **docker-flavored** `runscript.bash` whose body is described
  next.

---

## 4. Anatomy of the generated `runscript.bash`

The body has four parts, top to bottom:

```bash
#!/usr/bin/env bash
# /abs/path/runscript.bash
# regenerate with: sb init --docker --force
# …prose comment about docker variant…
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"
```

**Staleness check.** Compares `mtime(flow.yaml)` to `mtime($0)`. If
flow.yaml is newer, emits one stderr line and continues — assuming the
existing branches are still good enough:

```bash
__sb_flow='/home/el/.swarmbotix/workspaces/demo/flow.yaml'
__sb_self="$PWD/$(basename "${BASH_SOURCE[0]}")"
if [[ -f "$__sb_flow" ]] && [[ "$__sb_flow" -nt "$__sb_self" ]]; then
  echo "warning: $__sb_flow is newer than $__sb_self — instances may be stale; regenerate with: sb init --docker --force" >&2
fi
```

**`--id` parser.** Walks `"$@"`, grabs the value after `--id`. Unlike the
non-docker stub, `--id` is **required** here — there is no default
branch:

```bash
__sb_id=""
__sb_prev=""
for __arg in "$@"; do
  if [[ "$__sb_prev" == "--id" ]]; then __sb_id="$__arg"; break; fi
  __sb_prev="$__arg"
done
if [[ -z "$__sb_id" ]]; then
  echo "error: --id <name> required (known: left right)" >&2
  exit 2
fi
```

**Per-instance `case` branches.** One per entry in
`flow.yaml::instances.<module>`:

```bash
case "$__sb_id" in
  'left')
    exec docker run --rm -it \
      --name 'cam_gige_ht-left' \
      --hostname 'cam_gige_ht-left' \
      -e 'CAM_SERIAL=A123' \
      -e 'LOG_LEVEL=info' \
      --device '/dev/video0' \
      --gpus 'all' \
      'cam_gige_ht:latest' "$@"
    ;;
  'right')
    exec docker run --rm -it \
      --name 'cam_gige_ht-right' \
      --hostname 'cam_gige_ht-right' \
      -e 'CAM_SERIAL=B456' \
      --device '/dev/video1' \
      'cam_gige_ht:latest' "$@"
    ;;
  *)
    echo "error: unknown --id '$__sb_id' (known: left right)" >&2
    exit 2
    ;;
esac
```

Notes:

- `--name` and `--hostname` both default to `<module>-<id>`. The hostname
  lets in-container code that derives identity from `hostname` (common in
  ROS adapters and logging frameworks) pick up the instance identifier
  without re-parsing argv.
- `"$@"` after the image is the **complete original argv** — including
  the leading `--id <name>` that the script consumed. So your
  in-container `main()` still sees `--id left` and can rewrite its wire
  topic segment to match. Don't try to strip it on the host side; the
  shape is intentional.
- `exec` means the bash wrapper is replaced by `docker run`; signals
  reach docker cleanly.

---

## 5. Per-language in-container entry-point

In-container code consumes `--id <name>` exactly the same way as a
non-docker module (see [sbcli_pubsub_consuming.md §3](sbcli_pubsub_consuming.md)),
plus it reads its per-instance env vars from the container environment.

### 5.1 Rust

```rust
// src/main.rs
use std::env;

fn main() -> anyhow::Result<()> {
    // Per-instance identity from the runscript.
    let id = parse_id(env::args());
    let cam_serial = env::var("CAM_SERIAL")
        .map_err(|_| anyhow::anyhow!("CAM_SERIAL not set — check flow.yaml::instances"))?;

    let session = zenoh::open(zenoh::Config::default()).wait()?;
    let ov = swarmbotix_io::HelloTopicOverrides {
        module: id.as_deref(),  // None → default module; Some("left") → cam_gige_ht-left
        ..Default::default()
    };
    let pub_ = swarmbotix_io::HelloPublisher::open_with(&session, ov)?;
    // …use cam_serial to configure the camera, then publish frames…
    Ok(())
}

fn parse_id<I: Iterator<Item = String>>(mut args: I) -> Option<String> {
    while let Some(a) = args.next() {
        if a == "--id" { return args.next(); }
    }
    None
}
```

### 5.2 Python

```python
# main.py
import argparse, os, sys
import swarmbotix_io.publishers.hello as hello

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--id", default=None, help="instance discriminator")
    args, _ = ap.parse_known_args()

    cam_serial = os.environ.get("CAM_SERIAL")
    if cam_serial is None:
        print("CAM_SERIAL not set — check flow.yaml::instances", file=sys.stderr)
        sys.exit(2)

    pub = hello.HelloPublisher(node, payload_type, module=args.id)
    # …configure camera using cam_serial, then publish…
```

### 5.3 C++

```cpp
// main.cpp
#include <cstdlib>
#include <cstring>
#include <iostream>
#include "swarmbotix_io/publishers/hello.hpp"

int main(int argc, char** argv) {
  std::string id, mod_storage;
  for (int i = 1; i < argc - 1; ++i) {
    if (std::string(argv[i]) == "--id") { id = argv[i + 1]; break; }
  }

  const char* cam_serial = std::getenv("CAM_SERIAL");
  if (!cam_serial || !*cam_serial) {
    std::cerr << "CAM_SERIAL not set — check flow.yaml::instances\n";
    return 2;
  }

  swarmbotix_io::HelloTopicOverrides ov;
  if (!id.empty()) {
    mod_storage = std::string(swarmbotix_io::HELLO_DEFAULT_MODULE) + "-" + id;
    ov.module = mod_storage;
  }
  auto pub = swarmbotix_io::HelloPublisher::open_with(node, ov);
  // …use cam_serial, then publish…
}
```

The C++ `mod_storage` pattern (string outlives `string_view` field on
`HelloTopicOverrides`) is the same idiom as
[sbcli_pubsub_consuming.md §5.1](sbcli_pubsub_consuming.md). Nothing
docker-specific.

### 5.4 Flutter

Flutter modules typically run as desktop apps and rarely fit the docker
single-camera pattern, but the mechanism is identical: parse `--id` from
the command line, read env vars via `Platform.environment`.

### 5.5 Unity

Unity inside docker is exotic. If you do it, the host runscript launches a
headless Unity batch-mode container; argv reaches `Application.Arguments`
and env via `Environment.GetEnvironmentVariable`. The override mechanism
is the same shape as the others.

---

## 6. Composing with `TopicOverrides`

The runtime override API from
[sbcli_pubsub_consuming.md §3](sbcli_pubsub_consuming.md) was originally
documented with a numeric `--id N`. **String IDs work the same way**:
`--id left` produces wire topic `/<dev>/<ws>/cam_gige_ht-left/iox2/…`.
Mix-and-match:

| flow.yaml `id` | Wire `<module>` segment |
|---|---|
| `left` | `cam_gige_ht-left` |
| `2` | `cam_gige_ht-2` |
| `front-left` | `cam_gige_ht-front-left` |

The character class for `id` (`[A-Za-z0-9_-]+`) matches the wire-segment
validator at `crates/sb-core/src/topic.rs::is_valid_segment_char`. There
is no conversion step — what you write in flow.yaml is exactly what shows
up on the wire.

`is_owned` (§3.3 of the consuming doc) is **not affected** by string IDs:
a foreign subscriber registered with a leading-slash, fully-qualified
topic (`/dev01/ws/cam_gige_ht/iox2/frame`) still won't pair with a
publisher running as instance `left` (which writes to
`/dev01/ws/cam_gige_ht-left/iox2/frame`). The three workarounds
documented there still apply. Use Zenoh KeyExpr wildcards if you have
cross-module subscribers that need to fan-in across instances.

---

## 7. `sb pub add` does NOT take an instance ID

Instance IDs are a **launch-time** concept. `sb pub add` operates on the
module:

```bash
sb pub add image_raw --type std/ImageStamped --iox2 --module cam_gige_ht
```

There is no `--module cam_gige_ht-left`. The codegen writes the *base*
module name into the generated file as `<Name>_DEFAULT_MODULE`; the
runtime override (§5) appends `-<id>` at construction. Trying to add a
publisher with an instance-suffixed module would silently break the
identifier rule (`-` is not allowed in `module:` in `sb.dev.yml`) — `sb`
will refuse.

---

## 8. Pitfalls

**Image must exist on the host you're launching from.** The runscript
runs `docker run` — `docker pull` is your problem. CI typically pulls in
a preceding step.

**`--network host` and iox2.** iceoryx2 uses host shared memory; without
`--network host` and a `-v /run/swarmbotix/iox2:/run/swarmbotix/iox2`
mount (or wherever iox2's shm lives on your host), the in-container
publisher and a same-host subscriber outside the container can't
discover each other. Zenoh tolerates `--network bridge` because it falls
back to network transport.

**`--device` vs USB hot-plug.** A camera enumerated as `/dev/video0` at
container start can renumber after a USB replug. Mount the by-id path
(`/dev/v4l/by-id/usb-FLIR…`) instead, or accept that hot-plugs require a
container restart.

**GPU passthrough.** `gpus: all` requires nvidia-container-toolkit on the
host. Without it, docker silently ignores the flag and your in-container
CUDA code fails at runtime, not at launch — the runscript looks fine.
`docker info | grep -i nvidia` is the quick check.

**Permissions on mounted devices.** A user-namespaced docker daemon
won't be able to open `/dev/video0` unless you add `--group-add video`
or run privileged. The runscript doesn't add either by default; add
extra args via flow.yaml's `args:` field if you need them.

**Don't bake secrets into env.** `flow.yaml` is plaintext on disk and
usually gitignored, but treat it like source-of-truth config, not a
secret store. Pass tokens via `--env-file` (add to a future flow.yaml
field once we need it) or mount a docker secret.

---

## 9. Regenerating after a flow.yaml edit

```bash
# After editing ~/.swarmbotix/workspaces/<ws>/flow.yaml:
cd <module-root>
sb init --docker --force
```

If you forget, the next run of `runscript.bash` will emit:

```
warning: /home/.../flow.yaml is newer than /path/runscript.bash — instances may be stale; regenerate with: sb init --docker --force
```

…and continue with the old branches. That's deliberate — running with
stale-but-valid branches is usually fine; a hard fail would break
in-flight CI. Pay attention to the warning when you add or remove
instances; ignore it for label-only edits.
