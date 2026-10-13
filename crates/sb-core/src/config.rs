//! Serde types — one Rust struct per on-disk YAML file.
//!
//! File ↔ type ↔ owner mapping:
//!
//! | File             | Type              | Owner               |
//! | ---------------- | ----------------- | ------------------- |
//! | `sb.config.yml`  | `SbCliConfig`     | the CLI itself      |
//! | `sb.dev.yml`     | `ModuleDevConfig` | per-module, dev     |
//! | `sb.prd.yml`     | `ModulePrdConfig` | per-module, prod    |
//! | `flow.yaml`      | `WorkspaceFlow`   | workspace registry  |
//!
//! Every field is `Option<T>` where it can legitimately be absent in a
//! given layer; `SbCliConfig::merge` then resolves the layering.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::ident::{IdentError, validate_identifier};
use crate::style::Backend;
use crate::topic::{InstanceIdError, validate_instance_id};
use crate::transport::Transport;

// ─────────────────────────────────────────────────────────────────────
// Language — `sb.dev.yml`'s `language:` tag
// ─────────────────────────────────────────────────────────────────────

/// The language of the user's module. Picked at `sb init` time via a
/// `--<lang>` flag, written to `sb.dev.yml`, and consulted by L3 codegen
/// to choose file extensions / runtime APIs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Rust,
    Python,
    Cpp,
    Flutter,
    Unity,
}

impl Language {
    pub fn as_str(self) -> &'static str {
        match self {
            Language::Rust => "rust",
            Language::Python => "python",
            Language::Cpp => "cpp",
            Language::Flutter => "flutter",
            Language::Unity => "unity",
        }
    }

    /// Default `io_dir` for this language. Unity nests under
    /// `Assets/Scripts/` because Unity scripts must live there; the
    /// other four use a top-level `swarmbotix_io/`.
    pub fn default_io_dir(self) -> PathBuf {
        match self {
            Language::Unity => PathBuf::from("Assets/Scripts/swarmbotix_io"),
            _ => PathBuf::from("swarmbotix_io"),
        }
    }
}

impl FromStr for Language {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "rust" => Ok(Language::Rust),
            "python" => Ok(Language::Python),
            "cpp" => Ok(Language::Cpp),
            "flutter" => Ok(Language::Flutter),
            "unity" => Ok(Language::Unity),
            other => Err(format!("unknown language: {other:?}")),
        }
    }
}

impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ─────────────────────────────────────────────────────────────────────
// SbCliConfig — `sb.config.yml`
// ─────────────────────────────────────────────────────────────────────

/// swarmbotix's own style — `header/`, `primitives/`, `images/`, `sensors/`.
pub const STYLE_SWARMBOTIX: &str = "swarmbotix";

/// The ROS2-mirror style — `std/` plus `std_msgs`, `geometry_msgs`,
/// `sensor_msgs`, … 138 `.proto` files. Owns the forge-bundled `std/*`.
pub const STYLE_ROS2: &str = "ros2";

/// Styles the forge ships. **Not** a closed set: any subdirectory of
/// `messages_root` with a `message_definitions/` child is a style, and a
/// message name names its own. This list exists only for `sb doctor` hints
/// and shell completion — nothing resolves through it.
pub const BUNDLED_MESSAGE_STYLES: &[&str] = &[STYLE_SWARMBOTIX, STYLE_ROS2];

/// Reject style names that would escape `messages_root` when joined as a
/// path segment. Keeps `../../etc` from resolving outside the tree.
pub fn is_valid_message_style(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && !s.contains('/')
        && !s.contains('\\')
        && !s.contains(':')
}

/// CLI's own settings. Lives at:
///   - `~/.swarmbotix/sb.config.yml` (global)
///   - `<workspace>/sb.config.yml`   (workspace override)
///   - any path pointed to by `$SB_CONFIG`
///
/// Layering is resolved by [`SbCliConfig::merge`]. Every field is optional
/// at a given layer so absence can fall through to the next layer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SbCliConfig {
    // Root of the per-user swarmbotix tree. Defaults to `~/.swarmbotix/`
    // when unset. Workspaces, the active marker, and per-workspace
    // overrides all live under this root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sb_home_dir: Option<PathBuf>,

    // Host-specific tool paths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protoc: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flatc: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub libzenohc: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub libiceoryx2: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmux: Option<PathBuf>,

    // Root of the message-style tree. Defaults to `<sb_home>/messages/`.
    // Each subdirectory is one style holding its own `message_definitions/`
    // and `message_targets/`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages_root: Option<PathBuf>,
    // ── DEPRECATED, parsed but IGNORED ──────────────────────────────
    // These three encoded an "active style": one global mode that every
    // message name was resolved against. That is not expressible — a
    // module publishes one style and subscribes to another, and
    // `Header.metadata.msg_type` travels to consumers that share no config
    // with the publisher. Message names are now fully qualified
    // (`<style>/<namespace>/<Leaf>`) and carry their own style, so there is
    // nothing left for these to select.
    //
    // They are retained ONLY so a pre-0.1.35 `sb.config.yml` still parses
    // — `deny_unknown_fields` would otherwise reject the whole file. They
    // are absent from `KNOWN_KEYS`, so `sb config show/get/set` no longer
    // offers them, and `sb doctor` reports them as ignored. The installer
    // strips them on upgrade.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_style: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_definitions: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_targets: Option<PathBuf>,

    // Defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport_on_device: Option<Transport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport_cross_device: Option<Transport>,

    // Codegen caps. `sb message compile` reads these when no `--*-cap` CLI
    // override is given. `string_array_cap` is the global fallback for how
    // many strings fit in a `repeated string` field's fixed iox2 layout;
    // `string_array_caps` provides per-field overrides keyed by
    // `<pkg>.<Msg>.<field>` (e.g. `sensor_msgs.JointState.name: 32`).
    //
    // `bytes_caps` is the same shape for singular `bytes` fields, keyed by
    // `<pkg>.<Msg>.<field>` (e.g. `sensor_msgs.Image1280x1024.data: 1310720`).
    // Required for camera / lidar / audio payloads that exceed the iox2-typegen
    // 4 KiB default — iceoryx2 needs a compile-time-fixed buffer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub string_array_cap: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub string_array_caps: Option<BTreeMap<String, u32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_caps: Option<BTreeMap<String, u32>>,

    // Per-field capacity for `repeated <scalar>` fields, keyed by
    // `<pkg>.<Msg>.<field>`. Without an entry every repeated field gets one
    // global number, which is simultaneously far too large for a fixed 3x3
    // matrix and far too small for a lidar sweep.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vec_caps: Option<BTreeMap<String, u32>>,

    // Expand ONE message into SEVERAL fixed-capacity iox2 types.
    //
    //   iox2_variants:
    //     <pkg>.<Msg>:
    //       <OutputTypeName>:
    //         <field>: <capacity_bytes_or_elements>
    //
    // A variable-length payload has no single correct iox2 size — an image
    // is 230 KB at 360p mono8 and 6 MB at 1080p rgb8. Rather than forcing
    // one .proto per resolution, the schema stays one file and the build
    // fans out. When a message has variants the base type is NOT emitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iox2_variants: Option<Iox2Variants>,
}

