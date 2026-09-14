//! Language-agnostic flattened IR. Emitters consume this.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodegenConfig {
    /// Capacity for `string` fields (bytes, null-terminated).
    pub string_capacity: u32,
    /// Fallback capacity for `bytes` fields. Used only when no entry in
    /// [`Self::bytes_overrides`] matches a given field. A `<field>_len: u32`
    /// companion is emitted alongside the array.
    pub bytes_capacity: u32,
    /// Per-field overrides for `bytes` field capacity, keyed by
    /// `"<proto_pkg>.<MsgName>.<field>"` (e.g. `sensor_msgs.Image1280x1024.data`).
    /// An entry here wins over [`Self::bytes_capacity`]. Required for messages
    /// whose payload exceeds the 4 KiB default (image, lidar, audio, etc.),
    /// since iceoryx2 needs a compile-time-fixed buffer.
    pub bytes_overrides: std::collections::BTreeMap<String, u32>,
    /// Fallback capacity for `repeated T`. A `<field>_count: u32` companion
    /// is emitted. Used only when no entry in [`Self::vec_overrides`] matches.
    pub vec_capacity: u32,
    /// Per-field overrides for `repeated T` (scalar / message element)
    /// capacity, keyed by `"<proto_pkg>.<MsgName>.<field>"`. Wins over
    /// [`Self::vec_capacity`].
    ///
    /// The global fallback is one number for every repeated field in the
    /// vault, which is simultaneously far too big for a fixed 3x3 matrix
    /// and far too small for a lidar sweep. This is the same escape hatch
    /// [`Self::bytes_overrides`] provides for `bytes`.
    pub vec_overrides: std::collections::BTreeMap<String, u32>,
    /// Style being compiled. Stamped onto every [`FlatStruct`] so the
    /// emitted identity carries it.
    pub style: String,
    /// Fallback array length for `repeated string` fields. Each string still
    /// consumes [`Self::string_capacity`] bytes. Used only when no entry in
    /// [`Self::string_array_overrides`] matches a given field.
    pub string_array_capacity: u32,
    /// Per-field overrides for `repeated string` array length, keyed by
    /// `"<proto_pkg>.<MsgName>.<field>"` (matches the format the legacy skip
    /// warning used). An entry here wins over [`Self::string_array_capacity`].
    pub string_array_overrides: std::collections::BTreeMap<String, u32>,
    /// Expand ONE message into SEVERAL fixed-capacity structs.
    ///
    /// Keyed by the message's fully-qualified proto name (`"<pkg>.<Msg>"`).
    /// The value maps each variant's **output type name** to its per-field
    /// capacity overrides (`field -> capacity`).
    ///
    /// This exists because a variable-length payload has no single correct
    /// iox2 size. `images/Image` is one schema, but an image is 230 KB at
    /// 360p mono8 and 6 MB at 1080p rgb8 — sizing one struct for the worst
    /// case wastes shared memory on every small frame.
    ///
    /// When a message has variants the **base struct is not emitted**: the
    /// base is precisely the thing with no right size. Referencing it in
    /// generated code is always a mistake, so it is made impossible.
    pub variants: std::collections::BTreeMap<
        String,
        std::collections::BTreeMap<String, std::collections::BTreeMap<String, u32>>,
    >,
}

impl Default for CodegenConfig {
    fn default() -> Self {
        Self {
            string_capacity: 256,
            bytes_capacity: 4096,
            bytes_overrides: std::collections::BTreeMap::new(),
            vec_capacity: 256,
            vec_overrides: std::collections::BTreeMap::new(),
            style: String::new(),
            string_array_capacity: 10,
            string_array_overrides: std::collections::BTreeMap::new(),
            variants: std::collections::BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Rust,
    Cpp,
    Python,
    CSharp,
}

impl Lang {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Rust => "rs",
            Self::Cpp => "h",
            Self::Python => "py",
            Self::CSharp => "cs",
        }
    }
}

/// Scalar (non-message, non-string, non-bytes) primitive types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarTy {
    Bool,
    I32,
    I64,
    U32,
    U64,
    F32,
    F64,
}

/// Flat IR for a single field after recursive expansion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlatType {
    Scalar(ScalarTy),
    /// `[u8; capacity]` — null-terminated UTF-8.
    String {
        capacity: u32,
    },
    /// `[u8; capacity]` blob; pair with a `<name>_len: u32` field.
    Bytes {
        capacity: u32,
    },
    /// `[T; capacity]` array of scalars; pair with a `<name>_count: u32` field.
    RepeatedScalar {
        elem: ScalarTy,
        capacity: u32,
    },
    /// `[Struct; capacity]` array of (already flattened) structs; pair with `<name>_count: u32`.
    RepeatedStruct {
        /// Leaf name of the flat target struct (within the same emitted module).
        target_leaf: String,
        /// Source package of the target — used by emitters to compute import paths.
        target_package: String,
        capacity: u32,
    },
    /// `[[u8; string_capacity]; capacity]` — `repeated string` flattened to a
    /// fixed 2D byte buffer. Pair with a `<name>_count: u32` field for the
    /// number of strings actually populated.
    RepeatedString {
        string_capacity: u32,
        capacity: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlatField {
    pub name: String,
    pub ty: FlatType,
    /// Optional human-readable doc lines (no language prefix; emitter adds).
    pub doc: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlatStruct {
    /// Proto-declared package (may differ from the on-disk directory),
    /// e.g. `"swarmbotix.std"` for `message_definitions/std/Header.proto`.
    pub package: String,
    /// E.g. `"Header"`.
    pub leaf: String,
    /// On-disk path inside the vault, e.g. `"std/Header"` (no `.proto`).
    /// This is what `sb message list` shows; CLI filters match against this.
    pub vault_name: String,
    /// Style this message belongs to (`"ros2"`, `"swarmbotix"`, …). Kept
    /// separate from [`Self::vault_name`] because the two answer different
    /// questions: `vault_name` is the *layout* under a style's target root
    /// (`iox2/<ns>/<Leaf>/`), while style + vault_name together are the
    /// *identity* that goes on the wire.
    pub style: String,
    pub doc: Vec<String>,
    pub fields: Vec<FlatField>,
}

impl FlatStruct {
    /// Path-like full name `"std_msgs.Header"` — used as a map key during flattening.
    pub fn full_name(&self) -> String {
        format!("{}.{}", self.package, self.leaf)
    }

    /// Fully-qualified name: `<style>/<namespace>/<Leaf>`. This is what a
    /// user types and what `Header.metadata.msg_type` carries — the style
    /// segment is what makes it unambiguous between two styles that both
    /// define, say, `images/Image`.
    pub fn qualified_name(&self) -> String {
        format!("{}/{}", self.style, self.vault_name)
    }

    /// Directory component of [`Self::vault_name`] — what emitters use as the
    /// output subdirectory. `"std/Header"` → `"std"`. Style-relative: the
    /// targets root is already per-style.
    pub fn vault_dir(&self) -> &str {
        self.vault_name
            .split_once('/')
            .map(|(d, _)| d)
            .unwrap_or(self.vault_name.as_str())
    }
}

/// A single language emission for a single flat struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedModule {
    /// Relative target path inside the user-supplied --out dir.
    /// e.g. `std_msgs/Header.rs`.
    pub rel_path: std::path::PathBuf,
    pub contents: String,
}
