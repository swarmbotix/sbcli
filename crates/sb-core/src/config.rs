//! Serde types — one Rust struct per on-disk YAML file.
//!
//! File ↔ type ↔ owner mapping:
//!
//! | File                | Type              | Owner                    |
//! | ------------------- | ----------------- | ------------------------ |
//! | `sb.config.yml`     | `SbCliConfig`     | the CLI itself           |
//! | `sb.module.yaml`    | `ModuleDevConfig` | per-module, dev          |
//! | `sb.prd.yml`        | `ModulePrdConfig` | per-module, prod         |
//! | `sb.workspace.yaml` | `Workspace`       | workspace inventory      |
//! | `<name>.flow.yaml`  | `Flow`            | one launchable graph     |
//!
//! `sb.module.yaml` and `sb.flow.yaml` replaced `sb.dev.yml` and
//! `flow.yaml` in 0.6.0, and the old names are not read since 0.7.0.
//! Callers look for them only to refuse them with a pointer to the
//! migration notes ([`removed_module_file_message`],
//! [`removed_flow_file_message`]). sb-core does no file IO, so probing is
//! the caller's.
//!
//! In 0.8.0 the 0.7 `sb.flow.yaml` (module paths, messages, inline
//! instances, and wires in one file) split into `sb.workspace.yaml` and
//! one `*.flow.yaml` per flow, `sb.flow.yaml` being the flow named
//! `default`. A 0.7-shape `sb.flow.yaml` is still read in 0.8.0:
//! [`probe_legacy_flow_shape`] detects it and [`convert_legacy_flow`]
//! turns it into a [`Workspace`] and the default [`Flow`].
//!
//! Every field is `Option<T>` where it can legitimately be absent in a
//! given layer; `SbCliConfig::merge` then resolves the layering.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize};

use crate::host::{Host, is_valid_platform_key};
use crate::ident::{IdentError, validate_identifier};
use crate::style::Backend;
use crate::topic::{InstanceIdError, is_valid_segment_char, validate_instance_id};
use crate::transport::Transport;

// ─────────────────────────────────────────────────────────────────────
// File names
// ─────────────────────────────────────────────────────────────────────

/// A module's manifest, at the module root.
pub const MODULE_FILE: &str = "sb.module.yaml";
/// A workspace's inventory (modules and message sources), at the
/// repository root and in the installed copy.
pub const WORKSPACE_FILE: &str = "sb.workspace.yaml";
/// The file of the flow named [`DEFAULT_FLOW_NAME`], at the repository
/// root.
pub const DEFAULT_FLOW_FILE: &str = "sb.flow.yaml";
/// Every other flow's file is `<name>` followed by this suffix.
pub const FLOW_SUFFIX: &str = ".flow.yaml";
/// The flow `sb up` launches when no flow is named.
pub const DEFAULT_FLOW_NAME: &str = "default";
/// The default flow's file under its pre-0.8.0 constant name; the same
/// value as [`DEFAULT_FLOW_FILE`].
pub const FLOW_FILE: &str = DEFAULT_FLOW_FILE;

/// The flow a file name holds: [`DEFAULT_FLOW_FILE`] is
/// [`DEFAULT_FLOW_NAME`], `calib.flow.yaml` is `calib`.
///
/// `None` when `file_name` is not a flow file name: no [`FLOW_SUFFIX`],
/// an empty stem, a path separator, or `default.flow.yaml` (the default
/// flow's file is [`DEFAULT_FLOW_FILE`], so that name would shadow it).
pub fn flow_name_of(file_name: &str) -> Option<String> {
    if file_name == DEFAULT_FLOW_FILE {
        return Some(DEFAULT_FLOW_NAME.to_owned());
    }
    let stem = file_name.strip_suffix(FLOW_SUFFIX)?;
    let valid = !stem.is_empty() && stem != DEFAULT_FLOW_NAME && !stem.contains(['/', '\\']);
    valid.then(|| stem.to_owned())
}

/// The file name of the flow `flow`: [`DEFAULT_FLOW_FILE`] for
/// [`DEFAULT_FLOW_NAME`], `<flow>.flow.yaml` otherwise. The inverse of
/// [`flow_name_of`].
pub fn flow_file_name(flow: &str) -> String {
    if flow == DEFAULT_FLOW_NAME {
        DEFAULT_FLOW_FILE.to_owned()
    } else {
        format!("{flow}{FLOW_SUFFIX}")
    }
}

/// The names a module directory is probed for: [`MODULE_FILE`] only.
pub fn module_file_candidates() -> [&'static str; 1] {
    [MODULE_FILE]
}

/// The names a workspace directory is probed for: [`FLOW_FILE`] only.
pub fn flow_file_candidates() -> [&'static str; 1] {
    [FLOW_FILE]
}

/// The pre-0.6.0 manifest name. Not read since 0.7.0; callers look for it
/// only to refuse it ([`removed_module_file_message`]) and `sb doctor`
/// lists it.
pub const REMOVED_MODULE_FILE: &str = "sb.dev.yml";
/// The pre-0.6.0 flow name. Not read since 0.7.0; callers look for it
/// only to refuse it ([`removed_flow_file_message`]) and `sb doctor`
/// lists it.
pub const REMOVED_FLOW_FILE: &str = "flow.yaml";
/// Where every message about a removed file or form points.
pub const MIGRATING_HINT: &str = "see sbcli_module.md §Migrating";

/// The error for a module directory `dir` that holds
/// [`REMOVED_MODULE_FILE`] and no [`MODULE_FILE`].
pub fn removed_module_file_message(dir: &Path) -> String {
    format!(
        "module manifest {MODULE_FILE} not found at {}; {REMOVED_MODULE_FILE} is no longer \
         read since 0.7.0, rename it ({MIGRATING_HINT})",
        dir.display()
    )
}

/// The error for a workspace directory `dir` that holds
/// [`REMOVED_FLOW_FILE`] and no [`FLOW_FILE`].
pub fn removed_flow_file_message(dir: &Path) -> String {
    format!(
        "{FLOW_FILE} not found at {}; {REMOVED_FLOW_FILE} is no longer read since 0.7.0, \
         rename it ({MIGRATING_HINT})",
        dir.display()
    )
}

/// `path` with a final [`REMOVED_MODULE_FILE`] replaced by
/// [`MODULE_FILE`], any other path unchanged. Flows and installed copies
/// written before 0.7.0 may record `.../sb.dev.yml`; the entry then
/// stands for the manifest in that directory, and the caller's
/// missing-file check refuses a directory that still holds only the old
/// file. The old file itself is never read.
pub fn current_manifest_path(path: &Path) -> PathBuf {
    if path.file_name().is_some_and(|n| n == REMOVED_MODULE_FILE) {
        path.with_file_name(MODULE_FILE)
    } else {
        path.to_path_buf()
    }
}

/// One deprecation hit found while reading a file or planning a launch.
///
/// Crates collect these instead of printing; the CLI prints each one as
/// `warning: <item> is deprecated since ...; <hint>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deprecation {
    /// What is deprecated, as the warning names it.
    pub item: String,
    /// What to do instead.
    pub hint: String,
}

impl Deprecation {
    pub fn new(item: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            item: item.into(),
            hint: hint.into(),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// Language — the manifest's `language:` tag
// ─────────────────────────────────────────────────────────────────────

/// The language of the user's module. Picked at `sb init` time via a
/// `--<lang>` flag, written to the module manifest, and consulted by L3
/// codegen to choose file extensions / runtime APIs, and by
/// [`language_defaults`] to choose the build and start commands.
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
// ModuleDevConfig — `sb.module.yaml`
// ─────────────────────────────────────────────────────────────────────

/// Per-module manifest: identity, launch scripts, environment, and the
/// pub/sub interface.
///
/// `root` is optional: absent means "the directory holding this
/// manifest", which keeps the file free of host-specific paths so it
/// can be committed. Resolve it with [`ModuleDevConfig::module_root`],
/// never by reading the field directly.
///
/// Every launch key has a default from `language:` (see
/// [`language_defaults`]); [`ModuleDevConfig::resolve`] merges defaults,
/// the top level, and the host's `platforms:` overlays into one
/// [`Resolved`]. Every key added in 0.6.0 is omitted on write when empty,
/// so a manifest that uses none of them still reads on sb 0.5.
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
    /// The module's own version, for sharing. Free text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The sb version the module needs (`">=0.6"`). Free text; not
    /// enforced in 0.6.0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sb: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// `build` / `start` / `stop` / `test`; each overrides the language
    /// default individually.
    #[serde(default, skip_serializing_if = "Scripts::is_empty")]
    pub scripts: Scripts,
    /// Environment applied to every script. Values may use `${VAR}`,
    /// `${VAR:-default}`, and `{name}` placeholders.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Executables `sb doctor` must find on `PATH`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
    /// Supported hosts and their overlays. Absent = every host.
    #[serde(default, skip_serializing_if = "Platforms::is_empty")]
    pub platforms: Platforms,

    /// On disk a map keyed by entry name; the 0.7 list of `- name: ...`
    /// entries is also read. Written as the map, in this order.
    #[serde(
        default,
        deserialize_with = "ports_from_map_or_list",
        serialize_with = "ports_to_map",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub publishers: Vec<PubSpec>,
    /// Same forms as `publishers`.
    #[serde(
        default,
        deserialize_with = "ports_from_map_or_list",
        serialize_with = "ports_to_map",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub subscribers: Vec<SubSpec>,
    /// Written as a list of names; see [`ServiceSpec`] for what is read.
    #[serde(
        default,
        deserialize_with = "services_from_any",
        serialize_with = "services_to_list",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub services: Vec<ServiceSpec>,
}

fn default_io_dir() -> PathBuf {
    PathBuf::from("swarmbotix_io")
}

impl ModuleDevConfig {
    /// A manifest with only `module` and `language` set; every other key
    /// is at its default and `io_dir` is the language's default.
    pub fn new(module: impl Into<String>, language: Language) -> Self {
        Self {
            module: module.into(),
            language,
            root: None,
            io_dir: language.default_io_dir(),
            version: None,
            sb: None,
            description: None,
            scripts: Scripts::default(),
            env: BTreeMap::new(),
            requires: Vec::new(),
            platforms: Platforms::default(),
            publishers: Vec::new(),
            subscribers: Vec::new(),
            services: Vec::new(),
        }
    }

