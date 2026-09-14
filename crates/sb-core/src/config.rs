//! Serde types — one Rust struct per on-disk YAML file.
//!
//! File ↔ type ↔ owner mapping (see CLAUDE.md):
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
use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

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

/// Per-module development-time interface. Gitignored on user machines
/// because `root` is an absolute path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleDevConfig {
    pub module: String,
    pub language: Language,
    pub root: PathBuf,
    #[serde(default = "default_io_dir")]
    pub io_dir: PathBuf,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub publishers: Vec<PubSpec>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subscribers: Vec<SubSpec>,
}

fn default_io_dir() -> PathBuf {
    PathBuf::from("swarmbotix_io")
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
}

// ─────────────────────────────────────────────────────────────────────
// Pub / Sub specs (shared between dev and prd configs)
// ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PubSpec {
    pub name: String,
    /// Bare topic (`image_raw`) — resolved to full form at runtime.
    pub topic: String,
    /// Vault name, e.g. `std/ImageStamped`.
    #[serde(rename = "type")]
    pub msg_type: String,
    pub transport: Transport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubSpec {
    pub name: String,
    pub topic: String,
    #[serde(rename = "type")]
    pub msg_type: String,
    pub transport: Transport,
}

// ─────────────────────────────────────────────────────────────────────
// WorkspaceFlow — `flow.yaml`
// ─────────────────────────────────────────────────────────────────────

/// Workspace module registry. Maps module name → path to its config.
/// Defined here at L1; populated by `sb init` / `sb create app` at L5.
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
    #[serde(default)]
    pub modules: BTreeMap<String, PathBuf>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub instances: BTreeMap<String, Vec<ModuleInstance>>,
}

impl WorkspaceFlow {
    /// Look up a single instance by `(module, id)`. Returns `None` if
    /// the module has no instances declared or no instance with that id.
    pub fn instance(&self, module: &str, id: &str) -> Option<&ModuleInstance> {
        self.instances.get(module)?.iter().find(|i| i.id == id)
    }

