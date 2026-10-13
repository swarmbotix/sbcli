//! Decode bytes into the `sb topic listen` JSON envelope.
//!
//! Two paths:
//!
//! - [`decode_frame`] — Header-probe-only. Lifts `schema` and tags
//!   `encoding: "protobuf"` for any Header-leading `*Stamped` payload,
//!   but always renders the body as hex. Used when the caller has no
//!   decode resources handy.
//! - [`decode_frame_dynamic`] — resolves the schema, then decodes the body
//!   into a JSON object. Falls back to hex on any miss (schema unknown,
//!   decode error, layout size mismatch) so the CLI surface never silently
//!   lies about what's on the wire.
//!
//! Schema resolution order in [`decode_frame_dynamic`], first hit wins; the
//! winner is reported in the `schema_source` column:
//!
//! 1. `"flag"`   — `--type <style>/<ns>/<Leaf>` from the command line.
//! 2. `"wire"`   — what the transport carried: Zenoh's encoding schema (a
//!    vault name) or the iceoryx2 service's payload type name (a
//!    language-native name, mapped to a vault iox2 struct by leaf + size).
//! 3. `"header"` — the swarmbotix `Header.metadata.msg_type` probe.
//! 4. `"config"`: the `type` the publishing module's `sb.dev.yml` declares
//!    for the topic ([`DecodeContext::config_schema`]). Covers publishers
//!    that stamp neither an encoding schema nor a Header type (older
//!    templates).
//!
//! Nothing resolved → hex with a null schema.
//!
//! A Zenoh sample's encoding is read as a [`WireTag`]. Without `--type`:
//!
//! - `Protobuf(name)` resolves the schema as `"wire"` and decodes as
//!   protobuf, as before.
//! - `Flatbuffers(name)` decodes through `flatc` against the compiled
//!   binary schema `<message_targets>/fb/<ns>/<Leaf>/<Leaf>.bfbs` of the
//!   name's style ([`FbLookup`]); without one it prints hex with `schema`
//!   set and notes once on stderr how to get one.
//! - `Text(mime)` prints the payload as a string,
//!   `{"kind": "text", "text": "…"}`, with the MIME type as `schema`.
//! - `Octets` and `Unknown` add nothing; the Header probe and the module
//!   config still apply, which keeps untagged (0.4.x) samples as they were.
//!
//! `encoding` in the JSON line is `"protobuf"`, `"flatbuffers"`, `"text"`,
//! `"iox2"`, or `null`.
//!
//! Protobuf bodies decode through a [`DescriptorPool`]; iceoryx2 bodies are
//! `#[repr(C)]` structs and decode through [`crate::iox2_decode`]. `bytes`
//! fields render the same way on both paths: inline when at most
//! [`BYTES_INLINE_MAX`] bytes, otherwise as `{"len": N, "head_hex": …}`, and
//! dropped (only `<name>_len` kept) under `no_data`. With
//! [`DecodeContext::keep_bytes`] the body keeps every `bytes` field whole (for
//! `sb topic listen --show`), and [`DecodedFrame::summarize_bytes`] applies
//! those rules afterwards, so stdout is the same either way.
//!
//! `--raw` short-circuits BOTH paths to hex + null schema.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Once;

use prost_reflect::{DescriptorPool, DynamicMessage, Kind, ReflectMessage, Value as ProtoValue};
use sb_discover::{WireTag, probe_header};
use sb_vault::{MessageName, Vault};
use serde::Serialize;
use serde_json::Value;

use crate::Frame;
use crate::iceoryx::Iox2PayloadDetail;
use crate::iox2_decode::{
    Iox2DecodeOptions, Iox2Packing, Iox2Struct, decode_iox2_struct, summarize_iox2_bytes,
};

/// Transport tag that selects iceoryx2 `#[repr(C)]` decoding for flag and
/// wire schemas. Matches what `sb topic listen` passes as `transport`.
const TRANSPORT_ICEORYX2: &str = "iceoryx2";

const ENCODING_PROTOBUF: &str = "protobuf";
/// Body is an iceoryx2 flat `#[repr(C)]` struct (the `--iox2` backend).
const ENCODING_IOX2: &str = "iox2";
/// Body is FlatBuffers bytes (a [`WireTag::Flatbuffers`] sample).
const ENCODING_FLATBUFFERS: &str = "flatbuffers";
/// Body is text (a [`WireTag::Text`] sample).
const ENCODING_TEXT: &str = "text";

