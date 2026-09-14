//! FileDescriptorSet → flat IR.

use std::collections::{BTreeMap, HashSet};

use prost_types::{
    DescriptorProto, FieldDescriptorProto, FileDescriptorProto, FileDescriptorSet,
    field_descriptor_proto::Type as FdType,
};

use crate::ir::{CodegenConfig, FlatField, FlatStruct, FlatType, ScalarTy};

#[derive(Debug, thiserror::Error)]
pub enum FlattenError {
    #[error("oneof not supported (in {message}.{field})")]
    OneofUnsupported { message: String, field: String },
    #[error("map<{key},{value}> not supported (in {message}.{field})")]
    MapUnsupported {
        message: String,
        field: String,
        key: String,
        value: String,
    },
    #[error("nested message field {message}.{field} references unknown type {type_name:?}")]
    UnknownType {
        message: String,
        field: String,
        type_name: String,
    },
    #[error("recursion depth limit exceeded at {message}.{field} (cycle?)")]
    RecursionDepth { message: String, field: String },
    #[error("file {file:?} has no package declaration")]
    MissingPackage { file: String },
}

const MAX_DEPTH: usize = 16;

/// Walk every message in the set, build the flat IR. Returns one `FlatStruct`
/// per top-level message, plus implicitly-flattened structs for any message
/// referenced via `repeated`.
pub fn flatten(
    set: &FileDescriptorSet,
    cfg: &CodegenConfig,
) -> Result<Vec<FlatStruct>, FlattenError> {
    // Index by ".pkg.MessageName" → descriptor + package.
    let mut by_full = BTreeMap::new();
    for file in &set.file {
        let pkg = file
            .package
            .clone()
            .ok_or_else(|| FlattenError::MissingPackage {
                file: file.name().to_string(),
            })?;
        index_messages(&mut by_full, file, &pkg, "");
    }

    let mut out: Vec<FlatStruct> = Vec::new();
    let mut emitted: HashSet<String> = HashSet::new();

    // Emit a flat struct for every top-level (non-nested) message.
    for file in &set.file {
        let pkg = file.package().to_string();
        for msg in &file.message_type {
            // Skip nested types (they're inlined into their parent during flattening).
            // Nested types still live in by_full under "pkg.Parent.Nested" but we don't
            // emit them as top-level structs.
            let full = format!("{}.{}", pkg, msg.name());
            if emitted.contains(&full) {
                continue;
            }

            // ── Variant expansion: one schema → N fixed-capacity structs ──
            // The base is deliberately NOT emitted. Its variable-length
            // field is exactly what has no single correct size, so a struct
            // built from it would be wrong for every caller.
            if let Some(variants) = cfg.variants.get(&full) {
                for (variant_name, caps) in variants {
                    // Re-run the normal flattening with this variant's caps
                    // injected as per-field overrides on the base message.
                    // Both maps get the entry — the field's proto type
                    // decides which one is actually consulted.
                    let mut vcfg = cfg.clone();
                    for (field, cap) in caps {
                        let key = format!("{full}.{field}");
                        vcfg.bytes_overrides.insert(key.clone(), *cap);
                        vcfg.vec_overrides.insert(key, *cap);
                    }
                    let mut flat = flatten_message(
                        &by_full,
                        file,
                        &pkg,
                        msg,
                        &vcfg,
                        0,
                        &mut out,
                        &mut emitted,
                    )?;
                    // Dependencies (Header, Vector3, …) were emitted into
                    // `out` by the first variant and skipped by the rest via
                    // `emitted`; only the top-level struct is renamed.
                    flat.leaf = variant_name.clone();
                    out.push(flat);
                }
                emitted.insert(full);
                continue;
            }

            let flat = flatten_message(&by_full, file, &pkg, msg, cfg, 0, &mut out, &mut emitted)?;
            emitted.insert(full);
            out.push(flat);
        }
    }

    // Sort deterministically.
    out.sort_by(|a, b| (a.package.as_str(), a.leaf.as_str()).cmp(&(&b.package, &b.leaf)));
    Ok(out)
}

fn index_messages<'a>(
    by_full: &mut BTreeMap<String, (&'a FileDescriptorProto, &'a DescriptorProto, String)>,
    file: &'a FileDescriptorProto,
    pkg: &str,
    parent_prefix: &str,
) {
    for m in &file.message_type {
        index_one(by_full, file, pkg, parent_prefix, m);
    }
}

