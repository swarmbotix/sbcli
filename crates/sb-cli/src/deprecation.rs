//! Deprecation warnings.
//!
//! Anything deprecated that `sb` still accepts is announced on stderr
//! through [`warn_deprecated`], in one fixed format:
//!
//! ```text
//! warning: <item> is deprecated since <SINCE> and will be removed in <UNTIL>; <hint>
//! ```
//!
//! Each item is announced at most once per process, so a launch plan that
//! finds the same deprecated input in six modules prints one line, not six.
//! stdout is never written: scripted callers parse it.
//!
//! 0.8.0 deprecates one item: a repository `sb.flow.yaml` in the 0.7 shape
//! (modules and wiring in one file), which `sb install` still reads,
//! converted into `sb.workspace.yaml` plus the default flow.
//! 0.8.1 removes it. (The surface 0.6.0 deprecated was removed in 0.7.0;
//! `sb init --docker` was removed in 0.8.0 without a deprecation period.)

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock, PoisonError};

/// The release that started warning.
pub const SINCE: &str = "0.8.0";
/// The release that removes the deprecated surface.
pub const UNTIL: &str = "0.8.1";

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