/// `<pkg>.<Msg>` → output type name → field name → capacity. Three levels of
/// map, named once here so the signatures that carry it stay readable.
pub type Iox2Variants = BTreeMap<String, BTreeMap<String, BTreeMap<String, u32>>>;

impl SbCliConfig {
    /// Built-in defaults — the layer of last resort.
    pub fn builtin_defaults() -> Self {
        // Per-field iox2 array-length defaults for `repeated string` fields.
        // Picked from realistic robotics usage; users override per field in
        // sb.config.yml. The fallback `string_array_cap` covers everything
        // else. Keys match the legacy skip-warning format `<pkg>.<Msg>.<field>`.
        let mut string_array_caps: BTreeMap<String, u32> = BTreeMap::new();
        string_array_caps.insert("sensor_msgs.JointState.name".into(), 32);
        string_array_caps.insert("sensor_msgs.MultiDOFJointState.joint_names".into(), 16);
        string_array_caps.insert("trajectory_msgs.JointTrajectory.joint_names".into(), 32);
        string_array_caps.insert(
            "trajectory_msgs.MultiDOFJointTrajectory.joint_names".into(),
            16,
        );
        string_array_caps.insert(
            "visualization_msgs.InteractiveMarkerUpdate.erases".into(),
            32,
        );

        // Per-field iox2 `bytes` capacity overrides. Picked from realistic
        // robotics payloads where the 4 KiB iox2-typegen default is wrong by
        // 2–3 orders of magnitude (camera frames, lidar scans, etc.). Users
        // add their own here per-field in sb.config.yml::bytes_caps.
        let mut bytes_caps: BTreeMap<String, u32> = BTreeMap::new();
        // 1280 × 1024 × 1 byte (MONO8) — HuaTeng GigE camera native frame.
        bytes_caps.insert("sensor_msgs.Image1280x1024.data".into(), 1_310_720);
        // ~131k XYZI float32 points at 16 B/point.
        bytes_caps.insert("swarmbotix.sensors.PointCloud.data".into(), 2_097_152);

        // Per-field `repeated <scalar>` capacities. Matrices are pinned to
        // their exact element count so the flattened struct carries no
        // slack; the lidar sweeps are sized at one sample per 1/3 degree
        // over a full turn, which covers every common 2D unit.
        let mut vec_caps: BTreeMap<String, u32> = BTreeMap::new();
        vec_caps.insert("swarmbotix.primitives.Mat33.data".into(), 9);
        vec_caps.insert("swarmbotix.primitives.Mat44.data".into(), 16);
        vec_caps.insert("swarmbotix.sensors.LaserScan.ranges".into(), 1080);
        vec_caps.insert("swarmbotix.sensors.LaserScan.intensities".into(), 1080);

        // ── images/Image → one fixed-capacity iox2 type per resolution ──
        // capacity = width × height × bytes_per_pixel. mono8 is 1 B/px,
        // rgb8 is 3. Adding a resolution is an entry here, never another
        // .proto — see messages/swarmbotix/message_definitions/images/Image.proto.
        // u32 throughout: the largest entry is 1920 × 1080 × 3 = 6_220_800,
        // three orders of magnitude below u32::MAX, so no cast is needed and
        // a new resolution that did overflow would fail to compile.
        let image_variants: BTreeMap<String, BTreeMap<String, u32>> = [
            ("Image360pMono8", 640u32 * 360),
            ("Image360pRgb8", 640 * 360 * 3),
            ("Image480pMono8", 640 * 480),
            ("Image480pRgb8", 640 * 480 * 3),
            ("Image720pMono8", 1280 * 720),
            ("Image720pRgb8", 1280 * 720 * 3),
            ("Image1080pMono8", 1920 * 1080),
            ("Image1080pRgb8", 1920 * 1080 * 3),
        ]
        .into_iter()
        .map(|(name, cap)| (name.to_owned(), BTreeMap::from([("data".to_owned(), cap)])))
        .collect();
        let mut iox2_variants: BTreeMap<String, BTreeMap<String, BTreeMap<String, u32>>> =
            BTreeMap::new();
        iox2_variants.insert("swarmbotix.images.Image".into(), image_variants);

        Self {
            sb_home_dir: None,
            protoc: None,
            flatc: None,
            libzenohc: None,
            libiceoryx2: None,
            tmux: None,
            messages_root: None,
            // Deprecated + ignored (see the struct). Never defaulted: a
            // value here would look like a live setting in `sb config show`.
            message_style: None,
            message_definitions: None,
            message_targets: None,
            device: Some("dev01".to_owned()),
            transport_on_device: Some(Transport::Iceoryx2),
            transport_cross_device: Some(Transport::Zenoh),
            string_array_cap: Some(10),
            string_array_caps: Some(string_array_caps),
            bytes_caps: Some(bytes_caps),
            vec_caps: Some(vec_caps),
            iox2_variants: Some(iox2_variants),
        }
    }

    /// Layer configs highest → lowest priority. Later layers fill missing
    /// fields from earlier ones; non-`None` fields in an earlier layer win.
    ///
    /// Pass in priority order: `env_override`, `workspace`, `global`. The
    /// built-in defaults are always the final fallback.
    pub fn merge(
        env_override: Option<Self>,
        workspace: Option<Self>,
        global: Option<Self>,
    ) -> Self {
        let mut out = Self::default();
        for layer in [
            env_override,
            workspace,
            global,
            Some(Self::builtin_defaults()),
        ]
        .into_iter()
        .flatten()
        {
            out.fill_from(&layer);
        }
        out
    }

    /// For each field in `self` that is `None`, copy from `other`.
    fn fill_from(&mut self, other: &Self) {
        macro_rules! fill {
            ($($field:ident),* $(,)?) => {
                $(if self.$field.is_none() { self.$field = other.$field.clone(); })*
            };
        }
        fill!(
            sb_home_dir,
            protoc,
            flatc,
            libzenohc,
            libiceoryx2,
            tmux,
            messages_root,
            message_style,
            message_definitions,
            message_targets,
            device,
            transport_on_device,
            transport_cross_device,
            string_array_cap,
            string_array_caps,
            bytes_caps,
            vec_caps,
            iox2_variants,
        );
    }
}

// ─────────────────────────────────────────────────────────────────────
// ModuleDevConfig — `sb.dev.yml`
// ─────────────────────────────────────────────────────────────────────

/// Per-module development-time interface.
///
/// `root` is optional: absent means "the directory holding this
/// `sb.dev.yml`", which keeps the file free of host-specific paths so it
/// can be committed. Resolve it with [`ModuleDevConfig::module_root`],
/// never by reading the field directly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleDevConfig {
    pub module: String,
    pub language: Language,
    /// Module root. Absent = the yaml's own directory; relative = relative
    /// to that directory; absolute = used as is (pre-0.5.0 files).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<PathBuf>,
    #[serde(default = "default_io_dir")]
    pub io_dir: PathBuf,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub publishers: Vec<PubSpec>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subscribers: Vec<SubSpec>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<ServiceSpec>,
}