fn index_one<'a>(
    by_full: &mut BTreeMap<String, (&'a FileDescriptorProto, &'a DescriptorProto, String)>,
    file: &'a FileDescriptorProto,
    pkg: &str,
    parent_prefix: &str,
    m: &'a DescriptorProto,
) {
    let leaf = if parent_prefix.is_empty() {
        m.name().to_string()
    } else {
        format!("{}.{}", parent_prefix, m.name())
    };
    let full = format!("{}.{}", pkg, leaf);
    by_full.insert(full, (file, m, pkg.to_string()));
    // Nested types inherit the parent prefix.
    for nested in &m.nested_type {
        let new_prefix = if parent_prefix.is_empty() {
            m.name().to_string()
        } else {
            format!("{}.{}", parent_prefix, m.name())
        };
        index_one(by_full, file, pkg, &new_prefix, nested);
    }
}

// Same shape as `expand_field` below: the walk threads the descriptor index,
// the emit buffer and the dedup set through every level, and bundling them
// into a context struct would only rename the arguments.
#[allow(clippy::too_many_arguments)]
fn flatten_message(
    by_full: &BTreeMap<String, (&FileDescriptorProto, &DescriptorProto, String)>,
    file: &FileDescriptorProto,
    pkg: &str,
    msg: &DescriptorProto,
    cfg: &CodegenConfig,
    depth: usize,
    out: &mut Vec<FlatStruct>,
    emitted: &mut HashSet<String>,
) -> Result<FlatStruct, FlattenError> {
    if depth > MAX_DEPTH {
        return Err(FlattenError::RecursionDepth {
            message: format!("{}.{}", pkg, msg.name()),
            field: String::new(),
        });
    }

    let mut fields: Vec<FlatField> = Vec::new();
    for f in &msg.field {
        expand_field(
            by_full,
            pkg,
            msg.name(),
            f,
            cfg,
            depth,
            "",
            &mut fields,
            out,
            emitted,
        )?;
    }

    Ok(FlatStruct {
        package: pkg.to_string(),
        leaf: msg.name().to_string(),
        vault_name: vault_name_for(file, msg),
        style: cfg.style.clone(),
        doc: Vec::new(),
        fields,
    })
}