    /// Validate every instance id and reject duplicates within a single
    /// module's instance list. Call after deserializing `flow.yaml` so
    /// bad input fails at load time, not at launch time.
    pub fn validate(&self) -> Result<(), FlowValidationError> {
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

    #[test]
    fn sb_cli_config_round_trip() {
        let c = SbCliConfig {
            sb_home_dir: Some(PathBuf::from("/opt/sb_home")),
            protoc: Some(PathBuf::from("/opt/protoc/bin/protoc")),
            flatc: Some(PathBuf::from("/opt/flatc/bin/flatc")),
            libzenohc: Some(PathBuf::from("/opt/zenoh-c/lib/libzenohc.so")),
            libiceoryx2: Some(PathBuf::from("/opt/iceoryx2/lib/libiceoryx2_ffi_c.so")),
            tmux: Some(PathBuf::from("/usr/bin/tmux")),
            messages_root: Some(PathBuf::from("/home/u/.swarmbotix/messages")),
            message_style: Some(STYLE_ROS2.into()),
            message_definitions: Some(PathBuf::from(
                "/home/u/.swarmbotix/messages/ros2/message_definitions",
            )),
            message_targets: Some(PathBuf::from(
                "/home/u/.swarmbotix/messages/ros2/message_targets",
            )),
            device: Some("dev01".into()),
            transport_on_device: Some(Transport::Iceoryx2),
            transport_cross_device: Some(Transport::Zenoh),
            string_array_cap: None,
            string_array_caps: None,
            bytes_caps: None,
            vec_caps: None,
            iox2_variants: None,
        };
        yaml_fixed_point(&c);
    }

    #[test]
    fn module_dev_config_round_trip() {
        let m = ModuleDevConfig {
            module: "camera".into(),
            language: Language::Rust,
            root: PathBuf::from("/home/u/projects/camera_app"),
            io_dir: PathBuf::from("swarmbotix_io"),
            publishers: vec![PubSpec {
                name: "image_raw".into(),
                topic: "image_raw".into(),
                msg_type: "std/ImageStamped".into(),
                transport: Transport::Iceoryx2,
            }],
            subscribers: vec![SubSpec {
                name: "cmd_vel".into(),
                topic: "cmd_vel".into(),
                msg_type: "std/TwistStamped".into(),
                transport: Transport::Zenoh,
            }],
        };
        yaml_fixed_point(&m);
    }

    #[test]
    fn module_prd_config_round_trip() {
        let m = ModulePrdConfig {
            module: "camera".into(),
            publishers: vec![PubSpec {
                name: "image_raw".into(),
                topic: "image_raw".into(),
                msg_type: "std/ImageStamped".into(),
                transport: Transport::Iceoryx2,
            }],
            subscribers: vec![],
        };
        yaml_fixed_point(&m);
    }

    #[test]
    fn workspace_flow_round_trip() {
        let mut modules = BTreeMap::new();
        modules.insert(
            "camera".into(),
            PathBuf::from("/home/u/projects/camera/sb.dev.yml"),
        );
        modules.insert(
            "detector".into(),
            PathBuf::from("/home/u/projects/detector/sb.dev.yml"),
        );
        let f = WorkspaceFlow {
            modules,
            ..Default::default()
        };
        yaml_fixed_point(&f);
    }

    #[test]
    fn workspace_flow_with_instances_round_trip() {
        let mut modules = BTreeMap::new();
        modules.insert(
            "cam_gige_ht".into(),
            PathBuf::from("/home/u/projects/cam_gige_ht/sb.dev.yml"),
        );
        let mut env_left = BTreeMap::new();
        env_left.insert("CAM_SERIAL".into(), "A123".into());
        env_left.insert("LOG_LEVEL".into(), "info".into());
        let mut instances = BTreeMap::new();
        instances.insert(
            "cam_gige_ht".into(),
            vec![
                ModuleInstance {
                    id: "left".into(),
                    label: Some("Left Camera".into()),
                    docker: Some(DockerSpec {
                        image: Some("cam_gige_ht:latest".into()),
                        env: env_left,
                        devices: vec!["/dev/video0".into()],
                        mounts: vec![],
                        gpus: Some("all".into()),
                        network: None,
                    }),
                    args: vec![],
                },
                ModuleInstance {
                    id: "right".into(),
                    label: Some("Right Camera".into()),
                    docker: Some(DockerSpec {
                        image: Some("cam_gige_ht:latest".into()),
                        env: {
                            let mut m = BTreeMap::new();
                            m.insert("CAM_SERIAL".into(), "B456".into());
                            m
                        },
                        devices: vec!["/dev/video1".into()],
                        mounts: vec![],
                        gpus: None,
                        network: None,
                    }),
                    args: vec![],
                },
            ],
        );
        let f = WorkspaceFlow { modules, instances };
        yaml_fixed_point(&f);
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
    fn workspace_flow_instance_resolver_returns_match() {
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
                    id: "right".into(),
                    label: Some("Right".into()),
                    docker: None,
                    args: vec![],
                },
            ],
        );
        let f = WorkspaceFlow {
            instances,
            ..Default::default()
        };
        assert!(f.instance("cam", "left").is_some());
        assert_eq!(
            f.instance("cam", "right").unwrap().label.as_deref(),
            Some("Right")
        );
        assert!(f.instance("cam", "ghost").is_none());
        assert!(f.instance("missing", "left").is_none());
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

    // ── Merge tests (L1 TDD test #3) ──────────────────────────────────

    fn cfg_with_device(d: &str) -> SbCliConfig {
        SbCliConfig {
            device: Some(d.into()),
            ..Default::default()
        }
    }

