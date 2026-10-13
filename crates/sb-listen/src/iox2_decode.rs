//! Decode an iceoryx2 `#[repr(C)]` payload into JSON.
//!
//! `sb message compile --iox2` flattens every vault message into a fixed-size
//! struct (see `sb-iox2-typegen`): no nested messages, strings as
//! null-terminated `[u8; N]`, variable-length data as fixed arrays with a
//! `<name>_len` / `<name>_count` companion field emitted right after them.
//! [`Iox2Struct`] describes one such struct, field for field, in emitted
//! order; [`decode_iox2_struct`] walks a payload buffer with it.
//!
//! # Layout
//!
//! [`Iox2Packing::Natural`] is what the Rust (`#[repr(C)]`) and C++ emitters
//! produce: each field at the next offset aligned to its own alignment, the
//! struct size rounded up to the largest field alignment. Scalars align to
//! their size; byte arrays to 1; an array of structs to the element struct.
//! [`Iox2Packing::Packed`] is the Python (`ctypes`, `_pack_ = 1`) and C#
//! (`Pack = 1`) layout: no padding anywhere. The two differ only when a
//! struct has padding, and then their sizes differ, which is how the caller
//! picks one against the live service's payload size.
//!
//! Values are read in native byte order: iceoryx2 is same-host shared memory.
//!
//! # Rendering
//!
//! Scalars become JSON numbers / booleans (non-finite floats become `null`),
//! strings stop at the first NUL (lossy UTF-8), arrays are cut to their
//! companion count. `bytes` fields inline at most 64 bytes as a number array
//! and summarize anything longer as `{"len": N, "head_hex": "<first 16 bytes>"}`;
//! with [`Iox2DecodeOptions::no_data`] they are dropped and only their
//! `<name>_len` companion remains. [`Iox2DecodeOptions::keep_bytes`] skips
//! both rules and inlines every `bytes` field in full (up to its `_len`), for
//! a consumer such as `sb topic listen --show` that needs the data itself;
//! [`crate::DecodedFrame::summarize_bytes`] applies the rules afterwards.

use std::sync::Arc;

use serde_json::{Map, Number, Value};

use crate::decode::{BYTES_HEAD_LEN, BYTES_INLINE_MAX, bytes_summary};

/// Primitive field types (proto enums arrive as [`Iox2Scalar::I32`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Iox2Scalar {
    Bool,
    I32,
    I64,
    U32,
    U64,
    F32,
    F64,
}

impl Iox2Scalar {
    /// Size in bytes, which is also the natural alignment.
    pub const fn size(self) -> usize {
        match self {
            Self::Bool => 1,
            Self::I32 | Self::U32 | Self::F32 => 4,
            Self::I64 | Self::U64 | Self::F64 => 8,
        }
    }
}

/// Type of one flattened field. Mirrors `sb_iox2_typegen::FlatType`, with
/// the element struct of a repeated message resolved instead of named.
#[derive(Debug, Clone, PartialEq)]
pub enum Iox2Type {
    Scalar(Iox2Scalar),
    /// `[u8; capacity]`, null-terminated UTF-8.
    String {
        capacity: usize,
    },
    /// `[u8; capacity]`; the used length is the `<name>_len` field.
    Bytes {
        capacity: usize,
    },
    /// `[elem; capacity]`; the used count is the `<name>_count` field.
    RepeatedScalar {
        elem: Iox2Scalar,
        capacity: usize,
    },
    /// `[elem; capacity]`; the used count is the `<name>_count` field.
    RepeatedStruct {
        elem: Arc<Iox2Struct>,
        capacity: usize,
    },
    /// `[[u8; string_capacity]; capacity]`; the used count is the
    /// `<name>_count` field.
    RepeatedString {
        string_capacity: usize,
        capacity: usize,
    },
}

/// One flattened field, in emitted order.
#[derive(Debug, Clone, PartialEq)]
pub struct Iox2Field {
    pub name: String,
    pub ty: Iox2Type,
}

/// One flattened message, as `sb message compile --iox2` emits it.
#[derive(Debug, Clone, PartialEq)]
pub struct Iox2Struct {
    /// Vault identity `<style>/<ns>/<Leaf>` (`FlatStruct::qualified_name`).
    /// For an iox2 variant this names the base message.
    pub qualified_name: String,
    /// Emitted struct name, which the generated code also pins as the
    /// iceoryx2 type name. Differs from the last segment of
    /// [`Self::qualified_name`] only for a variant (`ImageSmall`).
    pub leaf: String,
    pub fields: Vec<Iox2Field>,
}

/// Which emitter's padding rules a payload follows. See the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Iox2Packing {
    /// Rust `#[repr(C)]` / C++: natural alignment.
    #[default]
    Natural,
    /// Python `ctypes` `_pack_ = 1` / C# `Pack = 1`: no padding.
    Packed,
}