fn default_io_dir() -> PathBuf {
    PathBuf::from("swarmbotix_io")
}

impl ModuleDevConfig {
    /// The module root for the `sb.dev.yml` at `dev_yml`.
    ///
    /// `root:` absent resolves to the yaml's directory, a relative `root:`
    /// is joined onto that directory, and an absolute one is returned
    /// unchanged. A bare `sb.dev.yml` (no directory part) resolves to `.`.
    pub fn module_root(&self, dev_yml: &Path) -> PathBuf {
        let dir = match dev_yml.parent() {
            Some(p) if !p.as_os_str().is_empty() => p,
            _ => Path::new("."),
        };
        match &self.root {
            // `join` with an absolute path replaces, which is the
            // "absolute is used as is" rule.
            Some(root) => dir.join(root),
            None => dir.to_path_buf(),
        }
    }

    /// Which entry, if any, is called `name`. Publishers, subscribers and
    /// services share one namespace; the first match wins in that order.
    pub fn find_name(&self, name: &str) -> Option<EntryKind> {
        find_entry_name(&self.publishers, &self.subscribers, &self.services, name)
    }

    /// Re-run the per-entry load-time rules (`topic:`/`key:`, `type:`,
    /// `encoding:`, `mime:`) on a config built in code. The error names the
    /// entry the way a load error would: `publishers[0]: ...`.
    pub fn validate(&self) -> Result<(), String> {
        validate_entries(&self.publishers, &self.subscribers)
    }
}

// ─────────────────────────────────────────────────────────────────────
// ModulePrdConfig — `sb.prd.yml`
// ─────────────────────────────────────────────────────────────────────

/// Per-module production interface. Ships with the module binary —
/// no absolute paths, no host-specific fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModulePrdConfig {
    pub module: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub publishers: Vec<PubSpec>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subscribers: Vec<SubSpec>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<ServiceSpec>,
}

impl ModulePrdConfig {
    /// See [`ModuleDevConfig::find_name`].
    pub fn find_name(&self, name: &str) -> Option<EntryKind> {
        find_entry_name(&self.publishers, &self.subscribers, &self.services, name)
    }

    /// See [`ModuleDevConfig::validate`].
    pub fn validate(&self) -> Result<(), String> {
        validate_entries(&self.publishers, &self.subscribers)
    }
}

/// Which list a named entry lives in, and at what index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntryKind {
    Publisher(usize),
    Subscriber(usize),
    Service(usize),
}

fn find_entry_name(
    publishers: &[PubSpec],
    subscribers: &[SubSpec],
    services: &[ServiceSpec],
    name: &str,
) -> Option<EntryKind> {
    if let Some(i) = publishers.iter().position(|p| p.name == name) {
        return Some(EntryKind::Publisher(i));
    }
    if let Some(i) = subscribers.iter().position(|s| s.name == name) {
        return Some(EntryKind::Subscriber(i));
    }
    services
        .iter()
        .position(|s| s.name == name)
        .map(EntryKind::Service)
}

fn validate_entries(publishers: &[PubSpec], subscribers: &[SubSpec]) -> Result<(), String> {
    for (i, p) in publishers.iter().enumerate() {
        p.validate(i, "publishers")?;
    }
    for (i, s) in subscribers.iter().enumerate() {
        s.validate(i, "subscribers")?;
    }
    Ok(())
}

/// One `services:` entry. Only the name is declared today.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceSpec {
    pub name: String,
}

// ─────────────────────────────────────────────────────────────────────
// TopicRef: where a pub/sub entry's wire key comes from
// ─────────────────────────────────────────────────────────────────────

/// The topic half of a pub/sub entry: either a bare topic the module
/// owns, or a verbatim wire key.
///
/// The fully-qualified form is `device/ws/module/transport/topic`, with no
/// leading `/`; it is byte-identical to the wire key on both transports.
///
/// In YAML an `Owned` entry is written `topic: hello` and a `Key` entry
/// `key: dev01/ws/pubber/zenoh/hello`. [`TopicRef::full_key`] is the only
/// place a bare topic gets its `<device>/<ws>/<module>/<transport>/`
/// prefix.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TopicRef {
    /// Bare topic (`hello`), never containing `/`. Prefixed with
    /// `<device>/<ws>/<module>/<transport>/` when the wire key is built.
    Owned(String),
    /// Verbatim wire key (`dev01/ws/pubber/zenoh/hello`), used as is.
    /// Never starts with `/`.
    Key(String),
}

impl TopicRef {
    /// A bare, module-owned topic. Rejects an empty (or whitespace-only)
    /// name and any `/`; nested names go through [`TopicRef::key`].
    pub fn owned(bare: &str) -> Result<Self, TopicRefError> {
        if bare.trim().is_empty() {
            return Err(TopicRefError::Empty);
        }
        if bare.contains('/') {
            return Err(TopicRefError::OwnedContainsSlash {
                topic: bare.to_owned(),
            });
        }
        Ok(Self::Owned(bare.to_owned()))
    }

    /// A verbatim wire key. Leading `/`s are stripped (the canonical form
    /// has none); an empty or whitespace-only remainder is rejected.
    pub fn key(key: &str) -> Result<Self, TopicRefError> {
        let key = key.trim_start_matches('/');
        if key.trim().is_empty() {
            return Err(TopicRefError::Empty);
        }
        Ok(Self::Key(key.to_owned()))
    }

    pub fn is_owned(&self) -> bool {
        matches!(self, Self::Owned(_))
    }

    /// The bare topic for `Owned`, the wire key for `Key`.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Owned(s) | Self::Key(s) => s,
        }
    }

    /// The last path segment: the bare topic itself for `Owned`, the
    /// segment after the final `/` for `Key`.
    pub fn leaf(&self) -> &str {
        match self {
            Self::Owned(bare) => bare,
            Self::Key(key) => key.rsplit_once('/').map_or(key.as_str(), |(_, leaf)| leaf),
        }
    }

    /// Whether a user-typed reference names this entry. `Owned` matches
    /// the bare topic exactly; `Key` matches the wire key with any
    /// leading `/` on the input ignored.
    pub fn matches_input(&self, input: &str) -> bool {
        match self {
            Self::Owned(bare) => input == bare,
            Self::Key(key) => input.trim_start_matches('/') == key,
        }
    }

    /// The wire key. `Owned` becomes
    /// `{device}/{workspace}/{module}/{transport}/{bare}` (transport as its
    /// path segment, `iox2` or `zenoh`); `Key` is returned verbatim.
    pub fn full_key(
        &self,
        device: &str,
        workspace: &str,
        module: &str,
        transport: Transport,
    ) -> String {
        match self {
            Self::Owned(bare) => format!(
                "{device}/{workspace}/{module}/{}/{bare}",
                transport.path_segment()
            ),
            Self::Key(key) => key.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TopicRefError {
    #[error("exactly one of topic: or key: is required (both given)")]
    BothTopicAndKey,
    #[error("exactly one of topic: or key: is required (neither given)")]
    NeitherTopicNorKey,
    #[error("topic {topic:?} may not contain '/'; use key: for a verbatim wire key")]
    OwnedContainsSlash { topic: String },
    /// An empty key. [`TopicRef::owned`] also returns it for an empty bare
    /// topic; config loading reports that case as `topic: is empty`.
    #[error("key: is empty")]
    Empty,
    /// A `topic:` with an inner `/` but no leading `/`. sb 0.4.x rendered
    /// `topic: a/b` as `<device>/<ws>/<module>/<transport>/a/b`; no
    /// slash-free `topic:` reproduces that wire key, so loading refuses
    /// it instead of silently moving the entry to another key.
    #[error(
        "topic {topic:?} may not contain '/'; use key: for a verbatim wire key \
         (sb 0.4.x rendered it as <device>/<ws>/<module>/<transport>/{topic}; \
         to keep that wire key, use key: <device>/<ws>/<module>/<transport>/{topic})"
    )]
    LegacyOwnedWithSlash { topic: String },
}

