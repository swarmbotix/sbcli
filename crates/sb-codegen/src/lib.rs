//! L3 codegen — render per-language pub/sub files via minijinja templates.
//!
//! Public surface:
//!
//! ```ignore
//! let rendered = sb_codegen::render(&CodegenInput { ... })?;
//! std::fs::write(io_dir.join(&rendered.rel_path), &rendered.contents)?;
//! ```
//!
//! The crate is **pure logic**: it does not touch the filesystem (other than
//! `include_dir!` at compile time). Callers handle file IO so tests can run
//! the renderer in memory and snapshot the output.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use include_dir::{Dir, include_dir};
use minijinja::{Environment, value::Value};
use serde::Serialize;

use sb_core::{Language, PubSpec, SubSpec, Transport};
use sb_vault::MessageName;

/// Render a path with forward slashes, whatever the host separator is.
///
/// These strings are baked verbatim into generated source, so a Windows
/// `\` is not cosmetic:
///
/// * Rust — `include!("C:\…\Foo.rs")` does not compile; `\U`, `\s`, `\t`
///   are read as string escapes (invalid, or silently wrong).
/// * C++ — a `\` in an `#include "…"` header-name is undefined behavior
///   per [lex.header]; MSVC tolerates it, other toolchains need not.
/// * Python — survives only because the emitted literal is a raw string.
///
/// Windows accepts `/` in every one of these, so normalizing is both the
/// portable and the simpler choice. It also keeps codegen goldens
/// byte-identical across platforms.
fn slash_path(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// Embedded template tree. Layout: `<lang>/<{publisher,subscriber}>_<transport>.<ext>.j2`.
const TEMPLATES: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/templates");

// ─────────────────────────────────────────────────────────────────────
// Inputs
// ─────────────────────────────────────────────────────────────────────

/// Which kind of entry we're rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Publisher,
    Subscriber,
}

impl Kind {
    fn dir(self) -> &'static str {
        match self {
            Self::Publisher => "publishers",
            Self::Subscriber => "subscribers",
        }
    }
    fn role(self) -> &'static str {
        match self {
            Self::Publisher => "publisher",
            Self::Subscriber => "subscriber",
        }
    }
}

/// Borrowed inputs to a single render.
#[derive(Debug, Clone)]
pub struct CodegenInput<'a> {
    /// Module name as written in `sb.dev.yml`.
    pub module: &'a str,
    /// Workspace name (from the active marker / `flow.yaml` parent).
    pub workspace: &'a str,
    /// Device identifier (from merged `sb.config.yml`).
    pub device: &'a str,
    /// Target language for codegen.
    pub language: Language,
    /// Pub or sub.
    pub kind: Kind,
    /// Identifier used for the filename and struct names.
    /// For publishers this is `PubSpec::name`; for subscribers,
    /// `SubSpec::name`.
    pub name: &'a str,
    /// Bare or fully-qualified topic from `*Spec::topic`.
    pub topic: &'a str,
    /// Vault name, e.g. `std/StringStamped`.
    pub msg_type: &'a MessageName,
    /// Transport.
    pub transport: Transport,
    /// Absolute path to the resolved `message_targets` root (from
    /// `sb config get message_targets`). The codegen joins this with
    /// `iox2/<namespace>/<leaf>/<leaf>.<ext>` to locate the iox2-flat
    /// payload type and bakes that absolute path into the generated
    /// pub/sub file so the type can be pulled in with no user
    /// scaffolding. Only consulted by the iceoryx2 templates; Zenoh
    /// templates ignore it (they ship raw bytes).
    pub message_targets: &'a Path,
}

impl<'a> CodegenInput<'a> {
    pub fn from_pub(
        module: &'a str,
        workspace: &'a str,
        device: &'a str,
        language: Language,
        spec: &'a PubSpec,
        msg_type: &'a MessageName,
        message_targets: &'a Path,
    ) -> Self {
        Self {
            module,
            workspace,
            device,
            language,
            kind: Kind::Publisher,
            name: &spec.name,
            topic: &spec.topic,
            msg_type,
            transport: spec.transport,
            message_targets,
        }
    }