    /// The module root for the manifest at `dev_yml`.
    ///
    /// `root:` absent resolves to the yaml's directory, a relative `root:`
    /// is joined onto that directory, and an absolute one is returned
    /// unchanged. A bare file name (no directory part) resolves to `.`.
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
    /// entry by its index, the list-form spelling: `publishers[0]: ...`.
    ///
    /// Also checks the launch keys: every `platforms:` key is known, and
    /// `scripts.start` (top level or in any overlay) is at most one
    /// command.
    pub fn validate(&self) -> Result<(), String> {
        validate_entries(&self.publishers, &self.subscribers)?;
        self.platforms.validate().map_err(|e| e.to_string())?;
        if self.scripts.has_multi_step_start() {
            return Err(ManifestError::StartNotSingle.to_string());
        }
        if let Some(key) = self
            .platforms
            .0
            .iter()
            .find_map(|(key, overlay)| overlay.scripts.has_multi_step_start().then_some(key))
        {
            return Err(format!(
                "platforms.{key}: {}",
                ManifestError::StartNotSingle
            ));
        }
        Ok(())
    }

    /// Whether `scripts:` is declared at the top level or in any
    /// `platforms:` overlay.
    pub fn declares_scripts(&self) -> bool {
        !self.scripts.is_empty() || self.platforms.0.values().any(|o| !o.scripts.is_empty())
    }

    /// Whether `host` may run this module. See [`Platforms::supports`].
    pub fn supports_host(&self, host: &Host) -> bool {
        self.platforms.supports(host)
    }

    /// The launch facts for `host`, before placeholder expansion.
    ///
    /// Merge order, lowest to highest: [`language_defaults`], the top
    /// level, then [`Platforms::overlays_for`] (`<os>` before
    /// `<os>-<arch>`). `scripts` merge per key, `env` per key (later
    /// wins), `requires` append and dedupe keeping the first occurrence,
    /// and `bin` comes from the language defaults only.
    ///
    /// Errors: [`ManifestError::Unsupported`] when `platforms:` does not
    /// list the host, [`ManifestError::StartNotSingle`] when the effective
    /// `start` is not exactly one command, and [`ManifestError::NoStart`]
    /// when neither the manifest nor the language supplies one.
    pub fn resolve(&self, host: &Host) -> Result<Resolved, ManifestError> {
        if !self.supports_host(host) {
            return Err(ManifestError::Unsupported(
                self.module.clone(),
                host.to_string(),
            ));
        }
        let mut out = language_defaults(self.language, host);
        let mut scripts = Scripts::default();
        let overlays = self.platforms.overlays_for(host);
        let layers = std::iter::once((&self.scripts, &self.env, self.requires.as_slice())).chain(
            overlays
                .iter()
                .map(|o| (&o.scripts, &o.env, o.requires.as_slice())),
        );
        for (layer_scripts, env, requires) in layers {
            scripts.merge_over(layer_scripts);
            out.env
                .extend(env.iter().map(|(k, v)| (k.clone(), v.clone())));
            out.requires.extend_from_slice(requires);
        }
        out.requires = dedupe_keep_first(std::mem::take(&mut out.requires));
        if let Some(build) = scripts.build {
            out.build = build.steps;
        }
        if let Some(stop) = scripts.stop {
            out.stop = stop.steps;
        }
        if let Some(test) = scripts.test {
            out.test = test.steps;
        }
        match scripts.start {
            Some(start) => {
                out.start = start
                    .single()
                    .cloned()
                    .ok_or(ManifestError::StartNotSingle)?;
            }
            None if out.start.is_empty() => return Err(ManifestError::NoStart(self.language)),
            None => {}
        }
        Ok(out)
    }
}

/// `items` with later duplicates removed, order otherwise kept.
fn dedupe_keep_first(items: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    items
        .into_iter()
        .filter(|item| seen.insert(item.clone()))
        .collect()
}

// ─────────────────────────────────────────────────────────────────────
// Scripts, platforms, and resolution for one host
// ─────────────────────────────────────────────────────────────────────

/// One argv, already split. `argv[0]` is the program.
pub type Argv = Vec<String>;

/// A script slot: one or more commands, run in order.
///
/// On disk a slot is a string (one command) or a list of strings (one
/// command each). Each string is split by [`Script::parse_line`] into an
/// argv; it is never handed to a shell, so `&&`, redirection, and
/// `export` have no meaning. An empty list declares the slot with no
/// commands, which overrides a language default with nothing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Script {
    pub steps: Vec<Argv>,
}

impl Script {
    /// Split one command line into argv.
    ///
    /// Whitespace separates tokens. `"..."` and `'...'` group, may appear
    /// inside a token (`a"b c"d` is the one token `ab cd`), and are
    /// removed; `""` is an empty token. There are no backslash escapes.
    pub fn parse_line(line: &str) -> Result<Argv, ScriptError> {
        let mut argv = Vec::new();
        let mut token = String::new();
        let mut in_token = false;
        let mut chars = line.chars();
        while let Some(c) = chars.next() {
            if c == '"' || c == '\'' {
                in_token = true;
                loop {
                    match chars.next() {
                        Some(q) if q == c => break,
                        Some(other) => token.push(other),
                        None => return Err(ScriptError::UnterminatedQuote(line.to_owned())),
                    }
                }
            } else if c.is_whitespace() {
                if in_token {
                    argv.push(std::mem::take(&mut token));
                    in_token = false;
                }
            } else {
                token.push(c);
                in_token = true;
            }
        }
        if in_token {
            argv.push(token);
        }
        if argv.is_empty() {
            return Err(ScriptError::Empty);
        }
        Ok(argv)
    }

    /// The command, when the slot holds exactly one.
    pub fn single(&self) -> Option<&Argv> {
        if let [only] = self.steps.as_slice() {
            Some(only)
        } else {
            None
        }
    }
}

/// A token that needs no quoting: non-empty, no whitespace, no quotes.
fn is_plain_token(token: &str) -> bool {
    !token.is_empty()
        && !token
            .chars()
            .any(|c| c.is_whitespace() || c == '"' || c == '\'')
}

/// `token` spelled so [`Script::parse_line`] reads it back unchanged.
fn quote_token(token: &str) -> String {
    if is_plain_token(token) {
        return token.to_owned();
    }
    if !token.contains('"') {
        return format!("\"{token}\"");
    }
    if !token.contains('\'') {
        return format!("'{token}'");
    }
    // Both quote kinds: double-quote each run between `"`s and spell each
    // `"` itself as `'"'`; adjacent quoted parts join into one token.
    let mut out = String::with_capacity(token.len() + 8);
    for (i, run) in token.split('"').enumerate() {
        if i > 0 {
            out.push_str("'\"'");
        }
        if !run.is_empty() {
            out.push('"');
            out.push_str(run);
            out.push('"');
        }
    }
    out
}

/// One step as one line, each token quoted only when it must be.
fn join_argv(step: &[String]) -> String {
    step.iter()
        .map(|t| quote_token(t))
        .collect::<Vec<_>>()
        .join(" ")
}

// One plain step is written as a string; anything else as a list of
// lines. Both forms read back to the same steps.
impl Serialize for Script {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let plain = self
            .single()
            .filter(|step| !step.is_empty() && step.iter().all(|t| is_plain_token(t)));
        if let Some(step) = plain {
            return s.serialize_str(&step.join(" "));
        }
        s.collect_seq(self.steps.iter().map(|step| join_argv(step)))
    }
}

impl<'de> Deserialize<'de> for Script {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct ScriptVisitor;

        impl<'de> serde::de::Visitor<'de> for ScriptVisitor {
            type Value = Script;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a command string, or a list of command strings")
            }

            fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<Script, E> {
                let argv = Script::parse_line(s).map_err(E::custom)?;
                Ok(Script { steps: vec![argv] })
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Script, A::Error> {
                let mut steps = Vec::with_capacity(seq.size_hint().unwrap_or(0));
                while let Some(line) = seq.next_element::<String>()? {
                    steps.push(Script::parse_line(&line).map_err(serde::de::Error::custom)?);
                }
                Ok(Script { steps })
            }
        }

        d.deserialize_any(ScriptVisitor)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScriptError {
    #[error("unterminated quote in command: {0}")]
    UnterminatedQuote(String),
    #[error("empty command")]
    Empty,
}

/// The `scripts:` block. Each key is optional; an absent key falls
/// through to the next lower layer (ultimately [`language_defaults`]).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scripts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<Script>,
    /// Exactly one command. sb appends `--id <id>` per instance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Script>,
    /// Extra teardown, run after sb has closed the module's windows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<Script>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test: Option<Script>,
}

impl Scripts {
    /// True when no key is declared.
    pub fn is_empty(&self) -> bool {
        self.build.is_none() && self.start.is_none() && self.stop.is_none() && self.test.is_none()
    }

    /// Per key: a key declared in `over` replaces this one.
    pub fn merge_over(&mut self, over: &Scripts) {
        let slots = [
            (&mut self.build, &over.build),
            (&mut self.start, &over.start),
            (&mut self.stop, &over.stop),
            (&mut self.test, &over.test),
        ];
        for (slot, value) in slots {
            if value.is_some() {
                slot.clone_from(value);
            }
        }
    }

    /// `start` declared with more than one command.
    fn has_multi_step_start(&self) -> bool {
        self.start.as_ref().is_some_and(|s| s.steps.len() > 1)
    }
}

/// One `platforms.<key>` value: the same launch keys as the top level,
/// merged over it on a matching host.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Overlay {
    #[serde(default, skip_serializing_if = "Scripts::is_empty")]
    pub scripts: Scripts,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
}

/// The `platforms:` block. Keys are `<os>` or `<os>-<arch>` from
/// [`crate::host::KNOWN_OS`] / [`crate::host::KNOWN_ARCH`], checked when
/// the file is read.
///
/// An empty block (absent, or `{}`) means every host is supported. A
/// non-empty block lists the supported hosts: a host is supported when
/// its `<os>` or `<os>-<arch>` key is present.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(transparent)]
pub struct Platforms(pub BTreeMap<String, Overlay>);

impl Platforms {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Absent block = every host supported. Present = the host's `<os>`
    /// key or `<os>-<arch>` key exists.
    pub fn supports(&self, host: &Host) -> bool {
        self.0.is_empty() || host.platform_keys().iter().any(|k| self.0.contains_key(k))
    }

    /// Overlays for this host in merge order: `<os>` then `<os>-<arch>`.
    pub fn overlays_for(&self, host: &Host) -> Vec<&Overlay> {
        host.platform_keys()
            .iter()
            .filter_map(|k| self.0.get(k))
            .collect()
    }

    /// Every key is `<os>` or `<os>-<arch>` with a known os and arch.
    pub fn validate(&self) -> Result<(), ManifestError> {
        match self.0.keys().find(|k| !is_valid_platform_key(k)) {
            Some(bad) => Err(ManifestError::BadPlatformKey(bad.clone())),
            None => Ok(()),
        }
    }
}

// Hand-written so a misspelt key (`linx`) fails at load instead of
// silently making the module unsupported everywhere.
impl<'de> Deserialize<'de> for Platforms {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let platforms = Platforms(BTreeMap::deserialize(d)?);
        platforms.validate().map_err(serde::de::Error::custom)?;
        Ok(platforms)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ManifestError {
    #[error(
        "platforms: unknown key {0:?} (expected <os> or <os>-<arch>, os in linux|windows|macos, \
         arch in x86_64|aarch64)"
    )]
    BadPlatformKey(String),
    #[error("scripts.start must be exactly one command")]
    StartNotSingle,
    #[error("language {0} has no start command (unity modules launch from the editor)")]
    NoStart(Language),
    #[error("module {0:?} is not supported on {1}")]
    Unsupported(String, String),
}

