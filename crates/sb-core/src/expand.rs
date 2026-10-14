//! Placeholder and environment expansion for manifest commands.
//!
//! Grammar, one left-to-right pass:
//!
//! | text                | becomes                                              |
//! | ------------------- | ---------------------------------------------------- |
//! | `${NAME}`           | the variable's value, or empty when unset            |
//! | `${NAME:-default}`  | the value when set and non-empty, else `default`      |
//! | `{name}`            | the [`ExpandCtx`] field of that name                 |
//! | `{{` / `}}`         | a literal `{` / `}`                                  |
//!
//! `default` may itself hold `{name}` placeholders; they are expanded as
//! part of the same pass. Because the scan never revisits output, an
//! environment value that happens to contain `{root}` is copied as is,
//! and `${X:-/usr/lib}` is consumed whole as one variable reference.
//!
//! Variables come from a caller-supplied lookup, so this module stays
//! free of process state and is testable with a fixed table.

use crate::config::Argv;

/// The values `{name}` placeholders expand to, for one module instance
/// on one host.
#[derive(Debug, Clone, Copy)]
pub struct ExpandCtx<'a> {
    /// `{module}`: the manifest's `module:`.
    pub module: &'a str,
    /// `{root}`: absolute module root, forward slashes.
    pub root: &'a str,
    /// `{bin}`: the built binary, already expanded.
    pub bin: &'a str,
    /// `{exe}`: `.exe` on Windows, empty elsewhere.
    pub exe: &'a str,
    /// `{id}`: the instance id; `None` for a module with no instances.
    pub id: Option<&'a str>,
    /// `{args}`: spliced when it is a whole token of an argv.
    pub args: &'a [String],
    /// `{ws}`: the workspace name.
    pub ws: &'a str,
    /// `{os}`: `linux`, `windows`, `macos`.
    pub os: &'a str,
    /// `{python}`: the interpreter the caller resolved.
    pub python: &'a str,
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum ExpandError {
    #[error("unknown placeholder {{{0}}}")]
    Unknown(String),
    #[error("{{id}} used but this module has no instances")]
    NoInstance,
    #[error("{{args}} must be a whole token")]
    ArgsNotWhole,
    #[error("unterminated placeholder in {0:?}")]
    Unterminated(String),
}

/// The token that splices `ctx.args` into an argv.
const ARGS_TOKEN: &str = "{args}";

/// Expand one argv. A token that is exactly `{args}` is replaced by the
/// entries of `ctx.args` (zero or more tokens, copied verbatim); every
/// other token goes through [`expand_str`]. Tokens are never split or
/// dropped, so a token that expands to the empty string stays as an
/// empty argument.
pub fn expand_argv(
    argv: &[String],
    ctx: &ExpandCtx,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Argv, ExpandError> {
    let mut out = Vec::with_capacity(argv.len() + ctx.args.len());
    for token in argv {
        if token == ARGS_TOKEN {
            out.extend(ctx.args.iter().cloned());
        } else {
            out.push(expand_str(token, ctx, env)?);
        }
    }
    Ok(out)
}

/// Expand one string (an env value, the bin path, one argv token).
/// `{args}` here is [`ExpandError::ArgsNotWhole`]: it only has a meaning
/// as a whole argv token.
pub fn expand_str(
    s: &str,
    ctx: &ExpandCtx,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<String, ExpandError> {
    let mut out = String::with_capacity(s.len());
    Expander { whole: s, ctx, env }.expand_into(s, &mut out)?;
    Ok(out)
}

/// One expansion: the original text (for error messages), the context,
/// and the variable lookup.
struct Expander<'s, 'c> {
    whole: &'s str,
    ctx: &'c ExpandCtx<'c>,
    env: &'c dyn Fn(&str) -> Option<String>,
}

impl Expander<'_, '_> {
    fn unterminated(&self) -> ExpandError {
        ExpandError::Unterminated(self.whole.to_owned())
    }

    /// Scan `s` left to right, appending the expansion to `out`. Every
    /// delimiter is ASCII, so byte offsets are always char boundaries.
    fn expand_into(&self, s: &str, out: &mut String) -> Result<(), ExpandError> {
        let bytes = s.as_bytes();
        let mut i = 0;
        let mut literal_from = 0;
        while i < bytes.len() {
            let next = bytes.get(i + 1).copied();
            match (bytes[i], next) {
                (b'$', Some(b'{')) => {
                    out.push_str(&s[literal_from..i]);
                    let body_start = i + 2;
                    let close = find_close(s, body_start).ok_or_else(|| self.unterminated())?;
                    self.expand_env(&s[body_start..close], out)?;
                    i = close + 1;
                    literal_from = i;
                }
                (b'{', Some(b'{')) | (b'}', Some(b'}')) => {
                    out.push_str(&s[literal_from..i]);
                    out.push(char::from(bytes[i]));
                    i += 2;
                    literal_from = i;
                }
                (b'{', _) => {
                    out.push_str(&s[literal_from..i]);
                    let name_start = i + 1;
                    let close = s[name_start..]
                        .find('}')
                        .map(|p| name_start + p)
                        .ok_or_else(|| self.unterminated())?;
                    out.push_str(self.placeholder(&s[name_start..close])?);
                    i = close + 1;
                    literal_from = i;
                }
                // A lone `}` or `$` is ordinary text.
                _ => i += 1,
            }
        }
        out.push_str(&s[literal_from..]);
        Ok(())
    }

    /// The body of `${...}`: `NAME` or `NAME:-default`. `:-` follows the
    /// shell: an unset or empty variable takes the default.
    fn expand_env(&self, body: &str, out: &mut String) -> Result<(), ExpandError> {
        let (name, default) = match body.split_once(":-") {
            Some((name, default)) => (name, Some(default)),
            None => (body, None),
        };
        if !is_env_name(name) {
            return Err(ExpandError::Unknown(format!("${{{body}}}")));
        }
        let value = (self.env)(name).filter(|v| !v.is_empty());
        match (value, default) {
            (Some(v), _) => out.push_str(&v),
            (None, Some(default)) => self.expand_into(default, out)?,
            (None, None) => {}
        }
        Ok(())
    }

    /// The value of `{name}`.
    fn placeholder(&self, name: &str) -> Result<&str, ExpandError> {
        let ctx = self.ctx;
        Ok(match name {
            "module" => ctx.module,
            "root" => ctx.root,
            "bin" => ctx.bin,
            "exe" => ctx.exe,
            "id" => ctx.id.ok_or(ExpandError::NoInstance)?,
            "args" => return Err(ExpandError::ArgsNotWhole),
            "ws" => ctx.ws,
            "os" => ctx.os,
            "python" => ctx.python,
            other => return Err(ExpandError::Unknown(other.to_owned())),
        })
    }
}

/// Index of the `}` that closes a `${` whose body starts at `from`.
/// Nested `{...}` (placeholders in a default) and `${...}` are balanced;
/// a `{{` escape is skipped so it never opens a level.
fn find_close(s: &str, from: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut depth = 0usize;
    let mut i = from;
    while i < bytes.len() {
        match bytes[i] {
            b'{' if bytes.get(i + 1) == Some(&b'{') => i += 1,
            b'{' => depth += 1,
            b'}' if depth == 0 => return Some(i),
            b'}' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    None
}

/// `[A-Za-z_][A-Za-z0-9_]*`, the portable environment variable name.
fn is_env_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