// ─────────────────────────────────────────────────────────────────────
// Encoding: how a pub/sub entry's payload is framed
// ─────────────────────────────────────────────────────────────────────

/// Payload encoding of a pub/sub entry (`encoding:` in YAML).
///
/// Only zenoh entries may set it. An iceoryx2 payload is always the
/// fixed-layout struct generated from the `.proto`, so iceoryx2 entries
/// omit the key (or say `proto`, the default).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Encoding {
    /// Protobuf bytes of the vault message named by `type:`.
    #[default]
    Proto,
    /// `FlatBuffers` bytes of the vault message named by `type:`.
    Fb,
    /// Opaque bytes with no vault schema. Takes no `type:`; `mime:` may
    /// describe the content (e.g. `text/json`).
    Raw,
}

impl Encoding {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proto => "proto",
            Self::Fb => "fb",
            Self::Raw => "raw",
        }
    }

    /// `true` for [`Encoding::Proto`], the value an omitted `encoding:`
    /// reads as. Serde uses it to leave the default off disk.
    // By reference because `skip_serializing_if` passes `&Encoding`.
    #[allow(clippy::trivially_copy_pass_by_ref)]
    pub fn is_default(&self) -> bool {
        *self == Self::Proto
    }
}

impl FromStr for Encoding {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "proto" => Ok(Self::Proto),
            "fb" => Ok(Self::Fb),
            "raw" => Ok(Self::Raw),
            other => Err(format!(
                "unknown encoding: {other:?} (expected proto, fb, or raw)"
            )),
        }
    }
}

impl std::fmt::Display for Encoding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ─────────────────────────────────────────────────────────────────────
// Pub / Sub specs (shared between dev and prd configs)
// ─────────────────────────────────────────────────────────────────────

/// One `publishers:` entry. On disk it carries exactly one of `topic:`
/// (bare, owned by the module) or `key:` (verbatim wire key).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(into = "RawSpec")]
pub struct PubSpec {
    pub name: String,
    /// Bare topic (`image_raw`) prefixed at codegen, or a verbatim key.
    pub topic: TopicRef,
    /// Vault name, e.g. `ros2/std/ImageStamped` (`type:` on disk).
    /// `None` only when `encoding` is [`Encoding::Raw`].
    pub msg_type: Option<String>,
    pub transport: Transport,
    /// Payload framing; zenoh only. Omitted on disk when `proto`.
    pub encoding: Encoding,
    /// Content type of a raw payload (e.g. `text/json`); raw only.
    pub mime: Option<String>,
}

/// One `subscribers:` entry. Same shape and YAML form as [`PubSpec`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(into = "RawSpec")]
pub struct SubSpec {
    pub name: String,
    /// Bare topic (`image_raw`) prefixed at codegen, or a verbatim key.
    pub topic: TopicRef,
    /// Vault name, e.g. `ros2/std/ImageStamped` (`type:` on disk).
    /// `None` only when `encoding` is [`Encoding::Raw`].
    pub msg_type: Option<String>,
    pub transport: Transport,
    /// Payload framing; zenoh only. Omitted on disk when `proto`.
    pub encoding: Encoding,
    /// Content type of a raw payload (e.g. `text/json`); raw only.
    pub mime: Option<String>,
}

impl PubSpec {
    /// Check this entry against the load-time rules, naming it as
    /// `<role>[<idx>]: ...` (e.g. `publishers[0]: encoding: raw takes no
    /// type:`). Passing means the entry survives a save and reload.
    pub fn validate(&self, idx: usize, role: &str) -> Result<(), String> {
        Self::try_from(RawSpec::from(self.clone()))
            .map(drop)
            .map_err(|e| format!("{role}[{idx}]: {e}"))
    }
}

impl SubSpec {
    /// See [`PubSpec::validate`].
    pub fn validate(&self, idx: usize, role: &str) -> Result<(), String> {
        Self::try_from(RawSpec::from(self.clone()))
            .map(drop)
            .map_err(|e| format!("{role}[{idx}]: {e}"))
    }
}

/// On-disk shape of a pub/sub entry, before the `topic:`/`key:` and
/// `type:`/`encoding:`/`mime:` rules are applied.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSpec {
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    topic: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    msg_type: Option<String>,
    transport: Transport,
    #[serde(default, skip_serializing_if = "Encoding::is_default")]
    encoding: Encoding,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mime: Option<String>,
}

/// A `type:` / `encoding:` / `mime:` combination that cannot be honoured.
/// The text is the part after the entry's `publishers[i]: ` prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
enum SpecRuleError {
    #[error("encoding: is only valid for transport: zenoh")]
    EncodingNotZenoh,
    #[error("encoding: raw takes no type:")]
    RawWithType,
    #[error("type: is required unless encoding: raw")]
    MissingType,
    #[error("mime: is only valid with encoding: raw")]
    MimeWithoutRaw,
}

