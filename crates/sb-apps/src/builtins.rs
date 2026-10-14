//! Names an app may not take, because `sb` already owns them.
//!
//! `sb <name>` dispatches to an installed app only when `<name>` is not a
//! built-in subcommand, so an app named after one would be unreachable. The
//! list mirrors the top-level `enum Cmd` in `sb-cli`; a unit test there
//! fails if the two drift apart.

/// Every top-level `sb` subcommand, plus clap's generated `help`.
pub const BUILTIN_NAMES: &[&str] = &[
    "doctor", "message", "config", "ws", "init", "io", "topic", "up", "run", "stop", "down",
    "attach", "gopro", "install", "app", "update", "help",
];

/// True if `name` is a built-in `sb` subcommand.
///
/// Compared case-insensitively: clap itself matches case-sensitively, but an
/// app called `Run` next to the built-in `run` would only ever confuse.
pub fn is_builtin(name: &str) -> bool {
    BUILTIN_NAMES.iter().any(|b| b.eq_ignore_ascii_case(name))
}
