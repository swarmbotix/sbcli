//! sb service router (static). REST/FastAPI-style request-response over Zenoh
//! queryables. Register routes with `app.get("/path", handler)`; a handler
//! takes a [`Req`] and returns a `serde_json::Value` (sent back as JSON).
//! Routes are served under this module's reserved service namespace:
//!
//! ```text
//! <device>/<workspace>/<module>/zenoh/service/<route>
//! ```
//!
//! `/` is the default root. [`ServiceApp::start`] is non-blocking (handlers run
//! on Zenoh's callback threads) so it composes with your own main loop;
//! [`ServiceApp::stop`] tears down; [`ServiceApp::run`] is the blocking
//! standalone convenience.
//!
//! Copied verbatim by `sb service init`, which fills in only the three
//! namespace constants below. You own this file — edit freely (re-running
//! `sb service init` overwrites it). Requires the `zenoh` and `serde_json`
//! crates as dependencies.

use std::sync::Arc;

use serde_json::Value;
use zenoh::query::{Query, Queryable};
use zenoh::Wait;

// ── Namespace — the only per-module values `sb service init` fills in. ───────
pub const DEVICE: &str = "__SB_DEVICE__";
pub const WORKSPACE: &str = "__SB_WORKSPACE__";
pub const MODULE: &str = "__SB_MODULE__";
// Fixed segment that keeps service routes disjoint from pub/sub topics.
pub const SERVICE_SEGMENT: &str = "service";

/// Base key expression every route in this module hangs off.
pub fn base() -> String {
    format!("{DEVICE}/{WORKSPACE}/{MODULE}/zenoh/{SERVICE_SEGMENT}")
}

/// Map a route (relative to this module) to its full Zenoh key expression.
fn resolve(route: &str) -> String {
    let route = route.trim();
    if route.is_empty() || route == "/" {
        base()
    } else {
        format!("{}/{}", base(), route.trim_start_matches('/'))
    }
}

/// One incoming query handed to a route handler.
pub struct Req {
    /// The concrete key expression that matched (useful with wildcard routes).
    pub key: String,
    payload: Vec<u8>,
    params: Vec<(String, String)>,
}

impl Req {
    /// Request body parsed as JSON (`Value::Null` if empty or not JSON).
    pub fn json(&self) -> Value {
        if self.payload.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.payload).unwrap_or(Value::Null)
    }

    /// Raw request body bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.payload
    }

    /// A query parameter from `?(name=Bob)`, if present.
    pub fn param(&self, key: &str) -> Option<String> {
        self.params
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    }
}

type Handler = Arc<dyn Fn(Req) -> Value + Send + Sync + 'static>;

/// A tiny FastAPI-style router over Zenoh queryables, scoped to this module.
pub struct ServiceApp {
    routes: Vec<(String, String, Handler)>,
    session: Option<zenoh::Session>,
    queryables: Vec<Queryable<()>>,
}

impl ServiceApp {
    /// Construct the router with a default `/` root handler (override it by
    /// registering `"/"` again).
    pub fn open() -> Self {
        let mut app = Self {
            routes: Vec::new(),
            session: None,
            queryables: Vec::new(),
        };
        app.get("/", |_req| serde_json::json!({ "module": MODULE }));
        app
    }

    /// This module's name.
    pub fn module(&self) -> &'static str {
        MODULE
    }

    /// Registered routes (relative form).
    pub fn routes(&self) -> Vec<String> {
        self.routes.iter().map(|(_, r, _)| r.clone()).collect()
    }

    /// Register a handler for `route` (relative to this module's service base).
    /// Re-registering the same route replaces the previous handler.
    pub fn get<F>(&mut self, route: &str, handler: F) -> &mut Self
    where
        F: Fn(Req) -> Value + Send + Sync + 'static,
    {
        let key = resolve(route);
        self.routes.retain(|(k, _, _)| k != &key);
        self.routes.push((key, route.to_string(), Arc::new(handler)));
        self
    }

    /// Open the session and declare every route. Non-blocking — handlers fire
    /// on Zenoh's threads, so your own loop is free to run.
    pub fn start(&mut self) -> zenoh::Result<()> {
        let session = zenoh::open(zenoh::Config::default()).wait()?;
        let mut queryables = Vec::new();
        for (key, _route, handler) in &self.routes {
            let handler = handler.clone();
            let queryable = session
                .declare_queryable(key.clone())
                .callback(move |query: Query| {
                    let req = Req {
                        key: query.key_expr().as_str().to_string(),
                        payload: query
                            .payload()
                            .map(|p| p.to_bytes().into_owned())
                            .unwrap_or_default(),
                        params: query
                            .parameters()
                            .iter()
                            .map(|(k, v)| (k.to_string(), v.to_string()))
                            .collect(),
                    };
                    let reply_key = query.key_expr().clone();
                    match serde_json::to_vec(&handler(req)) {
                        Ok(body) => {
                            let _ = query.reply(reply_key, body).wait();
                        }
                        Err(e) => {
                            let _ = query.reply_err(e.to_string().into_bytes()).wait();
                        }
                    }
                })
                .wait()?;
            queryables.push(queryable);
        }
        self.queryables = queryables;
        self.session = Some(session);
        Ok(())
    }

    /// Undeclare routes and close the session.
    pub fn stop(&mut self) {
        self.queryables.clear();
        if let Some(session) = self.session.take() {
            let _ = session.close().wait();
        }
    }

    /// Blocking convenience: `start()`, then serve until the process is killed.
    pub fn run(&mut self) -> zenoh::Result<()> {
        self.start()?;
        println!("service `{}` running on {}/** — Ctrl-C to stop.", MODULE, base());
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }
}