/// Fully merged launch facts for one module on one host, before
/// placeholder expansion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub build: Vec<Argv>,
    pub start: Argv,
    pub stop: Vec<Argv>,
    pub test: Vec<Argv>,
    pub env: BTreeMap<String, String>,
    pub requires: Vec<String>,
    /// Relative path of the built binary, the value of `{bin}` once
    /// expanded (it may use `{module}` and `{exe}`). Empty for python and
    /// flutter.
    pub bin: String,
}

/// The language defaults for `lang` on `host`, raw (before expansion).
///
/// | language | build | start | bin | test |
/// | -------- | ----- | ----- | --- | ---- |
/// | rust | `cargo build --release` | `{bin} {args}` | `target/release/{module}{exe}` | `cargo test` |
/// | cpp | `cmake -S . -B build -DCMAKE_BUILD_TYPE=Release`, `cmake --build build --config Release` | `{bin} {args}` | `build/{module}`; Windows `build/Release/{module}.exe` | none |
/// | python | none | `{python} main.py {args}` | empty | none |
/// | flutter | none | `flutter run -d {os}` | empty | none |
/// | unity | none | none | empty | none |
///
/// Unity's empty `start` is what makes [`ModuleDevConfig::resolve`]
/// return [`ManifestError::NoStart`] unless the manifest declares one.
pub fn language_defaults(lang: Language, host: &Host) -> Resolved {
    let cpp_bin = if host.os == "windows" {
        "build/Release/{module}.exe"
    } else {
        "build/{module}"
    };
    let none = Resolved {
        build: Vec::new(),
        start: Vec::new(),
        stop: Vec::new(),
        test: Vec::new(),
        env: BTreeMap::new(),
        requires: Vec::new(),
        bin: String::new(),
    };
    match lang {
        Language::Rust => Resolved {
            build: vec![argv(&["cargo", "build", "--release"])],
            start: argv(&["{bin}", "{args}"]),
            test: vec![argv(&["cargo", "test"])],
            bin: "target/release/{module}{exe}".to_owned(),
            ..none
        },
        Language::Cpp => Resolved {
            build: vec![
                argv(&[
                    "cmake",
                    "-S",
                    ".",
                    "-B",
                    "build",
                    "-DCMAKE_BUILD_TYPE=Release",
                ]),
                argv(&["cmake", "--build", "build", "--config", "Release"]),
            ],
            start: argv(&["{bin}", "{args}"]),
            bin: cpp_bin.to_owned(),
            ..none
        },
        Language::Python => Resolved {
            start: argv(&["{python}", "main.py", "{args}"]),
            ..none
        },
        Language::Flutter => Resolved {
            start: argv(&["flutter", "run", "-d", "{os}"]),
            ..none
        },
        Language::Unity => none,
    }
}

fn argv(tokens: &[&str]) -> Argv {
    tokens.iter().copied().map(str::to_owned).collect()
}

/// Where a module's virtualenv interpreter lives, relative to the module
/// root. The caller uses it for `{python}` when the file exists.
pub fn python_venv_relpath(host: &Host) -> &'static str {
    if host.os == "windows" {
        ".venv/Scripts/python.exe"
    } else {
        ".venv/bin/python"
    }
}

/// The interpreter `{python}` falls back to when there is no virtualenv.
pub fn python_fallback(host: &Host) -> &'static str {
    if host.os == "windows" {
        "python"
    } else {
        "python3"
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
    // Read in both forms like the dev manifest; still written as the
    // list, since this file ships to consumers outside sb.
    #[serde(
        default,
        deserialize_with = "ports_from_map_or_list",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub publishers: Vec<PubSpec>,
    #[serde(
        default,
        deserialize_with = "ports_from_map_or_list",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub subscribers: Vec<SubSpec>,
    #[serde(
        default,
        deserialize_with = "services_from_any",
        skip_serializing_if = "Vec::is_empty"
    )]
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
///
/// On disk `services:` is a list of names. A map keyed by name (each
/// value empty, or a map whose optional `name:` equals the key) and the
/// 0.7 list of `name:` maps are also read. The manifest writes the list
/// of names.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceSpec {
    pub name: String,
}

fn services_from_any<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<ServiceSpec>, D::Error> {
    d.deserialize_any(ServicesVisitor)
}

fn services_to_list<S: serde::Serializer>(
    services: &[ServiceSpec],
    s: S,
) -> Result<S::Ok, S::Error> {
    s.collect_seq(services.iter().map(|svc| svc.name.as_str()))
}

/// `services:` as a list (of names or `name:` maps), a map keyed by
/// name, or nothing.
struct ServicesVisitor;

impl<'de> serde::de::Visitor<'de> for ServicesVisitor {
    type Value = Vec<ServiceSpec>;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a list of service names, or a map keyed by service name")
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
        Ok(Vec::new())
    }

    fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
        Ok(Vec::new())
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut out = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(service) = seq.next_element_seed(ServiceSeed { key: None })? {
            out.push(service);
        }
        Ok(out)
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut out: Vec<ServiceSpec> = Vec::new();
        while let Some(key) = map.next_key::<String>()? {
            if out.iter().any(|svc| svc.name == key) {
                return Err(duplicate_key(&key));
            }
            out.push(map.next_value_seed(ServiceSeed { key: Some(key) })?);
        }
        Ok(out)
    }
}

/// One `services:` entry. `key` is its map key in the map form (the
/// value is then empty or a map with an optional `name:`), `None` in the
/// list form (a name, or a map with `name:`).
struct ServiceSeed {
    key: Option<String>,
}

/// The map spelling of one service, in either form.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ServiceMapValue {
    #[serde(default)]
    name: Option<String>,
}

impl<'de> serde::de::DeserializeSeed<'de> for ServiceSeed {
    type Value = ServiceSpec;

    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<ServiceSpec, D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> serde::de::Visitor<'de> for ServiceSeed {
    type Value = ServiceSpec;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.key.is_some() {
            f.write_str("nothing, or a map with an optional name:")
        } else {
            f.write_str("a service name, or a map with name:")
        }
    }

    fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<ServiceSpec, E> {
        if self.key.is_some() {
            return Err(E::invalid_type(
                serde::de::Unexpected::Str(s),
                &"nothing, or a map with an optional name:",
            ));
        }
        Ok(ServiceSpec { name: s.to_owned() })
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<ServiceSpec, E> {
        let Some(name) = self.key else {
            return Err(E::invalid_type(
                serde::de::Unexpected::Unit,
                &"a service name, or a map with name:",
            ));
        };
        Ok(ServiceSpec { name })
    }

    fn visit_none<E: serde::de::Error>(self) -> Result<ServiceSpec, E> {
        let Some(name) = self.key else {
            return Err(E::invalid_type(
                serde::de::Unexpected::Option,
                &"a service name, or a map with name:",
            ));
        };
        Ok(ServiceSpec { name })
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, map: A) -> Result<ServiceSpec, A::Error> {
        let inner =
            ServiceMapValue::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
        let name = settle_name::<A::Error>(self.key, inner.name)?;
        Ok(ServiceSpec { name })
    }
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
// Serde helpers for keys read as a map or a list
// ─────────────────────────────────────────────────────────────────────

/// A key present with no value (`modules:`) reads as the default, like
/// an absent key.
fn null_as_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// The error for a map key seen twice. `serde_yaml` reports duplicates
/// only for its own `Mapping`, so the hand-written map visitors check,
/// with the same wording.
fn duplicate_key<E: serde::de::Error>(key: &str) -> E {
    E::custom(format!("duplicate entry with key {key:?}"))
}

/// The name of an entry: its map key in the map form, its `name:` in the
/// list form. In the map form an inner `name:` must equal the key.
fn settle_name<E: serde::de::Error>(
    key: Option<String>,
    inner: Option<String>,
) -> Result<String, E> {
    match (key, inner) {
        (Some(key), Some(inner)) if inner != key => {
            Err(E::custom(format!("name {inner} does not match the key")))
        }
        (Some(key), _) => Ok(key),
        (None, Some(inner)) => Ok(inner),
        (None, None) => Err(E::missing_field("name")),
    }
}

// ─────────────────────────────────────────────────────────────────────
// Pub / Sub specs (shared between dev and prd configs)
// ─────────────────────────────────────────────────────────────────────

/// One `publishers:` entry. On disk it carries at most one of `topic:`
/// (bare, owned by the module) or `key:` (verbatim wire key); with
/// neither, `name` is the bare topic. `topic:` is always written back.
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
    /// Absent in the map form, where the key is the name (an inner
    /// `name:` must equal it); required in the list form. Never written
    /// in the map form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
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
    /// Neither `topic:` nor `key:`, and `name` is not a valid bare topic.
    #[error(
        "name {name:?} cannot double as the topic (allowed: [A-Za-z0-9_-]); add topic: or key:"
    )]
    NameNotTopic { name: String },
    #[error(transparent)]
    Rule(#[from] SpecRuleError),
    /// No name from a map key or `name:`. The visitors report the list
    /// form's missing `name:` themselves, so only a [`RawSpec`] built
    /// elsewhere reaches this.
    #[error("missing field `name`")]
    MissingName,
}