/// A `bytes` field longer than this renders as a summary object instead of
/// its full contents.
pub(crate) const BYTES_INLINE_MAX: usize = 64;
/// How many leading bytes a summary object shows in `head_hex`.
pub(crate) const BYTES_HEAD_LEN: usize = 16;

/// Where [`decode_frame_dynamic`] found the schema. Serialized into the
/// `schema_source` column via [`SchemaSource::as_str`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SchemaSource {
    Flag,
    Wire,
    Header,
    Config,
}

impl SchemaSource {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Flag => "flag",
            Self::Wire => "wire",
            Self::Header => "header",
            Self::Config => "config",
        }
    }
}

/// One frame as the CLI hands it to stdout. Field order is pinned —
/// snapshot tests assert against it, and downstream tooling (swarmctl)
/// will too.
#[derive(Debug, Clone, Serialize)]
pub struct DecodedFrame<'a> {
    pub transport: &'a str,
    pub topic: &'a str,
    pub schema: Option<String>,
    /// `"flag"` | `"wire"` | `"header"` | `"config"`: which source supplied
    /// `schema`.
    /// `None` whenever `schema` is `None`.
    pub schema_source: Option<&'static str>,
    pub hash: Option<String>,
    /// `"protobuf"` | `"flatbuffers"` | `"text"` | `"iox2"`, or `None` when
    /// nothing says how the body is framed.
    pub encoding: Option<&'static str>,
    pub payload: FramePayload,
    /// Set when the body was decoded with [`DecodeContext::keep_bytes`]:
    /// what [`Self::summarize_bytes`] needs to find the `bytes` fields.
    #[serde(skip)]
    unsummarized: Option<BytesSchema<'a>>,
}

/// Schema of a body whose `bytes` fields are still whole. The JSON alone
/// cannot say which fields are `bytes`: protobuf renders them as base64
/// strings (indistinguishable from text) and iox2 as number arrays
/// (indistinguishable from a repeated integer).
#[derive(Debug, Clone)]
enum BytesSchema<'a> {
    Protobuf(DynamicMessage),
    Iox2(&'a Iox2Struct),
}

impl DecodedFrame<'_> {
    /// Apply the `bytes` rendering rules (summary above
    /// [`BYTES_INLINE_MAX`], `no_data`) to a body decoded with
    /// [`DecodeContext::keep_bytes`], producing exactly what a decode
    /// without it would have printed. A no-op for every other frame.
    pub fn summarize_bytes(&mut self, no_data: bool) {
        let Some(schema) = self.unsummarized.take() else {
            return;
        };
        let FramePayload::Json { json } = &mut self.payload else {
            return;
        };
        match schema {
            BytesSchema::Protobuf(msg) => shape_proto_bytes(&msg, json, no_data),
            BytesSchema::Iox2(layout) => summarize_iox2_bytes(layout, json, no_data),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FramePayload {
    Hex {
        bytes: String,
    },
    Json {
        json: Value,
    },
    /// The body of a [`WireTag::Text`] sample, as UTF-8.
    Text {
        text: String,
    },
}

/// Everything [`decode_frame_dynamic`] needs beyond the frame itself. Built
/// once per listen session; every member is optional, and a missing one only
/// narrows which bodies can be decoded (the rest fall back to hex).
#[derive(Debug, Clone, Copy, Default)]
pub struct DecodeContext<'a> {
    /// Protobuf descriptors for the vault, see [`crate::build_pool`].
    pub pool: Option<&'a DescriptorPool>,
    /// iceoryx2 layouts plus the live service's payload type.
    pub iox2: Iox2Lookup<'a>,
    /// Drop `bytes` field contents and keep only their `<name>_len`.
    pub no_data: bool,
    /// Keep every `bytes` field whole in the decoded body, ignoring the
    /// summary rule and `no_data`, for a consumer that needs the data (the
    /// `--show` viewer). Call [`DecodedFrame::summarize_bytes`] before
    /// printing.
    pub keep_bytes: bool,
    /// Vault name the publishing module's `sb.dev.yml` declares for this
    /// topic (`sb_discover::ResolveOutcome::schema`). The last schema
    /// source, after the Header probe.
    pub config_schema: Option<&'a str>,
    /// What decodes a [`WireTag::Flatbuffers`] sample.
    pub flatbuffers: FbLookup<'a>,
}

/// What decodes a FlatBuffers payload: `flatc` plus the vault whose
/// `message_targets` hold the compiled binary schemas. Either missing
/// leaves FlatBuffers frames as hex.
#[derive(Debug, Clone, Copy, Default)]
pub struct FbLookup<'a> {
    /// The `flatc` binary (`sb.config.yml`'s `flatc:`).
    pub flatc: Option<&'a Path>,
    /// The vault; a message's binary schema is looked up at
    /// `<targets_dir(style)>/fb/<ns>/<Leaf>/<Leaf>.bfbs`.
    pub vault: Option<&'a Vault>,
}

