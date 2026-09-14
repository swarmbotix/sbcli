//! Identifier rules shared by workspace names and module names.
//!
//! L2 plan: "Must be a valid identifier (no spaces/slashes; same rule as
//! workspace names)." Test #11 requires rejecting `demo-app`, `123demo`,
//! empty strings, and names containing spaces or slashes — i.e. stricter
//! than the prose hint. This module is the single source of truth.
//!
//! Rules:
//!   - non-empty
//!   - first char is ASCII letter or `_`
//!   - remaining chars are ASCII letter, digit, or `_`

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentError {
    Empty,
    StartsWithDigit { found: char },
    IllegalChar { found: char },
}

impl fmt::Display for IdentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IdentError::Empty => write!(f, "identifier is empty"),
            IdentError::StartsWithDigit { found } => write!(
                f,
                "identifier must start with a letter or `_`, found digit {found:?}"
            ),
            IdentError::IllegalChar { found } => write!(
                f,
                "identifier may only contain ASCII letters, digits, and `_`; found {found:?}"
            ),
        }
    }
}

impl std::error::Error for IdentError {}

/// `Ok(())` if `s` is a valid workspace/module identifier.
pub fn validate_identifier(s: &str) -> Result<(), IdentError> {
    let mut chars = s.chars();
    let first = chars.next().ok_or(IdentError::Empty)?;
    if !is_ident_start(first) {
        if first.is_ascii_digit() {
            return Err(IdentError::StartsWithDigit { found: first });
        }
        return Err(IdentError::IllegalChar { found: first });
    }
    for c in chars {
        if !is_ident_cont(c) {
            return Err(IdentError::IllegalChar { found: c });
        }
    }
    Ok(())
}

pub fn is_valid_identifier(s: &str) -> bool {
    validate_identifier(s).is_ok()
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_ident_cont(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_typical_names() {
        for s in [
            "demo",
            "demo_app",
            "Cam01",
            "_internal",
            "x",
            "X123",
            "abc_123_xyz",
        ] {
            assert!(is_valid_identifier(s), "{s:?} should be valid");
        }
    }

    #[test]
    fn rejects_per_l2_test_11_table() {
        // Cases pulled directly from the L2 TDD plan, test #11.
        for s in [
            "demo-app", "123demo", "", "demo app", "a/b", "a b", "a-b-c", "a.b",
        ] {
            assert!(!is_valid_identifier(s), "{s:?} should be invalid");
        }
    }

    #[test]
    fn error_types_are_specific() {
        assert!(matches!(validate_identifier(""), Err(IdentError::Empty)));
        assert!(matches!(
            validate_identifier("123abc"),
            Err(IdentError::StartsWithDigit { found: '1' })
        ));
        assert!(matches!(
            validate_identifier("a-b"),
            Err(IdentError::IllegalChar { found: '-' })
        ));
        assert!(matches!(
            validate_identifier("a b"),
            Err(IdentError::IllegalChar { found: ' ' })
        ));
        assert!(matches!(
            validate_identifier("a/b"),
            Err(IdentError::IllegalChar { found: '/' })
        ));
    }
}
