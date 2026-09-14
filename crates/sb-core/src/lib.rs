//! Pure-logic substrate for the `sb` CLI.
//!
//! No filesystem IO, no process spawning. Every later level depends on
//! these types — keeping them IO-free keeps unit tests fast and the
//! contract testable in isolation.

pub mod config;
pub mod ident;
pub mod style;
pub mod topic;
pub mod transport;

pub use config::{
    BUNDLED_MESSAGE_STYLES, DockerSpec, FlowValidationError, Language, ModuleDevConfig,
    ModuleInstance, ModulePrdConfig, PubSpec, STYLE_ROS2, STYLE_SWARMBOTIX, SbCliConfig, SubSpec,
    WorkspaceFlow, is_valid_message_style,
};
pub use ident::{IdentError, is_valid_identifier, validate_identifier};
pub use style::{Backend, StyleConfig, StyleConfigError};
pub use topic::{
    InstanceIdError, TopicName, TopicNameError, is_valid_instance_id, validate_instance_id,
};
pub use transport::Transport;