impl FbLookup<'_> {
    /// Where the compiled binary schema of vault message `name` lives,
    /// whether or not it exists. `None` without a vault or for a name that
    /// is not `<style>/<ns>/<Leaf>`.
    pub fn bfbs_path(&self, name: &str) -> Option<PathBuf> {
        let vault = self.vault?;
        let msg = MessageName::parse(name).ok()?;
        Some(
            vault
                .targets_dir(&msg.style)
                .join("fb")
                .join(&msg.namespace)
                .join(&msg.leaf)
                .join(format!("{}.bfbs", msg.leaf)),
        )
    }
}

/// What maps an iceoryx2 payload to a vault message.
#[derive(Debug, Clone, Copy, Default)]
pub struct Iox2Lookup<'a> {
    /// Every vault message flattened for iceoryx2, as `sb message compile
    /// --iox2` would emit it.
    pub structs: &'a [Iox2Struct],
    /// The live service's payload type ([`crate::IceoryxFrames::payload_detail`]).
    /// Its `size` picks between same-leaf candidates and between the natural
    /// and packed layouts; without it the frame length stands in.
    pub payload: Option<&'a Iox2PayloadDetail>,
}

/// Decode `bytes` into a [`DecodedFrame`]. `raw=true` short-circuits the
/// Header probe so the schema column reports `null` even for sb-generated
/// frames (matches `sb topic listen --raw`).
pub fn decode_frame<'a>(
    transport: &'a str,
    topic: &'a str,
    bytes: &[u8],
    raw: bool,
) -> DecodedFrame<'a> {
    let schema = if raw {
        None
    } else {
        probe_header(bytes)
            .map(|m| m.msg_type)
            .filter(|s| !s.is_empty())
    };
    let (source, encoding) = if schema.is_some() {
        (Some(SchemaSource::Header), Some(ENCODING_PROTOBUF))
    } else {
        (None, None)
    };
    hex_frame(transport, topic, schema, source, encoding, bytes)
}

/// Resolve the schema (`type_override` > `frame.wire_schema` > Header
/// probe > `ctx.config_schema`) and decode the body with it.
///
/// - `transport == "iceoryx2"` with a flag, wire, or config schema decodes
///   the body as a `#[repr(C)]` struct from `ctx.iox2`. A wire type name
///   maps to the vault struct with the same leaf (last `::` / `.` segment)
///   whose layout size equals the payload's; no match renders hex with the
///   native name as `schema`. A `--type` or config name that matches no
///   iox2 layout is tried as protobuf next (a `[u8]` slab may carry
///   protobuf bytes).
/// - Everything else decodes as protobuf through `ctx.pool`.
///
/// Falls back to hex when `raw` is set, nothing resolved the schema, the
/// schema is unknown, or the body does not decode with it.
pub fn decode_frame_dynamic<'a>(
    transport: &'a str,
    topic: &'a str,
    frame: &Frame,
    raw: bool,
    type_override: Option<&str>,
    ctx: &DecodeContext<'a>,
) -> DecodedFrame<'a> {
    if raw {
        return decode_frame(transport, topic, &frame.bytes, true);
    }
    // A FlatBuffers or text tag says how to read the body; only `--type`
    // overrides it.
    let flag = type_override.map(str::trim).filter(|t| !t.is_empty());
    match (&frame.wire_tag, flag) {
        (Some(WireTag::Flatbuffers(name)), None) => {
            return decode_flatbuffers(transport, topic, name, &frame.bytes, ctx.flatbuffers);
        }
        (Some(WireTag::Text(mime)), None) => {
            return text_frame(transport, topic, mime, &frame.bytes);
        }
        _ => {}
    }
    let Some((schema, source)) = resolve_schema(type_override, frame, ctx.config_schema) else {
        return hex_frame(transport, topic, None, None, None, &frame.bytes);
    };
    if transport == TRANSPORT_ICEORYX2 && source != SchemaSource::Header {
        if let Some((layout, decoded)) = decode_iox2(&schema, source, &frame.bytes, ctx) {
            let name = layout.qualified_name.clone();
            return match decoded {
                Some(json) => DecodedFrame {
                    transport,
                    topic,
                    schema: Some(name),
                    schema_source: Some(source.as_str()),
                    hash: None,
                    encoding: Some(ENCODING_IOX2),
                    payload: FramePayload::Json { json },
                    unsummarized: ctx.keep_bytes.then_some(BytesSchema::Iox2(layout)),
                },
                None => hex_frame(
                    transport,
                    topic,
                    Some(name),
                    Some(source),
                    Some(ENCODING_IOX2),
                    &frame.bytes,
                ),
            };
        }
        if source == SchemaSource::Wire {
            // A typed service whose type maps to no vault layout: keep the
            // native name so the user sees what is on the wire.
            return hex_frame(
                transport,
                topic,
                Some(schema),
                Some(source),
                Some(ENCODING_IOX2),
                &frame.bytes,
            );
        }
    }
    decode_protobuf(transport, topic, schema, source, &frame.bytes, ctx)
}

