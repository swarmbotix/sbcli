//! Table and JSON-lines rendering for `sb topic list`.
//!
//! Two contracts that downstream snapshot tests pin (level4_output_format):
//!
//! 1. Table: ``  - [{tag}] {rate:>6.1} Hz  {pid:<13}  {schema:<22}  {topic}``
//!    `pid` is ``PID <n>`` (zero-padded to keep the column rectangular)
//!    with `(dead)` appended when the owning iox2 node is no longer alive,
//!    or `   -        ` when unknown (Zenoh today, foreign iox2 services
//!    with no registered nodes). `rate` uses `-` for missing values.
//!    Empty schema renders blank. Empty input prints a single
//!    "(no topics)" line so the user knows the discovery actually ran.
//! 2. JSON: one [`TopicEntry`] per line via `serde_json::to_string` — no
//!    trailing comma, no surrounding array. Easy to grep / pipe.

use crate::TopicEntry;

#[derive(Debug, Clone, Copy)]
pub struct TableLayout {
    pub rate_width: usize,
    /// Width of the PID cell — wide enough to fit ``PID 1234567 (dead)``
    /// on a 32-bit pid_t machine without rippling the schema/topic
    /// columns. 18 = ``PID `` (4) + 7-digit pid + ` (dead)` (7).
    pub pid_width: usize,
    pub schema_width: usize,
}

impl Default for TableLayout {
    fn default() -> Self {
        // Mirrors the prototype's format (level4.html §"sb topic list")
        // with an added PID column slotted between rate and schema:
        // ``  - [z] {rate:>6.1} Hz  {pid:<18}  {schema:<22}  {topic}``.
        Self {
            rate_width: 6,
            pid_width: 18,
            schema_width: 22,
        }
    }
}

/// Render `rows` as the L4 table. Pure: no IO. The caller picks how to
/// emit (stdout, log, capture for tests). One trailing newline per row so
/// concatenation onto an existing stream stays sane.
pub fn render_table(rows: &[TopicEntry]) -> String {
    render_table_with(rows, TableLayout::default())
}

pub fn render_table_with(rows: &[TopicEntry], layout: TableLayout) -> String {
    if rows.is_empty() {
        return "(no topics)\n".to_string();
    }
    let mut out = String::new();
    for r in rows {
        let rate = match r.rate_hz {
            Some(hz) => format!("{:>width$.1}", hz, width = layout.rate_width),
            None => format!("{:>width$}", "-", width = layout.rate_width),
        };
        let pid_cell = match r.publisher_pid {
            Some(p) if r.is_dead => format!("PID {p} (dead)"),
            Some(p) => format!("PID {p}"),
            None => "-".to_string(),
        };
        let pid = format!("{:<width$}", pid_cell, width = layout.pid_width);
        let schema = r.schema.clone().unwrap_or_default();
        out.push_str(&format!(
            "  - [{}] {} Hz  {}  {:<width$}  {}\n",
            r.transport.tag(),
            rate,
            pid,
            schema,
            r.topic,
            width = layout.schema_width,
        ));
    }
    out
}

