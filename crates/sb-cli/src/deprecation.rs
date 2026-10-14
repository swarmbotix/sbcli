//! Deprecation warnings for the surface 0.6.0 replaces.
//!
//! Everything deprecated that `sb` still accepts (the four pre-`sb io`
//! verbs, the legacy file names, run and stop scripts, top-level
//! `instances:`) is announced on stderr through [`warn_deprecated`], in one
//! fixed format:
//!
//! ```text
//! warning: <item> is deprecated since 0.6.0 and will be removed in 0.7.0; <hint>
//! ```
//!
//! Each item is announced at most once per process, so a launch plan that
//! finds the same legacy file name in six modules prints one line, not six.
//! stdout is never written: scripted callers parse it.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock, PoisonError};

/// The release that started warning.
pub const SINCE: &str = "0.6.0";
/// The release that removes the deprecated surface.
pub const UNTIL: &str = "0.7.0";

/// Item texts for deprecations `sb-cli` raises itself. Deprecations found by
/// other crates arrive as [`sb_core::Deprecation`] values carrying their own
/// item text.
pub mod items {
    pub const VERB_PUB: &str = "`sb pub`";
    pub const VERB_SUB: &str = "`sb sub`";
    pub const VERB_LIST: &str = "`sb list`";
    pub const VERB_SERVICE: &str = "`sb service`";
}

/// Items this process has already announced.
static ANNOUNCED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

/// Print the deprecation warning for `item` on stderr, unless this process
/// has already printed one for the same `item`.
pub fn warn_deprecated(item: &str, hint: &str) {
    let announced = ANNOUNCED.get_or_init(|| Mutex::new(HashSet::new()));
    // A poisoned lock only means another thread panicked while holding it;
    // the set is still a valid set, so keep using it rather than lose the
    // warning. The guard is released at the end of this statement, before
    // anything is printed.
    let first = announced
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(item.to_owned());
    if first {
        eprintln!(
            "warning: {item} is deprecated since {SINCE} and will be removed in {UNTIL}; {hint}"
        );
    }
}

/// [`warn_deprecated`] for every entry, in order. Convenience for the
/// `Vec<Deprecation>` that outcomes and launch plans carry.
pub fn warn_all(items: &[sb_core::Deprecation]) {
    for d in items {
        warn_deprecated(&d.item, &d.hint);
    }
}