/// First hit of `type_override`, `frame.wire_schema`, Header probe,
/// `config_schema`.
fn resolve_schema(
    type_override: Option<&str>,
    frame: &Frame,
    config_schema: Option<&str>,
) -> Option<(String, SchemaSource)> {
    if let Some(t) = type_override.map(str::trim).filter(|t| !t.is_empty()) {
        return Some((t.to_owned(), SchemaSource::Flag));
    }
    if let Some(w) = frame.wire_schema.as_deref().filter(|w| !w.is_empty()) {
        return Some((w.to_owned(), SchemaSource::Wire));
    }
    if let Some(h) = probe_header(&frame.bytes)
        .map(|m| m.msg_type)
        .filter(|s| !s.is_empty())
    {
        return Some((h, SchemaSource::Header));
    }
    config_schema
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(|c| (c.to_owned(), SchemaSource::Config))
}

/// Pick the vault iox2 layout for `schema` and decode `bytes` with it.
///
/// `None` when no layout matches. Otherwise the chosen layout, plus the
/// decoded body (`None` if the bytes do not fit it).
fn decode_iox2<'a>(
    schema: &str,
    source: SchemaSource,
    bytes: &[u8],
    ctx: &DecodeContext<'a>,
) -> Option<(&'a Iox2Struct, Option<Value>)> {
    let payload = ctx.iox2.payload;
    let elem_size = payload
        .map(|p| p.size)
        .filter(|&n| n > 0)
        .unwrap_or(bytes.len());
    let candidates: Vec<&'a Iox2Struct> = match source {
        SchemaSource::Wire => {
            let leaf = native_leaf(schema);
            ctx.iox2.structs.iter().filter(|s| s.leaf == leaf).collect()
        }
        _ => ctx
            .iox2
            .structs
            .iter()
            .filter(|s| matches_vault_name(s, schema))
            .collect(),
    };
    // Natural (Rust / C++) first; packed covers Python ctypes and C#.
    let (layout, packing) = [Iox2Packing::Natural, Iox2Packing::Packed]
        .into_iter()
        .find_map(|p| {
            candidates
                .iter()
                .find(|s| s.size(p) == elem_size)
                .map(|s| (*s, p))
        })?;
    let opts = Iox2DecodeOptions {
        packing,
        no_data: ctx.no_data,
        keep_bytes: ctx.keep_bytes,
    };
    let is_slice = payload.is_some_and(|p| p.is_slice);
    let decoded = if is_slice && elem_size > 0 && bytes.len() % elem_size == 0 {
        bytes
            .chunks_exact(elem_size)
            .map(|chunk| decode_iox2_struct(layout, chunk, opts))
            .collect::<Result<Vec<_>, _>>()
            .ok()
            .map(Value::Array)
    } else {
        decode_iox2_struct(layout, bytes, opts).ok()
    };
    Some((layout, decoded))
}

