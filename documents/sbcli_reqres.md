# Services: REST-style request/response

Reference for module services: a FastAPI/Express-style request-response layer
over Zenoh **queryables**, with **stringified-JSON** payloads. This is
independent of pub/sub: each service is a router file the user writes their own
routes against.

`sb 0.5.0+`: a service is a `services:` entry in `sb.module.yaml`, added with
`sb io add service <name>` and rendered to `<io_dir>/services/<name>.<ext>`.
Since 0.7.0 that is the only way to add one (§2.2).

This document is normative for the behavior shipped at L4 and, from 0.5.0,
the `services:` entries of `sb io`.

The canonical spec is `requirements.md` in the sbcli **source
repository** — a maintainer document that is not part of your install;
when it and this page disagree, it wins.

---

## 1. Conceptual model

Zenoh answers a `get` (query) with a **queryable**. `sb` ships a small
router — `ServiceApp` — that wraps `declare_queryable` so you define routes the
way you would in FastAPI:

- a `@app.get("/path")` decorator (Python) / `app.get("/path", handler)` closure
  (Rust) instead of manual `declare_queryable` calls
- automatic JSON encode/decode of handler return values
- query-parameter parsing (`?(name=Bob)` → a dict)
- a non-blocking `start()` / `stop()` lifecycle (and a blocking `run()`)

It is **GET-only** and **JSON-only**: a request is a Zenoh query carrying a JSON
string; a reply is a JSON string. There are no vault message types, no protobuf,
and no iceoryx2 path — services are always Zenoh.

The client side needs no codegen: any Zenoh `get` against the route key is a
call (see §6).

---

## 2. Declaring a service

### 2.1 `services:` entries (sb 0.5.0+)

```bash
sb io add service calib            # adds calib to services: and renders the router
```

```yaml
# sb.module.yaml
services: [calib]
```

`sb 0.8.0+` writes `services:` as a list of names; a map keyed by name
and the 0.7 list of single-key maps are also read
([sbcli_module.md](sbcli_module.md) §2.1).

Each entry renders `<io_dir>/services/<name>.<ext>` (`services/calib.py` for a
Python module, `services/calib.rs` for a Rust module) with this module's
namespace and the service name baked in. **Rust and Python modules only**;
Flutter and Unity have no service router.

- The name must be an identifier and is unique on the module: publishers,
  subscribers, and services share one name space
  ([sbcli_pubsub.md](sbcli_pubsub.md) §4.1).
- `sb io rm <name>` removes the entry and deletes its file.
- `sb io update --services` re-renders every service file; plain
  `sb io update` leaves service files alone, because you edit them
  ([sbcli_pubsub.md](sbcli_pubsub.md) §14.6). `sb install` (sb 0.6.0+)
  renders a service file only where it does not exist yet, so a fresh
  clone gets its routers and an edited one is never overwritten
  ([sbcli_ws.md](sbcli_ws.md) §5.2).

Module resolution is the chain every `sb io` verb uses (see
[sbcli_pubsub.md](sbcli_pubsub.md) §3): `--module` + sibling dir
(`./<name>/sb.module.yaml`), then `--module` + the active workspace's
installed `sb.workspace.yaml`, then `./sb.module.yaml` in cwd.

The router file is a **static, self-contained source file**: sb fills only the
namespace constants (`device`, `workspace`, `module`, and the service name);
everything else is verbatim. You own the file and may edit it; the only state
is in your own route handlers. Regenerating it overwrites it.

### 2.2 The pre-`services:` default router (removed in 0.7.0)

Removed in 0.7.0 ([sbcli.md](sbcli.md) §10): the verb that wrote the
default router to `<io_dir>/service.<ext>` without a `services:` entry.
`sb io add service service` adds the default service as an entry
instead (§3). A router file the old verb wrote keeps working as your own
file, but nothing regenerates it.

---

## 3. Route namespacing

Routes are written Express/FastAPI-style, as a leading-`/` path relative to the
service, and resolved against the service's **base key**. The base carries a
dedicated `service/` segment so routes **never collide with pub/sub topics**
under the same `…/<module>/zenoh/` namespace:

```
base = <device>/<workspace>/<module>/zenoh/service/<name>    # a services: entry named <name> (sb 0.5.0+)
base = <device>/<workspace>/<module>/zenoh/service           # the default service
```

The default service is a `services:` entry named `service` (and a
router file `<io_dir>/service.<ext>` written before 0.7.0, §2.2); it
serves on the bare base. Every other
named service hangs off its own `<name>` segment, so several services on one
module never share a route. The table below uses the default base; for
`services/calib.<ext>` insert `/calib` after `service`.

| `get(...)` arg | declared key |
|---|---|
| `"/"` | `<device>/<workspace>/<module>/zenoh/service` (default root) |
| `"/greet"` | `<device>/<workspace>/<module>/zenoh/service/greet` |
| `"/sensors/*/temp"` | `<device>/<workspace>/<module>/zenoh/service/sensors/*/temp` |

`/` is **always present**: the router pre-registers a default `/` handler
(returns `{module, routes}`); defining your own `@app.get("/")` overrides it.
Wildcards (`*`, `**`) and `?(name=Bob)` params behave exactly as in Zenoh.

---

## 4. Generated API — Python

```python
from swarmbotix_io.services.calib import ServiceApp       # a services: entry named calib
# from swarmbotix_io.services.service import ServiceApp   # the default service (named service)

app = ServiceApp()                       # namespace baked in


@app.get("/greet")
def greet(req):
    return {"msg": f"hello {req.params.get('name', 'world')}"}


@app.get("/add")
def add(req):
    b = req.json                         # request body parsed as JSON (dict)
    return {"sum": b["a"] + b["b"]}      # dict/list -> JSON; str -> text; bytes -> raw
```

