//! Build the dynamic-decode [`DescriptorPool`] for `sb topic listen` from
//! the message vault.
//!
//! One `protoc` run **per style**, never one run over the whole vault.
//! An `import "custom/A.proto"` inside a style is relative to that
//! style's `message_definitions/` directory, so protoc must get exactly
//! that directory as its include root (see `sb_vault::compile_style`).
//! Compiling every style from the vault root instead made every import
//! unresolvable, and because it was a single protoc call, one bad import
//! disabled decoding for every type in the vault.

use std::path::Path;

use anyhow::Result;
use prost_reflect::DescriptorPool;
use sb_vault::Vault;

/// Compile every style in `vault` with `protoc` and merge the resulting
/// descriptor sets into one pool.
///
/// A style that fails to compile is reported on stderr and skipped, so a
/// broken import in one style cannot take decoding away from the others.
/// Errors only when the vault has no styles or none of them compiled.
pub fn build_pool(vault: &Vault, protoc: &Path) -> Result<DescriptorPool> {
    let styles = vault.styles();
    if styles.is_empty() {
        anyhow::bail!("vault is empty");
    }
    let mut pool = DescriptorPool::new();
    let mut loaded = 0usize;
    for style in &styles {
        if vault.list_style(style)?.is_empty() {
            continue;
        }
        match compile_style_into(vault, style, protoc, &mut pool) {
            Ok(()) => loaded += 1,
            Err(e) => eprintln!("warning: dynamic decode skipped for style {style} ({e:#})"),
        }
    }
    if loaded == 0 {
        anyhow::bail!("no style in the vault compiled");
    }
    Ok(pool)
}

fn compile_style_into(
    vault: &Vault,
    style: &str,
    protoc: &Path,
    pool: &mut DescriptorPool,
) -> Result<()> {
    let tmp = tempfile::NamedTempFile::new()?;
    sb_vault::compile_style(vault, style, protoc, tmp.path())?;
    let bytes = std::fs::read(tmp.path())?;
    pool.decode_file_descriptor_set(bytes.as_slice())
        .map_err(|e| anyhow::anyhow!("DescriptorPool::decode: {e}"))?;
    Ok(())
}