impl RawSpec {
    /// Apply the load-time rules:
    /// - `topic: /a/b/...` (legacy leading slash) reads as `Key("a/b/...")`,
    ///   which is the wire key sb 0.4.x produced for it;
    /// - `topic: a/b` (inner `/`, no leading `/`) is refused, see
    ///   [`TopicRefError::LegacyOwnedWithSlash`];
    /// - `topic: hello` is `Owned`, `key: ...` is `Key`;
    /// - neither is `Owned(name)`, provided every char of `name` is valid
    ///   in a wire segment (since 0.6.0; serialization still writes the
    ///   `topic:` back, so the file reads on older sb);
    /// - both is an error.
    fn topic_ref(&self) -> Result<TopicRef, RawSpecError> {
        match (self.topic.as_deref(), self.key.as_deref()) {
            (Some(_), Some(_)) => Err(TopicRefError::BothTopicAndKey.into()),
            (None, None) => {
                let name = self.name.as_deref().unwrap_or_default();
                if name.is_empty() || !name.chars().all(is_valid_segment_char) {
                    return Err(RawSpecError::NameNotTopic {
                        name: name.to_owned(),
                    });
                }
                TopicRef::owned(name).map_err(|_| RawSpecError::EmptyTopic)
            }
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
            name: raw.name.ok_or(RawSpecError::MissingName)?,
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
            name: raw.name.ok_or(RawSpecError::MissingName)?,
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
            name: Some(p.name),
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
            name: Some(s.name),
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

/// What a ports key deserializes into: checked entries, or (for the
/// loose scan in [`legacy_topics_in`]) the raw ones.
trait FromRawSpec: Sized {
    fn from_raw_spec(raw: RawSpec) -> Result<Self, RawSpecError>;
}

impl FromRawSpec for PubSpec {
    fn from_raw_spec(raw: RawSpec) -> Result<Self, RawSpecError> {
        Self::try_from(raw)
    }
}

impl FromRawSpec for SubSpec {
    fn from_raw_spec(raw: RawSpec) -> Result<Self, RawSpecError> {
        Self::try_from(raw)
    }
}

impl FromRawSpec for RawSpec {
    fn from_raw_spec(raw: RawSpec) -> Result<Self, RawSpecError> {
        Ok(raw)
    }
}

/// Deserializes one pub/sub entry and converts it inside the visit, so a
/// `topic:`/`key:` or `type:`/`encoding:` error carries the entry's own
/// path (`publishers[0]: ...` in the list form, `publishers.markers: ...`
/// in the map form) rather than the enclosing key's.
///
/// `key` is the entry's map key in the map form, which becomes its name;
/// an inner `name:` must then equal it. The list form passes `None` and
/// requires `name:`.
struct SpecVisitor<T> {
    key: Option<String>,
    marker: std::marker::PhantomData<T>,
}

impl<T> SpecVisitor<T> {
    fn new(key: Option<String>) -> Self {
        Self {
            key,
            marker: std::marker::PhantomData,
        }
    }
}

impl<'de, T: FromRawSpec> serde::de::DeserializeSeed<'de> for SpecVisitor<T> {
    type Value = T;

    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<T, D::Error> {
        d.deserialize_struct("PortEntry", SPEC_FIELDS, self)
    }
}

impl<'de, T: FromRawSpec> serde::de::Visitor<'de> for SpecVisitor<T> {
    type Value = T;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a pub/sub entry with type and transport, and optional topic or key")
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, map: A) -> Result<T, A::Error> {
        let mut raw = RawSpec::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
        raw.name = Some(settle_name::<A::Error>(self.key, raw.name.take())?);
        T::from_raw_spec(raw).map_err(serde::de::Error::custom)
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, seq: A) -> Result<T, A::Error> {
        let mut raw = RawSpec::deserialize(serde::de::value::SeqAccessDeserializer::new(seq))?;
        raw.name = Some(settle_name::<A::Error>(self.key, raw.name.take())?);
        T::from_raw_spec(raw).map_err(serde::de::Error::custom)
    }
}

impl<'de> Deserialize<'de> for PubSpec {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        serde::de::DeserializeSeed::deserialize(SpecVisitor::<Self>::new(None), d)
    }
}

impl<'de> Deserialize<'de> for SubSpec {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        serde::de::DeserializeSeed::deserialize(SpecVisitor::<Self>::new(None), d)
    }
}

fn ports_from_map_or_list<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: FromRawSpec,
{
    d.deserialize_any(PortsVisitor(std::marker::PhantomData))
}

/// `publishers:` / `subscribers:` as a map keyed by entry name, the 0.7
/// list of entries with `name:`, or nothing.
struct PortsVisitor<T>(std::marker::PhantomData<T>);

impl<'de, T: FromRawSpec> serde::de::Visitor<'de> for PortsVisitor<T> {
    type Value = Vec<T>;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a map of entries keyed by name, or a list of entries with name:")
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<Vec<T>, E> {
        Ok(Vec::new())
    }

    fn visit_none<E: serde::de::Error>(self) -> Result<Vec<T>, E> {
        Ok(Vec::new())
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<T>, A::Error> {
        let mut out: Vec<T> = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(entry) = seq.next_element_seed(SpecVisitor::<T>::new(None))? {
            out.push(entry);
        }
        Ok(out)
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Vec<T>, A::Error> {
        let mut out: Vec<T> = Vec::with_capacity(map.size_hint().unwrap_or(0));
        let mut seen: BTreeSet<String> = BTreeSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !seen.insert(key.clone()) {
                return Err(duplicate_key(&key));
            }
            out.push(map.next_value_seed(SpecVisitor::<T>::new(Some(key)))?);
        }
        Ok(out)
    }
}

/// The ports as a map from name to the entry's other keys, in list
/// order. `topic:` is still written when it equals the name.
fn ports_to_map<S, T>(ports: &[T], s: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
    T: Clone + Into<RawSpec>,
{
    let mut map = s.serialize_map(Some(ports.len()))?;
    for port in ports {
        let mut raw: RawSpec = port.clone().into();
        let name = raw.name.take().unwrap_or_default();
        map.serialize_entry(&name, &raw)?;
    }
    map.end()
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

/// Scan raw module manifest text for legacy `topic: /...` entries.
///
/// Uses a loose parse that ignores every other top-level key, so it works
/// on files the strict [`ModuleDevConfig`] would reject; both the map and
/// the list form of the ports are read. Never errors: text that does not
/// parse yields an empty list.
pub fn legacy_topics_in(dev_yml_text: &str) -> Vec<LegacyTopic> {
    #[derive(Deserialize)]
    struct Loose {
        #[serde(default, deserialize_with = "ports_from_map_or_list")]
        publishers: Vec<RawSpec>,
        #[serde(default, deserialize_with = "ports_from_map_or_list")]
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
                name: spec.name.unwrap_or_default(),
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
// Workspace — `sb.workspace.yaml`
// ─────────────────────────────────────────────────────────────────────

/// The workspace inventory: its name, the modules it owns, and the
/// message directories it imports. Which modules run, how many copies of
/// each, and how they are wired belongs to a [`Flow`].
///
/// Two copies of this shape exist:
///
/// - the **repo copy**, [`WORKSPACE_FILE`] at a repository root, written
///   by hand (and by `sb init`). Module and message paths are relative to
///   that file's directory; `name:` optionally names the workspace.
/// - the **installed copy** under `<sb_home>/workspaces/<name>/`, derived
///   from the repo copy on install. Every path is absolute and `source:`
///   records where it came from.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workspace {
    /// Workspace name. Absent = the repo copy's directory name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The sb version the workspace needs (`">=0.8"`). Free text; not
    /// enforced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sb: Option<String>,
    /// Module name to its directory (or its manifest's path). A
    /// `modules:` key with no value reads as empty.
    #[serde(default, deserialize_with = "null_as_default")]
    pub modules: BTreeMap<String, PathBuf>,
    /// Message directories imported into the vault on install. On disk a
    /// map keyed by [`MessageSource::label`]; the 0.7 list is also read,
    /// and a `messages:` key with no value reads as empty.
    #[serde(
        default,
        deserialize_with = "messages_from_map_or_list",
        serialize_with = "messages_to_map",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub messages: Vec<MessageSource>,
    /// Provenance of an installed copy. Never present in a repo copy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<WorkspaceSource>,
}

impl Workspace {
    /// Validate the workspace name, every module name (an identifier,
    /// like a manifest's `module:`), every module path (non-empty), and
    /// every message source. Call after deserializing so bad input fails
    /// at load time, not at launch time.
    pub fn validate(&self) -> Result<(), FlowValidationError> {
        if let Some(name) = &self.name {
            validate_identifier(name).map_err(|e| FlowValidationError::BadName {
                name: name.clone(),
                source: e,
            })?;
        }
        for (module, path) in &self.modules {
            validate_identifier(module).map_err(|e| FlowValidationError::BadModuleName {
                module: module.clone(),
                source: e,
            })?;
            if path.as_os_str().is_empty() {
                return Err(FlowValidationError::EmptyModulePath {
                    module: module.clone(),
                });
            }
        }
        for (index, src) in self.messages.iter().enumerate() {
            src.validate(index)?;
        }
        Ok(())
    }
}

/// Where an installed workspace came from. Written only into the
/// installed copy under `<sb_home>/workspaces/<name>/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceSource {
    /// Absolute path of the repo copy that was installed.
    pub repo_workspace: PathBuf,
    /// Git URL the repo was cloned from; enables `update_from_source`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// RFC 3339 UTC time of the install, e.g. `2026-10-13T08:35:23Z`.
    pub installed_at: String,
}

/// One `messages:` entry in the workspace: a directory of `.proto` files
/// imported into the vault under `style`.
///
/// `path` holds `<ns>/<Leaf>.proto` files, or flat `<Leaf>.proto` files
/// (namespace = the directory's own name). `backends` picks what
/// `sb message compile` emits for it, among `proto`, `fb`, `iox2`;
/// empty means the style's own default.
///
/// On disk `messages:` is a map from `label` to the entry's `style`,
/// `path`, and `backends`. The 0.7 list of entries is also read; there
/// `label:` is optional and defaults to the last component of `path`,
/// with `_2`, `_3`, ... appended when that label is already taken.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MessageSource {
    /// The entry's key under `messages:`. It only names the entry;
    /// nothing resolves through it.
    pub label: String,
    pub style: String,
    pub path: PathBuf,
    #[serde(skip_serializing_if = "Vec::is_empty")]
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

/// The on-disk fields of a [`MessageSource`], in either form.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MessageSourceRaw {
    #[serde(default)]
    label: Option<String>,
    style: String,
    path: PathBuf,
    #[serde(default)]
    backends: Vec<String>,
}

impl MessageSourceRaw {
    fn into_source(self, label: String) -> MessageSource {
        MessageSource {
            label,
            style: self.style,
            path: self.path,
            backends: self.backends,
        }
    }
}

/// The label of a list-form entry without `label:`: the last component
/// of its path, or the whole path when it has none (`.`, `..`).
fn default_message_label(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |leaf| leaf.to_string_lossy().into_owned(),
    )
}

/// `base`, or `base_2`, `base_3`, ... whichever is first not taken by an
/// entry already in `taken`.
fn unused_message_label(taken: &[MessageSource], base: String) -> String {
    let free = |label: &str| taken.iter().all(|m| m.label != label);
    if free(&base) {
        return base;
    }
    let mut n = 2_usize;
    loop {
        let candidate = format!("{base}_{n}");
        if free(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

// One list-form element: `label:` optional, defaulting to the path's
// last component.
impl<'de> Deserialize<'de> for MessageSource {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let mut raw = MessageSourceRaw::deserialize(d)?;
        let label = raw
            .label
            .take()
            .unwrap_or_else(|| default_message_label(&raw.path));
        Ok(raw.into_source(label))
    }
}

fn messages_from_map_or_list<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<MessageSource>, D::Error> {
    d.deserialize_any(MessagesVisitor)
}

/// `messages:` as a map keyed by label, the 0.7 list, or nothing.
struct MessagesVisitor;

impl<'de> serde::de::Visitor<'de> for MessagesVisitor {
    type Value = Vec<MessageSource>;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a map of message sources keyed by label, or a list of message sources")
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
        Ok(Vec::new())
    }

    fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
        Ok(Vec::new())
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut out: Vec<MessageSource> = Vec::new();
        while let Some(mut raw) = seq.next_element::<MessageSourceRaw>()? {
            let label = match raw.label.take() {
                Some(label) if out.iter().any(|m| m.label == label) => {
                    return Err(duplicate_key(&label));
                }
                Some(label) => label,
                None => unused_message_label(&out, default_message_label(&raw.path)),
            };
            out.push(raw.into_source(label));
        }
        Ok(out)
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut out: Vec<MessageSource> = Vec::new();
        while let Some(label) = map.next_key::<String>()? {
            if out.iter().any(|m| m.label == label) {
                return Err(duplicate_key(&label));
            }
            out.push(map.next_value_seed(MessageSeed { label })?);
        }
        Ok(out)
    }
}

/// One map-form `messages:` value; its key is the label. Checked inside
/// the value so an error reads `messages.<label>: ...`.
struct MessageSeed {
    label: String,
}

impl<'de> serde::de::DeserializeSeed<'de> for MessageSeed {
    type Value = MessageSource;

    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<MessageSource, D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de> serde::de::Visitor<'de> for MessageSeed {
    type Value = MessageSource;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a message source with style:, path:, and optional backends:")
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, map: A) -> Result<MessageSource, A::Error> {
        let mut raw =
            MessageSourceRaw::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
        if let Some(inner) = raw.label.take().filter(|inner| *inner != self.label) {
            return Err(serde::de::Error::custom(format!(
                "label {inner} does not match the key"
            )));
        }
        Ok(raw.into_source(self.label))
    }
}

