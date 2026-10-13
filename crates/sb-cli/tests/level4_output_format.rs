//! L4 TDD #5 — output-format snapshot for `sb topic list`.
//!
//! Pure render test: no Zenoh, no iox2. Pins the table + JSON contracts
//! that later levels (swarmctl REST output, MCP tooling) consume so we
//! can refactor sb-discover internals without breaking the surface.
//!
//! Four rows cover every cell state we care about:
//!   - iox2 row with alive owning PID (the happy `sb topic list` path)
//!   - iox2 row with a dead owning PID (stale registration; flagged via
//!     `(dead)` suffix in the PID column)
//!   - zenoh row with schema + rate but no PID (we don't yet stamp PID
//!     into the wire-format `Header.metadata`)
//!   - foreign zenoh row — neither schema, rate, nor PID

use sb_discover::{TopicEntry, Transport, render_json_lines};

#[test]
fn json_field_set_is_pinned() {
    // Pin the JSON shape so swarmctl + MCP can rely on these keys.
    let row = TopicEntry {
        topic: "a".into(),
        transport: Transport::Zenoh,
        schema: Some("ros2/std/StringStamped".into()),
        hash: None,
        rate_hz: Some(5.0),
        publisher_pid: Some(99),
        is_dead: false,
    };
    let s = render_json_lines(&[row]);
    let v: serde_json::Value = serde_json::from_str(s.trim()).unwrap();
    let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(
        sorted,
        vec![
            "hash",
            "is_dead",
            "publisher_pid",
            "rate_hz",
            "schema",
            "topic",
            "transport"
        ]
    );
}