/// Why a [`RawSpec`] does not form a valid entry.
#[derive(Debug, thiserror::Error)]
enum RawSpecError {
    #[error(transparent)]
    Topic(#[from] TopicRefError),
    #[error("topic: is empty")]
    EmptyTopic,
    #[error(transparent)]
    Rule(#[from] SpecRuleError),
}

impl RawSpec {
    /// Apply the load-time rules:
    /// - `topic: /a/b/...` (legacy leading slash) reads as `Key("a/b/...")`,
    ///   which is the wire key sb 0.4.x produced for it;
    /// - `topic: a/b` (inner `/`, no leading `/`) is refused, see
    ///   [`TopicRefError::LegacyOwnedWithSlash`];
    /// - `topic: hello` is `Owned`, `key: ...` is `Key`;
    /// - both or neither is an error.
    fn topic_ref(&self) -> Result<TopicRef, RawSpecError> {
        match (self.topic.as_deref(), self.key.as_deref()) {
            (Some(_), Some(_)) => Err(TopicRefError::BothTopicAndKey.into()),
            (None, None) => Err(TopicRefError::NeitherTopicNorKey.into()),
            (None, Some(key)) => Ok(TopicRef::key(key)?),
            (Some(topic), None) if topic.starts_with('/') => {
                TopicRef::key(topic).map_err(|_| RawSpecError::EmptyTopic)
            }
            (Some(topic), None) if topic.contains('/') => {
                Err(TopicRefError::LegacyOwnedWithSlash {
                    topic: topic.to_owned(),
                }
                .into())
            }
            (Some(topic), None) => TopicRef::owned(topic).map_err(|_| RawSpecError::EmptyTopic),
        }
    }

    /// The `type:` / `encoding:` / `mime:` rules. The transport rule runs
    /// first because it is the root cause whenever it fires: an iceoryx2
    /// entry with `encoding: raw` is wrong for its encoding, not its type.
    fn check_rules(&self) -> Result<(), SpecRuleError> {
        if self.encoding != Encoding::Proto && self.transport != Transport::Zenoh {
            return Err(SpecRuleError::EncodingNotZenoh);
        }
        match (self.encoding, self.msg_type.is_some()) {
            (Encoding::Raw, true) => return Err(SpecRuleError::RawWithType),
            (Encoding::Proto | Encoding::Fb, false) => return Err(SpecRuleError::MissingType),
            _ => {}
        }
        if self.mime.is_some() && self.encoding != Encoding::Raw {
            return Err(SpecRuleError::MimeWithoutRaw);
        }
        Ok(())
    }

    /// Both rule sets, topic first (a missing `topic:` is reported even
    /// when the type rules also fail).
    fn checked_topic_ref(&self) -> Result<TopicRef, RawSpecError> {
        let topic = self.topic_ref()?;
        self.check_rules()?;
        Ok(topic)
    }
}

/// `TopicRef` → the (`topic:`, `key:`) pair written to disk.
fn split_topic(topic: TopicRef) -> (Option<String>, Option<String>) {
    match topic {
        TopicRef::Owned(bare) => (Some(bare), None),
        TopicRef::Key(key) => (None, Some(key)),
    }
}

impl TryFrom<RawSpec> for PubSpec {
    type Error = RawSpecError;
    fn try_from(raw: RawSpec) -> Result<Self, Self::Error> {
        let topic = raw.checked_topic_ref()?;
        Ok(Self {
            name: raw.name,
            topic,
            msg_type: raw.msg_type,
            transport: raw.transport,
            encoding: raw.encoding,
            mime: raw.mime,
        })
    }
}

impl TryFrom<RawSpec> for SubSpec {
    type Error = RawSpecError;
    fn try_from(raw: RawSpec) -> Result<Self, Self::Error> {
        let topic = raw.checked_topic_ref()?;
        Ok(Self {
            name: raw.name,
            topic,
            msg_type: raw.msg_type,
            transport: raw.transport,
            encoding: raw.encoding,
            mime: raw.mime,
        })
    }
}

impl From<PubSpec> for RawSpec {
    fn from(p: PubSpec) -> Self {
        let (topic, key) = split_topic(p.topic);
        Self {
            name: p.name,
            topic,
            key,
            msg_type: p.msg_type,
            transport: p.transport,
            encoding: p.encoding,
            mime: p.mime,
        }
    }
}

impl From<SubSpec> for RawSpec {
    fn from(s: SubSpec) -> Self {
        let (topic, key) = split_topic(s.topic);
        Self {
            name: s.name,
            topic,
            key,
            msg_type: s.msg_type,
            transport: s.transport,
            encoding: s.encoding,
            mime: s.mime,
        }
    }
}

/// Field names of [`RawSpec`] as they appear on disk.
const SPEC_FIELDS: &[&str] = &[
    "name",
    "topic",
    "key",
    "type",
    "transport",
    "encoding",
    "mime",
];

/// Deserializes a [`RawSpec`] and converts it inside the map visit, so a
/// `topic:`/`key:` or `type:`/`encoding:` error carries the entry's own
/// path (`publishers[0]: ...`) rather than the enclosing list's.
struct SpecVisitor<T>(std::marker::PhantomData<T>);

impl<'de, T> serde::de::Visitor<'de> for SpecVisitor<T>
where
    T: TryFrom<RawSpec, Error = RawSpecError>,
{
    type Value = T;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a pub/sub entry with name, topic or key, type, and transport")
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, map: A) -> Result<T, A::Error> {
        let raw = RawSpec::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
        T::try_from(raw).map_err(serde::de::Error::custom)
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, seq: A) -> Result<T, A::Error> {
        let raw = RawSpec::deserialize(serde::de::value::SeqAccessDeserializer::new(seq))?;
        T::try_from(raw).map_err(serde::de::Error::custom)
    }
}

impl<'de> Deserialize<'de> for PubSpec {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_struct(
            "PubSpec",
            SPEC_FIELDS,
            SpecVisitor(std::marker::PhantomData),
        )
    }
}

impl<'de> Deserialize<'de> for SubSpec {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_struct(
            "SubSpec",
            SPEC_FIELDS,
            SpecVisitor(std::marker::PhantomData),
        )
    }
}

/// A `topic: /...` entry found by [`legacy_topics_in`]. sb 0.5.0 reads it
/// as `key:` with the leading `/` dropped, which is the same wire key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyTopic {
    /// `"pub"` or `"sub"`.
    pub role: &'static str,
    pub name: String,
    /// The `topic:` value as written, leading `/` included.
    pub topic: String,
    /// The normalised wire key (leading `/`s removed).
    pub key: String,
}

/// Scan raw `sb.dev.yml` text for legacy `topic: /...` entries.
///
/// Uses a loose parse that ignores every other top-level key, so it works
/// on files the strict [`ModuleDevConfig`] would reject. Never errors:
/// text that does not parse yields an empty list.
pub fn legacy_topics_in(dev_yml_text: &str) -> Vec<LegacyTopic> {
    #[derive(Deserialize)]
    struct Loose {
        #[serde(default)]
        publishers: Vec<RawSpec>,
        #[serde(default)]
        subscribers: Vec<RawSpec>,
    }

    let Ok(loose) = serde_yaml::from_str::<Loose>(dev_yml_text) else {
        return Vec::new();
    };
    let tag = |role: &'static str| {
        move |spec: RawSpec| {
            let topic = spec.topic.filter(|t| t.starts_with('/'))?;
            Some(LegacyTopic {
                role,
                name: spec.name,
                key: topic.trim_start_matches('/').to_owned(),
                topic,
            })
        }
    };
    loose
        .publishers
        .into_iter()
        .filter_map(tag("pub"))
        .chain(loose.subscribers.into_iter().filter_map(tag("sub")))
        .collect()
}

// ─────────────────────────────────────────────────────────────────────
// WorkspaceFlow — `flow.yaml`
// ─────────────────────────────────────────────────────────────────────

