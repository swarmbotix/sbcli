//! jq-colored JSON for `sb topic listen` on a terminal.
//!
//! serde_json still serializes the frame (field order, layout, escaping);
//! `JqFormatter` only wraps its formatter to bracket keys, string values, and
//! `null` in ANSI styles, so stripping the escapes gives back exactly
//! [`crate::render_decoded_json_line`] / [`crate::render_decoded_json_pretty`].
//! Walking a `serde_json::Value` would not: its maps sort their keys (no
//! `preserve_order`), losing the pinned [`DecodedFrame`] field order.

use std::io::{self, Write};

use anstyle::{AnsiColor, Style};
use serde::Serialize;
use serde_json::ser::{CompactFormatter, Formatter, PrettyFormatter, Serializer};

use crate::decode::DecodedFrame;

/// jq 1.7's default palette. Numbers, booleans, and punctuation stay unstyled.
const KEY: Style = AnsiColor::Blue.on_default().bold();
const STRING: Style = AnsiColor::Green.on_default();
const NULL: Style = AnsiColor::BrightBlack.on_default();

/// Render `frame` as JSON with jq's default colors. `pretty` picks the
/// serde_json pretty layout, else compact. No trailing newline.
pub fn render_decoded_json_colored(frame: &DecodedFrame<'_>, pretty: bool) -> String {
    if pretty {
        render(frame, PrettyFormatter::new())
    } else {
        render(frame, CompactFormatter)
    }
}

fn render<F: Formatter>(frame: &DecodedFrame<'_>, layout: F) -> String {
    let jq = JqFormatter {
        layout,
        string_style: STRING,
    };
    let mut ser = Serializer::with_formatter(Vec::with_capacity(256), jq);
    if frame.serialize(&mut ser).is_err() {
        return "{}".into();
    }
    // serde_json writes UTF-8 and the style escapes are ASCII.
    String::from_utf8(ser.into_inner()).unwrap_or_default()
}

/// Hands every layout hook to `layout` (whitespace and indent state stay
/// serde_json's) and wraps keys, strings, and `null` in the palette. Number,
/// bool, and string-content hooks keep the trait defaults, as both wrapped
/// formatters do.
struct JqFormatter<F> {
    layout: F,
    /// `KEY` between `begin_object_key` and `end_object_key`, else `STRING`.
    string_style: Style,
}

impl<F: Formatter> Formatter for JqFormatter<F> {
    fn write_null<W: ?Sized + Write>(&mut self, w: &mut W) -> io::Result<()> {
        write!(w, "{NULL}")?;
        self.layout.write_null(w)?;
        write!(w, "{NULL:#}")
    }

    fn begin_string<W: ?Sized + Write>(&mut self, w: &mut W) -> io::Result<()> {
        let style = self.string_style;
        write!(w, "{style}")?;
        self.layout.begin_string(w)
    }

    fn end_string<W: ?Sized + Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.layout.end_string(w)?;
        let style = self.string_style;
        write!(w, "{style:#}")
    }

    fn begin_object_key<W: ?Sized + Write>(&mut self, w: &mut W, first: bool) -> io::Result<()> {
        self.string_style = KEY;
        self.layout.begin_object_key(w, first)
    }

    fn end_object_key<W: ?Sized + Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.string_style = STRING;
        self.layout.end_object_key(w)
    }

    // Layout only, straight through.

    fn begin_array<W: ?Sized + Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.layout.begin_array(w)
    }

    fn end_array<W: ?Sized + Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.layout.end_array(w)
    }

    fn begin_array_value<W: ?Sized + Write>(&mut self, w: &mut W, first: bool) -> io::Result<()> {
        self.layout.begin_array_value(w, first)
    }

    fn end_array_value<W: ?Sized + Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.layout.end_array_value(w)
    }

    fn begin_object<W: ?Sized + Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.layout.begin_object(w)
    }

    fn end_object<W: ?Sized + Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.layout.end_object(w)
    }

    fn begin_object_value<W: ?Sized + Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.layout.begin_object_value(w)
    }

    fn end_object_value<W: ?Sized + Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.layout.end_object_value(w)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FramePayload, decode_frame, render_decoded_json_line, render_decoded_json_pretty};
    use serde_json::json;

    /// Drop every `ESC [ ... m` sequence.
    fn strip(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                let _ = chars.by_ref().find(|&c| c == 'm');
            } else {
                out.push(c);
            }
        }
        out
    }

    /// Color adds escapes only: stripped, both layouts are byte-for-byte the
    /// plain render, struct field order and string escaping included.
    #[test]
    fn stripped_colors_are_the_plain_render() {
        let mut frame = decode_frame("zenoh", "demo/x", &[], true);
        frame.schema = Some("ros2/std/\"Quoted\"".into());
        frame.payload = FramePayload::Json {
            json: json!({
                "text": "say \"hi\"\n\tcaf\u{e9} \u{1f980} \u{7}",
                "rate": 2.5,
                "count": -7,
                "big": u64::MAX,
                "ok": true,
                "none": null,
                "empty": [],
                "blank": {},
                "items": [{ "id": 1, "tags": ["a", "b"] }, { "id": 2, "tags": [] }]
            }),
        };
        let compact = render_decoded_json_colored(&frame, false);
        let pretty = render_decoded_json_colored(&frame, true);
        assert_eq!(strip(&compact), render_decoded_json_line(&frame));
        assert_eq!(strip(&pretty), render_decoded_json_pretty(&frame));

        // jq's palette on a key, a string value, and a null.
        let key = format!("{KEY}\"topic\"{KEY:#}");
        let string = format!("{STRING}\"zenoh\"{STRING:#}");
        let null = format!("{NULL}null{NULL:#}");
        for colored in [&compact, &pretty] {
            for painted in [&key, &string, &null] {
                assert!(colored.contains(painted), "{painted:?} in {colored:?}");
            }
        }
    }
}
