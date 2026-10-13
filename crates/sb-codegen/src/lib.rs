//! L3 codegen — render per-language pub/sub files via minijinja templates.
//!
//! Public surface:
//!
//! ```text
//! let rendered = sb_codegen::render(&CodegenInput { ... })?;
//! std::fs::write(io_dir.join(&rendered.rel_path), &rendered.contents)?;
//! ```
//!
//! The crate is **pure logic**: it does not touch the filesystem (other than
//! `include_dir!` at compile time). Callers handle file IO so tests can run
//! the renderer in memory and snapshot the output.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use include_dir::{Dir, include_dir};
use minijinja::{Environment, value::Value};
use serde::Serialize;

use sb_core::{Encoding, Language, PubSpec, ServiceSpec, SubSpec, TopicRef, Transport};
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

/// Embedded template tree. Layout: `<lang>/<{publisher,subscriber}>_<transport>.<ext>.j2`
/// for pub/sub entries and `<lang>/service_app.<ext>.j2` for services.
const TEMPLATES: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/templates");

// ─────────────────────────────────────────────────────────────────────
// Inputs
// ─────────────────────────────────────────────────────────────────────

/// Which kind of entry we're rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Publisher,
    Subscriber,
    /// A request/response router (`services:` entry), Zenoh only.
    Service,
}

impl Kind {
    /// Subdirectory of the module's `io_dir` the rendered file lands in.
    pub fn dir(self) -> &'static str {
        match self {
            Self::Publisher => "publishers",
            Self::Subscriber => "subscribers",
            Self::Service => "services",
        }
    }
    fn role(self) -> &'static str {
        match self {
            Self::Publisher => "publisher",
            Self::Subscriber => "subscriber",
            Self::Service => "service",
        }
    }
    fn struct_suffix(self) -> &'static str {
        match self {
            Self::Publisher => "Publisher",
            Self::Subscriber => "Subscriber",
            Self::Service => "Service",
        }
    }
    /// Why a template for this kind can be missing; tail of
    /// [`CodegenError::NoTemplate`].
    fn missing_template_hint(self) -> &'static str {
        match self {
            Self::Publisher | Self::Subscriber => "Flutter/Unity have no iceoryx2",
            Self::Service => "services are generated for Rust and Python modules only",
        }
    }
}

/// Name of a module's default service: the router `sb service init`
/// writes to `<io_dir>/service.<ext>`. It serves on the bare service base
/// `<device>/<workspace>/<module>/zenoh/service`; every other service
/// serves on `<device>/<workspace>/<module>/zenoh/service/<name>`.
pub const DEFAULT_SERVICE_NAME: &str = "service";

