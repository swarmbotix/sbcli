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
    Argv, BUNDLED_MESSAGE_STYLES, Deprecation, DockerSpec, Encoding, EntryKind, FLOW_FILE,
    FlowSource, FlowValidationError, Language, LegacyTopic, MIGRATING_HINT, MODULE_FILE,
    ManifestError, MessageSource, ModuleDevConfig, ModuleInstance, ModulePrdConfig, ModuleRef,
    ModuleRefRaw, Overlay, Platforms, PubSpec, REMOVED_FLOW_FILE, REMOVED_MODULE_FILE, Resolved,
    STYLE_ROS2, STYLE_SWARMBOTIX, SbCliConfig, Script, ScriptError, Scripts, ServiceSpec, SubSpec,
    TopicRef, TopicRefError, Wire, WireEnd, WireSelector, WireSource, WorkspaceFlow,
    current_manifest_path, flow_file_candidates, is_valid_message_style, language_defaults,
    legacy_topics_in, module_file_candidates, parse_wire, probe_legacy_flow_keys, python_fallback,
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