/// Workspace module registry. Maps module name → path to its config.
///
/// Two copies of this shape exist:
///
/// - the **repo copy**, a `flow.yaml` at a repository root, written by
///   hand (and by `sb init`). Module and message paths are relative to
///   that file's directory; `name:` optionally names the workspace.
/// - the **installed copy** at `<sb_home>/workspaces/<name>/flow.yaml`,
///   derived from the repo copy on install. Every path is absolute and
///   `source:` records where it came from.
///
/// `instances` carries the workspace's deployment topology — one entry
/// per declared instance of a module (e.g. `cam_gige_ht: [left, right]`).
/// Instances are the unit of node identity in the L6 graph editor; they
/// also drive docker-aware runscript codegen at L2 (`sb init --docker`).
/// A module with no `instances:` entry runs as a single (un-suffixed)
/// process; that's the default and matches the pre-instances flow.yaml
/// shape.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceFlow {
    /// Workspace name. Absent = the repo copy's directory name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub modules: BTreeMap<String, ModuleRef>,
    /// Message directories imported into the vault on install.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<MessageSource>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub instances: BTreeMap<String, Vec<ModuleInstance>>,
    /// Provenance of an installed copy. Never present in a repo copy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<FlowSource>,
}

impl WorkspaceFlow {
    /// Every module's path, ignoring `up:`. The shape pre-0.5.0 readers of
    /// `modules` expect.
    pub fn module_paths(&self) -> BTreeMap<String, PathBuf> {
        self.modules
            .iter()
            .map(|(name, m)| (name.clone(), m.path.clone()))
            .collect()
    }

    /// Look up a single instance by `(module, id)`. Returns `None` if
    /// the module has no instances declared or no instance with that id.
    pub fn instance(&self, module: &str, id: &str) -> Option<&ModuleInstance> {
        self.instances.get(module)?.iter().find(|i| i.id == id)
    }

    /// Validate the workspace name, module paths, message sources, and
    /// every instance id (rejecting duplicates within one module's list).
    /// Call after deserializing `flow.yaml` so bad input fails at load
    /// time, not at launch time.
    pub fn validate(&self) -> Result<(), FlowValidationError> {
        if let Some(name) = &self.name {
            validate_identifier(name).map_err(|e| FlowValidationError::BadName {
                name: name.clone(),
                source: e,
            })?;
        }
        for (module, m) in &self.modules {
            if m.path.as_os_str().is_empty() {
                return Err(FlowValidationError::EmptyModulePath {
                    module: module.clone(),
                });
            }
        }
        for (index, src) in self.messages.iter().enumerate() {
            src.validate(index)?;
        }
        for (module, list) in &self.instances {
            let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
            for inst in list {
                validate_instance_id(&inst.id).map_err(|e| FlowValidationError::BadInstanceId {
                    module: module.clone(),
                    id: inst.id.clone(),
                    source: e,
                })?;
                if !seen.insert(inst.id.as_str()) {
                    return Err(FlowValidationError::DuplicateInstanceId {
                        module: module.clone(),
                        id: inst.id.clone(),
                    });
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FlowValidationError {
    #[error("module {module:?} instance id {id:?} is invalid: {source}")]
    BadInstanceId {
        module: String,
        id: String,
        #[source]
        source: InstanceIdError,
    },
    #[error("module {module:?} declares instance id {id:?} more than once")]
    DuplicateInstanceId { module: String, id: String },
    #[error("workspace name {name:?} is invalid: {source}")]
    BadName {
        name: String,
        #[source]
        source: IdentError,
    },
    #[error("module {module:?} has an empty path")]
    EmptyModulePath { module: String },
    #[error("messages[{index}]: style {style:?} is invalid (allowed: [A-Za-z0-9_])")]
    BadMessageStyle { index: usize, style: String },
    #[error("messages[{index}]: path is empty")]
    EmptyMessagePath { index: usize },
    #[error("messages[{index}]: unknown backend {backend:?} (expected one of: proto, fb, iox2)")]
    UnknownBackend { index: usize, backend: String },
}

fn default_true() -> bool {
    true
}

/// One `modules:` value in `flow.yaml`: where the module's `sb.dev.yml`
/// is, and whether `sb up` launches it.
///
/// On disk it is either a bare path (`pubber` or `pubber/sb.dev.yml`),
/// which means `up: true`, or a map `{ path: ..., up: false }`. A module
/// with `up: false` is still registered (`sb run` / `sb stop` work on it);
/// only the whole-workspace `sb up` skips it. Serialises back to the bare
/// form whenever `up` is true.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(into = "ModuleRefRaw")]
pub struct ModuleRef {
    pub path: PathBuf,
    pub up: bool,
}

impl ModuleRef {
    /// A module `sb up` launches (`up: true`).
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            up: true,
        }
    }
}

impl From<PathBuf> for ModuleRef {
    fn from(path: PathBuf) -> Self {
        Self::new(path)
    }
}

/// The two YAML spellings of a [`ModuleRef`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ModuleRefRaw {
    /// `name: path`, meaning `up: true`.
    Path(PathBuf),
    /// `name: { path: ..., up: ... }`; `up` defaults to `true`.
    Full {
        path: PathBuf,
        #[serde(default = "default_true")]
        up: bool,
    },
}

impl From<ModuleRefRaw> for ModuleRef {
    fn from(raw: ModuleRefRaw) -> Self {
        match raw {
            ModuleRefRaw::Path(path) => Self::new(path),
            ModuleRefRaw::Full { path, up } => Self { path, up },
        }
    }
}

impl From<ModuleRef> for ModuleRefRaw {
    fn from(m: ModuleRef) -> Self {
        if m.up {
            Self::Path(m.path)
        } else {
            Self::Full {
                path: m.path,
                up: false,
            }
        }
    }
}

/// The map spelling of a [`ModuleRef`], parsed strictly so a typo'd key
/// is reported by name instead of as "matched no variant".
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModuleRefMap {
    path: PathBuf,
    #[serde(default = "default_true")]
    up: bool,
}

// Hand-written rather than `#[serde(from = "ModuleRefRaw")]`: an untagged
// enum reports every mismatch as "data did not match any variant", which
// hides an `upp:` typo or a non-boolean `up:` from the user. Accepts
// exactly the shapes `ModuleRefRaw` describes.
impl<'de> Deserialize<'de> for ModuleRef {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct ModuleRefVisitor;

        impl<'de> serde::de::Visitor<'de> for ModuleRefVisitor {
            type Value = ModuleRef;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a module path, or a map with `path:` and optional `up:`")
            }

            fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<ModuleRef, E> {
                Ok(ModuleRef::new(s))
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                map: A,
            ) -> Result<ModuleRef, A::Error> {
                let m =
                    ModuleRefMap::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
                Ok(ModuleRef {
                    path: m.path,
                    up: m.up,
                })
            }
        }

        d.deserialize_any(ModuleRefVisitor)
    }
}

/// One `messages:` entry in `flow.yaml`: a directory of `.proto` files
/// imported into the vault under `style`.
///
/// `path` holds `<ns>/<Leaf>.proto` files, or flat `<Leaf>.proto` files
/// (namespace = the directory's own name). `backends` picks what
/// `sb message compile` emits for it, among `proto`, `fb`, `iox2`;
/// empty means the style's own default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageSource {
    pub style: String,
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub backends: Vec<String>,
}

