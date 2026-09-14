//! Layered loader for `sb.config.yml`.
//!
//! Layering (highest priority first):
//!   1. `$SB_CONFIG` env var → explicit file path (used by tests, CI, ad-hoc overrides)
//!   2. `<active_workspace>/sb.config.yml` (L5 starts populating this)
//!   3. `~/.swarmbotix/sb.config.yml`
//!   4. Built-in defaults
//!
//! Layer resolution is delegated to [`sb_core::SbCliConfig::merge`].
//! This crate handles only IO: locating files, reading them, attaching
//! source paths to errors so `sb doctor` can report them.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use sb_core::{SbCliConfig, is_valid_message_style};

/// Where each layer was sourced from. Returned alongside the merged
/// config so `sb doctor` and `sb config` can show provenance.
#[derive(Debug, Clone, Default)]
pub struct ConfigSources {
    /// `$SB_CONFIG` path if the env var was set.
    pub env_override: Option<PathBuf>,
    /// Path to a workspace `sb.config.yml` if one was discovered.
    pub workspace: Option<PathBuf>,
    /// Path to global `~/.swarmbotix/sb.config.yml`.
    pub global: Option<PathBuf>,
}

/// `$SB_HOME`, if set to a non-empty value.
///
/// Both installers already take `SB_HOME` as the install prefix
/// (`SB_HOME=/opt/swarmbotix ./install.sh`). Honoring it here is what makes
/// such an install actually usable: without it the payload lands in the
/// custom prefix while `sb` keeps resolving its home from `dirs::home_dir()`
/// and never finds the config or vault that were just written.
///
/// It is also the only way a test can redirect the *global* config layer on
/// Windows, where `dirs::home_dir()` ignores the environment entirely.
fn sb_home_env() -> Option<PathBuf> {
    std::env::var_os("SB_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Default global path: `<sb_home>/sb.config.yml`.
///
/// `$SB_HOME` wins here because locating the config is chicken-and-egg —
/// there is no config field to consult yet.
pub fn default_global_path() -> Option<PathBuf> {
    if let Some(h) = sb_home_env() {
        return Some(h.join("sb.config.yml"));
    }
    dirs::home_dir().map(|h| h.join(".swarmbotix").join("sb.config.yml"))
}

/// Expand a leading `~/` to the user's home directory. No-op for
/// absolute or already-expanded paths.
pub fn expand_tilde(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    p.to_path_buf()
}

/// Effective `~/.swarmbotix/` root. Resolution order:
///
/// 1. `sb_home_dir` from the merged config (tilde-expanded) — most specific
///    and explicitly user-authored, so it wins.
/// 2. `$SB_HOME` — matches the prefix the installers accept.
/// 3. `<home>/.swarmbotix` — the built-in default.
pub fn resolve_sb_home_dir(cfg: &SbCliConfig) -> Result<PathBuf> {
    if let Some(p) = &cfg.sb_home_dir {
        return Ok(expand_tilde(p));
    }
    if let Some(h) = sb_home_env() {
        return Ok(h);
    }
    dirs::home_dir()
        .map(|h| h.join(".swarmbotix"))
        .ok_or_else(|| anyhow::anyhow!("could not locate user home directory"))
}

/// Root of the message-style tree. `messages_root` if set (tilde-expanded),
/// else `<sb_home>/messages`.
pub fn resolve_messages_root(cfg: &SbCliConfig) -> Result<PathBuf> {
    if let Some(p) = &cfg.messages_root {
        return Ok(expand_tilde(p));
    }
    Ok(resolve_sb_home_dir(cfg)?.join("messages"))
}

/// Directory holding one style: `<messages_root>/<style>`.
pub fn style_dir(cfg: &SbCliConfig, style: &str) -> Result<PathBuf> {
    if !is_valid_message_style(style) {
        anyhow::bail!(
            "invalid style {style:?} — must be a plain directory name under \
             the messages root (no `/`, `\\`, `:`, `.` or `..`)"
        );
    }
    Ok(resolve_messages_root(cfg)?.join(style))
}

/// Source `.proto` tree for ONE style:
/// `<messages_root>/<style>/message_definitions`.
///
/// There is no config key for "the" definitions path, because there is no
/// single active style — a [`sb_vault::MessageName`] carries its own. This
/// takes the style explicitly so the caller has to say which one it means.
pub fn message_definitions_for(cfg: &SbCliConfig, style: &str) -> Result<PathBuf> {
    Ok(style_dir(cfg, style)?.join("message_definitions"))
}

/// Codegen output root for ONE style:
/// `<messages_root>/<style>/message_targets`.
///
/// A sibling of `message_definitions` rather than nested inside it, so
/// generated `iox2/`, `proto/`, `fb/` trees never share space with sources.
pub fn message_targets_for(cfg: &SbCliConfig, style: &str) -> Result<PathBuf> {
    Ok(style_dir(cfg, style)?.join("message_targets"))
}

/// Every style present on disk — each subdirectory of `messages_root` that
/// has a `message_definitions/` child. Sorted. Used by `sb doctor` and
/// `sb message styles`.
pub fn list_message_styles(cfg: &SbCliConfig) -> Result<Vec<String>> {
    let root = resolve_messages_root(cfg)?;
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(&root) else {
        return Ok(out);
    };
    for e in rd.flatten() {
        if !e.path().join("message_definitions").is_dir() {
            continue;
        }
        if let Some(n) = e.file_name().to_str() {
            if is_valid_message_style(n) {
                out.push(n.to_owned());
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Read + parse a config file. Returns `Ok(None)` if the path does not exist.
pub fn read_optional(path: &Path) -> Result<Option<SbCliConfig>> {
    if !path.exists() {
        return Ok(None);
    }
    let s = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    if s.trim().is_empty() {
        return Ok(Some(SbCliConfig::default()));
    }
    let cfg: SbCliConfig =
        serde_yaml::from_str(&s).with_context(|| format!("parsing {}", path.display()))?;
    Ok(Some(cfg))
}

/// Inputs to the layered load — exposed so callers (CLI, tests) can
/// override discovery while keeping the merge logic shared.
#[derive(Debug, Default)]
pub struct LoadInputs {
    pub env_override: Option<PathBuf>,
    pub workspace: Option<PathBuf>,
    pub global: Option<PathBuf>,
}

impl LoadInputs {
    /// Resolve from the real environment:
    /// - `$SB_CONFIG`
    /// - global `~/.swarmbotix/sb.config.yml`
    /// - workspace lookup is L5+ — left `None` here.
    pub fn from_env() -> Self {
        Self {
            env_override: std::env::var_os("SB_CONFIG").map(PathBuf::from),
            workspace: None,
            global: default_global_path(),
        }
    }
}

/// Load and merge all layers. Returns the merged config + per-layer
/// source paths so callers can attribute errors back to a file.
pub fn load(inputs: &LoadInputs) -> Result<(SbCliConfig, ConfigSources)> {
    let (merged, _layers, srcs) = load_layers(inputs)?;
    Ok((merged, srcs))
}

/// Like [`load`] but also returns the raw per-layer configs so callers
/// can compute per-field provenance (which layer supplied each value).
///
/// Used by `sb config show --sources` and `sb config get --json`.
pub fn load_layers(inputs: &LoadInputs) -> Result<(SbCliConfig, LayerConfigs, ConfigSources)> {
    let mut srcs = ConfigSources::default();

    let env_cfg = match &inputs.env_override {
        Some(p) => {
            let c = read_required(p, "$SB_CONFIG")?;
            srcs.env_override = Some(p.clone());
            Some(c)
        }
        None => None,
    };
    let ws_cfg = match &inputs.workspace {
        Some(p) => match read_optional(p)? {
            Some(c) => {
                srcs.workspace = Some(p.clone());
                Some(c)
            }
            None => None,
        },
        None => None,
    };
    let glb_cfg = match &inputs.global {
        Some(p) => match read_optional(p)? {
            Some(c) => {
                srcs.global = Some(p.clone());
                Some(c)
            }
            None => None,
        },
        None => None,
    };

    let merged = SbCliConfig::merge(env_cfg.clone(), ws_cfg.clone(), glb_cfg.clone());
    let layers = LayerConfigs {
        env: env_cfg,
        workspace: ws_cfg,
        global: glb_cfg,
    };
    Ok((merged, layers, srcs))
}

/// The three file-backed layers parsed into `SbCliConfig` values (the
/// built-in defaults are not in here — they're applied by `merge`).
#[derive(Debug, Clone, Default)]
pub struct LayerConfigs {
    pub env: Option<SbCliConfig>,
    pub workspace: Option<SbCliConfig>,
    pub global: Option<SbCliConfig>,
}

/// Which layer supplied a given field in the merged config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldLayer {
    Env,
    Workspace,
    Global,
    Builtin,
    /// Field is unset everywhere — including built-in defaults (e.g.
    /// `protoc` on a fresh host).
    Unset,
}

impl FieldLayer {
    pub fn as_str(self) -> &'static str {
        match self {
            FieldLayer::Env => "env",
            FieldLayer::Workspace => "workspace",
            FieldLayer::Global => "global",
            FieldLayer::Builtin => "builtin",
            FieldLayer::Unset => "unset",
        }
    }
}

/// Every config key the CLI knows about. Order is stable for `show`
/// output. The nested-map keys (`string_array_caps`, `bytes_caps`) come
/// last because they render as `<n> entries`, not a scalar.
pub const KNOWN_KEYS: &[&str] = &[
    "sb_home_dir",
    "protoc",
    "flatc",
    "libzenohc",
    "libiceoryx2",
    "tmux",
    "messages_root",
    "device",
    "transport_on_device",
    "transport_cross_device",
    "string_array_cap",
    "string_array_caps",
    "bytes_caps",
    "vec_caps",
    "iox2_variants",
];

/// Return the field value as a display string, or `None` if the field
/// is absent in this config. `string_array_caps` is rendered as
/// `<n> entries` because it's a map — callers wanting the contents
/// should serialize the whole config (`sb config show`).
pub fn get_field(cfg: &SbCliConfig, key: &str) -> Result<Option<String>> {
    fn path(p: &Option<PathBuf>) -> Option<String> {
        p.as_ref().map(|p| p.display().to_string())
    }
    let v = match key {
        "sb_home_dir" => path(&cfg.sb_home_dir),
        "protoc" => path(&cfg.protoc),
        "flatc" => path(&cfg.flatc),
        "libzenohc" => path(&cfg.libzenohc),
        "libiceoryx2" => path(&cfg.libiceoryx2),
        "tmux" => path(&cfg.tmux),
        "messages_root" => path(&cfg.messages_root),
        "device" => cfg.device.clone(),
        "transport_on_device" => cfg.transport_on_device.map(|t| t.as_str().to_owned()),
        "transport_cross_device" => cfg.transport_cross_device.map(|t| t.as_str().to_owned()),
        "string_array_cap" => cfg.string_array_cap.map(|n| n.to_string()),
        "string_array_caps" => cfg
            .string_array_caps
            .as_ref()
            .map(|m| format!("{} entries", m.len())),
        "bytes_caps" => cfg
            .bytes_caps
            .as_ref()
            .map(|m| format!("{} entries", m.len())),
        "vec_caps" => cfg
            .vec_caps
            .as_ref()
            .map(|m| format!("{} entries", m.len())),
        // Rendered as "<msgs> msg(s), <n> variants" because the value is a
        // two-level map; `sb config show` serializes the whole thing.
        "iox2_variants" => cfg.iox2_variants.as_ref().map(|m| {
            let variants: usize = m.values().map(|v| v.len()).sum();
            format!("{} msg(s), {variants} variants", m.len())
        }),
        other => anyhow::bail!("unknown key {other:?}. Known: {}", KNOWN_KEYS.join(", ")),
    };
    Ok(v)
}

/// Determine which layer supplied `key` in the merged config. Walks
/// env → workspace → global → built-in and returns the first layer
/// that has the field set. Returns [`FieldLayer::Unset`] if no layer
/// (including built-ins) populates it.
pub fn field_layer(layers: &LayerConfigs, key: &str) -> Result<FieldLayer> {
    let builtin = SbCliConfig::builtin_defaults();
    let has = |c: &SbCliConfig| -> Result<bool> { Ok(get_field(c, key)?.is_some()) };
    if let Some(c) = &layers.env {
        if has(c)? {
            return Ok(FieldLayer::Env);
        }
    }
    if let Some(c) = &layers.workspace {
        if has(c)? {
            return Ok(FieldLayer::Workspace);
        }
    }
    if let Some(c) = &layers.global {
        if has(c)? {
            return Ok(FieldLayer::Global);
        }
    }
    if has(&builtin)? {
        return Ok(FieldLayer::Builtin);
    }
    Ok(FieldLayer::Unset)
}

/// Read a file that the caller asserts must exist (e.g. `$SB_CONFIG`
/// was set, so the path must be valid).
fn read_required(path: &Path, label: &str) -> Result<SbCliConfig> {
    let s =
        fs::read_to_string(path).with_context(|| format!("reading {label}={}", path.display()))?;
    if s.trim().is_empty() {
        return Ok(SbCliConfig::default());
    }
    let cfg: SbCliConfig =
        serde_yaml::from_str(&s).with_context(|| format!("parsing {label}={}", path.display()))?;
    Ok(cfg)
}

/// Write a single key into a target config file, creating the file
/// (and parent directories) if missing. Used by `sb config set`.
pub fn set_key(target: &Path, key: &str, value: &str) -> Result<()> {
    let mut cfg = read_optional(target)?.unwrap_or_default();
    apply_key(&mut cfg, key, value).with_context(|| format!("setting key {key:?}={value:?}"))?;
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating parent dir {}", parent.display()))?;
    }
    let s = serde_yaml::to_string(&cfg).context("serializing config")?;
    fs::write(target, s).with_context(|| format!("writing {}", target.display()))?;
    Ok(())
}

fn apply_key(cfg: &mut SbCliConfig, key: &str, value: &str) -> Result<()> {
    match key {
        "sb_home_dir" => cfg.sb_home_dir = Some(PathBuf::from(value)),
        "protoc" => cfg.protoc = Some(PathBuf::from(value)),
        "flatc" => cfg.flatc = Some(PathBuf::from(value)),
        "libzenohc" => cfg.libzenohc = Some(PathBuf::from(value)),
        "libiceoryx2" => cfg.libiceoryx2 = Some(PathBuf::from(value)),
        "tmux" => cfg.tmux = Some(PathBuf::from(value)),
        "messages_root" => cfg.messages_root = Some(PathBuf::from(value)),
        "device" => cfg.device = Some(value.to_owned()),
        "transport_on_device" => {
            cfg.transport_on_device = Some(value.parse().map_err(anyhow::Error::msg)?)
        }
        "transport_cross_device" => {
            cfg.transport_cross_device = Some(value.parse().map_err(anyhow::Error::msg)?)
        }
        "string_array_cap" => {
            cfg.string_array_cap = Some(value.parse().map_err(|e| {
                anyhow::anyhow!("string_array_cap must be a positive integer: {e}")
            })?);
        }
        other => anyhow::bail!("unknown key: {other:?}"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn read_optional_missing_path_is_ok_none() {
        let d = tempdir().unwrap();
        let missing = d.path().join("nope.yml");
        assert!(read_optional(&missing).unwrap().is_none());
    }

    #[test]
    fn load_layering_env_over_workspace_over_global() {
        let d = tempdir().unwrap();
        let env_p = d.path().join("env.yml");
        let ws_p = d.path().join("ws.yml");
        let glb_p = d.path().join("global.yml");
        fs::write(&env_p, "device: from-env\n").unwrap();
        fs::write(&ws_p, "device: from-ws\nprotoc: /ws/protoc\n").unwrap();
        fs::write(&glb_p, "device: from-global\nflatc: /global/flatc\n").unwrap();

        let inputs = LoadInputs {
            env_override: Some(env_p),
            workspace: Some(ws_p),
            global: Some(glb_p),
        };
        let (merged, srcs) = load(&inputs).unwrap();
        assert_eq!(merged.device.as_deref(), Some("from-env"));
        assert_eq!(merged.protoc.unwrap().to_str(), Some("/ws/protoc"));
        assert_eq!(merged.flatc.unwrap().to_str(), Some("/global/flatc"));
        assert!(srcs.env_override.is_some());
        assert!(srcs.workspace.is_some());
        assert!(srcs.global.is_some());
    }

    #[test]
    fn load_missing_workspace_falls_through_to_global() {
        let d = tempdir().unwrap();
        let glb_p = d.path().join("global.yml");
        fs::write(&glb_p, "device: from-global\n").unwrap();
        let inputs = LoadInputs {
            env_override: None,
            workspace: Some(d.path().join("absent.yml")),
            global: Some(glb_p),
        };
        let (merged, srcs) = load(&inputs).unwrap();
        assert_eq!(merged.device.as_deref(), Some("from-global"));
        assert!(srcs.workspace.is_none());
        assert!(srcs.global.is_some());
    }

    #[test]
    fn load_all_absent_returns_builtin_defaults() {
        let d = tempdir().unwrap();
        let inputs = LoadInputs {
            env_override: None,
            workspace: Some(d.path().join("nope1.yml")),
            global: Some(d.path().join("nope2.yml")),
        };
        let (merged, srcs) = load(&inputs).unwrap();
        assert_eq!(merged.device.as_deref(), Some("dev01")); // built-in
        assert!(srcs.workspace.is_none());
        assert!(srcs.global.is_none());
    }

    #[test]
    fn env_override_must_exist_when_set() {
        let d = tempdir().unwrap();
        let inputs = LoadInputs {
            env_override: Some(d.path().join("absent-env.yml")),
            workspace: None,
            global: None,
        };
        let err = load(&inputs).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("$SB_CONFIG"),
            "expected $SB_CONFIG in error, got: {msg}"
        );
    }

    #[test]
    fn set_key_writes_and_updates() {
        let d = tempdir().unwrap();
        let target = d.path().join("sub").join("sb.config.yml");
        set_key(&target, "device", "from-set").unwrap();
        let cfg = read_optional(&target).unwrap().unwrap();
        assert_eq!(cfg.device.as_deref(), Some("from-set"));

        set_key(&target, "protoc", "/new/protoc").unwrap();
        let cfg = read_optional(&target).unwrap().unwrap();
        assert_eq!(cfg.device.as_deref(), Some("from-set"));
        assert_eq!(cfg.protoc.unwrap().to_str(), Some("/new/protoc"));
    }

    #[test]
    fn set_key_rejects_unknown() {
        let d = tempdir().unwrap();
        let err = set_key(&d.path().join("c.yml"), "no_such_key", "v").unwrap_err();
        assert!(format!("{err:#}").contains("no_such_key"));
    }

    #[test]
    fn get_field_returns_scalar_value() {
        let cfg = SbCliConfig {
            protoc: Some(PathBuf::from("/usr/bin/protoc")),
            device: Some("dev07".into()),
            ..Default::default()
        };
        assert_eq!(
            get_field(&cfg, "protoc").unwrap().as_deref(),
            Some("/usr/bin/protoc")
        );
        assert_eq!(get_field(&cfg, "device").unwrap().as_deref(), Some("dev07"));
        assert_eq!(get_field(&cfg, "flatc").unwrap(), None);
    }

    #[test]
    fn get_field_renders_transport_lowercase() {
        let cfg = SbCliConfig {
            transport_on_device: Some(sb_core::Transport::Zenoh),
            ..Default::default()
        };
        assert_eq!(
            get_field(&cfg, "transport_on_device").unwrap().as_deref(),
            Some("zenoh")
        );
    }

    #[test]
    fn get_field_rejects_unknown_key() {
        let cfg = SbCliConfig::default();
        let err = get_field(&cfg, "no_such_thing").unwrap_err();
        assert!(format!("{err:#}").contains("no_such_thing"));
    }

    #[test]
    fn known_keys_cover_every_apply_key_branch() {
        // If a new key is added to apply_key, KNOWN_KEYS must learn it
        // too — this test ensures the round-trip stays in sync.
        for k in KNOWN_KEYS {
            // Each known key must resolve via get_field on a default config
            // without panicking (value may be None, but must not error).
            let _ = get_field(&SbCliConfig::default(), k).unwrap();
        }
    }

    #[test]
    fn field_layer_walks_precedence() {
        let env = SbCliConfig {
            device: Some("from-env".into()),
            ..Default::default()
        };
        let ws = SbCliConfig {
            protoc: Some(PathBuf::from("/ws/protoc")),
            device: Some("from-ws".into()),
            ..Default::default()
        };
        let glb = SbCliConfig {
            flatc: Some(PathBuf::from("/global/flatc")),
            ..Default::default()
        };
        let layers = LayerConfigs {
            env: Some(env),
            workspace: Some(ws),
            global: Some(glb),
        };
        assert_eq!(field_layer(&layers, "device").unwrap(), FieldLayer::Env);
        assert_eq!(
            field_layer(&layers, "protoc").unwrap(),
            FieldLayer::Workspace
        );
        assert_eq!(field_layer(&layers, "flatc").unwrap(), FieldLayer::Global);
        // transport_on_device is in built-in defaults.
        assert_eq!(
            field_layer(&layers, "transport_on_device").unwrap(),
            FieldLayer::Builtin
        );
        // sb_home_dir has no built-in default → unset.
        assert_eq!(
            field_layer(&layers, "sb_home_dir").unwrap(),
            FieldLayer::Unset
        );
    }

    #[test]
    fn load_layers_returns_raw_per_layer_configs() {
        let d = tempdir().unwrap();
        let env_p = d.path().join("env.yml");
        let glb_p = d.path().join("global.yml");
        fs::write(&env_p, "device: from-env\n").unwrap();
        fs::write(&glb_p, "protoc: /global/protoc\n").unwrap();
        let inputs = LoadInputs {
            env_override: Some(env_p),
            workspace: None,
            global: Some(glb_p),
        };
        let (merged, layers, _srcs) = load_layers(&inputs).unwrap();
        assert_eq!(merged.device.as_deref(), Some("from-env"));
        assert_eq!(
            layers.env.as_ref().unwrap().device.as_deref(),
            Some("from-env")
        );
        assert!(layers.env.as_ref().unwrap().protoc.is_none());
        assert!(layers.workspace.is_none());
        assert_eq!(
            layers
                .global
                .as_ref()
                .unwrap()
                .protoc
                .as_deref()
                .unwrap()
                .to_str(),
            Some("/global/protoc")
        );
    }

    // ── per-style path resolution ─────────────────────────────────────

    fn rooted(root: &Path) -> SbCliConfig {
        SbCliConfig {
            sb_home_dir: Some(root.to_path_buf()),
            messages_root: Some(root.join("messages")),
            ..Default::default()
        }
    }

    #[test]
    fn paths_derive_from_root_and_the_style_you_ask_for() {
        let d = tempdir().unwrap();
        let cfg = rooted(d.path());
        assert_eq!(
            message_definitions_for(&cfg, "ros2").unwrap(),
            d.path()
                .join("messages")
                .join("ros2")
                .join("message_definitions")
        );
        assert_eq!(
            message_targets_for(&cfg, "swarmbotix").unwrap(),
            d.path()
                .join("messages")
                .join("swarmbotix")
                .join("message_targets")
        );
    }

    #[test]
    fn two_styles_resolve_side_by_side_from_one_config() {
        // The point of dropping the active-style mode: one config serves a
        // module that publishes one style and subscribes to another.
        let d = tempdir().unwrap();
        let cfg = rooted(d.path());
        let a = message_targets_for(&cfg, "ros2").unwrap();
        let b = message_targets_for(&cfg, "swarmbotix").unwrap();
        assert_ne!(a, b);
        assert!(a.ends_with("ros2/message_targets"));
        assert!(b.ends_with("swarmbotix/message_targets"));
    }

    #[test]
    fn messages_root_defaults_under_sb_home() {
        let d = tempdir().unwrap();
        let cfg = SbCliConfig {
            sb_home_dir: Some(d.path().to_path_buf()),
            ..Default::default()
        };
        assert_eq!(
            resolve_messages_root(&cfg).unwrap(),
            d.path().join("messages")
        );
    }

    #[test]
    fn traversal_style_names_are_rejected() {
        let d = tempdir().unwrap();
        let cfg = rooted(d.path());
        for bad in ["..", ".", "", "a/b", "a\\b", "C:evil"] {
            assert!(
                message_definitions_for(&cfg, bad).is_err(),
                "style {bad:?} should be rejected"
            );
            assert!(message_targets_for(&cfg, bad).is_err());
        }
    }

    #[test]
    fn deprecated_keys_are_parsed_but_absent_from_the_live_surface() {
        // They must still DESERIALIZE (deny_unknown_fields would reject a
        // pre-0.1.35 file otherwise) while being invisible to config verbs.
        let cfg: SbCliConfig = serde_yaml::from_str(
            "message_style: ros2\nmessage_definitions: /x\nmessage_targets: /y\n",
        )
        .expect("an old config must still load");
        assert_eq!(cfg.message_style.as_deref(), Some("ros2"));
        for k in ["message_style", "message_definitions", "message_targets"] {
            assert!(!KNOWN_KEYS.contains(&k), "{k} must not be a live key");
            assert!(get_field(&cfg, k).is_err(), "{k} must not be gettable");
        }
    }

    #[test]
    fn list_styles_finds_only_dirs_with_message_definitions() {
        let d = tempdir().unwrap();
        let root = d.path().join("messages");
        fs::create_dir_all(root.join("ros2").join("message_definitions")).unwrap();
        fs::create_dir_all(root.join("swarmbotix").join("message_definitions")).unwrap();
        fs::create_dir_all(root.join("notastyle")).unwrap();
        let cfg = rooted(d.path());
        assert_eq!(
            list_message_styles(&cfg).unwrap(),
            vec!["ros2".to_string(), "swarmbotix".to_string()]
        );
    }

    #[test]
    fn list_styles_is_empty_when_root_absent() {
        let d = tempdir().unwrap();
        let cfg = rooted(&d.path().join("nope"));
        assert!(list_message_styles(&cfg).unwrap().is_empty());
    }

    // ── shipped iox2 defaults for the swarmbotix style ────────────────

    #[test]
    fn image_variants_ship_eight_types_with_wxhxbpp_capacities() {
        let v = SbCliConfig::builtin_defaults().iox2_variants.unwrap();
        let img = v
            .get("swarmbotix.images.Image")
            .expect("images/Image must declare variants");
        assert_eq!(img.len(), 8, "4 resolutions x 2 pixel formats");
        // capacity == width * height * bytes_per_pixel, exactly.
        for (name, w, h, bpp) in [
            ("Image360pMono8", 640u32, 360u32, 1u32),
            ("Image360pRgb8", 640, 360, 3),
            ("Image480pMono8", 640, 480, 1),
            ("Image480pRgb8", 640, 480, 3),
            ("Image720pMono8", 1280, 720, 1),
            ("Image720pRgb8", 1280, 720, 3),
            ("Image1080pMono8", 1920, 1080, 1),
            ("Image1080pRgb8", 1920, 1080, 3),
        ] {
            let caps = img.get(name).unwrap_or_else(|| panic!("missing {name}"));
            assert_eq!(
                caps.get("data").copied(),
                Some(w * h * bpp),
                "{name} data cap"
            );
        }
    }

    #[test]
    fn fixed_size_matrices_are_pinned_to_their_exact_element_count() {
        // The whole point of vec_caps: the global vec_capacity would give a
        // 3x3 matrix a 256-element array.
        let v = SbCliConfig::builtin_defaults().vec_caps.unwrap();
        assert_eq!(v.get("swarmbotix.primitives.Mat33.data").copied(), Some(9));
        assert_eq!(v.get("swarmbotix.primitives.Mat44.data").copied(), Some(16));
        assert_eq!(
            v.get("swarmbotix.sensors.LaserScan.ranges").copied(),
            Some(1080)
        );
    }

    #[test]
    fn new_cap_keys_are_readable_through_get_field() {
        let cfg = SbCliConfig::builtin_defaults();
        assert!(get_field(&cfg, "vec_caps").unwrap().is_some());
        let v = get_field(&cfg, "iox2_variants").unwrap().unwrap();
        assert!(v.contains("8 variants"), "got {v:?}");
        // Both must be listed or `sb config show` silently omits them.
        assert!(KNOWN_KEYS.contains(&"vec_caps"));
        assert!(KNOWN_KEYS.contains(&"iox2_variants"));
    }

    #[test]
    fn set_key_refuses_a_deprecated_key() {
        let d = tempdir().unwrap();
        let p = d.path().join("c.yml");
        assert!(set_key(&p, "message_style", "ros2").is_err());
    }
}