/// Knobs for [`decode_iox2_struct`].
#[derive(Debug, Clone, Copy, Default)]
pub struct Iox2DecodeOptions {
    pub packing: Iox2Packing,
    /// Drop `bytes` fields; their `<name>_len` companion still renders.
    pub no_data: bool,
    /// Inline every `bytes` field in full, ignoring the summary rule and
    /// `no_data`.
    pub keep_bytes: bool,
}

/// Why a buffer could not be decoded with a layout.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Iox2DecodeError {
    #[error("payload is {got} byte(s) but layout {layout} needs {need}")]
    TooShort {
        layout: String,
        got: usize,
        need: usize,
    },
}

impl Iox2Type {
    fn size(&self, packing: Iox2Packing) -> usize {
        match self {
            Self::Scalar(s) => s.size(),
            Self::String { capacity } | Self::Bytes { capacity } => *capacity,
            Self::RepeatedScalar { elem, capacity } => elem.size().saturating_mul(*capacity),
            Self::RepeatedStruct { elem, capacity } => elem.size(packing).saturating_mul(*capacity),
            Self::RepeatedString {
                string_capacity,
                capacity,
            } => string_capacity.saturating_mul(*capacity),
        }
    }

    fn align(&self, packing: Iox2Packing) -> usize {
        if packing == Iox2Packing::Packed {
            return 1;
        }
        match self {
            Self::Scalar(s) | Self::RepeatedScalar { elem: s, .. } => s.size(),
            Self::String { .. } | Self::Bytes { .. } | Self::RepeatedString { .. } => 1,
            Self::RepeatedStruct { elem, .. } => elem.align(packing),
        }
    }
}

/// Field offsets plus total size of one struct under one packing.
struct StructLayout {
    offsets: Vec<usize>,
    size: usize,
}

impl Iox2Struct {
    /// `size_of` the emitted struct under `packing`. Saturates instead of
    /// overflowing, so an absurd layout simply never matches a payload.
    pub fn size(&self, packing: Iox2Packing) -> usize {
        self.layout(packing).size
    }

    /// `align_of` the emitted struct under `packing` (at least 1).
    pub fn align(&self, packing: Iox2Packing) -> usize {
        self.fields
            .iter()
            .map(|f| f.ty.align(packing))
            .max()
            .unwrap_or(1)
            .max(1)
    }

    fn layout(&self, packing: Iox2Packing) -> StructLayout {
        let mut offsets = Vec::with_capacity(self.fields.len());
        let mut end = 0usize;
        for f in &self.fields {
            let offset = align_up(end, f.ty.align(packing));
            offsets.push(offset);
            end = offset.saturating_add(f.ty.size(packing));
        }
        StructLayout {
            offsets,
            size: align_up(end, self.align(packing)),
        }
    }
}

/// Decode one `layout`-shaped struct from the start of `bytes` into a JSON
/// object keyed by field name. Bytes past the struct size are ignored.
pub fn decode_iox2_struct(
    layout: &Iox2Struct,
    bytes: &[u8],
    opts: Iox2DecodeOptions,
) -> Result<Value, Iox2DecodeError> {
    let shape = layout.layout(opts.packing);
    let body = bytes
        .get(..shape.size)
        .ok_or_else(|| Iox2DecodeError::TooShort {
            layout: layout.qualified_name.clone(),
            got: bytes.len(),
            need: shape.size,
        })?;
    Ok(decode_body(layout, &shape, body, opts))
}

/// `body` is exactly `shape.size` bytes, so every field slice is in range.
fn decode_body(
    layout: &Iox2Struct,
    shape: &StructLayout,
    body: &[u8],
    opts: Iox2DecodeOptions,
) -> Value {
    let field_bytes = |i: usize| -> &[u8] {
        let start = shape.offsets[i];
        &body[start..start + layout.fields[i].ty.size(opts.packing)]
    };
    // Used length / count of a variable-length field, from its companion.
    // A missing companion means "all of it"; a corrupt one is clamped.
    let companion = |name: &str, suffix: &str, capacity: usize| -> usize {
        let wanted = format!("{name}{suffix}");
        layout
            .fields
            .iter()
            .position(|f| f.name == wanted)
            .and_then(|i| match layout.fields[i].ty {
                Iox2Type::Scalar(s) => read_count(s, field_bytes(i)),
                _ => None,
            })
            .map_or(capacity, |n| n.min(capacity))
    };

    let mut obj = Map::new();
    for (i, field) in layout.fields.iter().enumerate() {
        let raw = field_bytes(i);
        let value = match &field.ty {
            Iox2Type::Scalar(s) => read_scalar(*s, raw),
            Iox2Type::String { .. } => Value::String(c_string(raw)),
            Iox2Type::Bytes { capacity } => {
                if opts.no_data && !opts.keep_bytes {
                    continue;
                }
                let data = &raw[..companion(&field.name, "_len", *capacity)];
                if data.len() > BYTES_INLINE_MAX && !opts.keep_bytes {
                    bytes_summary(data.len(), data)
                } else {
                    Value::Array(data.iter().map(|&b| Value::from(b)).collect())
                }
            }
            Iox2Type::RepeatedScalar { elem, capacity } => {
                let n = companion(&field.name, "_count", *capacity);
                Value::Array(
                    raw.chunks_exact(elem.size())
                        .take(n)
                        .map(|chunk| read_scalar(*elem, chunk))
                        .collect(),
                )
            }
            Iox2Type::RepeatedString {
                string_capacity,
                capacity,
            } => {
                let n = companion(&field.name, "_count", *capacity);
                let strings = if *string_capacity == 0 {
                    vec![Value::String(String::new()); n]
                } else {
                    raw.chunks_exact(*string_capacity)
                        .take(n)
                        .map(|chunk| Value::String(c_string(chunk)))
                        .collect()
                };
                Value::Array(strings)
            }
            Iox2Type::RepeatedStruct { elem, capacity } => {
                let n = companion(&field.name, "_count", *capacity);
                let elem_shape = elem.layout(opts.packing);
                let items = if elem_shape.size == 0 {
                    vec![Value::Object(Map::new()); n]
                } else {
                    raw.chunks_exact(elem_shape.size)
                        .take(n)
                        .map(|chunk| decode_body(elem, &elem_shape, chunk, opts))
                        .collect()
                };
                Value::Array(items)
            }
        };
        obj.insert(field.name.clone(), value);
    }
    Value::Object(obj)
}

