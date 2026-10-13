//! Pull image payloads back out of a decoded `sb topic listen` body, for the
//! `--show` viewer ([`crate::view`]).
//!
//! The body must have been decoded with
//! [`DecodeContext::keep_bytes`](crate::DecodeContext::keep_bytes), so `data`
//! is whole. It then has one of two shapes:
//!
//! - **protobuf**: a base64 string (proto3 JSON). Zero-valued scalars are
//!   omitted by the serializer, so a missing `step` reads as `0`.
//! - **iox2**: an array of byte values, already cut to the `data_len`
//!   companion; `data_len` is honoured again here in case it is not.
//!
//! # Where the image is
//!
//! `width`, `height`, `encoding`, `step` and `data` are top-level fields of
//! `ros2/sensor_msgs/Image`, `ros2/std/Image` and `swarmbotix/images/Image`
//! (only `header` nests), and `format` + `data` of
//! `ros2/sensor_msgs/CompressedImage`. A `*Stamped` wrapper such as
//! `ros2/std/ImageStamped` holds the image one level down, in a message
//! field (`image`). So the image is looked for, first hit wins:
//!
//! 1. in the body itself;
//! 2. in a top-level object-valued field (protobuf keeps a nested message as
//!    an object), in key order;
//! 3. under a `<field>_` prefix among the top-level keys (`image_width`,
//!    `image_data`, …): iox2 flattens a nested message into its parent
//!    that way.
//!
//! Nothing deeper is searched. "Image-like" uses the criteria of
//! [`crate::view::looks_like_image`] and
//! [`crate::view::looks_like_compressed_image`], applied to the prefixed
//! keys in case 3.

use serde_json::{Map, Value};

/// The fields [`crate::view::ImageView::show_raw`] takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageFields {
    pub width: u32,
    pub height: u32,
    pub encoding: String,
    /// Bytes per row; `0` means tightly packed.
    pub step: u32,
    pub data: Vec<u8>,
}

/// Read a raw image (`sensor_msgs/Image` shape) out of a decoded body, from
/// the body itself or one level down (see the module docs). `None` when no
/// image-like fields are found or they are malformed.
pub fn extract_image(obj: &Map<String, Value>) -> Option<ImageFields> {
    let f = locate(obj, is_raw_image)?;
    Some(ImageFields {
        width: as_u32(f.get("width")?)?,
        height: as_u32(f.get("height")?)?,
        encoding: f.get("encoding")?.as_str()?.to_owned(),
        step: match f.get("step") {
            None => 0,
            Some(v) => as_u32(v)?,
        },
        data: f.data()?,
    })
}

/// Read a compressed image (`sensor_msgs/CompressedImage` shape) out of a
/// decoded body as `(format, data)`, from the body itself or one level down
/// (see the module docs). `None` when no such fields are found or they are
/// malformed.
pub fn extract_compressed(obj: &Map<String, Value>) -> Option<(String, Vec<u8>)> {
    let f = locate(obj, is_compressed_image)?;
    Some((f.get("format")?.as_str()?.to_owned(), f.data()?))
}

/// One candidate set of image fields: the keys of `obj`, each read as
/// `<prefix><name>`.
struct Fields<'v> {
    obj: &'v Map<String, Value>,
    prefix: &'v str,
}

impl<'v> Fields<'v> {
    fn get(&self, name: &str) -> Option<&'v Value> {
        if self.prefix.is_empty() {
            self.obj.get(name)
        } else {
            self.obj.get(&format!("{}{name}", self.prefix))
        }
    }

    /// `data` as bytes, from either shape described in the module docs.
    fn data(&self) -> Option<Vec<u8>> {
        match self.get("data")? {
            Value::String(text) => base64_decode(text),
            Value::Array(items) => {
                let used = match self.get("data_len") {
                    None => items.len(),
                    Some(n) => usize::try_from(n.as_u64()?).ok()?.min(items.len()),
                };
                items[..used]
                    .iter()
                    .map(|b| b.as_u64().and_then(|b| u8::try_from(b).ok()))
                    .collect()
            }
            _ => None,
        }
    }
}

/// [`crate::view::looks_like_image`], on possibly prefixed keys.
fn is_raw_image(f: &Fields<'_>) -> bool {
    f.get("width").is_some_and(Value::is_number)
        && f.get("height").is_some_and(Value::is_number)
        && f.get("encoding").is_some_and(Value::is_string)
        && f.get("data").is_some()
}

/// [`crate::view::looks_like_compressed_image`], on possibly prefixed keys.
fn is_compressed_image(f: &Fields<'_>) -> bool {
    f.get("format").is_some_and(Value::is_string)
        && f.get("data").is_some()
        && f.get("width").is_none()
}

/// First image-like field set, in the order the module docs give.
fn locate<'v>(
    obj: &'v Map<String, Value>,
    image_like: fn(&Fields<'_>) -> bool,
) -> Option<Fields<'v>> {
    let body = Fields { obj, prefix: "" };
    if image_like(&body) {
        return Some(body);
    }
    let nested = obj.values().find_map(|v| match v {
        Value::Object(child) => Some(Fields {
            obj: child,
            prefix: "",
        })
        .filter(|f| image_like(f)),
        _ => None,
    });
    if nested.is_some() {
        return nested;
    }
    // Flattened (iox2): every `<p>data` key proposes the prefix `<p>`.
    obj.keys()
        .filter_map(|k| k.strip_suffix("data").filter(|p| p.ends_with('_')))
        .map(|prefix| Fields { obj, prefix })
        .find(|f| image_like(f))
}

fn as_u32(v: &Value) -> Option<u32> {
    v.as_u64().and_then(|n| u32::try_from(n).ok())
}

/// Decode base64: the standard alphabet prost-reflect writes, plus the
/// URL-safe one; padding optional. `None` on any other character.
fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let text = text.trim_end_matches('=');
    let mut out = Vec::with_capacity(text.len() / 4 * 3 + 2);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for c in text.bytes() {
        let sextet = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(sextet);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}