fn messages_to_map<S: serde::Serializer>(
    sources: &[MessageSource],
    s: S,
) -> Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    struct Value<'a> {
        style: &'a str,
        path: &'a Path,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        backends: &'a Vec<String>,
    }

    let mut map = s.serialize_map(Some(sources.len()))?;
    for src in sources {
        let value = Value {
            style: &src.style,
            path: &src.path,
            backends: &src.backends,
        };
        map.serialize_entry(&src.label, &value)?;
    }
    map.end()
}

// ─────────────────────────────────────────────────────────────────────
// Flow — `sb.flow.yaml`, `<name>.flow.yaml`
// ─────────────────────────────────────────────────────────────────────

/// One launchable graph over the workspace's modules: which modules run
/// (`nodes`), how many copies of each (`instances`), per-copy settings
/// (`instance_settings`), and which publisher feeds which subscribers
/// (`wires`).
///
/// A workspace has any number of flows, one per `*.flow.yaml` at the
/// repository root; [`DEFAULT_FLOW_FILE`] holds the flow named
/// [`DEFAULT_FLOW_NAME`]. `sb up [FLOW]` launches one flow's nodes.
///
/// An instance is one tmux window under `sb up` and one node in the L6
/// graph editor. Its id is the suffix appended to the module on the wire
/// and in the window name (`<module>-<id>`, see
/// [`crate::topic::instance_module`]).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Flow {
    /// Flow name. Absent = the file's flow name ([`flow_name_of`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The workspace modules this flow launches, each listed once.
    #[serde(default, deserialize_with = "null_as_default")]
    pub nodes: Vec<String>,
    /// Node to its instance ids. A node absent here runs as one
    /// un-suffixed process.
    #[serde(
        default,
        deserialize_with = "null_as_default",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub instances: BTreeMap<String, Vec<String>>,
    /// Node to instance id to that instance's settings.
    #[serde(
        default,
        deserialize_with = "null_as_default",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub instance_settings: BTreeMap<String, BTreeMap<String, InstanceSettings>>,
    /// Publisher-first links, in file order. On disk a map from
    /// `<module>[<selector>].<entry>` to one `<module>.<entry>` or a list
    /// of them.
    #[serde(
        default,
        deserialize_with = "wires_from_map",
        serialize_with = "wires_to_map",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub wires: Vec<Wire>,
}

impl Flow {
    /// Whether `module` is one of this flow's nodes.
    pub fn is_node(&self, module: &str) -> bool {
        self.nodes.iter().any(|n| n == module)
    }

    /// The node's instance ids; empty for a singleton or a module that is
    /// not a node.
    pub fn instances_for(&self, node: &str) -> &[String] {
        self.instances.get(node).map_or(&[][..], Vec::as_slice)
    }

    /// The settings of instance `id` of `node`, if any are declared.
    pub fn settings_for(&self, node: &str, id: &str) -> Option<&InstanceSettings> {
        self.instance_settings.get(node)?.get(id)
    }

    /// One [`ModuleInstance`] per instance of `node`, in declaration
    /// order, carrying its settings (or none). Empty for a singleton.
    pub fn resolved_instances(&self, node: &str) -> Vec<ModuleInstance> {
        self.instances_for(node)
            .iter()
            .map(|id| {
                let settings = self.settings_for(node, id).cloned().unwrap_or_default();
                ModuleInstance {
                    id: id.clone(),
                    label: settings.label,
                    args: settings.args,
                    env: settings.env,
                }
            })
            .collect()
    }

    /// Check the flow on its own, without the manifests:
    ///
    /// - every node is an identifier and listed once;
    /// - every `instances` and `instance_settings` key is a node;
    /// - instance ids pass [`validate_instance_id`] and are unique per
    ///   node;
    /// - every `instance_settings` id is one of that node's instances;
    /// - every wire parses, both of its sides name nodes, and a `[{id}]`
    ///   selector has only subscribers with instances.
    ///
    /// Whether the wired entries exist, and their transports and types,
    /// are `sb doctor`'s checks.
    pub fn validate(&self) -> Result<(), FlowValidationError> {
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for node in &self.nodes {
            validate_identifier(node).map_err(|e| FlowValidationError::BadModuleName {
                module: node.clone(),
                source: e,
            })?;
            if !seen.insert(node.as_str()) {
                return Err(FlowValidationError::DuplicateNode(node.clone()));
            }
        }
        for (node, ids) in &self.instances {
            if !self.is_node(node) {
                return Err(FlowValidationError::UnknownNode(
                    "instances".to_owned(),
                    node.clone(),
                ));
            }
            validate_instance_list(node, ids)?;
        }
        for (node, by_id) in &self.instance_settings {
            if !self.is_node(node) {
                return Err(FlowValidationError::UnknownNode(
                    "instance_settings".to_owned(),
                    node.clone(),
                ));
            }
            let ids = self.instances_for(node);
            if let Some(id) = by_id.keys().find(|id| !ids.contains(id)) {
                return Err(FlowValidationError::SettingsForUnknownInstance(
                    node.clone(),
                    id.clone(),
                ));
            }
        }
        self.wires
            .iter()
            .try_for_each(|wire| self.validate_wire(wire))
    }

    fn validate_wire(&self, wire: &Wire) -> Result<(), FlowValidationError> {
        let lhs = wire.source.to_string();
        let rhs: Vec<String> = wire.sinks.iter().map(ToString::to_string).collect();
        // Grammar again, for wires built in code rather than parsed.
        parse_wire(&lhs, &rhs)?;
        let modules =
            std::iter::once(&wire.source.module).chain(wire.sinks.iter().map(|s| &s.module));
        for module in modules {
            if !self.is_node(module) {
                return Err(FlowValidationError::UnknownNode(
                    format!("wires.{lhs}"),
                    module.clone(),
                ));
            }
        }
        if wire.source.selector == WireSelector::SameId {
            if let Some(sink) = wire
                .sinks
                .iter()
                .find(|s| self.instances_for(&s.module).is_empty())
            {
                return Err(FlowValidationError::BadWire(
                    lhs,
                    format!("{{id}} but {} has no instances", sink.module),
                ));
            }
        }
        Ok(())
    }
}

/// Every id in one node's instance list is valid and unique.
fn validate_instance_list(node: &str, ids: &[String]) -> Result<(), FlowValidationError> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for id in ids {
        validate_instance_id(id).map_err(|e| FlowValidationError::BadInstanceId {
            module: node.to_owned(),
            id: id.clone(),
            source: e,
        })?;
        if !seen.insert(id.as_str()) {
            return Err(FlowValidationError::DuplicateInstanceId {
                module: node.to_owned(),
                id: id.clone(),
            });
        }
    }
    Ok(())
}

/// One `instance_settings.<node>.<id>` value. Every key is optional.
///
/// `label` is a free-form display string for the L6 graph editor (absent
/// = the id). `args` are appended to the start command and `env` is
/// merged over the manifest's, for this instance only. On disk `args` is
/// one string, split like a script line ([`Script::parse_line`]), or a
/// list with one argument per item.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(
        default,
        deserialize_with = "argv_from_str_or_list",
        serialize_with = "argv_to_str",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

impl InstanceSettings {
    /// True when no key is set.
    pub fn is_empty(&self) -> bool {
        self.label.is_none() && self.args.is_empty() && self.env.is_empty()
    }
}

fn argv_from_str_or_list<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    d.deserialize_any(ArgvVisitor)
}

/// `args:` as one line split like a script line, a list of arguments
/// taken verbatim, or nothing.
struct ArgvVisitor;