/// Leaf of a language-native type name: the last `::` (Rust, C++) or `.`
/// (Python, C#) segment. `my_crate::iox2_types::std::StringStamped` →
/// `StringStamped`.
fn native_leaf(type_name: &str) -> &str {
    let tail = type_name.rsplit("::").next().unwrap_or(type_name);
    tail.rsplit('.').next().unwrap_or(tail)
}

/// Whether `name` (from `--type` or the module config) names `layout`: the full
/// `<style>/<ns>/<Leaf>`, the same with a variant's struct name as leaf, or
/// the legacy two-segment `<ns>/<Leaf>`.
fn matches_vault_name(layout: &Iox2Struct, name: &str) -> bool {
    let name = name.trim_matches('/');
    let qualified = layout.qualified_name.as_str();
    if qualified == name || qualified.ends_with(&format!("/{name}")) {
        return true;
    }
    qualified
        .rsplit_once('/')
        .is_some_and(|(dir, _)| name == format!("{dir}/{}", layout.leaf))
}

/// Protobuf half of [`decode_frame_dynamic`].
fn decode_protobuf<'a>(
    transport: &'a str,
    topic: &'a str,
    schema: String,
    source: SchemaSource,
    bytes: &[u8],
    ctx: &DecodeContext<'_>,
) -> DecodedFrame<'a> {
    let desc = ctx.pool.and_then(|pool| {
        protobuf_name_candidates(&schema)
            .iter()
            .find_map(|n| pool.get_message_by_name(n))
    });
    let Some(desc) = desc else {
        // Schema known to publisher but not in the consumer's pool —
        // fall through to hex with a populated schema column so the
        // user knows what they're missing. A Header probe or a protobuf
        // wire tag (the only wire schema that reaches here) proves the
        // body is protobuf; a flag or config name alone does not.
        let encoding = matches!(source, SchemaSource::Header | SchemaSource::Wire)
            .then_some(ENCODING_PROTOBUF);
        return hex_frame(
            transport,
            topic,
            Some(schema),
            Some(source),
            encoding,
            bytes,
        );
    };
    let decoded = DynamicMessage::decode(desc, bytes).ok().and_then(|msg| {
        let mut json = serde_json::to_value(&msg).ok()?;
        if ctx.keep_bytes {
            return Some((json, Some(BytesSchema::Protobuf(msg))));
        }
        shape_proto_bytes(&msg, &mut json, ctx.no_data);
        Some((json, None))
    });
    let Some((json, unsummarized)) = decoded else {
        return hex_frame(
            transport,
            topic,
            Some(schema),
            Some(source),
            Some(ENCODING_PROTOBUF),
            bytes,
        );
    };
    DecodedFrame {
        transport,
        topic,
        schema: Some(schema),
        schema_source: Some(source.as_str()),
        hash: None,
        encoding: Some(ENCODING_PROTOBUF),
        payload: FramePayload::Json { json },
        unsummarized,
    }
}

/// A [`WireTag::Text`] frame: the body as a string, the MIME type as
/// `schema`. A body that is not UTF-8 prints as hex, schema kept.
fn text_frame<'a>(
    transport: &'a str,
    topic: &'a str,
    mime: &str,
    bytes: &[u8],
) -> DecodedFrame<'a> {
    match std::str::from_utf8(bytes) {
        Ok(text) => DecodedFrame {
            transport,
            topic,
            schema: Some(mime.to_owned()),
            schema_source: Some(SchemaSource::Wire.as_str()),
            hash: None,
            encoding: Some(ENCODING_TEXT),
            payload: FramePayload::Text {
                text: text.to_owned(),
            },
            unsummarized: None,
        },
        Err(_) => hex_frame(
            transport,
            topic,
            Some(mime.to_owned()),
            Some(SchemaSource::Wire),
            Some(ENCODING_TEXT),
            bytes,
        ),
    }
}

/// Printed (once per process) when a FlatBuffers frame cannot be decoded
/// for lack of a compiled binary schema.
static FB_NO_BFBS_NOTE: Once = Once::new();
/// Printed (once per process) when a FlatBuffers frame cannot be decoded
/// for lack of `flatc`.
static FB_NO_FLATC_NOTE: Once = Once::new();

