"""sb service router (static). REST/FastAPI-style request-response over Zenoh
queryables. Define routes with `@app.get("/path")`; return a dict/list
(-> JSON), str, or bytes. Routes are served under this module's reserved
service namespace:

    <device>/<workspace>/<module>/zenoh/service/<route>

`/` is the default root. `start()` is non-blocking (handlers run on Zenoh's
own threads) so it composes with your main loop; `stop()` tears down; `run()`
is the blocking standalone convenience.

Copied verbatim by `sb service init`, which fills in only the three namespace
constants below. You own this file — edit freely (re-running `sb service init`
overwrites it). Targets the Zenoh 1.x Python API.
"""

import json
import time

import zenoh

# ── Namespace — the only per-module values `sb service init` fills in. ───────
DEVICE = "__SB_DEVICE__"
WORKSPACE = "__SB_WORKSPACE__"
MODULE = "__SB_MODULE__"
# Fixed segment that keeps service routes disjoint from pub/sub topics.
SERVICE_SEGMENT = "service"

# Base key expression every route in this module hangs off.
BASE = f"{DEVICE}/{WORKSPACE}/{MODULE}/zenoh/{SERVICE_SEGMENT}"


def _resolve(route: str) -> str:
    """Map a route (relative to this module) to its full Zenoh key expression."""
    route = route.strip()
    if route in ("", "/"):
        return BASE
    return f"{BASE}/{route.lstrip('/')}"


class Request:
    """One incoming query, handed to a route handler."""

    __slots__ = ("query", "params")

    def __init__(self, query, params):
        self.query = query
        self.params = params

    @property
    def key(self) -> str:
        """The concrete key expression that matched (useful with wildcards)."""
        return str(self.query.key_expr)

    @property
    def json(self):
        """Request body parsed as JSON, or None if empty / not JSON."""
        payload = self.query.payload
        if payload is None:
            return None
        raw = payload.to_bytes()
        if not raw:
            return None
        try:
            return json.loads(raw)
        except (ValueError, UnicodeDecodeError):
            return None

    def text(self) -> str:
        """Request body as text."""
        payload = self.query.payload
        return payload.to_bytes().decode("utf-8") if payload is not None else ""


class ServiceApp:
    """A tiny FastAPI-style router over Zenoh queryables, scoped to this module."""

    def __init__(self, config=None):
        self._config = config or zenoh.Config()
        self._routes = []          # list of (key, route, handler)
        self._session = None
        self._queryables = []
        self.module = MODULE
        # Default root handler — overridden if the user defines @app.get("/").
        self.get("/")(self._default_index)

    @property
    def routes(self):
        return [route for (_key, route, _handler) in self._routes]

    def get(self, route: str):
        """Decorator: register a handler for GETs on `route` (relative to this module)."""
        key = _resolve(route)

        def decorator(func):
            # Re-registering the same route replaces the previous handler.
            self._routes = [t for t in self._routes if t[0] != key]
            self._routes.append((key, route, func))
            return func

        return decorator

    def _default_index(self, req):
        return {"module": MODULE, "routes": self.routes}

    def _make_callback(self, handler):
        def on_query(query):
            try:
                params = dict(query.parameters.iter()) if query.parameters else {}
                result = handler(Request(query, params))

                if isinstance(result, (dict, list)):
                    payload = json.dumps(result).encode("utf-8")
                    encoding = zenoh.Encoding.APPLICATION_JSON
                elif isinstance(result, bytes):
                    payload, encoding = result, zenoh.Encoding.APPLICATION_OCTET_STREAM
                else:
                    payload = str(result).encode("utf-8")
                    encoding = zenoh.Encoding.TEXT_PLAIN

                query.reply(query.key_expr, payload, encoding=encoding)
            except Exception as e:  # surface handler errors back to the caller
                query.reply_err(str(e).encode("utf-8"))

        return on_query

    def start(self):
        """Open the session and declare every route. Non-blocking — returns at once."""
        self._session = zenoh.open(self._config)
        for key, _route, handler in self._routes:
            qa = self._session.declare_queryable(key, self._make_callback(handler))
            self._queryables.append(qa)
        return self

    def stop(self):
        """Undeclare routes and close the session."""
        for qa in self._queryables:
            qa.undeclare()
        self._queryables = []
        if self._session is not None:
            self._session.close()
            self._session = None

    def run(self):
        """Blocking convenience: start(), then serve until Ctrl-C."""
        self.start()
        print(f"service `{MODULE}` running on {BASE}/** — Ctrl-C to stop.")
        try:
            while True:
                time.sleep(1)
        except KeyboardInterrupt:
            pass
        finally:
            self.stop()