impl<'de> serde::de::Visitor<'de> for ArgvVisitor {
    type Value = Vec<String>;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("an argument string, or a list of arguments")
    }

    fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<Vec<String>, E> {
        if s.trim().is_empty() {
            return Ok(Vec::new());
        }
        Script::parse_line(s).map_err(E::custom)
    }

    fn visit_u64<E: serde::de::Error>(self, n: u64) -> Result<Vec<String>, E> {
        Ok(vec![n.to_string()])
    }

    fn visit_i64<E: serde::de::Error>(self, n: i64) -> Result<Vec<String>, E> {
        Ok(vec![n.to_string()])
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<Vec<String>, E> {
        Ok(Vec::new())
    }

    fn visit_none<E: serde::de::Error>(self) -> Result<Vec<String>, E> {
        Ok(Vec::new())
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<String>, A::Error> {
        let mut out = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(arg) = seq.next_element::<String>()? {
            out.push(arg);
        }
        Ok(out)
    }
}

/// One string when every argument is a plain token (so
/// [`Script::parse_line`] splits it back unchanged), a list otherwise.
fn argv_to_str<S: serde::Serializer>(args: &[String], s: S) -> Result<S::Ok, S::Error> {
    if args.iter().all(|a| is_plain_token(a)) {
        s.serialize_str(&args.join(" "))
    } else {
        s.collect_seq(args)
    }
}

/// One instance of a node, as the launcher sees it: the id plus its
/// [`InstanceSettings`]. Built by [`Flow::resolved_instances`]; never
/// read or written directly.
///
/// `id` is the suffix appended to the module on the wire and in the
/// window name (`<module>-<id>`, e.g. `cam_gige_ht-left`; see
/// [`crate::topic::instance_module`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleInstance {
    pub id: String,
    pub label: Option<String>,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

impl ModuleInstance {
    /// An instance with only `id` set.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: None,
            args: Vec::new(),
            env: BTreeMap::new(),
        }
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
    /// A workspace `modules:` key or a flow node that is not an
    /// identifier.
    #[error("module name {module:?} is invalid: {source}")]
    BadModuleName {
        module: String,
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
    /// `(flow_item, name)`: an `instances` or `instance_settings` key, or
    /// a module named by `wires.<lhs>`, that is not a node of the flow.
    #[error("{0}: unknown node {1}")]
    UnknownNode(String, String),
    /// A node listed more than once under `nodes:`.
    #[error("nodes: {0} is listed more than once")]
    DuplicateNode(String),
    /// `(node, id)`: settings for an id that is not one of the node's
    /// instances.
    #[error("instance_settings.{0}: {1} is not an instance of {0}")]
    SettingsForUnknownInstance(String, String),
    /// `(lhs, reason)`: the wire's key and what is wrong with it.
    #[error("wires.{0}: {1}")]
    BadWire(String, String),
    /// A `sb.flow.yaml` that [`probe_legacy_flow_shape`] took for the 0.7
    /// shape but that does not parse as one. Carries the parse error.
    #[error("sb.flow.yaml in the 0.7 shape does not parse: {0}")]
    LegacyFlowShape(String),
}

// ─────────────────────────────────────────────────────────────────────
// Wires — `wires:` in a flow
// ─────────────────────────────────────────────────────────────────────

/// One `wires:` entry: a publisher entry and every subscriber entry it
/// feeds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wire {
    pub source: WireSource,
    /// One or more; never empty in a parsed wire.
    pub sinks: Vec<WireEnd>,
}

/// A subscriber side, `<module>.<entry>`. Takes no selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireEnd {
    pub module: String,
    pub entry: String,
}

/// Which instances of the publisher module a wire reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireSelector {
    /// No brackets: the publisher is a singleton.
    None,
    /// `[{id}]`: each subscriber instance reads the publisher instance
    /// with its own id.
    SameId,
    /// `[*]`: every instance.
    All,
    /// `[<id>]`: one named instance.
    Id(String),
}

/// The publisher side, `<module>[<selector>].<entry>` or
/// `<module>.<entry>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireSource {
    pub module: String,
    pub selector: WireSelector,
    pub entry: String,
}

/// The bracketed spelling, empty for [`WireSelector::None`].
impl std::fmt::Display for WireSelector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => Ok(()),
            Self::SameId => f.write_str("[{id}]"),
            Self::All => f.write_str("[*]"),
            Self::Id(id) => write!(f, "[{id}]"),
        }
    }
}

/// `<module>[<selector>].<entry>`, the wire's key on disk.
impl std::fmt::Display for WireSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}.{}", self.module, self.selector, self.entry)
    }
}

/// `<module>.<entry>`.
impl std::fmt::Display for WireEnd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.module, self.entry)
    }
}

/// Parse one wire: `lhs` is the publisher, `<module>[<selector>].<entry>`
/// or `<module>.<entry>`; `rhs` is one or more subscribers, each
/// `<module>.<entry>`.
///
/// Each side splits on its first `.`. Modules and entries must pass
/// [`validate_identifier`]; the selector is `{id}`, `*`, or an id passing
/// [`validate_instance_id`], and only the publisher may carry one.
/// Errors are [`FlowValidationError::BadWire`].
pub fn parse_wire(lhs: &str, rhs: &[String]) -> Result<Wire, FlowValidationError> {
    parse_wire_parts(lhs, rhs)
        .map_err(|reason| FlowValidationError::BadWire(lhs.to_owned(), reason))
}

fn parse_wire_parts(lhs: &str, rhs: &[String]) -> Result<Wire, String> {
    let source = parse_wire_source(lhs)?;
    if rhs.is_empty() {
        return Err("no subscriber on the right side".to_owned());
    }
    let sinks = rhs
        .iter()
        .map(|sink| parse_wire_sink(sink))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Wire { source, sinks })
}

/// A subscriber side; a selector there is the classic mistake of the
/// 0.7 direction, so it gets its own message.
fn parse_wire_sink(sink: &str) -> Result<WireEnd, String> {
    if sink.contains(['[', ']']) {
        return Err(format!(
            "subscriber {sink:?} takes no selector; selectors go on the publisher (left) side"
        ));
    }
    parse_wire_end(sink)
}

/// A subscriber side, `<module>.<entry>`.
fn parse_wire_end(sink: &str) -> Result<WireEnd, String> {
    let (module, entry) = sink
        .split_once('.')
        .ok_or_else(|| format!("subscriber {sink:?} is not <module>.<entry>"))?;
    check_wire_ident("subscriber module", module)?;
    check_wire_ident("subscriber entry", entry)?;
    Ok(WireEnd {
        module: module.to_owned(),
        entry: entry.to_owned(),
    })
}

/// The publisher side, `<module>[<selector>].<entry>` or `<module>.<entry>`.
fn parse_wire_source(source: &str) -> Result<WireSource, String> {
    let (head, entry) = source.split_once('.').ok_or_else(|| {
        format!("publisher {source:?} is not <module>.<entry> or <module>[<selector>].<entry>")
    })?;
    let (module, selector) = if let Some((module, rest)) = head.split_once('[') {
        let sel = rest
            .strip_suffix(']')
            .ok_or_else(|| format!("publisher {source:?} has no ']' closing the selector"))?;
        (module, parse_wire_selector(sel)?)
    } else {
        (head, WireSelector::None)
    };
    check_wire_ident("publisher module", module)?;
    check_wire_ident("publisher entry", entry)?;
    Ok(WireSource {
        module: module.to_owned(),
        selector,
        entry: entry.to_owned(),
    })
}

fn parse_wire_selector(sel: &str) -> Result<WireSelector, String> {
    match sel {
        "{id}" => Ok(WireSelector::SameId),
        "*" => Ok(WireSelector::All),
        id => validate_instance_id(id)
            .map(|()| WireSelector::Id(id.to_owned()))
            .map_err(|e| format!("selector [{id}] is not {{id}}, *, or an instance id ({e})")),
    }
}

fn check_wire_ident(what: &str, s: &str) -> Result<(), String> {
    validate_identifier(s).map_err(|e| format!("{what} {s:?} is invalid: {e}"))
}

fn wires_from_map<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Wire>, D::Error> {
    d.deserialize_any(WiresVisitor)
}

/// `wires:` as a map from publisher to subscribers, or nothing.
struct WiresVisitor;

impl<'de> serde::de::Visitor<'de> for WiresVisitor {
    type Value = Vec<Wire>;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "a map from <module>[<selector>].<entry> to one <module>.<entry> or a list of them",
        )
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<Vec<Wire>, E> {
        Ok(Vec::new())
    }

    fn visit_none<E: serde::de::Error>(self) -> Result<Vec<Wire>, E> {
        Ok(Vec::new())
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Vec<Wire>, A::Error> {
        let mut out = Vec::new();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        while let Some(lhs) = map.next_key::<String>()? {
            if !seen.insert(lhs.clone()) {
                return Err(duplicate_key(&lhs));
            }
            out.push(map.next_value_seed(WireSeed { lhs })?);
        }
        Ok(out)
    }
}

/// One `wires:` value, parsed against its key inside the value so an
/// error reads `wires.<lhs>: <reason>`.
struct WireSeed {
    lhs: String,
}

impl<'de> serde::de::DeserializeSeed<'de> for WireSeed {
    type Value = Wire;

    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Wire, D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> serde::de::Visitor<'de> for WireSeed {
    type Value = Wire;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a subscriber <module>.<entry>, or a list of them")
    }

    fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<Wire, E> {
        parse_wire_parts(&self.lhs, &[s.to_owned()]).map_err(E::custom)
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Wire, A::Error> {
        let mut rhs = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(sink) = seq.next_element::<String>()? {
            rhs.push(sink);
        }
        parse_wire_parts(&self.lhs, &rhs).map_err(serde::de::Error::custom)
    }
}

fn wires_to_map<S: serde::Serializer>(wires: &[Wire], s: S) -> Result<S::Ok, S::Error> {
    let mut map = s.serialize_map(Some(wires.len()))?;
    for wire in wires {
        map.serialize_entry(&wire.source.to_string(), &WireSinks(&wire.sinks))?;
    }
    map.end()
}

/// A wire's right side on disk: one subscriber as a string, several as a
/// list.
struct WireSinks<'a>(&'a [WireEnd]);