/// Topic stand-in for [`Kind::Service`] inputs, which have none.
static NO_TOPIC: TopicRef = TopicRef::Owned(String::new());

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
    /// Pub, sub, or service.
    pub kind: Kind,
    /// Identifier used for the filename and struct names: the entry's
    /// `name` (`PubSpec::name`, `SubSpec::name`, `ServiceSpec::name`).
    pub name: &'a str,
    /// Bare topic or verbatim wire key from `*Spec::topic`. Ignored for
    /// [`Kind::Service`].
    pub topic: &'a TopicRef,
    /// Vault name, e.g. `ros2/std/StringStamped`. `None` for
    /// [`Encoding::Raw`] entries and for services; required otherwise.
    pub msg_type: Option<&'a MessageName>,
    /// Payload framing of the entry. Iceoryx2 entries are always
    /// [`Encoding::Proto`] (sb-core rejects anything else at load time).
    pub encoding: Encoding,
    /// Content type of a raw payload (e.g. `text/json`). Raw only.
    pub mime: Option<&'a str>,
    /// Transport. Services are always [`Transport::Zenoh`].
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
    /// Inputs for a publisher. `msg_type` is `spec.msg_type` parsed by the
    /// caller (`None` for a raw entry); encoding and mime come from `spec`.
    pub fn from_pub(
        module: &'a str,
        workspace: &'a str,
        device: &'a str,
        language: Language,
        spec: &'a PubSpec,
        msg_type: Option<&'a MessageName>,
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
            encoding: spec.encoding,
            mime: spec.mime.as_deref(),
            transport: spec.transport,
            message_targets,
        }
    }

    /// Inputs for a subscriber. See [`CodegenInput::from_pub`].
    pub fn from_sub(
        module: &'a str,
        workspace: &'a str,
        device: &'a str,
        language: Language,
        spec: &'a SubSpec,
        msg_type: Option<&'a MessageName>,
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
            encoding: spec.encoding,
            mime: spec.mime.as_deref(),
            transport: spec.transport,
            message_targets,
        }
    }

    /// Inputs for a service router. Renders to `services/<name>.<ext>`.
    pub fn from_service(
        spec: &'a ServiceSpec,
        module: &'a str,
        workspace: &'a str,
        device: &'a str,
        language: Language,
    ) -> Self {
        Self {
            module,
            workspace,
            device,
            language,
            kind: Kind::Service,
            name: &spec.name,
            topic: &NO_TOPIC,
            msg_type: None,
            encoding: Encoding::default(),
            mime: None,
            transport: Transport::Zenoh,
            message_targets: Path::new(""),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// Wire tag: the Zenoh encoding string a publisher stamps on every sample
// ─────────────────────────────────────────────────────────────────────

/// MIME half of a protobuf wire tag.
pub const MIME_PROTOBUF: &str = "application/protobuf";
/// MIME half of a FlatBuffers wire tag.
pub const MIME_FLATBUFFERS: &str = "application/flatbuffers";
/// Wire tag of a raw entry that declares no `mime:`.
pub const MIME_OCTET_STREAM: &str = "application/octet-stream";

/// The Zenoh encoding string an entry's samples carry:
///
/// - proto: `application/protobuf;<style>/<ns>/<Leaf>`
/// - fb: `application/flatbuffers;<style>/<ns>/<Leaf>`
/// - raw: the entry's `mime:`, or `application/octet-stream`
///
/// `None` when a proto or fb entry has no message type. `sb topic list` and
/// `sb topic listen` read the tag back (`sb_discover::parse_wire_tag`).
pub fn wire_tag(
    encoding: Encoding,
    msg_type: Option<&MessageName>,
    mime: Option<&str>,
) -> Option<String> {
    match encoding {
        Encoding::Proto => msg_type.map(|m| format!("{MIME_PROTOBUF};{m}")),
        Encoding::Fb => msg_type.map(|m| format!("{MIME_FLATBUFFERS};{m}")),
        Encoding::Raw => Some(mime.unwrap_or(MIME_OCTET_STREAM).to_owned()),
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
    #[error(
        "no template for {language:?} {transport:?} {kind:?} ({})",
        .kind.missing_template_hint()
    )]
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

/// Path inside the templates dir, e.g. `rust/publisher_zenoh.rs.j2` or
/// `python/service_app.py.j2`.
fn template_rel_path(language: Language, transport: Transport, kind: Kind) -> String {
    let lang = language.as_str();
    let ext = ext_for(language);
    match kind {
        Kind::Service => format!("{lang}/service_app.{ext}.j2"),
        Kind::Publisher | Kind::Subscriber => format!(
            "{lang}/{role}_{transport}.{ext}.j2",
            role = kind.role(),
            transport = transport_template_tag(transport),
        ),
    }
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

#[derive(Debug, Clone, Default, Serialize)]
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
    /// Leaf segment: the bare topic itself, or the last segment of a
    /// verbatim `key:`.
    topic_bare: String,
    /// The wire key, `<device>/<workspace>/<module>/<transport_segment>/<bare>`
    /// for an owned topic (canonical form, no leading `/`), or the
    /// verbatim `key:` value. Built by [`TopicRef::full_key`].
    topic_full: String,
    /// Zenoh key expression: identical to `topic_full`. Zenoh forbids
    /// leading slashes (the canonical form has none); internal `/` is fine.
    topic_zenoh: String,
    /// iceoryx2 service name: identical to `topic_full`. iceoryx2
    /// `ServiceName` accepts `/`; no `__` transliteration is needed.
    topic_iox: String,

    // Transport
    transport: &'static str,
    /// Literal `iox2` / `zenoh` segment embedded in `topic_full`.
    transport_segment: &'static str,
    is_zenoh: bool,
    is_iceoryx: bool,

    /// True for a bare `topic:` entry (so the wire path is
    /// `<device>/<workspace>/<module>/<transport>/<topic>`, every segment
    /// derived from sb config). Templates use this to decide whether to
    /// emit per-segment override constructors: runtime overrides only
    /// make sense for *owned* topics; a verbatim `key:` belongs to the
    /// publisher's instance and stays as written.
    is_owned: bool,

    // Message type. All three are empty for a raw entry (and for a
    // service); templates read them only outside the `is_raw` branch.
    msg_type: String,
    msg_namespace: &'a str,
    msg_leaf: &'a str,

    // Payload encoding
    /// `"proto"` | `"fb"` | `"raw"`.
    encoding: &'static str,
    is_proto: bool,
    is_fb: bool,
    is_raw: bool,
    /// The entry's `mime:` (raw only).
    mime: Option<String>,
    /// Zenoh encoding string stamped on every sample, see [`wire_tag`].
    /// `None` only for services.
    wire_tag: Option<String>,
    /// How doc comments name the payload: `` `<style>/<ns>/<Leaf>` `` for
    /// proto and fb, ``raw `<wire_tag>` bytes`` for raw.
    payload_label: String,

    // Role
    is_publisher: bool,
    is_subscriber: bool,
    is_service: bool,
    role: &'static str,

    // Service
    /// The `services:` entry's name. Empty for pub/sub.
    service_name: &'a str,
    /// True for [`DEFAULT_SERVICE_NAME`], which serves on the bare
    /// `<device>/<workspace>/<module>/zenoh/service` base.
    is_default_service: bool,
}

fn build_ctx<'a>(input: &'a CodegenInput<'a>) -> Result<TemplateCtx<'a>> {
    if input.kind == Kind::Service {
        return Ok(build_service_ctx(input));
    }
    // The iceoryx2 templates always emit the typed flat struct; sb-core
    // refuses any other encoding on iceoryx2 at load time, and so does this.
    if input.transport == Transport::Iceoryx2 && input.encoding != Encoding::Proto {
        bail!(
            "{role} {name:?}: encoding {encoding} is zenoh-only; iceoryx2 payloads are \
             always the struct generated from the .proto",
            role = input.kind.role(),
            name = input.name,
            encoding = input.encoding,
        );
    }
    let msg = match (input.encoding, input.msg_type) {
        (Encoding::Raw, _) => None,
        (_, Some(msg)) => Some(msg),
        (encoding, None) => bail!(
            "{role} {name:?} has encoding {encoding} but no message type",
            role = input.kind.role(),
            name = input.name,
        ),
    };
    let wire_tag = wire_tag(input.encoding, msg, input.mime)
        .expect("a message type is present for every non-raw encoding");

    let transport_segment = input.transport.path_segment();
    let is_owned = input.topic.is_owned();
    // A `key:` entry is honoured verbatim: the user is responsible for the
    // transport segment they want (or no segment at all, for
    // legacy/cross-namespace traffic per §13.7).
    let topic_full =
        input
            .topic
            .full_key(input.device, input.workspace, input.module, input.transport);
    // iceoryx2 0.9 accepts `/` in `ServiceName` (per upstream's own example
    // `My/Funk/ServiceName`), so both transports share the same wire form;
    // only the `iox2`/`zenoh` segment in the path differs.
    let topic_zenoh = topic_full.clone();
    let topic_iox = topic_full.clone();
    let bare = input.topic.leaf().to_string();

    let struct_name_base = to_pascal_case(input.name);
    let struct_name = format!("{struct_name_base}{}", input.kind.struct_suffix());

    // Per-language iox2-flat type file locations. Resolved for every typed
    // entry (cheap path arithmetic); only the iceoryx2 templates actually
    // emit include/import statements that reference them. A raw entry has
    // no type, so all of them stay empty.
    let iox2 = msg.map(|m| Iox2Paths::new(input.message_targets, m));

    // For C++ iceoryx2 only: resolve the proto-derived namespace by
    // peeking the generated header. Required because the proto package
    // varies per message (`swarmbotix.std`→`swarmbotix_std`, `sensor_msgs`
    // verbatim, …) and the vault directory alone doesn't tell us.
    let iox2_cpp_namespace = match (&iox2, msg, input.language, input.transport) {
        (Some(paths), Some(m), Language::Cpp, Transport::Iceoryx2) => {
            read_cpp_namespace(Path::new(&paths.h)).with_context(|| {
                format!(
                    "resolving C++ namespace for iox2 payload {pkg}/{leaf} \
                     (run `sb message compile --iox2 {pkg}/{leaf}` first if the \
                     file is missing)",
                    pkg = m.namespace,
                    leaf = m.leaf,
                )
            })?
        }
        _ => String::new(),
    };
    let iox2 = iox2.unwrap_or_default();

    let payload_label = match msg {
        Some(m) => format!("`{m}`"),
        None => format!("raw `{wire_tag}` bytes"),
    };

    Ok(TemplateCtx {
        module: input.module,
        workspace: input.workspace,
        device: input.device,
        name: input.name,
        struct_name_base,
        struct_name,
        iox2_type_dir: iox2.dir,
        iox2_type_path_rs: iox2.rs,
        iox2_type_path_h: iox2.h,
        iox2_type_path_py: iox2.py,
        iox2_cpp_namespace,
        topic_bare: bare,
        topic_full,
        topic_zenoh,
        topic_iox,
        transport: input.transport.as_str(),
        transport_segment,
        is_zenoh: matches!(input.transport, Transport::Zenoh),
        is_iceoryx: matches!(input.transport, Transport::Iceoryx2),
        is_owned,
        msg_type: msg.map(ToString::to_string).unwrap_or_default(),
        msg_namespace: msg.map_or("", |m| m.namespace.as_str()),
        msg_leaf: msg.map_or("", |m| m.leaf.as_str()),
        encoding: input.encoding.as_str(),
        is_proto: input.encoding == Encoding::Proto,
        is_fb: input.encoding == Encoding::Fb,
        is_raw: input.encoding == Encoding::Raw,
        mime: input.mime.map(str::to_owned),
        wire_tag: Some(wire_tag),
        payload_label,
        is_publisher: matches!(input.kind, Kind::Publisher),
        is_subscriber: matches!(input.kind, Kind::Subscriber),
        is_service: false,
        role: input.kind.role(),
        service_name: "",
        is_default_service: false,
    })
}

/// Context for a [`Kind::Service`] render: the namespace and the service
/// name. Topic, message, and encoding fields stay empty.
fn build_service_ctx<'a>(input: &'a CodegenInput<'a>) -> TemplateCtx<'a> {
    let struct_name_base = to_pascal_case(input.name);
    TemplateCtx {
        module: input.module,
        workspace: input.workspace,
        device: input.device,
        name: input.name,
        struct_name: format!("{struct_name_base}{}", Kind::Service.struct_suffix()),
        struct_name_base,
        transport: Transport::Zenoh.as_str(),
        transport_segment: Transport::Zenoh.path_segment(),
        is_zenoh: true,
        is_service: true,
        role: Kind::Service.role(),
        service_name: input.name,
        is_default_service: input.name == DEFAULT_SERVICE_NAME,
        ..TemplateCtx::default()
    }
}

/// Forward-slashed locations of one message's iox2-flat type files under
/// `<message_targets>/iox2/<namespace>/<leaf>/`. See [`slash_path`].
#[derive(Debug, Default)]
struct Iox2Paths {
    dir: String,
    rs: String,
    h: String,
    py: String,
}

impl Iox2Paths {
    fn new(message_targets: &Path, msg: &MessageName) -> Self {
        let dir = message_targets
            .join("iox2")
            .join(&msg.namespace)
            .join(&msg.leaf);
        let file = |ext: &str| slash_path(&dir.join(format!("{}.{ext}", msg.leaf)));
        Self {
            rs: file("rs"),
            h: file("h"),
            py: file("py"),
            dir: slash_path(&dir),
        }
    }
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

/// Render a single pub/sub or service file. Returns the relative path
/// under the module's `io_dir/` (`publishers/<name>.<ext>`,
/// `subscribers/<name>.<ext>`, or `services/<name>.<ext>`) plus the file
/// contents.
pub fn render(input: &CodegenInput<'_>) -> Result<Rendered> {
    let tpl_path = template_rel_path(input.language, input.transport, input.kind);
    let tpl_file = TEMPLATES.get_file(&tpl_path).ok_or_else(|| {
        anyhow::Error::from(CodegenError::NoTemplate {
            language: input.language,
            transport: input.transport,
            kind: input.kind,
        })
        .context(format!("looking for template at {tpl_path}"))
    })?;
    let ctx = build_ctx(input)?;
    let contents = render_template(&tpl_path, tpl_file.contents(), &ctx)?;

    let rel_path = PathBuf::from(input.kind.dir()).join(format!(
        "{name}.{ext}",
        name = input.name,
        ext = ext_for(input.language)
    ));

    Ok(Rendered { rel_path, contents })
}

/// Render one embedded template against `ctx`; the output always ends in
/// a newline.
fn render_template(tpl_path: &str, src: &[u8], ctx: &TemplateCtx<'_>) -> Result<String> {
    let src = std::str::from_utf8(src).with_context(|| format!("non-utf8 template {tpl_path}"))?;
    let mut env = Environment::new();
    env.add_template(tpl_path, src)
        .with_context(|| format!("parsing template {tpl_path}"))?;
    let mut contents = env
        .get_template(tpl_path)?
        .render(Value::from_serialize(ctx))
        .with_context(|| format!("rendering {tpl_path}"))?;
    if !contents.ends_with('\n') {
        contents.push('\n');
    }
    Ok(contents)
}

// ─────────────────────────────────────────────────────────────────────
// Service router: REST-style req/res over Zenoh.
//
// A service is rendered like any other entry (`render` with
// `Kind::Service`, `<lang>/service_app.<ext>.j2`) into
// `services/<name>.<ext>`. JSON payloads only, Zenoh only, Rust + Python
// only. `render_service_app` keeps the pre-`services:` layout that
// `sb service init` writes: the default service, at `service.<ext>`.
// ─────────────────────────────────────────────────────────────────────

/// Reserved key segment that isolates request/response service routes from
/// pub/sub topics under the same module's `zenoh/` namespace. The default
/// service's base key is `<device>/<workspace>/<module>/zenoh/<SERVICE_SEGMENT>`;
/// a named service appends `/<name>`.
pub const SERVICE_SEGMENT: &str = "service";

/// Inputs for the per-module default service router. Just the namespace;
/// the router logic is fully static.
#[derive(Debug, Clone)]
pub struct ServiceAppInput<'a> {
    pub module: &'a str,
    pub workspace: &'a str,
    pub device: &'a str,
    pub language: Language,
}

/// Render the module's default service ([`DEFAULT_SERVICE_NAME`]) to
/// `service.<ext>` (relative to the io_dir), the file `sb service init`
/// has always written. A thin wrapper over [`render`] with
/// [`Kind::Service`]; only the output path differs from
/// `services/service.<ext>`.
///
/// Only Rust and Python are supported (the FastAPI-style router targets app
/// runtimes, not the Flutter/Unity sandboxes); other languages get a clear
/// error.
pub fn render_service_app(input: &ServiceAppInput<'_>) -> Result<Rendered> {
    if !template_exists(input.language, Transport::Zenoh, Kind::Service) {
        bail!(
            "`sb service init` supports Rust and Python modules only — \
             no service router for {} modules",
            input.language.as_str()
        );
    }
    let spec = ServiceSpec {
        name: DEFAULT_SERVICE_NAME.to_owned(),
    };
    let cg = CodegenInput::from_service(
        &spec,
        input.module,
        input.workspace,
        input.device,
        input.language,
    );
    let mut rendered = render(&cg)?;
    rendered.rel_path = PathBuf::from(format!("service.{}", ext_for(input.language)));
    Ok(rendered)
}

/// True if a template exists for this (language, transport, kind).
/// Flutter / Unity have no iceoryx2 templates, and services exist for
/// Rust and Python only; the CLI uses this to give an actionable error
/// before mutation. `transport` is ignored for [`Kind::Service`].
pub fn template_exists(language: Language, transport: Transport, kind: Kind) -> bool {
    TEMPLATES
        .get_file(template_rel_path(language, transport, kind))
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGETS: &str = "/tmp/sb-fixture/message_targets";

    #[test]
    fn legacy_and_key_configs_render_identical_wire() {
        let legacy: SubSpec = serde_yaml::from_str(
            "name: hello\ntopic: /dev01/demo/pubber/zenoh/hello\ntype: ros2/std/StringStamped\n\
             transport: zenoh\n",
        )
        .unwrap();
        let keyed: SubSpec = serde_yaml::from_str(
            "name: hello\nkey: dev01/demo/pubber/zenoh/hello\ntype: ros2/std/StringStamped\n\
             transport: zenoh\n",
        )
        .unwrap();
        let msg = MessageName::parse("ros2/std/StringStamped").unwrap();
        let targets = Path::new(TARGETS);
        let legacy_in = CodegenInput::from_sub(
            "subber",
            "demo",
            "dev01",
            Language::Rust,
            &legacy,
            Some(&msg),
            targets,
        );
        let keyed_in = CodegenInput::from_sub(
            "subber",
            "demo",
            "dev01",
            Language::Rust,
            &keyed,
            Some(&msg),
            targets,
        );

        let legacy_ctx = build_ctx(&legacy_in).unwrap();
        assert_eq!(legacy_ctx.topic_zenoh, "dev01/demo/pubber/zenoh/hello");
        assert_eq!(
            build_ctx(&keyed_in).unwrap().topic_zenoh,
            "dev01/demo/pubber/zenoh/hello"
        );

        let a = render(&legacy_in).unwrap();
        let b = render(&keyed_in).unwrap();
        assert_eq!(a.rel_path, b.rel_path);
        assert_eq!(
            a.contents, b.contents,
            "legacy topic: /... and key: must render identically"
        );
    }

    #[test]
    fn wire_tag_per_encoding() {
        let msg = MessageName::parse("ros2/std/String").unwrap();
        assert_eq!(
            wire_tag(Encoding::Proto, Some(&msg), None).as_deref(),
            Some("application/protobuf;ros2/std/String")
        );
        assert_eq!(
            wire_tag(Encoding::Fb, Some(&msg), None).as_deref(),
            Some("application/flatbuffers;ros2/std/String")
        );
        assert_eq!(
            wire_tag(Encoding::Raw, None, Some("text/json")).as_deref(),
            Some("text/json")
        );
        assert_eq!(
            wire_tag(Encoding::Raw, None, None).as_deref(),
            Some("application/octet-stream")
        );
        assert_eq!(wire_tag(Encoding::Proto, None, None), None);
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
