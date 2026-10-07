//! External app packages for `sb`.
//!
//! An *app package* is any folder (usually a git repository) with an
//! `sb.app.yml` manifest at its root. Package authors create one with
//! `sb app init`; consumers install it with `sb install <url|path>` and then
//! run it as `sb <name> [args...]`, with no change to their `PATH`.
//!
//! Filesystem layout owned by this crate:
//!
//! ```text
//! <sb_home>/                       (~/.swarmbotix by default)
//!   apps.yml                       registry: one entry per installed app
//!   apps/
//!     <name>/                      git clones made by `sb install <url>`
//!     .tmp-<repo>-<id>/            in-flight clone, renamed once validated
//! ```
//!
//! Packages installed from a local folder are registered in place and never
//! copied; only clones under `<sb_home>/apps/` are deleted by `sb app remove`.
//!
//! Every function takes the `sb_home` root explicitly so tests can point it
//! at a tempdir without touching the real `~/.swarmbotix/`.

pub mod builtins;
pub mod exec;
pub mod install;
pub mod manifest;
pub mod registry;
pub mod source;
pub mod template;

mod util;

pub use builtins::{BUILTIN_NAMES, is_builtin};
pub use exec::exec_app;
pub use install::{
    AppInfo, InstallOptions, InstallOutcome, RemoveOutcome, RequireStatus, UpdateOutcome, info,
    install, list, remove, update,
};
pub use manifest::{AppKind, AppManifest, DockerSpec, HostSpec, MANIFEST_FILE, validate_app_name};
pub use registry::{AppEntry, Registry, SourceKind};
pub use source::{PullStatus, Source, classify};
pub use template::{InitAppOptions, InitAppOutcome, default_entry, default_name, init_app};