/// A [`WireTag::Flatbuffers`] frame, decoded to JSON by `flatc` when the
/// message's `.bfbs` and `flatc` are both available, else hex with
/// `schema` set (and a one-time stderr note naming what is missing). A
/// body `flatc` rejects also prints as hex. `bytes` fields are not
/// summarized: `flatc` renders a byte vector as a plain number array.
fn decode_flatbuffers<'a>(
    transport: &'a str,
    topic: &'a str,
    name: &str,
    bytes: &[u8],
    fb: FbLookup<'_>,
) -> DecodedFrame<'a> {
    let hex = || {
        hex_frame(
            transport,
            topic,
            Some(name.to_owned()),
            Some(SchemaSource::Wire),
            Some(ENCODING_FLATBUFFERS),
            bytes,
        )
    };
    let Some(bfbs) = fb.bfbs_path(name).filter(|p| p.is_file()) else {
        FB_NO_BFBS_NOTE.call_once(|| {
            eprintln!(
                "note: flatbuffers decode needs a compiled .bfbs; run sb message compile --fb \
                 (showing {name} as hex)"
            );
        });
        return hex();
    };
    let Some(flatc) = fb.flatc else {
        FB_NO_FLATC_NOTE.call_once(|| {
            eprintln!(
                "note: flatbuffers decode needs flatc; set it in sb.config.yml \
                 (showing {name} as hex)"
            );
        });
        return hex();
    };
    match flatc_to_json(flatc, &bfbs, name, bytes) {
        Some(json) => DecodedFrame {
            transport,
            topic,
            schema: Some(name.to_owned()),
            schema_source: Some(SchemaSource::Wire.as_str()),
            hash: None,
            encoding: Some(ENCODING_FLATBUFFERS),
            payload: FramePayload::Json { json },
            unsummarized: None,
        },
        None => hex(),
    }
}

/// Convert one FlatBuffers payload to JSON with
/// `flatc --json --raw-binary --strict-json -o <tmp> <schema.bfbs> -- <payload>`.
///
/// Schemas converted from `.proto` declare no `root_type`, so the root is
/// named with `--root-type`, trying the same package candidates as the
/// protobuf decoder ([`protobuf_name_candidates`]). `None` when no
/// candidate converts or anything about the run fails.
fn flatc_to_json(flatc: &Path, bfbs: &Path, name: &str, bytes: &[u8]) -> Option<Value> {
    let tmp = tempfile::tempdir().ok()?;
    let payload = tmp.path().join("payload.bin");
    std::fs::write(&payload, bytes).ok()?;
    let out = tmp.path().join("payload.json");
    protobuf_name_candidates(name).iter().find_map(|root| {
        let status = Command::new(flatc)
            .args(["--json", "--raw-binary", "--strict-json", "--root-type"])
            .arg(root)
            .arg("-o")
            .arg(tmp.path())
            .arg(bfbs)
            .arg("--")
            .arg(&payload)
            .output()
            .ok()?;
        if !status.status.success() {
            return None;
        }
        serde_json::from_slice(&std::fs::read(&out).ok()?).ok()
    })
}