/// Apply the `bytes` rules to `json`, decoded from `layout` with
/// [`Iox2DecodeOptions::keep_bytes`]: the same output a decode without it
/// would have produced. `json` is one struct object or, for a slice
/// payload, an array of them.
pub(crate) fn summarize_iox2_bytes(layout: &Iox2Struct, json: &mut Value, no_data: bool) {
    let obj = match json {
        Value::Array(items) => {
            for item in items {
                summarize_iox2_bytes(layout, item, no_data);
            }
            return;
        }
        Value::Object(obj) => obj,
        _ => return,
    };
    for field in &layout.fields {
        match &field.ty {
            Iox2Type::Bytes { .. } => {
                if no_data {
                    obj.remove(&field.name);
                    continue;
                }
                let Some(Value::Array(data)) = obj.get(&field.name) else {
                    continue;
                };
                if data.len() > BYTES_INLINE_MAX {
                    let head: Vec<u8> = data
                        .iter()
                        .take(BYTES_HEAD_LEN)
                        .map(|b| b.as_u64().and_then(|b| u8::try_from(b).ok()).unwrap_or(0))
                        .collect();
                    let summary = bytes_summary(data.len(), &head);
                    obj.insert(field.name.clone(), summary);
                }
            }
            Iox2Type::RepeatedStruct { elem, .. } => {
                if let Some(Value::Array(items)) = obj.get_mut(&field.name) {
                    for item in items {
                        summarize_iox2_bytes(elem, item, no_data);
                    }
                }
            }
            _ => {}
        }
    }
}

fn align_up(offset: usize, align: usize) -> usize {
    match offset % align {
        0 => offset,
        rem => offset.saturating_add(align - rem),
    }
}

/// `raw` is exactly `scalar.size()` bytes long.
fn read_scalar(scalar: Iox2Scalar, raw: &[u8]) -> Value {
    fn arr<const N: usize>(raw: &[u8]) -> [u8; N] {
        let mut out = [0u8; N];
        out.copy_from_slice(&raw[..N]);
        out
    }
    match scalar {
        Iox2Scalar::Bool => Value::Bool(raw[0] != 0),
        Iox2Scalar::I32 => Value::from(i32::from_ne_bytes(arr(raw))),
        Iox2Scalar::I64 => Value::from(i64::from_ne_bytes(arr(raw))),
        Iox2Scalar::U32 => Value::from(u32::from_ne_bytes(arr(raw))),
        Iox2Scalar::U64 => Value::from(u64::from_ne_bytes(arr(raw))),
        // Through the shortest decimal form, so 0.1f32 renders as 0.1
        // rather than 0.10000000149011612.
        Iox2Scalar::F32 => float_value(
            f32::from_ne_bytes(arr(raw))
                .to_string()
                .parse::<f64>()
                .unwrap_or(f64::NAN),
        ),
        Iox2Scalar::F64 => float_value(f64::from_ne_bytes(arr(raw))),
    }
}

/// A companion `_len` / `_count` value as a length.
fn read_count(scalar: Iox2Scalar, raw: &[u8]) -> Option<usize> {
    match read_scalar(scalar, raw) {
        Value::Number(n) => n.as_u64().and_then(|n| usize::try_from(n).ok()),
        _ => None,
    }
}

fn float_value(v: f64) -> Value {
    Number::from_f64(v).map_or(Value::Null, Value::Number)
}

/// Bytes up to the first NUL, as lossy UTF-8.
fn c_string(raw: &[u8]) -> String {
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    String::from_utf8_lossy(&raw[..end]).into_owned()
}