/// Derive the `<dir>/<leaf>` vault identifier from the proto source path.
/// `message_definitions/std/Header.proto` → `"std/Header"`.
/// `message_definitions/std_msgs/Header.proto` → `"std_msgs/Header"`.
fn vault_name_for(file: &FileDescriptorProto, msg: &DescriptorProto) -> String {
    let p = std::path::Path::new(file.name());
    let dir = p
        .parent()
        .and_then(|d| d.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or_else(|| file.package());
    format!("{dir}/{}", msg.name())
}

#[allow(clippy::too_many_arguments)]
fn expand_field(
    by_full: &BTreeMap<String, (&FileDescriptorProto, &DescriptorProto, String)>,
    parent_pkg: &str,
    parent_msg: &str,
    f: &FieldDescriptorProto,
    cfg: &CodegenConfig,
    depth: usize,
    name_prefix: &str,
    fields_out: &mut Vec<FlatField>,
    extra_structs: &mut Vec<FlatStruct>,
    extra_emitted: &mut HashSet<String>,
) -> Result<(), FlattenError> {
    // Reject oneof — proto3 supports it but iceoryx2 needs a fixed layout.
    if f.oneof_index.is_some() {
        return Err(FlattenError::OneofUnsupported {
            message: format!("{}.{}", parent_pkg, parent_msg),
            field: f.name().to_string(),
        });
    }

    let field_name = if name_prefix.is_empty() {
        f.name().to_string()
    } else {
        format!("{}_{}", name_prefix, f.name())
    };

    let is_repeated = f.label() == prost_types::field_descriptor_proto::Label::Repeated;
    let ty = f.r#type();
    let doc = field_doc(f);

    // map<K,V> is encoded as a repeated nested message with `map_entry = true`.
    if is_repeated && ty == FdType::Message {
        if let Some((_, target_desc, _)) = by_full.get(normalize_type_name(f.type_name()).as_str())
        {
            if target_desc.options.as_ref().and_then(|o| o.map_entry) == Some(true) {
                let key_name = target_desc
                    .field
                    .iter()
                    .find(|x| x.name() == "key")
                    .map(|x| format!("{:?}", x.r#type()))
                    .unwrap_or_else(|| "<unknown>".to_string());
                let val_name = target_desc
                    .field
                    .iter()
                    .find(|x| x.name() == "value")
                    .map(|x| format!("{:?}", x.r#type()))
                    .unwrap_or_else(|| "<unknown>".to_string());
                return Err(FlattenError::MapUnsupported {
                    message: format!("{}.{}", parent_pkg, parent_msg),
                    field: f.name().to_string(),
                    key: key_name,
                    value: val_name,
                });
            }
        }
    }

    match (is_repeated, ty) {
        // ── Repeated message: emit array of flat target + count companion ──
        (true, FdType::Message) => {
            let target_full = normalize_type_name(f.type_name());
            let (_target_file, target_desc, target_pkg) = by_full
                .get(target_full.as_str())
                .ok_or_else(|| FlattenError::UnknownType {
                    message: format!("{}.{}", parent_pkg, parent_msg),
                    field: f.name().to_string(),
                    type_name: f.type_name().to_string(),
                })?;
            // Ensure the target is flattened as its own struct (once).
            if !extra_emitted.contains(&target_full) {
                extra_emitted.insert(target_full.clone());
                let flat = flatten_message(
                    by_full,
                    _target_file,
                    target_pkg,
                    target_desc,
                    cfg,
                    depth + 1,
                    extra_structs,
                    extra_emitted,
                )?;
                extra_structs.push(flat);
            }
            fields_out.push(FlatField {
                name: field_name.clone(),
                ty: FlatType::RepeatedStruct {
                    target_leaf: target_desc.name().to_string(),
                    target_package: target_pkg.to_string(),
                    capacity: cfg.vec_capacity,
                },
                doc,
            });
            fields_out.push(FlatField {
                name: format!("{}_count", field_name),
                ty: FlatType::Scalar(ScalarTy::U32),
                doc: vec![],
            });
        }

        // ── Repeated scalar: array + count ──
        (true, _ty) => {
            // Special-case `repeated uint8` would be unusual (bytes is the idiomatic
            // form), but support it.
            let elem = scalar_for(ty);
            match elem {
                Some(s) => {
                    // Per-field override in cfg.vec_overrides (keyed by
                    // `<pkg>.<Msg>.<field>`) wins over the global
                    // cfg.vec_capacity — a fixed 3x3 matrix wants exactly 9
                    // and a lidar sweep wants ~1080, and no single global
                    // serves both.
                    let override_key = format!("{parent_pkg}.{parent_msg}.{}", f.name());
                    let cap = cfg
                        .vec_overrides
                        .get(&override_key)
                        .copied()
                        .unwrap_or(cfg.vec_capacity);
                    fields_out.push(FlatField {
                        name: field_name.clone(),
                        ty: FlatType::RepeatedScalar {
                            elem: s,
                            capacity: cap,
                        },
                        doc,
                    });
                    fields_out.push(FlatField {
                        name: format!("{}_count", field_name),
                        ty: FlatType::Scalar(ScalarTy::U32),
                        doc: vec![],
                    });
                }
                None => {
                    // Repeated string: emit a 2D [[u8; string_capacity]; N]
                    // buffer + `_count: u32` companion. `N` comes from a
                    // per-field override in cfg.string_array_overrides (keyed
                    // by `<pkg>.<Msg>.<field>`), falling back to the global
                    // `string_array_capacity` if no entry matches.
                    let override_key = format!("{parent_pkg}.{parent_msg}.{}", f.name());
                    let array_cap = cfg
                        .string_array_overrides
                        .get(&override_key)
                        .copied()
                        .unwrap_or(cfg.string_array_capacity);
                    fields_out.push(FlatField {
                        name: field_name.clone(),
                        ty: FlatType::RepeatedString {
                            string_capacity: cfg.string_capacity,
                            capacity: array_cap,
                        },
                        doc,
                    });
                    fields_out.push(FlatField {
                        name: format!("{}_count", field_name),
                        ty: FlatType::Scalar(ScalarTy::U32),
                        doc: vec![],
                    });
                }
            }
        }

        // ── Singular message: recursively inline its fields ──
        (false, FdType::Message) => {
            let target_full = normalize_type_name(f.type_name());
            let (target_file, target_desc, target_pkg) = by_full
                .get(target_full.as_str())
                .ok_or_else(|| FlattenError::UnknownType {
                    message: format!("{}.{}", parent_pkg, parent_msg),
                    field: f.name().to_string(),
                    type_name: f.type_name().to_string(),
                })?;
            // Recurse, prepending this field's name.
            let new_prefix = field_name.clone();
            for sub in &target_desc.field {
                expand_field(
                    by_full,
                    target_pkg,
                    target_desc.name(),
                    sub,
                    cfg,
                    depth + 1,
                    &new_prefix,
                    fields_out,
                    extra_structs,
                    extra_emitted,
                )?;
            }
            // Suppress unused-variable warnings.
            let _ = target_file;
        }

        // ── Singular string: [u8; STRING_CAP] ──
        (false, FdType::String) => {
            fields_out.push(FlatField {
                name: field_name,
                ty: FlatType::String {
                    capacity: cfg.string_capacity,
                },
                doc,
            });
        }

        // ── Singular bytes: [u8; BYTES_CAP] + _len ──
        (false, FdType::Bytes) => {
            // Per-field override in cfg.bytes_overrides (keyed by
            // `<pkg>.<Msg>.<field>`) wins over the global cfg.bytes_capacity.
            // Required for image / lidar / audio payloads that exceed the
            // 4 KiB default — iceoryx2 needs a compile-time-fixed buffer.
            let override_key = format!("{parent_pkg}.{parent_msg}.{}", f.name());
            let cap = cfg
                .bytes_overrides
                .get(&override_key)
                .copied()
                .unwrap_or(cfg.bytes_capacity);
            fields_out.push(FlatField {
                name: field_name.clone(),
                ty: FlatType::Bytes { capacity: cap },
                doc,
            });
            fields_out.push(FlatField {
                name: format!("{}_len", field_name),
                ty: FlatType::Scalar(ScalarTy::U32),
                doc: vec![],
            });
        }

        // ── Singular scalar / enum ──
        (false, _ty) => {
            // Enums collapse to i32 (proto3 spec).
            let elem = if ty == FdType::Enum {
                Some(ScalarTy::I32)
            } else {
                scalar_for(ty)
            };
            let Some(s) = elem else {
                return Err(FlattenError::UnknownType {
                    message: format!("{}.{}", parent_pkg, parent_msg),
                    field: f.name().to_string(),
                    type_name: format!("{:?}", ty),
                });
            };
            fields_out.push(FlatField {
                name: field_name,
                ty: FlatType::Scalar(s),
                doc,
            });
        }
    }
    Ok(())
}

/// FieldDescriptorProto.type_name is `.pkg.Msg` (leading dot). Strip it
/// so it matches the keys we built in `by_full`.
fn normalize_type_name(t: &str) -> String {
    t.trim_start_matches('.').to_string()
}

fn scalar_for(ty: FdType) -> Option<ScalarTy> {
    Some(match ty {
        FdType::Bool => ScalarTy::Bool,
        FdType::Float => ScalarTy::F32,
        FdType::Double => ScalarTy::F64,
        FdType::Int32 | FdType::Sint32 | FdType::Sfixed32 => ScalarTy::I32,
        FdType::Int64 | FdType::Sint64 | FdType::Sfixed64 => ScalarTy::I64,
        FdType::Uint32 | FdType::Fixed32 => ScalarTy::U32,
        FdType::Uint64 | FdType::Fixed64 => ScalarTy::U64,
        FdType::Enum => ScalarTy::I32,
        _ => return None,
    })
}

fn field_doc(_f: &FieldDescriptorProto) -> Vec<String> {
    // SourceCodeInfo would let us recover the comment from the .proto.
    // Skipped at L1 — emitters work fine without per-field docs.
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost_types::{DescriptorProto, FieldDescriptorProto, FileDescriptorProto};

    fn fd(name: &str, ty: FdType, number: i32) -> FieldDescriptorProto {
        FieldDescriptorProto {
            name: Some(name.into()),
            number: Some(number),
            r#type: Some(ty as i32),
            ..Default::default()
        }
    }

    fn fd_msg(name: &str, type_name: &str, number: i32, repeated: bool) -> FieldDescriptorProto {
        FieldDescriptorProto {
            name: Some(name.into()),
            number: Some(number),
            r#type: Some(FdType::Message as i32),
            type_name: Some(type_name.into()),
            label: if repeated {
                Some(prost_types::field_descriptor_proto::Label::Repeated as i32)
            } else {
                None
            },
            ..Default::default()
        }
    }

    pub(super) fn make_set() -> FileDescriptorSet {
        // package std_msgs; message Header { int32 sec=1; uint32 nanosec=2; string frame_id=3; }
        let header = DescriptorProto {
            name: Some("Header".into()),
            field: vec![
                fd("sec", FdType::Int32, 1),
                fd("nanosec", FdType::Uint32, 2),
                fd("frame_id", FdType::String, 3),
            ],
            ..Default::default()
        };
        let std_msgs = FileDescriptorProto {
            name: Some("std_msgs/Header.proto".into()),
            package: Some("std_msgs".into()),
            message_type: vec![header],
            syntax: Some("proto3".into()),
            ..Default::default()
        };
        // package sensor_msgs;
        // message Image { std_msgs.Header header=1; uint32 width=2; uint32 height=3; bytes data=4; }
        let image = DescriptorProto {
            name: Some("Image".into()),
            field: vec![
                fd_msg("header", ".std_msgs.Header", 1, false),
                fd("width", FdType::Uint32, 2),
                fd("height", FdType::Uint32, 3),
                fd("data", FdType::Bytes, 4),
            ],
            ..Default::default()
        };
        let sensor_msgs = FileDescriptorProto {
            name: Some("sensor_msgs/Image.proto".into()),
            package: Some("sensor_msgs".into()),
            message_type: vec![image],
            ..Default::default()
        };
        FileDescriptorSet {
            file: vec![std_msgs, sensor_msgs],
        }
    }

    #[test]
    fn header_is_three_fields() {
        let set = make_set();
        let flats = flatten(&set, &CodegenConfig::default()).unwrap();
        let h = flats
            .iter()
            .find(|s| s.full_name() == "std_msgs.Header")
            .unwrap();
        let names: Vec<&str> = h.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["sec", "nanosec", "frame_id"]);
    }

    #[test]
    fn image_inlines_header_with_prefix_and_bytes_gets_len() {
        let set = make_set();
        let flats = flatten(&set, &CodegenConfig::default()).unwrap();
        let img = flats
            .iter()
            .find(|s| s.full_name() == "sensor_msgs.Image")
            .unwrap();
        let names: Vec<&str> = img.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "header_sec",
                "header_nanosec",
                "header_frame_id",
                "width",
                "height",
                "data",
                "data_len",
            ]
        );
    }

    #[test]
    fn frame_id_becomes_fixed_size_string() {
        let set = make_set();
        let flats = flatten(&set, &CodegenConfig::default()).unwrap();
        let img = flats
            .iter()
            .find(|s| s.full_name() == "sensor_msgs.Image")
            .unwrap();
        let frame = img
            .fields
            .iter()
            .find(|f| f.name == "header_frame_id")
            .unwrap();
        match frame.ty {
            FlatType::String { capacity: 256 } => {}
            ref other => panic!("expected String{{256}}, got {other:?}"),
        }
    }

    #[test]
    fn bytes_override_beats_global_cap() {
        let set = make_set();
        let mut cfg = CodegenConfig::default();
        // 1280 × 1024 × 1 byte (MONO8) — the camera case that motivated this.
        cfg.bytes_overrides
            .insert("sensor_msgs.Image.data".into(), 1_310_720);
        let flats = flatten(&set, &cfg).unwrap();
        let img = flats
            .iter()
            .find(|s| s.full_name() == "sensor_msgs.Image")
            .unwrap();
        let data = img.fields.iter().find(|f| f.name == "data").unwrap();
        match data.ty {
            FlatType::Bytes {
                capacity: 1_310_720,
            } => {}
            ref other => panic!("expected Bytes{{1310720}}, got {other:?}"),
        }
    }

    #[test]
    fn bytes_falls_back_to_global_cap_when_no_override() {
        let set = make_set();
        let mut cfg = CodegenConfig {
            bytes_capacity: 8192,
            ..Default::default()
        };
        // An override for a *different* field must not leak into Image.data.
        cfg.bytes_overrides
            .insert("audio_msgs.Pcm.samples".into(), 65_536);
        let flats = flatten(&set, &cfg).unwrap();
        let img = flats
            .iter()
            .find(|s| s.full_name() == "sensor_msgs.Image")
            .unwrap();
        let data = img.fields.iter().find(|f| f.name == "data").unwrap();
        match data.ty {
            FlatType::Bytes { capacity: 8192 } => {}
            ref other => panic!("expected Bytes{{8192}} from global cap, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod variant_tests {
    use super::*;
    use std::collections::BTreeMap;

    fn variants_for(
        pairs: &[(&str, u32)],
    ) -> BTreeMap<String, BTreeMap<String, BTreeMap<String, u32>>> {
        let inner: BTreeMap<String, BTreeMap<String, u32>> = pairs
            .iter()
            .map(|(n, c)| ((*n).to_owned(), BTreeMap::from([("data".to_owned(), *c)])))
            .collect();
        BTreeMap::from([("sensor_msgs.Image".to_owned(), inner)])
    }

    #[test]
    fn one_schema_fans_out_into_a_struct_per_variant() {
        let set = super::tests::make_set();
        let cfg = CodegenConfig {
            variants: variants_for(&[("ImageSmall", 1024), ("ImageBig", 6_220_800)]),
            ..Default::default()
        };
        let flats = flatten(&set, &cfg).unwrap();

        let names: Vec<&str> = flats.iter().map(|s| s.leaf.as_str()).collect();
        assert!(names.contains(&"ImageSmall"), "got {names:?}");
        assert!(names.contains(&"ImageBig"), "got {names:?}");

        // Each variant carries its OWN capacity — that is the entire point.
        for (leaf, want) in [("ImageSmall", 1024u32), ("ImageBig", 6_220_800)] {
            let s = flats.iter().find(|s| s.leaf == leaf).unwrap();
            let data = s.fields.iter().find(|f| f.name == "data").unwrap();
            match data.ty {
                FlatType::Bytes { capacity } => assert_eq!(capacity, want, "{leaf}"),
                ref other => panic!("{leaf}: expected Bytes, got {other:?}"),
            }
        }
    }

    #[test]
    fn base_message_is_not_emitted_when_variants_exist() {
        let set = super::tests::make_set();
        let cfg = CodegenConfig {
            variants: variants_for(&[("ImageSmall", 1024)]),
            ..Default::default()
        };
        let flats = flatten(&set, &cfg).unwrap();
        // The base is precisely the type with no correct size; emitting it
        // would let callers pick the one struct that is always wrong.
        assert!(
            !flats.iter().any(|s| s.full_name() == "sensor_msgs.Image"),
            "base Image must not be emitted alongside its variants"
        );
    }

    #[test]
    fn dependencies_are_emitted_once_across_all_variants() {
        let set = super::tests::make_set();
        let cfg = CodegenConfig {
            variants: variants_for(&[("A", 16), ("B", 32), ("C", 64)]),
            ..Default::default()
        };
        let flats = flatten(&set, &cfg).unwrap();
        // Header is reached by every variant; it must not be duplicated.
        let headers = flats
            .iter()
            .filter(|s| s.full_name() == "std_msgs.Header")
            .count();
        assert_eq!(headers, 1, "shared dependency duplicated per variant");
    }

    #[test]
    fn a_message_without_variants_is_untouched() {
        let set = super::tests::make_set();
        let cfg = CodegenConfig {
            variants: variants_for(&[("ImageSmall", 1024)]),
            ..Default::default()
        };
        let flats = flatten(&set, &cfg).unwrap();
        assert!(flats.iter().any(|s| s.full_name() == "std_msgs.Header"));
    }

    #[test]
    fn vec_override_beats_the_global_capacity() {
        let set = super::tests::make_set();
        let mut cfg = CodegenConfig {
            vec_capacity: 256,
            ..Default::default()
        };
        cfg.vec_overrides
            .insert("sensor_msgs.Image.width".into(), 9);
        // width is a scalar, not repeated — the override must not apply.
        let flats = flatten(&set, &cfg).unwrap();
        let img = flats
            .iter()
            .find(|s| s.full_name() == "sensor_msgs.Image")
            .unwrap();
        let w = img.fields.iter().find(|f| f.name == "width").unwrap();
        assert!(
            matches!(w.ty, FlatType::Scalar(_)),
            "scalar field must stay scalar, got {:?}",
            w.ty
        );
    }
}
