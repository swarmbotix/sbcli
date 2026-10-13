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

/// One style's compiled `FileDescriptorSet` (protoc `--include_imports`
/// output, encoded). The same bytes feed the protobuf [`DescriptorPool`]
/// and, in `sb topic listen`, the iox2 flattening that maps iceoryx2
/// payloads back to vault messages, so protoc runs once per style.
#[derive(Debug, Clone)]
pub struct CompiledStyle {
    pub style: String,
    pub descriptor_set: Vec<u8>,
}

/// Compile every style in `vault` with `protoc` and merge the resulting
/// descriptor sets into one pool.
///
/// A style that fails to compile is reported on stderr and skipped, so a
/// broken import in one style cannot take decoding away from the others.
/// Errors only when the vault has no styles or none of them compiled.
pub fn build_pool(vault: &Vault, protoc: &Path) -> Result<DescriptorPool> {
    pool_from_styles(&compile_styles(vault, protoc)?)
}

/// Compile every non-empty style in `vault`, one `protoc` run each.
///
/// Same failure policy as [`build_pool`]: a style that fails is warned
/// about on stderr and skipped; errors only when the vault has no styles
/// or none compiled.
pub fn compile_styles(vault: &Vault, protoc: &Path) -> Result<Vec<CompiledStyle>> {
    let styles = vault.styles();
    if styles.is_empty() {
        anyhow::bail!("vault is empty");
    }
    let mut compiled = Vec::new();
    for style in &styles {
        if vault.list_style(style)?.is_empty() {
            continue;
        }
        match compile_style(vault, style, protoc) {
            Ok(descriptor_set) => compiled.push(CompiledStyle {
                style: style.clone(),
                descriptor_set,
            }),
            Err(e) => eprintln!("warning: dynamic decode skipped for style {style} ({e:#})"),
        }
    }
    if compiled.is_empty() {
        anyhow::bail!("no style in the vault compiled");
    }
    Ok(compiled)
}

/// Merge compiled styles into one pool. A descriptor set the pool rejects
/// is warned about and skipped; errors only when none could be added.
pub fn pool_from_styles(styles: &[CompiledStyle]) -> Result<DescriptorPool> {
    let mut pool = DescriptorPool::new();
    let mut loaded = 0usize;
    for s in styles {
        match pool.decode_file_descriptor_set(s.descriptor_set.as_slice()) {
            Ok(()) => loaded += 1,
            Err(e) => eprintln!(
                "warning: dynamic decode skipped for style {} (DescriptorPool::decode: {e})",
                s.style
            ),
        }
    }
    if loaded == 0 {
        anyhow::bail!("no style in the vault compiled");
    }
    Ok(pool)
}

fn compile_style(vault: &Vault, style: &str, protoc: &Path) -> Result<Vec<u8>> {
    let tmp = tempfile::NamedTempFile::new()?;
    sb_vault::compile_style(vault, style, protoc, tmp.path())?;
    Ok(std::fs::read(tmp.path())?)
}