    #[test]
    fn merge_env_wins_over_workspace_and_global() {
        let env = cfg_with_device("from-env");
        let ws = cfg_with_device("from-ws");
        let glb = cfg_with_device("from-global");
        let m = SbCliConfig::merge(Some(env), Some(ws), Some(glb));
        assert_eq!(m.device.as_deref(), Some("from-env"));
    }

    #[test]
    fn merge_workspace_wins_over_global() {
        let ws = cfg_with_device("from-ws");
        let glb = cfg_with_device("from-global");
        let m = SbCliConfig::merge(None, Some(ws), Some(glb));
        assert_eq!(m.device.as_deref(), Some("from-ws"));
    }

    #[test]
    fn merge_missing_keys_fall_through() {
        let env = SbCliConfig {
            protoc: Some(PathBuf::from("/env/protoc")),
            ..Default::default()
        };
        let ws = SbCliConfig {
            flatc: Some(PathBuf::from("/ws/flatc")),
            ..Default::default()
        };
        let glb = SbCliConfig {
            tmux: Some(PathBuf::from("/global/tmux")),
            device: Some("dev07".into()),
            ..Default::default()
        };
        let m = SbCliConfig::merge(Some(env), Some(ws), Some(glb));
        assert_eq!(m.protoc.as_deref().unwrap().to_str(), Some("/env/protoc"));
        assert_eq!(m.flatc.as_deref().unwrap().to_str(), Some("/ws/flatc"));
        assert_eq!(m.tmux.as_deref().unwrap().to_str(), Some("/global/tmux"));
        assert_eq!(m.device.as_deref(), Some("dev07"));
        // Falls through to built-in defaults for transports.
        assert_eq!(m.transport_on_device, Some(Transport::Iceoryx2));
        assert_eq!(m.transport_cross_device, Some(Transport::Zenoh));
    }

    #[test]
    fn merge_all_missing_returns_builtin_defaults() {
        let m = SbCliConfig::merge(None, None, None);
        assert_eq!(m.device.as_deref(), Some("dev01"));
        assert_eq!(m.transport_on_device, Some(Transport::Iceoryx2));
        assert_eq!(m.transport_cross_device, Some(Transport::Zenoh));
        assert!(m.protoc.is_none());
    }

    // ── Language tests ────────────────────────────────────────────────

    #[test]
    fn language_yaml_round_trip() {
        for lang in [
            Language::Rust,
            Language::Python,
            Language::Cpp,
            Language::Flutter,
            Language::Unity,
        ] {
            let y = serde_yaml::to_string(&lang).unwrap();
            let back: Language = serde_yaml::from_str(&y).unwrap();
            assert_eq!(lang, back);
        }
    }

    #[test]
    fn language_serializes_lowercase() {
        assert_eq!(serde_yaml::to_string(&Language::Cpp).unwrap().trim(), "cpp");
        assert_eq!(
            serde_yaml::to_string(&Language::Rust).unwrap().trim(),
            "rust"
        );
    }

    #[test]
    fn language_default_io_dir_unity_special_cases() {
        assert_eq!(
            Language::Unity.default_io_dir(),
            PathBuf::from("Assets/Scripts/swarmbotix_io")
        );
        for lang in [
            Language::Rust,
            Language::Python,
            Language::Cpp,
            Language::Flutter,
        ] {
            assert_eq!(lang.default_io_dir(), PathBuf::from("swarmbotix_io"));
        }
    }

    #[test]
    fn merge_env_partial_does_not_clobber_workspace_other_fields() {
        let env = SbCliConfig {
            device: Some("from-env".into()),
            ..Default::default()
        };
        let ws = SbCliConfig {
            protoc: Some(PathBuf::from("/ws/protoc")),
            device: Some("from-ws".into()),
            ..Default::default()
        };
        let m = SbCliConfig::merge(Some(env), Some(ws), None);
        assert_eq!(m.device.as_deref(), Some("from-env"));
        assert_eq!(m.protoc.as_deref().unwrap().to_str(), Some("/ws/protoc"));
    }
}