    pub fn from_sub(
        module: &'a str,
        workspace: &'a str,
        device: &'a str,
        language: Language,
        spec: &'a SubSpec,
        msg_type: &'a MessageName,
        message_targets: &'a Path,
    ) -> Self {
        Self {
            module,
            workspace,
            device,
            language,
            kind: Kind::Subscriber,
            name: &spec.name,
            topic: &spec.topic,
            msg_type,
            transport: spec.transport,
            message_targets,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// Output
// ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Rendered {
    /// Path relative to `<module>/<io_dir>/`, e.g. `publishers/hello.rs`.
    pub rel_path: PathBuf,
    pub contents: String,
}

// ─────────────────────────────────────────────────────────────────────
// Errors
// ─────────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum CodegenError {
    #[error("no template for {language:?} {transport:?} {kind:?} (Flutter/Unity have no iceoryx2)")]
    NoTemplate {
        language: Language,
        transport: Transport,
        kind: Kind,
    },
}

// ─────────────────────────────────────────────────────────────────────
// File extension per language
// ─────────────────────────────────────────────────────────────────────

fn ext_for(lang: Language) -> &'static str {
    match lang {
        Language::Rust => "rs",
        Language::Python => "py",
        Language::Cpp => "cpp",
        Language::Flutter => "dart",
        Language::Unity => "cs",
    }
}

/// Path inside the templates dir, e.g. `rust/publisher_zenoh.rs.j2`.
fn template_rel_path(input: &CodegenInput<'_>) -> String {
    format!(
        "{lang}/{role}_{transport}.{ext}.j2",
        lang = input.language.as_str(),
        role = input.kind.role(),
        transport = transport_template_tag(input.transport),
        ext = ext_for(input.language),
    )
}

fn transport_template_tag(t: Transport) -> &'static str {
    match t {
        // File-system safe tag: `iceoryx` (no `2`) keeps template filenames
        // sane. The user-facing flag is `--iox2`.
        Transport::Iceoryx2 => "iceoryx",
        Transport::Zenoh => "zenoh",
    }
}

// ─────────────────────────────────────────────────────────────────────
// Topic + struct name derivation (exposed to templates via `ctx`)
// ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
struct TemplateCtx<'a> {
    // Identifiers
    module: &'a str,
    workspace: &'a str,
    device: &'a str,
    name: &'a str,
    /// PascalCase form of `name`, e.g. `hello_world` → `HelloWorld`.
    struct_name_base: String,
    /// `struct_name_base + "Publisher"` or `+ "Subscriber"`.
    struct_name: String,

    // Absolute paths to the iox2-flat payload type files emitted by
    // `sb message compile --iox2`. Baked into the generated pub/sub so
    // users don't have to wire imports/includes manually. Only the
    // iceoryx2 templates read these.
    /// `<message_targets>/iox2/<vault_dir>/<leaf>/`. Always forward-slashed
    /// — see [`slash_path`].
    iox2_type_dir: String,
    iox2_type_path_rs: String,
    iox2_type_path_h: String,
    iox2_type_path_py: String,
    /// C++ namespace declared inside `<leaf>.h` — derived from the
    /// proto package (varies per message: `swarmbotix_std`, `sensor_msgs`,
    /// `geometry_msgs`, …). Empty for non-cpp / non-iox2 contexts.
    iox2_cpp_namespace: String,

    // Topic forms
    /// Bare leaf — whatever the user typed, with leading `/` stripped if present.
    topic_bare: String,
    /// `/<device>/<workspace>/<module>/<transport_segment>/<bare_leaf>` (canonical, leading /).
    /// If the user passed a fully-qualified topic, that wins verbatim.
    topic_full: String,
    /// Zenoh key expression — `topic_full` without the leading `/`. Zenoh
    /// forbids leading slashes; internal `/` is fine.
    topic_zenoh: String,
    /// iceoryx2 service name — identical to `topic_zenoh`. iceoryx2
    /// `ServiceName` accepts `/`; no `__` transliteration is needed.
    topic_iox: String,

    // Transport
    transport: &'static str,
    /// Literal `iox2` / `zenoh` segment embedded in `topic_full`.
    transport_segment: &'static str,
    is_zenoh: bool,
    is_iceoryx: bool,

    /// True when the user passed a bare topic (so the wire path is
    /// `/<device>/<workspace>/<module>/<transport>/<topic>`, every segment
    /// derived from sb config). Templates use this to decide whether to
    /// emit per-segment override constructors — runtime overrides only
    /// make sense for *owned* topics; foreign leading-slash topics belong
    /// to the publisher's instance and stay verbatim.
    is_owned: bool,

    // Message type
    msg_type: String,
    msg_namespace: &'a str,
    msg_leaf: &'a str,

    // Role
    is_publisher: bool,
    is_subscriber: bool,
    role: &'static str,
}