impl MessageSource {
    fn validate(&self, index: usize) -> Result<(), FlowValidationError> {
        let style_ok = is_valid_message_style(&self.style)
            && self
                .style
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_');
        if !style_ok {
            return Err(FlowValidationError::BadMessageStyle {
                index,
                style: self.style.clone(),
            });
        }
        if self.path.as_os_str().is_empty() {
            return Err(FlowValidationError::EmptyMessagePath { index });
        }
        if let Some(bad) = self
            .backends
            .iter()
            .find(|b| !Backend::ALL.iter().any(|k| k.as_str() == b.as_str()))
        {
            return Err(FlowValidationError::UnknownBackend {
                index,
                backend: bad.clone(),
            });
        }
        Ok(())
    }
}

/// Where an installed `flow.yaml` came from. Written only into the
/// installed copy under `<sb_home>/workspaces/<name>/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowSource {
    /// Absolute path of the repo copy that was installed.
    pub repo_flow: PathBuf,
    /// Git URL the repo was cloned from; enables `update_from_source`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// RFC 3339 UTC time of the install, e.g. `2026-10-13T08:35:23Z`.
    pub installed_at: String,
}

/// One deployed instance of a module — a node in the L6 graph and a
/// dispatch branch in the docker runscript.
///
/// `id` is the wire-form suffix appended to the module on the wire
/// (`<module>-<id>` → e.g. `cam_gige_ht-left`). `label` is a free-form
/// human-readable display string used by the L6 graph editor; if
/// absent, the UI should fall back to `id`. `docker` carries per-instance
/// container settings consumed by `sb init --docker`; if absent, the
/// instance runs as a plain (non-docker) process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleInstance {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docker: Option<DockerSpec>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