/// Apply the `bytes` rendering rules to a serialized protobuf message, in
/// place: a field over [`BYTES_INLINE_MAX`] becomes a [`bytes_summary`];
/// under `no_data` the field is replaced by `<key>_len`. Recurses into
/// singular and repeated message fields. Keys absent from `json` (default
/// values the serializer skipped) are left alone.
fn shape_proto_bytes(msg: &DynamicMessage, json: &mut Value, no_data: bool) {
    let Value::Object(obj) = json else {
        return;
    };
    for field in msg.descriptor().fields() {
        let key = field.json_name();
        if !obj.contains_key(key) {
            continue;
        }
        let value = msg.get_field(&field);
        match (field.kind(), &*value) {
            (Kind::Bytes, ProtoValue::Bytes(data)) => {
                if no_data {
                    obj.remove(key);
                    obj.insert(format!("{key}_len"), Value::from(data.len()));
                } else if data.len() > BYTES_INLINE_MAX {
                    obj.insert(key.to_owned(), bytes_summary(data.len(), data));
                }
            }
            (Kind::Bytes, ProtoValue::List(items)) => {
                if no_data {
                    obj.remove(key);
                    let lens = items
                        .iter()
                        .map(|item| Value::from(item.as_bytes().map_or(0, |b| b.len())))
                        .collect();
                    obj.insert(format!("{key}_len"), Value::Array(lens));
                } else if let Some(Value::Array(slots)) = obj.get_mut(key) {
                    for (slot, item) in slots.iter_mut().zip(items) {
                        if let Some(data) = item.as_bytes().filter(|b| b.len() > BYTES_INLINE_MAX) {
                            *slot = bytes_summary(data.len(), data);
                        }
                    }
                }
            }
            (Kind::Message(_), ProtoValue::Message(child)) => {
                if let Some(slot) = obj.get_mut(key) {
                    shape_proto_bytes(child, slot, no_data);
                }
            }
            (Kind::Message(_), ProtoValue::List(items)) => {
                if let Some(Value::Array(slots)) = obj.get_mut(key) {
                    for (slot, item) in slots.iter_mut().zip(items) {
                        if let ProtoValue::Message(child) = item {
                            shape_proto_bytes(child, slot, no_data);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Summary object for a `bytes` value of `len` bytes too long to inline.
/// `head` holds its leading bytes; at most [`BYTES_HEAD_LEN`] are shown.
pub(crate) fn bytes_summary(len: usize, head: &[u8]) -> Value {
    serde_json::json!({
        "len": len,
        "head_hex": hex_string(&head[..head.len().min(BYTES_HEAD_LEN)]),
    })
}

/// Candidate protobuf full names for a vault message name, most qualified
/// first. The caller takes the first that resolves in the pool.
///
/// A vault name is `<style>/<namespace>/<Leaf>` (older publishers emit the
/// 2-segment `<namespace>/<Leaf>`). Two things make a single mechanical
/// mapping impossible, and getting either wrong sends every frame to the
/// hex fallback:
///
///   - **The style is not part of the protobuf package.** `ros2/std/Header`
///     lives in package `swarmbotix.std`, so the leading segment is dropped.
///     Taking the last two segments handles the 2- and 3-segment forms alike.
///   - **Two package conventions ship.** `std/` and the whole `swarmbotix`
///     style use `swarmbotix.<ns>`; the ROS2 mirror uses a bare `<ns>`
///     (`sensor_msgs`, `geometry_msgs`, … — 130 of the 138 ros2 protos).
///
/// Known limit: the package encodes no style, so two styles defining the
/// same `<ns>/<Leaf>` are indistinguishable here and the pool's entry wins.
fn protobuf_name_candidates(schema: &str) -> Vec<String> {
    let parts: Vec<&str> = schema.split('/').filter(|s| !s.is_empty()).collect();
    match parts.as_slice() {
        [.., ns, leaf] => vec![format!("swarmbotix.{ns}.{leaf}"), format!("{ns}.{leaf}")],
        [leaf] => vec![(*leaf).to_string()],
        [] => vec![schema.to_string()],
    }
}

/// A hex-body frame. `schema_source` follows `schema`: both or neither.
fn hex_frame<'a>(
    transport: &'a str,
    topic: &'a str,
    schema: Option<String>,
    source: Option<SchemaSource>,
    encoding: Option<&'static str>,
    bytes: &[u8],
) -> DecodedFrame<'a> {
    DecodedFrame {
        transport,
        topic,
        schema_source: schema.as_ref().and(source).map(SchemaSource::as_str),
        schema,
        hash: None,
        encoding,
        payload: FramePayload::Hex {
            bytes: hex_string(bytes),
        },
        unsummarized: None,
    }
}

/// One JSON line, no trailing newline. The caller writes its own `\n`
/// so the output stream stays platform-consistent.
pub fn render_decoded_json_line(frame: &DecodedFrame<'_>) -> String {
    serde_json::to_string(frame).unwrap_or_else(|_| "{}".into())
}

pub(crate) fn hex_string(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;
    use sb_discover::{HeaderProbe, MetadataProbe, StampedProbe};

    fn synthetic_stamped(msg_type: &str, rate: f64) -> Vec<u8> {
        let probe = StampedProbe {
            header: Some(HeaderProbe {
                timestamp_ns: 0,
                frame_id: String::new(),
                ros2_metadata: Some(MetadataProbe {
                    topic_full_name: String::new(),
                    msg_type: msg_type.into(),
                    msg_freq_desired: rate,
                }),
                ..Default::default()
            }),
        };
        let mut buf = Vec::new();
        probe.encode(&mut buf).unwrap();
        buf
    }

    #[test]
    fn header_probe_populates_schema() {
        let bytes = synthetic_stamped("std/StringStamped", 5.0);
        let frame = decode_frame("zenoh", "x", &bytes, false);
        assert_eq!(frame.schema.as_deref(), Some("std/StringStamped"));
        assert_eq!(frame.encoding, Some("protobuf"));
    }

    /// `--type` beats the transport's schema, which beats the Header probe,
    /// which beats the module config, and the winner is named in the
    /// rendered `schema_source` column.
    #[test]
    fn schema_precedence_is_flag_then_wire_then_header_then_config() {
        let ctx = DecodeContext {
            config_schema: Some("ros2/custom/CubePoses"),
            ..DecodeContext::default()
        };
        let header_only = Frame::from(synthetic_stamped("ros2/std/StringStamped", 5.0));
        let with_wire = Frame {
            wire_schema: Some("ros2/std/String".into()),
            ..header_only.clone()
        };

        let flag = decode_frame_dynamic(
            "zenoh",
            "x",
            &with_wire,
            false,
            Some("ros2/std/Vector3"),
            &ctx,
        );
        assert_eq!(flag.schema.as_deref(), Some("ros2/std/Vector3"));
        assert_eq!(flag.schema_source, Some("flag"));
        assert!(render_decoded_json_line(&flag).contains("\"schema_source\":\"flag\""));

        let wire = decode_frame_dynamic("zenoh", "x", &with_wire, false, None, &ctx);
        assert_eq!(wire.schema.as_deref(), Some("ros2/std/String"));
        assert_eq!(wire.schema_source, Some("wire"));

        let header = decode_frame_dynamic("zenoh", "x", &header_only, false, None, &ctx);
        assert_eq!(header.schema.as_deref(), Some("ros2/std/StringStamped"));
        assert_eq!(header.schema_source, Some("header"));

        // No flag, no wire schema, no Header: the module config names it.
        let untagged = Frame::from(b"foo".to_vec());
        let config = decode_frame_dynamic("zenoh", "x", &untagged, false, None, &ctx);
        assert_eq!(config.schema.as_deref(), Some("ros2/custom/CubePoses"));
        assert_eq!(config.schema_source, Some("config"));
        assert!(render_decoded_json_line(&config).contains("\"schema_source\":\"config\""));

        let none = decode_frame_dynamic(
            "zenoh",
            "x",
            &untagged,
            false,
            None,
            &DecodeContext::default(),
        );
        assert_eq!((none.schema, none.schema_source), (None, None));
    }

    /// The style segment is dropped: it is part of the vault identity but
    /// never of the protobuf package. Before this, a 3-segment name produced
    /// `swarmbotix.ros2.std/StringStamped`, which resolves in no pool — so
    /// `sb topic listen` fell back to hex for every sb-generated frame.
    #[test]
    fn fully_qualified_name_drops_the_style_segment() {
        let c = protobuf_name_candidates("ros2/std/StringStamped");
        assert!(
            c.contains(&"swarmbotix.std.StringStamped".to_string()),
            "style must be dropped, got {c:?}"
        );
        assert!(
            c.iter().all(|n| !n.contains('/')),
            "no candidate may keep a path separator: {c:?}"
        );
    }

    /// The 2-segment form older publishers emit maps to the same candidates.
    #[test]
    fn legacy_two_segment_name_maps_the_same_way() {
        assert_eq!(
            protobuf_name_candidates("std/StringStamped"),
            protobuf_name_candidates("ros2/std/StringStamped"),
        );
    }

    /// Two package conventions ship. `swarmbotix.<ns>` covers `std/` and the
    /// swarmbotix style; the ROS2 mirror uses a bare `<ns>`. Hard-coding the
    /// prefix broke 130 of the 138 ros2 messages, so both must be offered.
    #[test]
    fn both_package_conventions_are_offered() {
        let c = protobuf_name_candidates("ros2/nav_msgs/Path");
        assert!(
            c.contains(&"nav_msgs.Path".to_string()),
            "bare ROS2 package: {c:?}"
        );
        assert!(
            c.contains(&"swarmbotix.nav_msgs.Path".to_string()),
            "swarmbotix-prefixed package: {c:?}"
        );
    }
}