fn build_ctx<'a>(input: &'a CodegenInput<'a>) -> Result<TemplateCtx<'a>> {
    let transport_segment = input.transport.path_segment();
    let is_owned = !input.topic.starts_with('/');
    let topic_full = if is_owned {
        format!(
            "/{}/{}/{}/{}/{}",
            input.device, input.workspace, input.module, transport_segment, input.topic
        )
    } else {
        // Fully-qualified topic from the user — honor verbatim. The user
        // is responsible for the transport segment they want (or no
        // segment at all, for legacy/cross-namespace traffic per §13.7).
        input.topic.to_string()
    };
    let trimmed = topic_full.trim_start_matches('/').to_string();
    // iceoryx2 0.9 accepts `/` in `ServiceName` (per upstream's own example
    // `My/Funk/ServiceName`), so both transports share the same wire form
    // — only the `iox2`/`zenoh` segment in the path differs.
    let topic_iox = trimmed.clone();
    let bare = input
        .topic
        .rsplit('/')
        .next()
        .unwrap_or(input.topic)
        .to_string();

    let struct_name_base = to_pascal_case(input.name);
    let struct_name = format!(
        "{}{}",
        struct_name_base,
        match input.kind {
            Kind::Publisher => "Publisher",
            Kind::Subscriber => "Subscriber",
        }
    );

    // Per-language iox2-flat type file locations. Resolved unconditionally
    // (cheap path arithmetic); only the iceoryx2 templates actually emit
    // include/import statements that reference them.
    let iox2_type_dir_pb = input
        .message_targets
        .join("iox2")
        .join(&input.msg_type.namespace)
        .join(&input.msg_type.leaf);
    let iox2_type_dir = slash_path(&iox2_type_dir_pb);
    let iox2_type_path_rs =
        slash_path(&iox2_type_dir_pb.join(format!("{}.rs", input.msg_type.leaf)));
    let iox2_type_path_h = slash_path(&iox2_type_dir_pb.join(format!("{}.h", input.msg_type.leaf)));
    let iox2_type_path_py =
        slash_path(&iox2_type_dir_pb.join(format!("{}.py", input.msg_type.leaf)));

    // For C++ iceoryx2 only: resolve the proto-derived namespace by
    // peeking the generated header. Required because the proto package
    // varies per message (`swarmbotix.std`→`swarmbotix_std`, `sensor_msgs`
    // verbatim, …) and the vault directory alone doesn't tell us.
    let iox2_cpp_namespace = if matches!(
        (input.language, input.transport),
        (Language::Cpp, Transport::Iceoryx2)
    ) {
        read_cpp_namespace(Path::new(&iox2_type_path_h)).with_context(|| {
            format!(
                "resolving C++ namespace for iox2 payload {pkg}/{leaf} \
                 (run `sb message compile --iox2 {pkg}/{leaf}` first if the \
                 file is missing)",
                pkg = input.msg_type.namespace,
                leaf = input.msg_type.leaf,
            )
        })?
    } else {
        String::new()
    };

    Ok(TemplateCtx {
        module: input.module,
        workspace: input.workspace,
        device: input.device,
        name: input.name,
        struct_name_base,
        struct_name,
        iox2_type_dir,
        iox2_type_path_rs,
        iox2_type_path_h,
        iox2_type_path_py,
        iox2_cpp_namespace,
        topic_bare: bare,
        topic_full: topic_full.clone(),
        topic_zenoh: trimmed,
        topic_iox,
        transport: input.transport.as_str(),
        transport_segment,
        is_zenoh: matches!(input.transport, Transport::Zenoh),
        is_iceoryx: matches!(input.transport, Transport::Iceoryx2),
        is_owned,
        msg_type: format!("{}", input.msg_type),
        msg_namespace: &input.msg_type.namespace,
        msg_leaf: &input.msg_type.leaf,
        is_publisher: matches!(input.kind, Kind::Publisher),
        is_subscriber: matches!(input.kind, Kind::Subscriber),
        role: input.kind.role(),
    })
}

/// Open the iox2-flat C++ header and pull the `namespace X {` token. The
/// emitter writes exactly one such line per file (see sb-iox2-typegen's
/// cpp.rs), so a single regex match is enough.
fn read_cpp_namespace(header: &Path) -> Result<String> {
    let contents =
        std::fs::read_to_string(header).with_context(|| format!("reading {}", header.display()))?;
    for line in contents.lines() {
        let line = line.trim_start();
        if let Some(rest) = line.strip_prefix("namespace ") {
            let ns: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !ns.is_empty() {
                return Ok(ns);
            }
        }
    }
    anyhow::bail!("no `namespace X {{` found in {}", header.display())
}