impl Serialize for WireSinks<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if let [one] = self.0 {
            s.collect_str(one)
        } else {
            s.collect_seq(self.0.iter().map(ToString::to_string))
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// The 0.7 `sb.flow.yaml`, read for 0.8.0 only
// ─────────────────────────────────────────────────────────────────────

/// Keys only the 0.7 flow shape had at the top level.
const LEGACY_FLOW_KEYS: [&str; 3] = ["modules", "messages", "source"];

/// Whether `yaml` is a flow in the 0.7 shape: a mapping with no `nodes:`
/// key and at least one key only that shape had (`modules:`, `messages:`,
/// `source:`). Text that is not a YAML mapping gives `false`; the strict
/// [`Flow`] parse then reports what is wrong with it. Pure; no IO.
pub fn probe_legacy_flow_shape(yaml: &str) -> bool {
    let Ok(serde_yaml::Value::Mapping(map)) = serde_yaml::from_str::<serde_yaml::Value>(yaml)
    else {
        return false;
    };
    !map.contains_key("nodes") && LEGACY_FLOW_KEYS.iter().any(|key| map.contains_key(*key))
}

/// The deprecation a caller pushes when it reads a flow through
/// [`convert_legacy_flow`].
pub fn legacy_flow_deprecation() -> Deprecation {
    Deprecation::new(
        "sb.flow.yaml in the 0.7 shape",
        "split it into sb.workspace.yaml and sb.flow.yaml; see sbcli_workspace.md §Migrating",
    )
}

/// Convert a 0.7-shape `sb.flow.yaml` into the workspace and the default
/// flow it stands for. Pure; both results are validated before they are
/// returned.
///
/// - `name:`, `modules:` (paths only), `messages:`, and `source:` go to
///   the [`Workspace`]; `source.repo_flow` becomes `repo_workspace`.
/// - The nodes are the modules without `up: false`, in name order.
/// - Inline instances become `instances`; an instance's `label`, `args`,
///   and `env` become its `instance_settings`; `docker:` is dropped.
/// - `wires:` flip from `<sub>: <pub>` to publisher-first, and the
///   subscribers of one publisher spelling are grouped into one wire. A
///   wire end on an `up: false` module is dropped, since that module is
///   not a node.
pub fn convert_legacy_flow(yaml: &str) -> Result<(Workspace, Flow), FlowValidationError> {
    let legacy: LegacyFlow = serde_yaml::from_str(yaml)
        .map_err(|e| FlowValidationError::LegacyFlowShape(e.to_string()))?;
    let skipped: BTreeSet<&str> = legacy
        .modules
        .iter()
        .filter(|(_, m)| !m.up)
        .map(|(name, _)| name.as_str())
        .collect();
    let mut flow = Flow::default();
    for (name, module) in legacy.modules.iter().filter(|(_, m)| m.up) {
        flow.nodes.push(name.clone());
        if module.instances.is_empty() {
            continue;
        }
        let ids = module.instances.iter().map(|i| i.id.clone()).collect();
        flow.instances.insert(name.clone(), ids);
        let settings: BTreeMap<String, InstanceSettings> = module
            .instances
            .iter()
            .filter(|i| !i.settings.is_empty())
            .map(|i| (i.id.clone(), i.settings.clone()))
            .collect();
        if !settings.is_empty() {
            flow.instance_settings.insert(name.clone(), settings);
        }
    }
    flow.wires = flip_legacy_wires(&legacy.wires, &skipped)?;
    let workspace = Workspace {
        name: legacy.name,
        sb: None,
        modules: legacy
            .modules
            .into_iter()
            .map(|(name, m)| (name, m.path))
            .collect(),
        messages: legacy.messages,
        source: legacy.source.map(|s| WorkspaceSource {
            repo_workspace: s.repo_flow,
            url: s.url,
            installed_at: s.installed_at,
        }),
    };
    workspace.validate()?;
    flow.validate()?;
    Ok((workspace, flow))
}

/// The 0.7 `<sub>: <pub>` wires, publisher-first and grouped by publisher
/// spelling. Ends on a module in `skipped` (`up: false`) are dropped.
fn flip_legacy_wires(
    wires: &BTreeMap<String, String>,
    skipped: &BTreeSet<&str>,
) -> Result<Vec<Wire>, FlowValidationError> {
    let mut by_source: BTreeMap<String, Wire> = BTreeMap::new();
    for (lhs, rhs) in wires {
        let bad = |reason: String| FlowValidationError::BadWire(lhs.clone(), reason);
        let sink = parse_wire_end(lhs).map_err(bad)?;
        let source = parse_wire_source(rhs).map_err(bad)?;
        if skipped.contains(sink.module.as_str()) || skipped.contains(source.module.as_str()) {
            continue;
        }
        by_source
            .entry(source.to_string())
            .or_insert_with(|| Wire {
                source,
                sinks: Vec::new(),
            })
            .sinks
            .push(sink);
    }
    Ok(by_source.into_values().collect())
}

/// The 0.7 `sb.flow.yaml`, read only by [`convert_legacy_flow`].
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyFlow {
    #[serde(default)]
    name: Option<String>,
    #[serde(default, deserialize_with = "null_as_default")]
    modules: BTreeMap<String, LegacyModule>,
    #[serde(default, deserialize_with = "messages_from_map_or_list")]
    messages: Vec<MessageSource>,
    #[serde(default, deserialize_with = "null_as_default")]
    wires: BTreeMap<String, String>,
    #[serde(default)]
    source: Option<LegacySource>,
}

/// The 0.7 `source:` of an installed copy.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacySource {
    repo_flow: PathBuf,
    #[serde(default)]
    url: Option<String>,
    installed_at: String,
}

fn default_true() -> bool {
    true
}

/// One 0.7 `modules:` value: a bare path (`up: true`, no instances), or
/// a map with `path:` and optional `up:` and `instances:`.
struct LegacyModule {
    path: PathBuf,
    up: bool,
    instances: Vec<LegacyInstance>,
}

/// The map spelling of a [`LegacyModule`], parsed strictly so a typo'd
/// key is reported by name.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyModuleMap {
    path: PathBuf,
    #[serde(default = "default_true")]
    up: bool,
    #[serde(default)]
    instances: Vec<LegacyInstance>,
}

// Hand-written rather than an untagged enum, which would report every
// mismatch as "data did not match any variant" and hide an `upp:` typo.
impl<'de> Deserialize<'de> for LegacyModule {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct LegacyModuleVisitor;

        impl<'de> serde::de::Visitor<'de> for LegacyModuleVisitor {
            type Value = LegacyModule;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a module path, or a map with `path:` and optional `up:`, `instances:`")
            }

            fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<LegacyModule, E> {
                Ok(LegacyModule {
                    path: PathBuf::from(s),
                    up: true,
                    instances: Vec::new(),
                })
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                map: A,
            ) -> Result<LegacyModule, A::Error> {
                let m = LegacyModuleMap::deserialize(
                    serde::de::value::MapAccessDeserializer::new(map),
                )?;
                Ok(LegacyModule {
                    path: m.path,
                    up: m.up,
                    instances: m.instances,
                })
            }
        }

        d.deserialize_any(LegacyModuleVisitor)
    }
}

/// One 0.7 inline instance: a bare id (`left`, or a number such as `1`),
/// or a map with `id:` and optional `label:`, `args:`, `env:`, `docker:`.
struct LegacyInstance {
    id: String,
    settings: InstanceSettings,
}

/// The map spelling of a [`LegacyInstance`]. `docker:` is accepted in
/// any shape and dropped.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyInstanceMap {
    id: String,
    #[serde(default)]
    label: Option<String>,
    #[serde(default, deserialize_with = "argv_from_str_or_list")]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default, rename = "docker")]
    _docker: Option<serde::de::IgnoredAny>,
}

impl<'de> Deserialize<'de> for LegacyInstance {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct LegacyInstanceVisitor;

        impl<'de> serde::de::Visitor<'de> for LegacyInstanceVisitor {
            type Value = LegacyInstance;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(
                    "an instance id, or a map with `id:` and optional `label:`, `args:`, \
                     `env:`, `docker:`",
                )
            }

            fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<LegacyInstance, E> {
                Ok(LegacyInstance {
                    id: s.to_owned(),
                    settings: InstanceSettings::default(),
                })
            }

            fn visit_u64<E: serde::de::Error>(self, n: u64) -> Result<LegacyInstance, E> {
                Ok(LegacyInstance {
                    id: n.to_string(),
                    settings: InstanceSettings::default(),
                })
            }