`req` fields: `req.json` (parsed body or `None`), `req.params` (dict), `req.key`
(matched key expr), `req.text()` (raw body as str). Raising inside a handler
sends a Zenoh error reply.

**Lifecycle** — `start()` is non-blocking (handlers run on Zenoh's threads), so
it composes with your own loop:

```python
def main():
    app.start()                          # routes now live; returns immediately
    try:
        while True:
            ...                          # your other logic, same thread
            time.sleep(0.01)
    except KeyboardInterrupt:
        pass
    finally:
        app.stop()
```

`app.run()` is the blocking standalone convenience (`start()` + sleep loop +
`stop()` on Ctrl-C).

---

## 5. Generated API — Rust

Same model, closures instead of decorators. Handlers take `Req`, return
`serde_json::Value`. Requires the `zenoh` and `serde_json` crates.

```rust
use swarmbotix_io::services::calib::{ServiceApp, Req};
use serde_json::json;

let mut app = ServiceApp::open();        // default `/` auto-registered
app.get("/greet", |req: Req| {
    let name = req.param("name").unwrap_or_else(|| "world".into());
    json!({ "msg": format!("hello {name}") })
});
app.get("/add", |req: Req| {
    let b = req.json();                  // serde_json::Value
    json!({ "sum": b["a"].as_i64().unwrap() + b["b"].as_i64().unwrap() })
});

app.start()?;                            // non-blocking; callbacks on zenoh threads
loop {
    // your other logic
}
// app.stop() on shutdown
```

`Req` methods: `json()`, `bytes()`, `param(key)`, field `key`. `open()` returns
`Self` (infallible); `start()` returns `zenoh::Result<()>`. Include the file the
same way as generated pub/sub: `#[path = "../swarmbotix_io/services/calib.rs"] pub mod calib;`
for a `services:` entry (sb 0.5.0+); the default service's file is
`services/service.rs`.

---

## 6. Calling a service

A client is just a Zenoh `get` against the route key — **no codegen needed**.
The key is the full namespaced form: `<device>/<workspace>/<module>/zenoh/service/<route>`
for the default service, `<device>/<workspace>/<module>/zenoh/service/<name>/<route>`
for a named one (sb 0.5.0+).
Send inputs as a JSON **body** (read by `req.json`) or as selector **params**
(read by `req.params`).

**Python (peer — no router needed):**

```python
import zenoh, json
with zenoh.open(zenoh.Config()) as s:
    for r in s.get("elcom/demo/api/zenoh/service/add",
                   payload=json.dumps({"a": 2, "b": 3})):
        if r.ok:
            print(json.loads(r.ok.payload.to_bytes()))     # {"sum": 5}
        else:
            print("error:", r.err.payload.to_bytes().decode())
```

**Rust (peer):**

```rust
use zenoh::Wait;

let session = zenoh::open(zenoh::Config::default()).wait()?;
let replies = session
    .get("elcom/demo/api/zenoh/service/add")
    .payload(r#"{"a":2,"b":3}"#)
    .wait()?;
for reply in replies {
    match reply.result() {
        Ok(sample) => {
            let bytes = sample.payload().to_bytes();
            println!("{}", String::from_utf8_lossy(&bytes));   // {"sum":5}
        }
        Err(err) => eprintln!("service error: {err:?}"),
    }
}
```

Peer clients and the service must share a Zenoh network — automatic via local
scouting; across hosts, set a connect endpoint in the `Config`. Because routes
can fan out (overlapping queryables all reply), a `get` may yield more than one
reply — iterate them.

### 6.1 Calling over HTTP/REST

Routes are reachable from `curl`/browsers via Zenoh's **REST plugin**: an HTTP
GET is bridged to a Zenoh `get`, which answers the queryable. The URL path is
the **full key** (no leading slash — Zenoh keys have none):

```bash
# requires a `zenohd` router with the REST plugin (default :8000) on the same
# Zenoh network as the service app — sb does not bundle the router or plugin.

curl 'http://localhost:8000/elcom/demo/api/zenoh/service/greet?(name=Bob)'
# -> {"msg":"hello Bob"}

curl http://localhost:8000/elcom/demo/api/zenoh/service        # the default "/" root
# -> {"module":"api","routes":["/","/greet","/add"]}
```

Three rules for REST-facing routes:

- **Inputs come via query params, not a body.** HTTP GET has no body, so read
  `req.params` (the `?(key=val)` selector), not `req.json`. The JSON-body
  request style works only for peer/code clients (`session.get(key, payload=…)`).
- **GET only.** HTTP PUT/POST become Zenoh *puts* (writes) → those reach
  **subscribers**, not queryables, so they never hit a service handler.
- **Full namespaced key.** Wildcards work and fan out to an array of replies.

---

## 7. Limitations

- **GET only / JSON only.** Writes (PUT/DELETE) are pub/sub, not services. Use
  publishers and subscribers (`sb io add pub` / `sb io add sub`) for streaming
  data.
- **Handlers run on Zenoh's callback threads.** Keep them short; guard shared
  state with a lock; hand heavy work to your own loop/queue.
- **Overlapping routes fan out** — multiple matching queryables all reply.
- **No path-param capture.** Split `req.key` yourself for wildcard segments.
- **Rust + Python only.** No service router for Flutter/Unity
  (`sb io add service` errors with a clear message).