/// Convert `snake_case` / `kebab-case` / `camelCase` identifiers to
/// `PascalCase`. Non-alphanumeric chars act as separators. Leading digits
/// are kept (the L2 identifier validator guarantees an alpha prefix, so
/// this is a no-op for legitimate input).
fn to_pascal_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut capitalize_next = true;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            if capitalize_next {
                out.extend(c.to_uppercase());
                capitalize_next = false;
            } else {
                out.push(c);
            }
        } else {
            capitalize_next = true;
        }
    }
    out
}

// ─────────────────────────────────────────────────────────────────────
// Render
// ─────────────────────────────────────────────────────────────────────

/// Render a single pub/sub file. Returns the relative path under the
/// module's `io_dir/` plus the file contents.
pub fn render(input: &CodegenInput<'_>) -> Result<Rendered> {
    let tpl_path = template_rel_path(input);
    let tpl_file = TEMPLATES.get_file(&tpl_path).ok_or_else(|| {
        anyhow::Error::from(CodegenError::NoTemplate {
            language: input.language,
            transport: input.transport,
            kind: input.kind,
        })
        .context(format!("looking for template at {tpl_path}"))
    })?;
    let src = std::str::from_utf8(tpl_file.contents())
        .with_context(|| format!("non-utf8 template {tpl_path}"))?;

    let mut env = Environment::new();
    env.add_template(&tpl_path, src)
        .with_context(|| format!("parsing template {tpl_path}"))?;
    let tpl = env.get_template(&tpl_path)?;
    let ctx = build_ctx(input)?;
    let contents = tpl
        .render(Value::from_serialize(&ctx))
        .with_context(|| format!("rendering {tpl_path}"))?;

    let mut contents = contents;
    if !contents.ends_with('\n') {
        contents.push('\n');
    }

    let rel_path = PathBuf::from(input.kind.dir()).join(format!(
        "{name}.{ext}",
        name = input.name,
        ext = ext_for(input.language)
    ));

    Ok(Rendered { rel_path, contents })
}

// ─────────────────────────────────────────────────────────────────────
// Service router (`sb service init`) — REST-style req/res over Zenoh.
//
// Distinct from the per-entry pub/sub `render`: this emits ONE whole-module
// router file (`service.<ext>`) the user writes their own routes against.
// JSON payloads only, Zenoh only, Rust + Python only. No pub/sub state is
// touched.
// ─────────────────────────────────────────────────────────────────────

/// Reserved key segment that isolates request/response service routes from
/// pub/sub topics under the same module's `zenoh/` namespace. The full base
/// key is `<device>/<workspace>/<module>/zenoh/<SERVICE_SEGMENT>`.
pub const SERVICE_SEGMENT: &str = "service";

/// Inputs for the per-module service router. Just the namespace — the router
/// logic is fully static; only these three values get filled in.
#[derive(Debug, Clone)]
pub struct ServiceAppInput<'a> {
    pub module: &'a str,
    pub workspace: &'a str,
    pub device: &'a str,
    pub language: Language,
}

/// Copy the static service router for a module, filling only the three
/// namespace sentinels (`__SB_DEVICE__`, `__SB_WORKSPACE__`, `__SB_MODULE__`).
/// Returns `service.<ext>` (relative to the io_dir) and its contents.
///
/// The embedded `templates/<lang>/service_app.<ext>` files are real,
/// directly-lintable source — NOT minijinja templates. This is a verbatim copy
/// plus three string substitutions, no template engine.
///
/// Only Rust and Python are supported — the FastAPI-style router targets app
/// runtimes, not the Flutter/Unity sandboxes. Unsupported languages return a
/// clear error.
pub fn render_service_app(input: &ServiceAppInput<'_>) -> Result<Rendered> {
    let ext = ext_for(input.language);
    let src_path = format!("{lang}/service_app.{ext}", lang = input.language.as_str());
    let file = TEMPLATES.get_file(&src_path).ok_or_else(|| {
        anyhow::anyhow!(
            "`sb service init` supports Rust and Python modules only — \
             no service router for {} modules",
            input.language.as_str()
        )
    })?;
    let src = std::str::from_utf8(file.contents())
        .with_context(|| format!("non-utf8 router source {src_path}"))?;

    let mut contents = src
        .replace("__SB_DEVICE__", input.device)
        .replace("__SB_WORKSPACE__", input.workspace)
        .replace("__SB_MODULE__", input.module);
    if !contents.ends_with('\n') {
        contents.push('\n');
    }

    Ok(Rendered {
        rel_path: PathBuf::from(format!("service.{ext}")),
        contents,
    })
}