/// Per-instance container settings. Every field is optional so an
/// instance can opt into individual flags without listing the others.
/// `image` is the docker image tag (e.g. `cam_gige_ht:latest`); a
/// missing image is a preflight error at `sb init --docker` time.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DockerSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub devices: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mounts: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpus: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── YAML round-trip tests (L1 TDD test #2) ────────────────────────

    fn yaml_fixed_point<T>(orig: &T)
    where
        T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        let y = serde_yaml::to_string(orig).unwrap();
        let back: T = serde_yaml::from_str(&y).unwrap();
        let y2 = serde_yaml::to_string(&back).unwrap();
        assert_eq!(y, y2, "YAML not a fixed point");
        assert_eq!(*orig, back, "round-trip changed the value");
    }

    // ── topic: / key: entries ─────────────────────────────────────────

    fn dev_yml_with(entries: &str) -> String {
        format!("module: subber\nlanguage: rust\nroot: /tmp/subber\n{entries}")
    }

    fn load_err(entries: &str) -> String {
        serde_yaml::from_str::<ModuleDevConfig>(&dev_yml_with(entries))
            .unwrap_err()
            .to_string()
    }

    #[test]
    fn spec_legacy_slashed_topic_reads_as_key() {
        let y = dev_yml_with(
            "subscribers:\n  - name: hello\n    topic: /dev01/demo/pubber/zenoh/hello\n    \
             type: std/String\n    transport: zenoh\n",
        );
        let m: ModuleDevConfig = serde_yaml::from_str(&y).unwrap();
        let topic = &m.subscribers[0].topic;
        assert_eq!(
            *topic,
            TopicRef::Key("dev01/demo/pubber/zenoh/hello".into())
        );
        // Same wire key sb 0.4.x produced (leading slash stripped).
        assert_eq!(
            topic.full_key("devX", "wsX", "subber", Transport::Zenoh),
            "dev01/demo/pubber/zenoh/hello"
        );
        // Re-serialising writes the canonical `key:` form.
        let out = serde_yaml::to_string(&m).unwrap();
        assert!(out.contains("key: dev01/demo/pubber/zenoh/hello"), "{out}");
        assert!(!out.contains("topic:"), "{out}");
    }

    #[test]
    fn spec_rejects_both_topic_and_key() {
        let e = load_err(
            "publishers:\n  - name: hello\n    topic: hello\n    key: a/b/c/zenoh/hello\n    \
             type: std/String\n    transport: zenoh\n",
        );
        assert!(
            e.contains("publishers[0]: exactly one of topic: or key: is required (both given)"),
            "{e}"
        );
    }

    #[test]
    fn spec_rejects_neither() {
        let e =
            load_err("publishers:\n  - name: hello\n    type: std/String\n    transport: zenoh\n");
        assert!(
            e.contains("publishers[0]: exactly one of topic: or key: is required (neither given)"),
            "{e}"
        );
    }

    #[test]
    fn spec_rejects_empty_key() {
        let e = load_err(
            "subscribers:\n  - name: hello\n    key: \"/\"\n    type: std/String\n    \
             transport: zenoh\n",
        );
        assert!(e.contains("subscribers[0]: key: is empty"), "{e}");
    }

    #[test]
    fn spec_rejects_legacy_unslashed_nested_topic() {
        let e = load_err(
            "publishers:\n  - name: hello\n    topic: a/b\n    type: std/String\n    \
             transport: zenoh\n",
        );
        assert!(
            e.contains(
                "publishers[0]: topic \"a/b\" may not contain '/'; use key: for a verbatim wire key"
            ),
            "{e}"
        );
        assert!(
            e.contains("key: <device>/<ws>/<module>/<transport>/a/b"),
            "{e}"
        );
    }

    #[test]
    fn workspace_flow_without_instances_key_still_parses() {
        // Pre-instances flow.yaml shape — must continue to parse so
        // existing user workspaces keep working when sb is upgraded.
        let y = "modules:\n  camera: /home/u/projects/camera/sb.dev.yml\n";
        let f: WorkspaceFlow = serde_yaml::from_str(y).unwrap();
        assert_eq!(f.modules.len(), 1);
        assert!(f.instances.is_empty());
        assert!(f.validate().is_ok());
    }

    #[test]
    fn workspace_flow_validate_rejects_bad_instance_id() {
        for bad in ["", "a b", "a/b", "a.b"] {
            let mut instances = BTreeMap::new();
            instances.insert(
                "cam".into(),
                vec![ModuleInstance {
                    id: bad.into(),
                    label: None,
                    docker: None,
                    args: vec![],
                }],
            );
            let f = WorkspaceFlow {
                instances,
                ..Default::default()
            };
            assert!(
                matches!(f.validate(), Err(FlowValidationError::BadInstanceId { .. })),
                "{bad:?} should fail validation",
            );
        }
    }

    #[test]
    fn workspace_flow_validate_rejects_duplicate_instance_id() {
        let mut instances = BTreeMap::new();
        instances.insert(
            "cam".into(),
            vec![
                ModuleInstance {
                    id: "left".into(),
                    label: None,
                    docker: None,
                    args: vec![],
                },
                ModuleInstance {
                    id: "left".into(),
                    label: None,
                    docker: None,
                    args: vec![],
                },
            ],
        );
        let f = WorkspaceFlow {
            instances,
            ..Default::default()
        };
        assert!(matches!(
            f.validate(),
            Err(FlowValidationError::DuplicateInstanceId { .. })
        ));
    }

    // ── encoding: / type: / mime: rules ───────────────────────────────

    #[test]
    fn encoding_defaults_to_proto_and_is_omitted() {
        let y = dev_yml_with(
            "publishers:\n  - name: out\n    topic: out\n    type: ros2/std/String\n    \
             transport: zenoh\n",
        );
        let m: ModuleDevConfig = serde_yaml::from_str(&y).unwrap();
        assert_eq!(m.publishers[0].encoding, Encoding::Proto);
        assert_eq!(m.publishers[0].mime, None);
        let out = serde_yaml::to_string(&m).unwrap();
        assert!(!out.contains("encoding:"), "{out}");
        assert!(!out.contains("mime:"), "{out}");
        assert_eq!(serde_yaml::to_string(&Encoding::Fb).unwrap().trim(), "fb");
        assert_eq!("raw".parse::<Encoding>(), Ok(Encoding::Raw));
        assert!(Encoding::default().is_default());
        assert!(!Encoding::Raw.is_default());
    }

    #[test]
    fn spec_rule_raw_with_type_is_rejected() {
        let e = load_err(
            "publishers:\n  - name: out\n    topic: out\n    type: ros2/std/String\n    \
             transport: zenoh\n    encoding: raw\n",
        );
        assert!(
            e.contains("publishers[0]: encoding: raw takes no type:"),
            "{e}"
        );
    }

    #[test]
    fn spec_rule_missing_type_is_rejected() {
        let e = load_err("publishers:\n  - name: out\n    topic: out\n    transport: zenoh\n");
        assert!(
            e.contains("publishers[0]: type: is required unless encoding: raw"),
            "{e}"
        );
    }

    #[test]
    fn spec_rule_mime_without_raw_is_rejected() {
        let e = load_err(
            "publishers:\n  - name: out\n    topic: out\n    type: ros2/std/String\n    \
             transport: zenoh\n    mime: text/json\n",
        );
        assert!(
            e.contains("publishers[0]: mime: is only valid with encoding: raw"),
            "{e}"
        );
    }

    #[test]
    fn spec_rule_encoding_on_iox2_is_rejected() {
        let e = load_err(
            "publishers:\n  - name: out\n    topic: out\n    type: ros2/std/String\n    \
             transport: iceoryx2\n    encoding: fb\n",
        );
        assert!(
            e.contains("publishers[0]: encoding: is only valid for transport: zenoh"),
            "{e}"
        );
        // `encoding: proto` is the default, so iceoryx2 may spell it out.
        let y = dev_yml_with(
            "publishers:\n  - name: out\n    topic: out\n    type: ros2/std/String\n    \
             transport: iceoryx2\n    encoding: proto\n",
        );
        assert!(serde_yaml::from_str::<ModuleDevConfig>(&y).is_ok());
    }

    // ── sb.dev.yml root: ──────────────────────────────────────────────

    #[test]
    fn root_absent_resolves_to_the_yaml_dir() {
        let m: ModuleDevConfig = serde_yaml::from_str("module: m\nlanguage: rust\n").unwrap();
        assert_eq!(m.root, None);
        assert_eq!(
            m.module_root(Path::new("/repo/apps/m/sb.dev.yml")),
            PathBuf::from("/repo/apps/m")
        );
        assert_eq!(m.module_root(Path::new("sb.dev.yml")), PathBuf::from("."));
        assert!(!serde_yaml::to_string(&m).unwrap().contains("root:"));

        let rel = ModuleDevConfig {
            root: Some(PathBuf::from("src")),
            ..m.clone()
        };
        assert_eq!(
            rel.module_root(Path::new("/repo/m/sb.dev.yml")),
            PathBuf::from("/repo/m/src")
        );
        let abs = ModuleDevConfig {
            root: Some(PathBuf::from("/elsewhere/m")),
            ..m
        };
        assert_eq!(
            abs.module_root(Path::new("/repo/m/sb.dev.yml")),
            PathBuf::from("/elsewhere/m")
        );
    }

    // ── flow.yaml modules: / messages: / source: ──────────────────────

    #[test]
    fn module_ref_string_and_map_forms() {
        let y = "modules:\n  a: apps/a\n  b:\n    path: apps/b\n    up: false\n  \
                 c:\n    path: apps/c\n";
        let f: WorkspaceFlow = serde_yaml::from_str(y).unwrap();
        assert_eq!(f.modules["a"], ModuleRef::new("apps/a"));
        assert_eq!(
            f.modules["b"],
            ModuleRef {
                path: PathBuf::from("apps/b"),
                up: false
            }
        );
        // Map form without `up:` means up: true and re-emits as a string.
        assert_eq!(f.modules["c"], ModuleRef::new("apps/c"));
        let out = serde_yaml::to_string(&f).unwrap();
        assert!(out.contains("a: apps/a\n"), "{out}");
        assert!(out.contains("c: apps/c\n"), "{out}");
        assert!(out.contains("path: apps/b"), "{out}");
        assert!(out.contains("up: false"), "{out}");
        yaml_fixed_point(&f);
        assert_eq!(
            f.module_paths(),
            BTreeMap::from([
                ("a".to_owned(), PathBuf::from("apps/a")),
                ("b".to_owned(), PathBuf::from("apps/b")),
                ("c".to_owned(), PathBuf::from("apps/c")),
            ])
        );
        assert_eq!(
            ModuleRef::from(ModuleRefRaw::Full {
                path: "x".into(),
                up: true
            }),
            ModuleRef::new("x")
        );
    }

    #[test]
    fn module_ref_rejects_unknown_keys_by_name() {
        let e = serde_yaml::from_str::<WorkspaceFlow>("modules:\n  a:\n    path: a\n    upp: no\n")
            .unwrap_err()
            .to_string();
        assert!(e.contains("unknown field `upp`"), "{e}");
    }

    #[test]
    fn flow_validate_rejects_bad_name_style_and_backend() {
        let bad_name = WorkspaceFlow {
            name: Some("my-ws".into()),
            ..Default::default()
        };
        assert!(matches!(
            bad_name.validate(),
            Err(FlowValidationError::BadName { .. })
        ));
        let bad_style = WorkspaceFlow {
            messages: vec![MessageSource {
                style: "../x".into(),
                path: "m".into(),
                backends: vec![],
            }],
            ..Default::default()
        };
        assert!(matches!(
            bad_style.validate(),
            Err(FlowValidationError::BadMessageStyle { index: 0, .. })
        ));
        let bad_backend = WorkspaceFlow {
            messages: vec![MessageSource {
                style: "acme".into(),
                path: "m".into(),
                backends: vec!["proto".into(), "capnp".into()],
            }],
            ..Default::default()
        };
        let e = bad_backend.validate().unwrap_err();
        assert!(e.to_string().contains("\"capnp\""), "{e}");
    }
}