/// One JSON object per line — the contract surfaced by `sb topic list --json`.
/// Used by both the CLI and (later) swarmctl's REST proxy.
pub fn render_json_lines(rows: &[TopicEntry]) -> String {
    let mut out = String::new();
    for r in rows {
        // `TopicEntry` derives Serialize — `to_string` cannot fail in
        // practice, but stay honest about the Result with a fallback.
        match serde_json::to_string(r) {
            Ok(s) => {
                out.push_str(&s);
                out.push('\n');
            }
            Err(_) => out.push_str("{}\n"),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Transport;

    fn row(
        topic: &str,
        transport: Transport,
        schema: Option<&str>,
        rate: Option<f64>,
    ) -> TopicEntry {
        TopicEntry {
            topic: topic.into(),
            transport,
            schema: schema.map(str::to_string),
            hash: None,
            rate_hz: rate,
            publisher_pid: None,
            is_dead: false,
        }
    }

    fn row_with_pid(
        topic: &str,
        transport: Transport,
        schema: Option<&str>,
        rate: Option<f64>,
        pid: Option<u32>,
        is_dead: bool,
    ) -> TopicEntry {
        TopicEntry {
            topic: topic.into(),
            transport,
            schema: schema.map(str::to_string),
            hash: None,
            rate_hz: rate,
            publisher_pid: pid,
            is_dead,
        }
    }

    #[test]
    fn empty_table_renders_placeholder() {
        assert_eq!(render_table(&[]), "(no topics)\n");
    }

    #[test]
    fn table_rate_dash_for_missing() {
        let out = render_table(&[row(
            "/dev01/ws/cam/iox2/image",
            Transport::Iceoryx2,
            Some("std::ImageStamped"),
            None,
        )]);
        assert!(out.contains("[i]"), "transport tag missing: {out}");
        assert!(out.contains("     - Hz"), "missing rate dash: {out}");
        assert!(out.contains("/dev01/ws/cam/iox2/image"));
    }

    #[test]
    fn table_rate_formatted_one_decimal() {
        let out = render_table(&[row(
            "/x",
            Transport::Zenoh,
            Some("std/StringStamped"),
            Some(10.0),
        )]);
        // `10.0` → `"  10.0"` (right-aligned in width 6, 1 decimal).
        assert!(
            out.contains("  10.0 Hz"),
            "expected `  10.0 Hz`, got: {out}"
        );
    }

    #[test]
    fn table_pid_alive_renders_as_pid_n() {
        let out = render_table(&[row_with_pid(
            "/dev01/ws/app1/iox2/hello",
            Transport::Iceoryx2,
            Some("std/StringStamped"),
            Some(50.0),
            Some(12345),
            false,
        )]);
        assert!(
            out.contains("PID 12345"),
            "expected alive PID column: {out}"
        );
        assert!(
            !out.contains("(dead)"),
            "alive row must NOT contain (dead): {out}"
        );
    }

    #[test]
    fn table_pid_dead_appends_dead_suffix() {
        let out = render_table(&[row_with_pid(
            "/dev01/ws/app1-1/iox2/hello",
            Transport::Iceoryx2,
            Some("std/StringStamped"),
            None,
            Some(67890),
            true,
        )]);
        assert!(
            out.contains("PID 67890 (dead)"),
            "expected dead-suffixed PID: {out}"
        );
    }

    #[test]
    fn table_pid_missing_renders_as_dash() {
        let out = render_table(&[row_with_pid(
            "/x",
            Transport::Zenoh,
            None,
            None,
            None,
            false,
        )]);
        // The PID cell is left-padded to a fixed width; the single dash
        // must appear with at least one trailing space before the
        // schema/topic columns so the layout stays rectangular.
        assert!(
            out.contains(" -                "),
            "expected blank PID cell: {out}"
        );
    }

    #[test]
    fn json_lines_one_object_per_line() {
        let rows = vec![
            row_with_pid(
                "/a",
                Transport::Zenoh,
                Some("std/StringStamped"),
                Some(2.0),
                None,
                false,
            ),
            row_with_pid("/b", Transport::Iceoryx2, None, None, Some(4242), true),
        ];
        let out = render_json_lines(&rows);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["topic"], "/a");
        assert_eq!(first["transport"], "zenoh");
        assert_eq!(first["schema"], "std/StringStamped");
        assert_eq!(first["rate_hz"], 2.0);
        assert!(first["publisher_pid"].is_null());
        assert_eq!(first["is_dead"], false);
        let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(second["transport"], "iceoryx2");
        assert!(second["schema"].is_null());
        assert!(second["rate_hz"].is_null());
        assert_eq!(second["publisher_pid"], 4242);
        assert_eq!(second["is_dead"], true);
    }
}