/// True if a template exists for this (language, transport, kind).
/// Flutter / Unity have no iceoryx2 templates — the CLI uses this to
/// give an actionable error before mutation.
pub fn template_exists(language: Language, transport: Transport, kind: Kind) -> bool {
    let stub = CodegenInput {
        module: "",
        workspace: "",
        device: "",
        language,
        kind,
        name: "",
        topic: "",
        // Build a dummy MessageName via parse; cannot fail with hard-coded
        // valid input.
        msg_type: &MessageName {
            style: "ros2".into(),
            namespace: "std".into(),
            leaf: "Header".into(),
        },
        transport,
        message_targets: Path::new(""),
    };
    TEMPLATES.get_file(template_rel_path(&stub)).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pascal_case_basic_separators() {
        assert_eq!(to_pascal_case("hello_world"), "HelloWorld");
        assert_eq!(to_pascal_case("hello-world"), "HelloWorld");
        assert_eq!(to_pascal_case("helloWorld"), "HelloWorld");
        assert_eq!(to_pascal_case("hello"), "Hello");
        assert_eq!(to_pascal_case("HELLO"), "HELLO");
    }

    #[test]
    fn template_rel_path_for_rust_zenoh_publisher() {
        let msg = MessageName::parse("ros2/std/StringStamped").unwrap();
        let input = CodegenInput {
            module: "pubber",
            workspace: "demo",
            device: "dev01",
            language: Language::Rust,
            kind: Kind::Publisher,
            name: "hello",
            topic: "hello",
            msg_type: &msg,
            transport: Transport::Zenoh,
            message_targets: Path::new("/tmp/sb-fixture/message_targets"),
        };
        assert_eq!(template_rel_path(&input), "rust/publisher_zenoh.rs.j2");
    }

    #[test]
    fn topic_derivation_appends_full_prefix_when_bare() {
        let msg = MessageName::parse("ros2/std/StringStamped").unwrap();
        let input = CodegenInput {
            module: "pubber",
            workspace: "demo",
            device: "dev01",
            language: Language::Rust,
            kind: Kind::Publisher,
            name: "hello",
            topic: "hello",
            msg_type: &msg,
            transport: Transport::Zenoh,
            message_targets: Path::new("/tmp/sb-fixture/message_targets"),
        };
        let ctx = build_ctx(&input).unwrap();
        assert_eq!(ctx.topic_full, "/dev01/demo/pubber/zenoh/hello");
        assert_eq!(ctx.topic_zenoh, "dev01/demo/pubber/zenoh/hello");
        // iox2 wire form uses `/` (no `__` transliteration) — identical to
        // topic_zenoh, only the `iox2`/`zenoh` segment differs per entry.
        assert_eq!(ctx.topic_iox, "dev01/demo/pubber/zenoh/hello");
        assert_eq!(ctx.topic_bare, "hello");
        assert_eq!(ctx.transport_segment, "zenoh");
        assert!(ctx.is_owned, "bare-name input must be flagged as owned");
    }

    #[test]
    fn topic_derivation_uses_iox2_segment_for_iceoryx_transport() {
        let msg = MessageName::parse("ros2/std/ImageStamped").unwrap();
        let input = CodegenInput {
            module: "camera",
            workspace: "demo",
            device: "dev01",
            language: Language::Rust,
            kind: Kind::Publisher,
            name: "image_raw",
            topic: "image_raw",
            msg_type: &msg,
            transport: Transport::Iceoryx2,
            message_targets: Path::new("/tmp/sb-fixture/message_targets"),
        };
        // Rust template needs only path arithmetic — no .h read. C++ test
        // skipped here to avoid coupling this unit test to a real on-disk
        // header (snapshot tests in sb-cli cover the C++ slice end-to-end).
        let ctx = build_ctx(&input).unwrap();
        assert_eq!(ctx.topic_full, "/dev01/demo/camera/iox2/image_raw");
        // iceoryx2 ServiceName accepts `/` (see iceoryx2 0.9 docs:
        // `ServiceName::new("My/Funk/ServiceName")`), so the wire form is
        // the canonical 5-segment slash form, no `__` encoding.
        assert_eq!(ctx.topic_iox, "dev01/demo/camera/iox2/image_raw");
        assert_eq!(ctx.transport_segment, "iox2");
    }

    #[test]
    fn topic_derivation_passes_through_fully_qualified_input() {
        let msg = MessageName::parse("ros2/std/StringStamped").unwrap();
        let input = CodegenInput {
            module: "pubber",
            workspace: "ws",
            device: "alt01",
            language: Language::Rust,
            kind: Kind::Publisher,
            name: "hello",
            topic: "/other/scope/path/leaf",
            msg_type: &msg,
            transport: Transport::Zenoh,
            message_targets: Path::new("/tmp/sb-fixture/message_targets"),
        };
        let ctx = build_ctx(&input).unwrap();
        assert_eq!(ctx.topic_full, "/other/scope/path/leaf");
        assert_eq!(ctx.topic_zenoh, "other/scope/path/leaf");
        assert_eq!(ctx.topic_bare, "leaf");
        assert!(
            !ctx.is_owned,
            "leading-slash input must NOT be flagged as owned"
        );
    }

    #[test]
    fn template_exists_negative_for_flutter_iceoryx() {
        assert!(!template_exists(
            Language::Flutter,
            Transport::Iceoryx2,
            Kind::Publisher
        ));
    }

    #[test]
    fn service_app_renders_python_with_baked_namespace() {
        let r = render_service_app(&ServiceAppInput {
            module: "api",
            workspace: "ips",
            device: "elcom",
            language: Language::Python,
        })
        .unwrap();
        assert_eq!(r.rel_path, PathBuf::from("service.py"));
        assert!(
            !r.contents.contains("__SB_"),
            "unfilled namespace sentinel left"
        );
        assert!(r.contents.contains(r#"DEVICE = "elcom""#));
        assert!(r.contents.contains(r#"WORKSPACE = "ips""#));
        assert!(r.contents.contains(r#"MODULE = "api""#));
        assert!(r.contents.contains(r#"SERVICE_SEGMENT = "service""#));
        assert!(r.contents.contains("class ServiceApp"));
        assert!(r.contents.contains("def get(self, route"));
    }

    #[test]
    fn service_app_renders_rust_with_baked_namespace() {
        let r = render_service_app(&ServiceAppInput {
            module: "api",
            workspace: "ips",
            device: "elcom",
            language: Language::Rust,
        })
        .unwrap();
        assert_eq!(r.rel_path, PathBuf::from("service.rs"));
        assert!(
            !r.contents.contains("__SB_"),
            "unfilled namespace sentinel left"
        );
        assert!(r.contents.contains(r#"pub const MODULE: &str = "api";"#));
        assert!(
            r.contents
                .contains(r#"pub const SERVICE_SEGMENT: &str = "service";"#)
        );
        assert!(r.contents.contains("pub struct ServiceApp"));
        assert!(
            r.contents
                .contains("pub fn start(&mut self) -> zenoh::Result<()>")
        );
    }

    #[test]
    fn service_app_rejects_unsupported_language() {
        let err = render_service_app(&ServiceAppInput {
            module: "api",
            workspace: "ips",
            device: "elcom",
            language: Language::Flutter,
        })
        .unwrap_err();
        assert!(err.to_string().contains("Rust and Python"));
    }

    /// Every embedded template is LF-only, on every platform.
    ///
    /// `include_dir!` bakes the working tree's bytes into the binary, so a
    /// checkout that translated line endings (Git for Windows defaults to
    /// `core.autocrlf=true`) ships CRLF templates, and every rendered file
    /// then carries CRLF while the LF goldens do not. That failure is
    /// invisible in a diff and reads as whole-file content drift in four
    /// snapshot suites at once. `.gitattributes` pins these to LF; this test
    /// is what notices if that pin is lost or a template lands outside it.
    #[test]
    fn embedded_templates_are_lf_only() {
        fn walk(dir: &Dir<'_>, offenders: &mut Vec<String>) {
            for file in dir.files() {
                if file.contents().contains(&b'\r') {
                    offenders.push(file.path().display().to_string());
                }
            }
            for sub in dir.dirs() {
                walk(sub, offenders);
            }
        }

        let mut offenders = Vec::new();
        walk(&TEMPLATES, &mut offenders);
        assert!(
            offenders.is_empty(),
            "templates embedded with CR bytes, expected LF-only: {offenders:?}\n\
             re-checkout with the `eol=lf` pin in .gitattributes in place"
        );
    }
}
