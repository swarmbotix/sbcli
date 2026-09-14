//! Iceoryx2-compatible code generator.
//!
//! Takes a `prost_types::FileDescriptorSet` and produces per-language
//! data definitions where every struct is FLAT (no nested message
//! fields) and every variable-length field has a fixed-size storage +
//! companion length/count field. This is what iceoryx2 needs for
//! zero-copy shared-memory IPC.
//!
//! Flattening rules:
//!
//! - Primitive fields: kept as-is, mapped to per-language types.
//! - `string`:  stored as `[u8; STRING_CAP]`, null-terminated.
//! - `bytes`:   stored as `[u8; BYTES_CAP]` + companion `<field>_len: u32`.
//! - `repeated <scalar>`: `[T; VEC_CAP]` + companion `<field>_count: u32`.
//! - Singular message field: fields are inlined into the parent with
//!   `<field>_<child>` naming (recursive).
//! - `repeated <message>`: the target message is also flattened into its
//!   own struct; field stored as `[FlatChild; VEC_CAP]` + `<field>_count: u32`.
//! - Enums: treated as `i32`.
//! - `oneof` / `map<K,V>`: not supported; flattening errors.
//!
//! Caps are configurable via [`CodegenConfig`].

pub mod cpp;
pub mod csharp;
pub mod flatten;
pub mod ir;
pub mod python;
pub mod rust;

pub use flatten::{FlattenError, flatten};
pub use ir::{CodegenConfig, FlatField, FlatStruct, FlatType, GeneratedModule, Lang, ScalarTy};