            fn visit_i64<E: serde::de::Error>(self, n: i64) -> Result<LegacyInstance, E> {
                Ok(LegacyInstance {
                    id: n.to_string(),
                    settings: InstanceSettings::default(),
                })
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                map: A,
            ) -> Result<LegacyInstance, A::Error> {
                let m = LegacyInstanceMap::deserialize(
                    serde::de::value::MapAccessDeserializer::new(map),
                )?;
                Ok(LegacyInstance {
                    id: m.id,
                    settings: InstanceSettings {
                        label: m.label,
                        args: m.args,
                        env: m.env,
                    },
                })
            }
        }

        d.deserialize_any(LegacyInstanceVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expand::{ExpandCtx, ExpandError, expand_argv, expand_str};

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
    fn spec_neither_topic_nor_key_uses_name() {
        let y = dev_yml_with(
            "publishers:\n  - name: hello\n    type: std/String\n    transport: zenoh\n",
        );
        let m: ModuleDevConfig = serde_yaml::from_str(&y).unwrap();
        assert_eq!(m.publishers[0].topic, TopicRef::Owned("hello".into()));
        // `topic:` is still written back, so the file reads on sb 0.5.
        let out = serde_yaml::to_string(&m).unwrap();
        assert!(out.contains("topic: hello"), "{out}");
        let e =
            load_err("publishers:\n  - name: a.b\n    type: std/String\n    transport: zenoh\n");
        assert!(
            e.contains("publishers[0]: name \"a.b\" cannot double as the topic"),
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

    // ── the 0.7 sb.flow.yaml ──────────────────────────────────────────

    #[test]
    fn legacy_flow_splits_into_workspace_and_flow() {
        let y = concat!(
            "name: ws_ips\n",
            "modules:\n",
            "  a: apps/a\n",
            "  b:\n",
            "    path: apps/b\n",
            "    up: false\n",
            "  c:\n",
            "    path: apps/c\n",
            "  d:\n",
            "    path: apps/d\n",
            "    instances: [left, 2, { id: right, args: [--debug], env: { K: v }, ",
            "docker: { image: cam } }]\n",
            "messages:\n",
            "- style: acme\n",
            "  path: msgs/custom\n",
            "wires:\n",
            "  a.markers: d[*].markers\n",
            "  c.markers: d[*].markers\n",
            "  b.markers: d[left].markers\n",
        );
        assert!(probe_legacy_flow_shape(y));
        assert!(!probe_legacy_flow_shape("nodes: [a]\n"));
        let (ws, flow) = convert_legacy_flow(y).unwrap();
        assert_eq!(ws.name.as_deref(), Some("ws_ips"));
        assert_eq!(ws.modules.len(), 4);
        assert_eq!(ws.modules["b"], PathBuf::from("apps/b"));
        assert_eq!(ws.messages[0].label, "custom");
        // `up: false` is not a node; inline instances split into ids and
        // settings, and `docker:` is dropped.
        assert_eq!(flow.nodes, ["a", "c", "d"]);
        assert_eq!(flow.instances_for("d"), ["left", "2", "right"]);
        let right = flow.settings_for("d", "right").unwrap();
        assert_eq!(right.args, ["--debug"]);
        assert_eq!(right.env["K"], "v");
        assert!(flow.settings_for("d", "left").is_none());
        // Wires flip publisher-first and group by publisher; the wire into
        // the `up: false` module goes with it.
        assert_eq!(flow.wires.len(), 1);
        assert_eq!(flow.wires[0].source.to_string(), "d[*].markers");
        let sinks: Vec<String> = flow.wires[0]
            .sinks
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(sinks, ["a.markers", "c.markers"]);
        // A typo'd key is still reported by name.
        let e = convert_legacy_flow("modules:\n  a:\n    path: a\n    upp: no\n")
            .unwrap_err()
            .to_string();
        assert!(e.contains("unknown field `upp`"), "{e}");
    }

    // ── sb.module.yaml launch keys ────────────────────────────────────

    #[test]
    fn manifest_platform_merge_and_expansion() {
        let y = "module: ips_detect\nlanguage: rust\nenv: { RUST_LOG: info, MODE: base }\n\
                 requires: [cargo]\nplatforms:\n  linux:\n    \
                 env: { LIBCLANG_PATH: \"${LIBCLANG_PATH:-/usr/lib/llvm-18/lib}\", MODE: linux }\n    \
                 requires: [libclang-dev, cargo]\n    \
                 scripts: { test: 'cargo test --features \"a b\"' }\n  \
                 linux-x86_64: { env: { MODE: linux-x86_64 } }\n  windows: {}\n";
        let m: ModuleDevConfig = serde_yaml::from_str(y).unwrap();
        m.validate().unwrap();
        yaml_fixed_point(&m);

        // linux-x86_64: defaults <- top level <- linux <- linux-x86_64.
        let linux = Host {
            os: "linux",
            arch: "x86_64",
        };
        let on_linux = m.resolve(&linux).unwrap();
        assert_eq!(on_linux.build, [["cargo", "build", "--release"]]);
        assert_eq!(on_linux.start, ["{bin}", "{args}"]);
        assert_eq!(on_linux.test, [["cargo", "test", "--features", "a b"]]);
        assert!(on_linux.stop.is_empty());
        assert_eq!(on_linux.requires, ["cargo", "libclang-dev"]);
        assert_eq!(on_linux.env["RUST_LOG"], "info");
        assert_eq!(on_linux.env["MODE"], "linux-x86_64");

        // Expansion: `${VAR:-default}` with the variable unset and set,
        // then `{bin} {args}` for two instances.
        let unset = |_: &str| -> Option<String> { None };
        let set = |key: &str| (key == "LIBCLANG_PATH").then(|| "/opt/llvm/lib".to_owned());
        let base = ExpandCtx {
            module: &m.module,
            root: "/ws/modules/ips_detect",
            bin: "",
            exe: linux.exe_suffix(),
            id: None,
            args: &[],
            ws: "ws_ips",
            os: linux.os,
            python: "python3",
        };
        let lib = &on_linux.env["LIBCLANG_PATH"];
        assert_eq!(
            expand_str(lib, &base, &unset).unwrap(),
            "/usr/lib/llvm-18/lib"
        );
        assert_eq!(expand_str(lib, &base, &set).unwrap(), "/opt/llvm/lib");
        let bin = expand_str(&on_linux.bin, &base, &unset).unwrap();
        assert_eq!(bin, "target/release/ips_detect");
        for (id, args) in [("sb00", vec![]), ("sb01", vec!["--debug".to_owned()])] {
            let ctx = ExpandCtx {
                bin: &bin,
                id: Some(id),
                args: &args,
                ..base
            };
            let mut want = vec![bin.clone()];
            want.extend(args.iter().cloned());
            assert_eq!(expand_argv(&on_linux.start, &ctx, &unset).unwrap(), want);
            assert_eq!(
                expand_str("{module}-{id}", &ctx, &unset).unwrap(),
                format!("ips_detect-{id}")
            );
        }
        assert_eq!(
            expand_str("{id}", &base, &unset),
            Err(ExpandError::NoInstance)
        );

        // windows: the linux overlays do not apply; `{exe}` adds `.exe`.
        let windows = Host {
            os: "windows",
            arch: "x86_64",
        };
        let on_windows = m.resolve(&windows).unwrap();
        assert_eq!(on_windows.env["MODE"], "base");
        assert!(!on_windows.env.contains_key("LIBCLANG_PATH"));
        assert_eq!(on_windows.test, [["cargo", "test"]]);
        let win_ctx = ExpandCtx {
            exe: windows.exe_suffix(),
            ..base
        };
        assert_eq!(
            expand_str(&on_windows.bin, &win_ctx, &unset).unwrap(),
            "target/release/ips_detect.exe"
        );

        // macos is not listed under platforms:.
        let mac = Host {
            os: "macos",
            arch: "aarch64",
        };
        assert!(!m.supports_host(&mac));
        assert_eq!(
            m.resolve(&mac),
            Err(ManifestError::Unsupported(
                "ips_detect".into(),
                "macos-aarch64".into()
            ))
        );
    }

    #[test]
    fn workspace_validate_rejects_bad_name_style_and_backend() {
        let bad_name = Workspace {
            name: Some("my-ws".into()),
            ..Default::default()
        };
        assert!(matches!(
            bad_name.validate(),
            Err(FlowValidationError::BadName { .. })
        ));
        let bad_module = Workspace {
            modules: BTreeMap::from([("my-cam".to_owned(), PathBuf::from("cam"))]),
            ..Default::default()
        };
        assert!(matches!(
            bad_module.validate(),
            Err(FlowValidationError::BadModuleName { .. })
        ));
        let bad_style = Workspace {
            messages: vec![MessageSource {
                label: "x".into(),
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
        let bad_backend = Workspace {
            messages: vec![MessageSource {
                label: "m".into(),
                style: "acme".into(),
                path: "m".into(),
                backends: vec!["proto".into(), "capnp".into()],
            }],
            ..Default::default()
        };
        let e = bad_backend.validate().unwrap_err();
        assert!(e.to_string().contains("\"capnp\""), "{e}");
    }

    // ── 0.8 ports map form ────────────────────────────────────────────

    #[test]
    fn ports_map_and_list_forms_write_the_same_map() {
        let map_form = concat!(
            "module: ips_detect\n",
            "language: rust\n",
            "publishers:\n",
            "  markers:\n",
            "    type: ros2/custom/MarkerDetections\n",
            "    transport: zenoh\n",
            "  status:\n",
            "    topic: health\n",
            "    type: ros2/std/StringStamped\n",
            "    transport: zenoh\n",
            "subscribers:\n",
            "  image:\n",
            "    name: image\n",
            "    type: ros2/sensor_msgs/Image1280x1024\n",
            "    transport: iceoryx2\n",
            "  commands:\n",
            "    key: dev01/ops/console/zenoh/cmd\n",
            "    type: ros2/std/StringStamped\n",
            "    transport: zenoh\n",
            "services:\n",
            "  cam_param:\n",
            "  reset: {}\n",
        );
        let list_form = concat!(
            "module: ips_detect\n",
            "language: rust\n",
            "publishers:\n",
            "- name: markers\n",
            "  type: ros2/custom/MarkerDetections\n",
            "  transport: zenoh\n",
            "- name: status\n",
            "  topic: health\n",
            "  type: ros2/std/StringStamped\n",
            "  transport: zenoh\n",
            "subscribers:\n",
            "- name: image\n",
            "  type: ros2/sensor_msgs/Image1280x1024\n",
            "  transport: iceoryx2\n",
            "- name: commands\n",
            "  key: dev01/ops/console/zenoh/cmd\n",
            "  type: ros2/std/StringStamped\n",
            "  transport: zenoh\n",
            "services:\n",
            "- name: cam_param\n",
            "- reset\n",
        );
        let from_map: ModuleDevConfig = serde_yaml::from_str(map_form).unwrap();
        let from_list: ModuleDevConfig = serde_yaml::from_str(list_form).unwrap();
        assert_eq!(from_map, from_list);
        let text = serde_yaml::to_string(&from_map).unwrap();
        assert_eq!(text, serde_yaml::to_string(&from_list).unwrap());
        // The map form, in file order (not sorted), with no `name:`;
        // services as a list of names.
        assert!(
            text.contains("publishers:\n  markers:\n    topic: markers\n"),
            "{text}"
        );
        assert!(text.find("  image:") < text.find("  commands:"), "{text}");
        assert!(!text.contains("name:"), "{text}");
        assert!(text.contains("- cam_param\n"), "{text}");
        assert!(text.contains("- reset\n"), "{text}");
        yaml_fixed_point(&from_map);
        // In the map form an inner `name:` must equal the key, and errors
        // name the entry by its key.
        let e = load_err(
            "publishers:\n  markers:\n    name: other\n    type: a/b/C\n    transport: zenoh\n",
        );
        assert!(
            e.contains("publishers.markers: name other does not match the key"),
            "{e}"
        );
    }

    // ── 0.8 flows ─────────────────────────────────────────────────────

    /// `flow` with `edit` applied, which must fail validation; the error.
    fn flow_fails(flow: &Flow, edit: impl FnOnce(&mut Flow)) -> String {
        let mut broken = flow.clone();
        edit(&mut broken);
        broken.validate().unwrap_err().to_string()
    }

    #[test]
    fn flow_validate_rules() {
        let y = concat!(
            "nodes: [cam, detect, track]\n",
            "instances:\n",
            "  cam: [sb00, sb01]\n",
            "  detect: [sb00, 1]\n",
            "instance_settings:\n",
            "  detect:\n",
            "    sb00:\n",
            "      label: East wall\n",
            "      args: --debug --fps 30\n",
            "      env:\n",
            "        RUST_LOG: debug\n",
            "wires:\n",
            "  cam[{id}].image: detect.image\n",
            "  detect[*].markers: [track.markers]\n",
            "  cam[sb01].status: [track.status, detect.status]\n",
        );
        let flow: Flow = serde_yaml::from_str(y).unwrap();
        flow.validate().unwrap();
        assert_eq!(flow.instances_for("detect"), ["sb00", "1"]);
        assert!(flow.instances_for("track").is_empty());
        let detect = flow.resolved_instances("detect");
        assert_eq!(detect[0].args, ["--debug", "--fps", "30"]);
        assert_eq!(detect[0].label.as_deref(), Some("East wall"));
        assert_eq!(detect[1], ModuleInstance::new("1"));
        assert_eq!(flow.wires[0].source.selector, WireSelector::SameId);
        assert_eq!(flow.wires[2].sinks.len(), 2);
        yaml_fixed_point(&flow);

        // One broken rule at a time.
        assert_eq!(
            flow_fails(&flow, |f| f.nodes.push("cam".into())),
            "nodes: cam is listed more than once"
        );
        let e = flow_fails(&flow, |f| f.nodes.push("my-node".into()));
        assert!(e.starts_with("module name \"my-node\" is invalid"), "{e}");
        assert_eq!(
            flow_fails(&flow, |f| {
                f.instances.insert("viz".into(), vec!["a".into()]);
            }),
            "instances: unknown node viz"
        );
        let e = flow_fails(&flow, |f| {
            f.instances.get_mut("cam").unwrap().push("a b".into());
        });
        assert!(e.contains("instance id \"a b\" is invalid"), "{e}");
        let e = flow_fails(&flow, |f| {
            f.instances.get_mut("cam").unwrap().push("sb00".into());
        });
        assert!(
            e.contains("declares instance id \"sb00\" more than once"),
            "{e}"
        );
        assert_eq!(
            flow_fails(&flow, |f| {
                let detect = f.instance_settings.get_mut("detect").unwrap();
                detect.insert("sb09".into(), InstanceSettings::default());
            }),
            "instance_settings.detect: sb09 is not an instance of detect"
        );
        assert_eq!(
            flow_fails(&flow, |f| f.nodes.retain(|n| n != "track")),
            "wires.detect[*].markers: unknown node track"
        );
        assert_eq!(
            flow_fails(&flow, |f| {
                f.instances.remove("detect");
                f.instance_settings.clear();
            }),
            "wires.cam[{id}].image: {id} but detect has no instances"
        );
        // Selectors go on the publisher side only; caught at load.
        let e = serde_yaml::from_str::<Flow>("nodes: [a, b]\nwires:\n  a.x: b[*].y\n")
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("wires.a.x: subscriber \"b[*].y\" takes no selector"),
            "{e}"
        );
    }
}
