//! Pure-logic substrate for the `sb` CLI.
//!
//! No filesystem IO, no process spawning. Every later level depends on
//! these types — keeping them IO-free keeps unit tests fast and the
//! contract testable in isolation.

pub mod config;
pub mod expand;
pub mod host;
pub mod ident;
pub mod style;
pub mod topic;
pub mod transport;

pub use config::{
    Argv, BUNDLED_MESSAGE_STYLES, DEFAULT_FLOW_FILE, DEFAULT_FLOW_NAME, Deprecation, Encoding,
    EntryKind, FLOW_FILE, FLOW_SUFFIX, Flow, FlowValidationError, InstanceSettings, Language,
    LegacyTopic, MIGRATING_HINT, MODULE_FILE, ManifestError, MessageSource, ModuleDevConfig,
    ModuleInstance, ModulePrdConfig, Overlay, Platforms, PubSpec, REMOVED_FLOW_FILE,
    REMOVED_MODULE_FILE, Resolved, STYLE_ROS2, STYLE_SWARMBOTIX, SbCliConfig, Script, ScriptError,
    Scripts, ServiceSpec, SubSpec, TopicRef, TopicRefError, WORKSPACE_FILE, Wire, WireEnd,
    WireSelector, WireSource, Workspace, WorkspaceSource, convert_legacy_flow,
    current_manifest_path, flow_file_candidates, flow_file_name, flow_name_of,
    is_valid_message_style, language_defaults, legacy_flow_deprecation, legacy_topics_in,
    module_file_candidates, parse_wire, probe_legacy_flow_shape, python_fallback,
    python_venv_relpath, removed_flow_file_message, removed_module_file_message,
};
pub use expand::{ExpandCtx, ExpandError, expand_argv, expand_str};
pub use host::{Host, KNOWN_ARCH, KNOWN_OS};
pub use ident::{IdentError, is_valid_identifier, validate_identifier};
pub use style::{Backend, StyleConfig, StyleConfigError};
pub use topic::{
    InstanceIdError, TopicName, TopicNameError, instance_module, is_valid_instance_id,
    validate_instance_id,
};
pub use transport::Transport;
