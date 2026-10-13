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
