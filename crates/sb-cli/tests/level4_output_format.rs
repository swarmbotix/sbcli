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

use sb_discover::{TopicEntry, Transport, render_json_lines, render_table};

fn fixture_rows() -> Vec<TopicEntry> {
    vec![
        TopicEntry {
            topic: "/dev01/ws/cam/iox2/image".into(),
            transport: Transport::Iceoryx2,
            schema: Some("std::ImageStamped".into()),
            hash: None,
            rate_hz: None,
            publisher_pid: Some(12345),
            is_dead: false,
        },
        TopicEntry {
            topic: "/dev01/ws/app1-1/iox2/hello".into(),
            transport: Transport::Iceoryx2,
            schema: Some("ros2/std/StringStamped".into()),
            hash: None,
            rate_hz: None,
            publisher_pid: Some(67890),
            is_dead: true,
        },
        TopicEntry {
            topic: "/dev01/ws/talker/zenoh/chatter".into(),
            transport: Transport::Zenoh,
            schema: Some("ros2/std/StringStamped".into()),
            hash: None,
            rate_hz: Some(10.0),
            publisher_pid: None,
            is_dead: false,
        },
        TopicEntry {
            topic: "/foreign/peer/random".into(),
            transport: Transport::Zenoh,
            schema: None,
            hash: None,
            rate_hz: None,
            publisher_pid: None,
            is_dead: false,
        },
    ]
}

#[test]
fn table_renders_byte_exact() {
    // Column widths (see render::TableLayout default): rate 6, pid 18,
    // schema 22. PID cell is `PID <n>` or `PID <n> (dead)` or `-`,
    // left-aligned in the 18-char slot.
    let expected = concat!(
        "  - [i]      - Hz  PID 12345           std::ImageStamped       /dev01/ws/cam/iox2/image\n",
        "  - [i]      - Hz  PID 67890 (dead)    ros2/std/StringStamped  /dev01/ws/app1-1/iox2/hello\n",
        "  - [z]   10.0 Hz  -                   ros2/std/StringStamped  /dev01/ws/talker/zenoh/chatter\n",
        "  - [z]      - Hz  -                                           /foreign/peer/random\n",
    );
    let out = render_table(&fixture_rows());
    assert_eq!(
        out, expected,
        "table render drifted:\n--- got ---\n{out}--- expected ---\n{expected}"
    );
}

#[test]
fn empty_table_prints_placeholder() {
    assert_eq!(render_table(&[]), "(no topics)\n");
}

#[test]
fn json_lines_one_object_per_line_with_pinned_field_set() {
    let out = render_json_lines(&fixture_rows());
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines.len(),
        4,
        "expected 4 lines, got {}: {out}",
        lines.len()
    );

    // First row — iox2, schema + alive PID, no rate.
    let r0: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(r0["topic"], "/dev01/ws/cam/iox2/image");
    assert_eq!(r0["transport"], "iceoryx2");
    assert_eq!(r0["schema"], "std::ImageStamped");
    assert!(r0["rate_hz"].is_null());
    assert_eq!(r0["publisher_pid"], 12345);
    assert_eq!(r0["is_dead"], false);

    // Second row — iox2 with dead owner.
    let r1: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(r1["publisher_pid"], 67890);
    assert_eq!(r1["is_dead"], true);

    // Third row — zenoh, schema + rate, no PID.
    let r2: serde_json::Value = serde_json::from_str(lines[2]).unwrap();
    assert_eq!(r2["transport"], "zenoh");
    assert_eq!(r2["rate_hz"], 10.0);
    assert!(r2["publisher_pid"].is_null());

    // Fourth row — neither.
    let r3: serde_json::Value = serde_json::from_str(lines[3]).unwrap();
    assert!(r3["schema"].is_null());
    assert!(r3["rate_hz"].is_null());
    assert!(r3["publisher_pid"].is_null());
}

#[test]
fn json_field_set_is_pinned() {
    // Pin the JSON shape so swarmctl + MCP can rely on these keys.
    let row = TopicEntry {
        topic: "/a".into(),
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
