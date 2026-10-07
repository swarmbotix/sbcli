//! `sb` — the swarmbotix CLI binary.
//!
//! L1 surface: `doctor`, `message *`, `config *`. Later levels add verbs.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use sb_config::LoadInputs;
use sb_core::{Language, SbCliConfig, Transport};
use sb_pubsub::{
    Action, Collider, ConflictPolicy, ModuleHandle, PubAddArgs, PubEditArgs, Scope, SubAddArgs,
    find_pub_colliders, find_sub_colliders, format_colliders, list as pubsub_list, render_table,
    resolve_module,
};
use sb_vault::{MessageName, Vault};
use sb_workspace::{InitOptions, SbHome, active_workspace};

// `--version` is a plain flag rather than clap's built-in one, so it can add
// the update notice. `arg_required_else_help` and `override_usage` keep a bare
// `sb` (help on stderr, exit 2) and the usage line exactly as they were when
// the subcommand was required.
#[derive(Parser)]
#[command(
    name = "sb",
    about = "swarmbotix — unified pub/sub tooling for Rust / Python / C++ / Flutter / Unity",
    arg_required_else_help = true,
    override_usage = "sb <COMMAND>"
)]
struct Cli {
    /// Print version, and whether a newer release exists (set SB_NO_UPDATE_CHECK=1 to skip the check)
    // Listed after `-h, --help`, where clap's built-in flag used to sit.
    #[arg(long, short = 'V', global = false, display_order = 1000)]
    version: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Validate this host's tooling against sb.config.yml.
    Doctor,
    /// Manage the message vault (`~/.swarmbotix/message_definitions/`).
    #[command(subcommand)]
    Message(MessageCmd),
    /// Edit `sb.config.yml` (interactive or set a single key).
    #[command(subcommand)]
    Config(ConfigCmd),
    /// Manage swarmbotix workspaces under `~/.swarmbotix/workspaces/`.
    #[command(subcommand)]
    Ws(WsCmd),
    /// Adopt an existing project directory as a swarmbotix module.
    Init(InitArgs),
    /// List publishers and subscribers across the resolved module.
    List(ListArgs),
    /// Manage publishers (alias for `sb pub *`).
    #[command(subcommand)]
    Pub(PubCmd),
    /// Manage subscribers.
    #[command(subcommand)]
    Sub(SubCmd),
    /// REST-style request/response over Zenoh (FastAPI-style route router).
    #[command(subcommand)]
    Service(ServiceCmd),
    /// Inspect live topics — list, listen, or one-shot publish.
    #[command(subcommand)]
    Topic(TopicCmd),
    /// Launch every module in the active workspace in tmux.
    Up,
    /// Launch a single module — idempotent (kill + restart if already up).
    ///
    /// Any args after `<module>` are forwarded verbatim to the module's
    /// `runscript.bash`. Use `--` if your args clash with `sb` flags.
    Run(RunArgs),
    /// Stop a single module by running its `stopscript.bash`.
    ///
    /// Mirror of `sb run`: same resolution (active workspace's
    /// flow.yaml), same tmux wrap (`sb-<workspace>` session, one window
    /// per module, idempotent respawn), passthrough args appended to
    /// `stopscript.bash`. Errors if the module has no stopscript.
    Stop(StopArgs),
    /// Kill the workspace's tmux session.
    Down,
    /// Print the workspace's session name (or exec `tmux attach` in a TTY).
    Attach,
    /// Freeze a module for production — write sb.prd.yml next to sb.dev.yml.
    Gopro(GoproArgs),
    /// Install an app package from a git URL or a local folder; then run it as `sb <name> ...`.
    ///
    /// The argument is either a git URL or a local folder that holds an
    /// `sb.app.yml` manifest. Git URLs are anything starting with https://,
    /// http://, ssh://, git://, file:// or git@ (as in git@github.com:org/repo.git),
    /// plus any path ending in .git. Append @<branch-or-tag> to a git URL to pin
    /// it (refs containing `/` cannot be pinned this way).
    ///
    /// A git package is cloned (shallow) into <sb_home>/apps/<name>/, where <name>
    /// comes from its manifest. Installing the same URL again pulls that clone in
    /// place; a different @<ref> replaces it. A local folder is registered where
    /// it is: never copied, never deleted, and edits to it apply on the next run.
    ///
    /// The install step then depends on the manifest's `kind`: `none` does
    /// nothing more; `docker` runs `docker pull <image>` when the manifest sets
    /// `prefetch: true` or --prefetch is given (otherwise the app pulls on first
    /// run); `host` runs the manifest's `host.install` script with bash from the
    /// package folder, and a non-zero exit aborts the install. Binaries listed in
    /// `requires` that are missing from PATH produce warnings, not errors.
    ///
    /// Finally the app is recorded in <sb_home>/apps.yml. From then on,
    /// `sb <name> [args...]` runs the package's entry in your current directory
    /// with your arguments, and exits with the app's exit code. No PATH change is
    /// needed. An app name may not be an sb built-in command, and a name already
    /// taken by another package must be freed first with `sb app remove <name>`.
    #[command(after_long_help = INSTALL_EXAMPLES)]
    Install(InstallArgs),
    /// Create and manage app packages: init, install, list, info, update, remove.
    ///
    /// Package authors run `sb app init` once in their repository. Users run
    /// `sb install <url|path>` and then `sb <name> [args...]`; the other verbs
    /// manage what is installed (registry: <sb_home>/apps.yml).
    #[command(subcommand)]
    App(AppCmd),
    /// Update sb itself to the latest release, or to the one given with --version.
    ///
    /// Looks up the latest release of github.com/swarmbotix/sbcli, downloads the
    /// zip for this platform (swarmbotix-<version>-<platform>.zip) and its .sha256
    /// into a temporary folder, verifies the checksum, unpacks the zip, and runs the
    /// package's own installer (install.sh --yes on Linux, install.ps1 -Yes on
    /// Windows) with SB_HOME set to this sb's home. The result is the same as
    /// installing that zip by hand: bin/sb, documents/ and VERSION are replaced and
    /// new bundled message files are added, while sb.config.yml, workspaces and
    /// installed apps are kept. The new binary is then run once to confirm that it
    /// reports the expected version.
    ///
    /// Only an installed sb updates itself: the running binary must be
    /// <sb_home>/bin/sb. A development build (cargo run, target/...) is refused;
    /// rebuild it with cargo instead.
    ///
    /// Without --yes, sb shows `sb <old> -> <new>` and asks before installing; when
    /// stdin is not a terminal, --yes is required. Installing an older release with
    /// --version is allowed, with a downgrade warning.
    ///
    /// --check changes nothing: it prints the installed and latest versions, the
    /// platform, the download URL and a status, and exits 0 when sb is up to date
    /// (or newer than the latest release) and 10 when an update is available, so
    /// `sb update --check || sb update -y` updates only when there is something new.
    ///
    /// `sb --version` uses the same lookup, cached for 24 hours in
    /// <sb_home>/update-check.json, to say whether an update exists. Set
    /// SB_NO_UPDATE_CHECK=1 to turn that off.
    #[command(after_long_help = UPDATE_EXAMPLES)]
    Update(UpdateArgs),
    // `sb <name> [args...]`: any other subcommand is an installed app. Clap hands
    // over the raw words (name first), so app flags such as `--help` pass through.
    #[command(external_subcommand)]
    External(Vec<String>),
}

const INSTALL_EXAMPLES: &str = "\
Examples:
  sb install https://github.com/swarmbotix/sb_kalibr.git        clone and install
  sb install https://github.com/swarmbotix/sb_kalibr.git@v1.2   pin a tag or branch
  sb install git@github.com:swarmbotix/sb_kalibr.git            clone over ssh
  sb install ~/dev/sb_kalibr                                    use a local folder in place
  sb install ~/dev/sb_kalibr --prefetch                         also docker pull the image now
  sb camcalib --help                                            run the app it installed

Manage installed apps with `sb app list`, `sb app info <name>`,
`sb app update [name]` and `sb app remove <name>`.";

const APP_INIT_EXAMPLES: &str = "\
Examples:
  sb app init                                        ask for name, entry, kind
  sb app init --kind none --entry ./calib.py         wrap an existing script
  sb app init --kind docker --image org/tool:1.0     app that runs a container
  sb app init ~/dev/mytool --kind host --name mytool host install step + stubs
  sb install . && sb mytool --help                   try the package locally

Resulting sb.app.yml (comments abridged) in a folder named camcalib, after
`sb app init --kind docker --image swarmbotix/sb_kalibr:latest`:
  name: camcalib             # subcommand: `sb camcalib ...`
  version: 0.1.0
  entry: ./run.bash          # run with the user's args, from the user's cwd
  kind: docker               # docker | host | none
  docker:
    image: swarmbotix/sb_kalibr:latest
    prefetch: false          # true = pull at `sb install`
  requires: [docker]         # binaries that must be on PATH";

const UPDATE_EXAMPLES: &str = "\
Examples:
  sb update                            install the latest release (asks first)
  sb update -y                         the same, without asking
  sb update --check                    compare installed and latest; exit 10 if newer exists
  sb update --check || sb update -y    update only when a newer release exists
  sb update --version 0.2.1            install a specific release (older = downgrade)
  sb update --force -y                 reinstall the current version
  SB_NO_UPDATE_CHECK=1 sb --version    print the version without looking online";

#[derive(clap::Args)]
struct UpdateArgs {
    /// Report the installed and latest versions and change nothing. Exit code 0:
    /// up to date or newer than the latest release; 10: an update is available.
    #[arg(long, conflicts_with_all = ["version", "force"])]
    check: bool,
    /// Install this release instead of the latest, e.g. 0.2.1 (a leading `v` is
    /// accepted). A version older than the running one is a downgrade.
    #[arg(long, value_name = "X.Y.Z")]
    version: Option<String>,
    /// Do not ask before installing. Required when stdin is not a terminal.
    #[arg(long, short = 'y')]
    yes: bool,
    /// Reinstall even when the target version is the one already running.
    #[arg(long)]
    force: bool,
}

#[derive(clap::Args)]
struct InstallArgs {
    /// Git URL, optionally pinned as `<url>@<branch-or-tag>`, or the path of a
    /// local package folder (one that contains sb.app.yml).
    #[arg(value_name = "URL|PATH")]
    source: String,
    /// For `kind: docker` packages, run `docker pull <image>` now even when the
    /// manifest says `prefetch: false`. No effect on other kinds.
    #[arg(long)]
    prefetch: bool,
}

#[derive(Subcommand)]
enum AppCmd {
    /// Make a folder an app package: write sb.app.yml and starter scripts.
    ///
    /// Writes a fully commented sb.app.yml in PATH (default: the current folder).
    /// The app name defaults to the folder name, with characters other than
    /// letters, digits and `_` turned into `_`. The entry defaults to ./<name>
    /// when that file exists, else ./run.bash.
    ///
    /// Missing scripts are created as executable stubs: the entry (prints a TODO
    /// and exits 1 until you put your command in it) and, for --kind host,
    /// install.bash (exits 0 until you add setup steps). Existing scripts are
    /// never overwritten; an existing sb.app.yml is replaced only with --force.
    ///
    /// Without --kind, on an interactive terminal, sb asks for the name, entry,
    /// kind and (for docker) image, offering defaults in brackets. Without a
    /// terminal, the kind defaults to `none`, or to `docker` when --image is given.
    ///
    /// The manifest is validated before it is written, so a fresh package always
    /// installs. Commit the folder and share its git URL: users run
    /// `sb install <url>` and then `sb <name> [args...]`.
    #[command(after_long_help = APP_INIT_EXAMPLES)]
    Init(AppInitArgs),
    /// Install an app package (same as `sb install`).
    #[command(after_long_help = INSTALL_EXAMPLES)]
    Install(InstallArgs),
    /// List installed apps: name, version, kind, source and folder.
    #[command(alias = "ls")]
    List,
    /// Show one installed app: manifest, source, commit, and `requires` status.
    Info {
        /// Installed app name (see `sb app list`).
        name: String,
    },
    /// Update one installed app, or all of them.
    ///
    /// Git-installed apps are fast-forwarded with `git pull --ff-only` (apps
    /// pinned to a tag stay put); local-folder apps are used as they are. Then
    /// the kind's install step runs again (`host.install`, or `docker pull` when
    /// `prefetch: true`) and the recorded version and commit are refreshed.
    Update {
        /// App to update. Omit to update every installed app.
        name: Option<String>,
    },
    /// Uninstall an app.
    ///
    /// Removes the app from <sb_home>/apps.yml. Its folder is deleted only if
    /// sb cloned it (it lives under <sb_home>/apps/); a local folder installed
    /// in place is left untouched.
    #[command(alias = "rm")]
    Remove {
        /// Installed app name (see `sb app list`).
        name: String,
    },
}

#[derive(clap::Args)]
struct AppInitArgs {
    /// Package folder. Defaults to the current directory.
    path: Option<PathBuf>,
    /// App name, so the app runs as `sb <name>`: letters, digits and `_`, not
    /// starting with a digit, and not an sb built-in. Default: the folder name.
    #[arg(long)]
    name: Option<String>,
    /// File that `sb <name>` runs, relative to the package folder. Default:
    /// ./<name> if that file exists, else ./run.bash (created as a stub).
    #[arg(long)]
    entry: Option<String>,
    /// What `sb install` does besides registering the app. Asked for on an
    /// interactive terminal; otherwise `none` (or `docker` with --image).
    #[arg(long, value_enum)]
    kind: Option<AppKindArg>,
    /// Image for --kind docker, e.g. `swarmbotix/sb_kalibr:latest`.
    #[arg(long)]
    image: Option<String>,
    /// One-line description, shown by `sb app info`.
    #[arg(long)]
    description: Option<String>,
    /// Replace an existing sb.app.yml. Scripts are never overwritten.
    #[arg(long)]
    force: bool,
}

#[derive(Copy, Clone, Debug, clap::ValueEnum)]
#[clap(rename_all = "lowercase")]
enum AppKindArg {
    /// Only register the app; its entry runs as shipped.
    None,
    /// The app runs a container; `sb install` can pre-pull its image.
    Docker,
    /// `sb install` and `sb app update` run install.bash on this machine.
    Host,
}

impl AppKindArg {
    fn to_kind(self) -> sb_apps::AppKind {
        match self {
            AppKindArg::None => sb_apps::AppKind::None,
            AppKindArg::Docker => sb_apps::AppKind::Docker,
            AppKindArg::Host => sb_apps::AppKind::Host,
        }
    }
}

#[derive(clap::Args)]
struct RunArgs {
    /// Module name registered in the active workspace's flow.yaml.
    module: String,
    /// Extra args forwarded verbatim to `runscript.bash`. Anything after
    /// `<module>` is collected here; use `--` to disambiguate args that
    /// start with `-` from `sb`'s own flags.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

#[derive(clap::Args)]
struct StopArgs {
    /// Module name registered in the active workspace's flow.yaml.
    module: String,
    /// Extra args forwarded verbatim to `stopscript.bash`.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

#[derive(clap::Args)]
struct GoproArgs {
    /// Target a specific module (default: cwd's module).
    #[arg(long, conflicts_with = "all")]
    module: Option<String>,
    /// Freeze every module in the active workspace.
    #[arg(long)]
    all: bool,
}

#[derive(clap::Args)]
struct ListArgs {
    /// Operate on this module — see resolution order in level3.html.
    #[arg(long)]
    module: Option<String>,
}

#[derive(Subcommand)]
enum PubCmd {
    /// List publishers on the resolved module.
    List {
        #[arg(long)]
        module: Option<String>,
    },
    /// Add a publisher.
    Add(PubAddCli),
    /// Edit a publisher (change its msg type or topic).
    Edit(PubEditCli),
    /// Remove a publisher and its generated file.
    Rm {
        name: String,
        #[arg(long)]
        module: Option<String>,
    },
}

#[derive(clap::Args)]
struct PubAddCli {
    /// Topic — bare (`hello`) or fully-qualified (`/dev01/ws/mod/iox2/hello`).
    topic: String,
    /// Message type from the vault, e.g. `std/StringStamped`. `-m` is
    /// reserved for this — module selection uses `--module`.
    #[arg(short = 'm', long = "msg")]
    msg: String,
    /// Identifier override; defaults to the topic's last `/`-segment.
    #[arg(short = 'n', long)]
    name: Option<String>,
    /// Force iceoryx2 transport.
    #[arg(long = "iox2", group = "transport")]
    iox2: bool,
    /// Force Zenoh transport.
    #[arg(long, group = "transport")]
    zenoh: bool,
    /// Overwrite any existing publisher / subscriber that collides on
    /// name or topic. Mutually exclusive with `--skip-existing`.
    #[arg(long, group = "conflict")]
    force: bool,
    /// No-op if a publisher / subscriber already occupies the name or
    /// topic. Mutually exclusive with `--force`.
    #[arg(long, group = "conflict")]
    skip_existing: bool,
    /// Operate on this module — see resolution order in level3.html.
    #[arg(long)]
    module: Option<String>,
}

#[derive(clap::Args)]
struct PubEditCli {
    /// Existing publisher name.
    name: String,
    /// New message type from the vault.
    #[arg(short = 'm', long = "msg")]
    msg: Option<String>,
    /// New topic.
    #[arg(short = 't', long)]
    topic: Option<String>,
    #[arg(long)]
    module: Option<String>,
}

#[derive(Subcommand)]
enum SubCmd {
    /// List subscribers on the resolved module.
    List {
        #[arg(long)]
        module: Option<String>,
    },
    /// Add a subscriber.
    Add(SubAddCli),
    /// Re-render a subscriber file from current `sb.dev.yml`.
    Edit {
        topic: String,
        #[arg(long)]
        module: Option<String>,
    },
    /// Remove a subscriber and its generated file.
    Rm {
        topic: String,
        #[arg(long)]
        module: Option<String>,
    },
}

#[derive(Subcommand)]
enum ServiceCmd {
    /// Copy the service router (`service.py` / `service.rs`) into the module's
    /// io dir, with this module's namespace baked in. Re-run to refresh after
    /// a config change. Rust and Python modules only.
    Init {
        /// Target module (default: resolve from cwd / active workspace).
        #[arg(long)]
        module: Option<String>,
    },
}

#[derive(clap::Args)]
struct SubAddCli {
    /// Topic — bare or fully-qualified.
    topic: String,
    /// Message type from the vault.
    #[arg(short = 'm', long = "msg")]
    msg: String,
    #[arg(long = "iox2", group = "transport")]
    iox2: bool,
    #[arg(long, group = "transport")]
    zenoh: bool,
    /// Overwrite any existing publisher / subscriber that collides on
    /// name or topic. Mutually exclusive with `--skip-existing`.
    #[arg(long, group = "conflict")]
    force: bool,
    /// No-op if a publisher / subscriber already occupies the name or
    /// topic. Mutually exclusive with `--force`.
    #[arg(long, group = "conflict")]
    skip_existing: bool,
    #[arg(long)]
    module: Option<String>,
}

#[derive(Subcommand)]
enum TopicCmd {
    /// Snapshot every currently-active topic on either transport.
    #[command(alias = "ls")]
    List(TopicListCli),
    /// Subscribe to `<topic>` and stream JSON-encoded frames to stdout.
    Listen(TopicListenCli),
    /// Publish raw hex bytes on `<topic>` exactly once.
    Pub(TopicPubCli),
    /// Remove stale iceoryx2 service registrations whose owning process is
    /// gone (the entries `sb topic list` flags `(dead)`). Zenoh is
    /// session-based and has no on-disk state to prune — this verb is
    /// iceoryx2-only.
    Prune(TopicPruneCli),
}

#[derive(clap::Args)]
struct TopicListCli {
    /// Zenoh discovery window. iceoryx2 uses Service::list and ignores
    /// this. Defaults to 0.5 s — just long enough for any 2 Hz+ publisher.
    #[arg(short = 't', long = "timeout", default_value_t = 0.5)]
    timeout: f64,
    /// Substring filter on topic name.
    #[arg(short = 'k', long)]
    keyword: Option<String>,
    /// Match keyword case-sensitively (default: insensitive).
    #[arg(long)]
    case_sensitive: bool,
    /// Restrict discovery to a single transport. Default: both.
    #[arg(long, value_enum)]
    transport: Option<TransportArg>,
    /// Emit one JSON line per topic instead of the table.
    #[arg(long)]
    json: bool,
}

#[derive(clap::Args)]
struct TopicListenCli {
    /// Bare (`chatter`) or fully-qualified (`/dev01/ws/mod/zenoh/chatter`).
    topic: String,
    /// Which transport to subscribe on. Default: zenoh.
    #[arg(long, value_enum, default_value_t = TransportArg::Zenoh)]
    transport: TransportArg,
    /// Skip the Header probe and emit hex bytes only.
    #[arg(long)]
    raw: bool,
    /// Stop after N frames. `0` (default) runs until SIGINT / EOF.
    #[arg(short = 'n', long = "count", default_value_t = 0)]
    count: u64,
}

#[derive(clap::Args)]
struct TopicPruneCli {
    /// Emit a single JSON object `{cleaned_pids: [...], failed: [...]}` instead
    /// of the human table. Same shape as `sb_discover::PruneReport`.
    #[arg(long)]
    json: bool,
}

#[derive(clap::Args)]
struct TopicPubCli {
    /// Bare or fully-qualified topic.
    topic: String,
    /// Hex bytes (no `0x` prefix, whitespace OK).
    bytes_hex: String,
    /// Default: zenoh.
    #[arg(long, value_enum, default_value_t = TransportArg::Zenoh)]
    transport: TransportArg,
}

#[derive(Copy, Clone, Debug, clap::ValueEnum)]
#[clap(rename_all = "lowercase")]
enum TransportArg {
    Zenoh,
    Iceoryx2,
}

impl TransportArg {
    fn to_discover(self) -> sb_discover::Transport {
        match self {
            TransportArg::Zenoh => sb_discover::Transport::Zenoh,
            TransportArg::Iceoryx2 => sb_discover::Transport::Iceoryx2,
        }
    }
}

#[derive(Subcommand)]
enum WsCmd {
    /// Create a workspace dir and an empty flow.yaml.
    Create { name: String },
    /// Mark a workspace active (writes `~/.swarmbotix/active`).
    Set { name: String },
    /// List workspaces; the active one is prefixed with `*`.
    List,
    /// Remove the workspace dir; clears `active` if it was the active workspace.
    Delete { name: String },
}

#[derive(clap::Args)]
struct InitArgs {
    /// Project root to adopt. Defaults to the current working directory.
    rootpath: Option<PathBuf>,
    /// Language tag. Required if no `sb.dev.yml` exists yet at `<rootpath>`.
    /// Exactly one of these may be set.
    #[arg(long, group = "lang")]
    rust: bool,
    #[arg(long, group = "lang")]
    python: bool,
    #[arg(long, group = "lang")]
    cpp: bool,
    #[arg(long, group = "lang")]
    flutter: bool,
    #[arg(long, group = "lang")]
    unity: bool,
    /// Overwrite an existing `sb.dev.yml` / `runscript.bash` at `<rootpath>`.
    #[arg(long)]
    force: bool,
    /// Emit a docker-flavored `runscript.bash` whose `--id <name>` branches
    /// dispatch to one pre-resolved `docker run` per instance declared in
    /// `flow.yaml::instances.<module>`. Requires that block to be populated;
    /// see documents/sbcli_docker_runscript.md.
    #[arg(long)]
    docker: bool,
}

#[derive(Subcommand)]
enum MessageCmd {
    /// List all messages, grouped by namespace.
    List,
    /// Create a new `.proto` skeleton under the user namespace, then
    /// auto-compile it so every backend has bindings ready.
    New { name: String },
    /// Open a message's `.proto` in `$EDITOR` (works for std/* and user
    /// messages). On a clean editor exit, the message is recompiled
    /// automatically across every available backend.
    Edit { name: String },
    /// Delete a user message (refuses std/*) and remove every generated
    /// artifact for it under `iox2/`, `proto/`, and `fb/` in the vault.
    Rm { name: String },
    /// Show or pin which backends `sb message compile` emits for a style.
    ///
    /// The pin is written to `<style>/sb.style.yml`, beside
    /// `message_definitions/`, so it travels with the style rather than
    /// living on one machine. Use it when the default selection is wrong for
    /// the project that consumes the output — a Unity host, for instance,
    /// cannot compile the iox2 C# bindings at all.
    Backends {
        /// Style to show or pin (e.g. `vrobots_msgs`).
        style: String,
        /// Backends to emit: any of `iox2`, `proto`, `fb`. Omit to show the
        /// current setting. Pass `none` to pin an empty list, which emits
        /// nothing automatically (descriptor only).
        #[arg(value_name = "BACKEND")]
        backends: Vec<String>,
        /// Remove the pin so the style falls back to automatic selection.
        #[arg(long, conflicts_with = "backends")]
        clear: bool,
    },
    /// Compile messages. Always writes the FileDescriptorSet IR to
    /// `<vault>/.cache/descriptor.bin`, then emits language outputs
    /// selected by `--iox2`, `--proto`, `--fb` (or all three if none
    /// is set). Outputs land alongside the source `.proto` inside the
    /// vault unless `--out <dir>` overrides the destination.
    Compile {
        /// Override output directory for codegen. Defaults to
        /// `<vault>/targets/` so generated bindings sit in their own
        /// subtree, isolated from the `.proto` source package dirs.
        #[arg(long, short = 'o')]
        out: Option<PathBuf>,

        /// Emit iceoryx2-compatible flat Rust / C++ / Python types
        /// (via sb-iox2-typegen). All fields fixed-size; nested messages inlined.
        #[arg(long)]
        iox2: bool,
        /// Emit standard protobuf language bindings via protoc plugins.
        /// (Not yet implemented at L1.)
        #[arg(long)]
        proto: bool,
        /// Emit FlatBuffers bindings (.proto → .fbs → flatc).
        /// (Not yet implemented at L1.)
        #[arg(long)]
        fb: bool,

        /// Override iox2 default capacity for `string` fields (bytes).
        #[arg(long)]
        string_cap: Option<u32>,
        /// Override iox2 default capacity for `bytes` fields.
        #[arg(long)]
        bytes_cap: Option<u32>,
        /// Override iox2 default capacity for `repeated T` fields.
        #[arg(long)]
        vec_cap: Option<u32>,
        /// Override iox2 array length for `repeated string` fields (number of
        /// strings; each still consumes `string_cap` bytes). Defaults to
        /// `string_array_cap` from sb.config.yml, then 10.
        #[arg(long)]
        string_array_cap: Option<u32>,

        /// Optional: compile only this message, fully qualified
        /// (e.g. `ros2/std_msgs/Image`). The style comes from the name.
        name: Option<String>,

        /// Optional: compile only this style (e.g. `ros2`). Omit to compile
        /// every style. This is a FILTER, not a mode — it selects which
        /// styles this one run touches and changes nothing persistently.
        #[arg(long, value_name = "NAME", conflicts_with = "name")]
        style: Option<String>,
    },
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Open the merged `sb.config.yml` in `$EDITOR`.
    Open,
    /// Set a single key non-interactively.
    Set {
        key: String,
        value: String,
        /// Write to the active workspace's sb.config.yml (no-op at L1 — placeholder).
        #[arg(long)]
        workspace: bool,
    },
    /// Print the merged `sb.config.yml` that every other `sb` verb sees.
    Show {
        /// Emit JSON instead of YAML (one object, every resolved field).
        #[arg(long)]
        json: bool,
        /// Annotate each field with the layer it came from
        /// (env / workspace / global / builtin / unset).
        #[arg(long)]
        sources: bool,
    },
    /// Print a single field's resolved value (one line, no quoting).
    Get {
        /// Field name — same keys as `sb config set`.
        key: String,
        /// Emit a JSON object `{key, value, source}` instead of the bare value.
        #[arg(long)]
        json: bool,
    },
}

fn main() -> ExitCode {
    match real_main() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(1)
        }
    }
}

/// Pre-load iceoryx2's global config with its logger turned down.
///
/// iceoryx2 resolves its config file exactly once, lazily, on the first
/// `Config::global_config()` — which `Service::list`, `Node::list` and
/// every `NodeBuilder::new()` reach internally. That single lookup emits
/// up to two warnings that are pure noise for `sb` users:
///
/// - `Config::global_config()` — "No config file was loaded, a config
///   with default values will be used." Fires on any box without an
///   `iceoryx2.toml`. The defaults are exactly what `sb` wants, so it is
///   never actionable. Both platforms.
/// - `User::from_uid(4294967295)` — the per-user config path needs the
///   home dir via `getpwuid_r`, which iceoryx2's Windows PAL stubs out
///   (Windows has no `/etc/passwd`). Windows only; on Linux the lookup
///   succeeds and this one never fires.
///
/// Running that lookup here, quietly, populates iceoryx2's once-cell
/// before any command can trigger it. Every later call hits the
/// `is_initialized()` early-return and stays silent. No `#[cfg]` needed —
/// the Linux case is a strict subset of the Windows one — and genuine
/// iceoryx2 warnings raised after startup still print normally.
///
/// `IOX2_LOG_LEVEL` opts out: if it is set the user is explicitly asking
/// to see iceoryx2's logs, so suppress nothing.
fn warm_iceoryx_global_config() {
    use iceoryx2::prelude::*;

    if std::env::var_os("IOX2_LOG_LEVEL").is_none() {
        set_log_level(LogLevel::Error);
        let _ = Config::global_config();
    }
    set_log_level_from_env_or_default();
}

/// Run the parsed command. Built-in commands exit 0 on success; an installed
/// app (`sb <name> ...`) passes its own exit code through.
fn real_main() -> Result<ExitCode> {
    let cli = Cli::parse();
    if cfg!(windows) {
        // A Windows `sb update` leaves the replaced binary as sb.exe.old.
        if let Ok(home) = sb_home() {
            sb_update::cleanup_stale_binary(home.root());
        }
    }
    if cli.version {
        return Ok(cmd_version());
    }
    let Some(cmd) = cli.cmd else {
        // `arg_required_else_help` answers a bare `sb` before this point; this
        // is the same answer for anything else that parses to no command.
        use clap::CommandFactory;
        eprint!("{}", Cli::command().render_help());
        return Ok(ExitCode::from(2));
    };
    warm_iceoryx_global_config();
    match cmd {
        Cmd::Doctor => cmd_doctor(),
        Cmd::Message(m) => match m {
            MessageCmd::List => cmd_message_list(),
            MessageCmd::New { name } => cmd_message_new(&name),
            MessageCmd::Edit { name } => cmd_message_edit(&name),
            MessageCmd::Rm { name } => cmd_message_rm(&name),
            MessageCmd::Backends {
                style,
                backends,
                clear,
            } => cmd_message_backends(&style, &backends, clear),
            MessageCmd::Compile {
                out,
                iox2,
                proto,
                fb,
                string_cap,
                bytes_cap,
                vec_cap,
                string_array_cap,
                name,
                style,
            } => cmd_message_compile(
                out.as_deref(),
                iox2,
                proto,
                fb,
                string_cap,
                bytes_cap,
                vec_cap,
                string_array_cap,
                name.as_deref(),
                style.as_deref(),
            ),
        },
        Cmd::Config(c) => match c {
            ConfigCmd::Open => cmd_config_open(),
            ConfigCmd::Set {
                key,
                value,
                workspace,
            } => cmd_config_set(&key, &value, workspace),
            ConfigCmd::Show { json, sources } => cmd_config_show(json, sources),
            ConfigCmd::Get { key, json } => cmd_config_get(&key, json),
        },
        Cmd::Ws(w) => match w {
            WsCmd::Create { name } => cmd_ws_create(&name),
            WsCmd::Set { name } => cmd_ws_set(&name),
            WsCmd::List => cmd_ws_list(),
            WsCmd::Delete { name } => cmd_ws_delete(&name),
        },
        Cmd::Init(a) => cmd_init(a),
        Cmd::List(a) => cmd_list(a.module.as_deref(), Scope::All),
        Cmd::Pub(p) => match p {
            PubCmd::List { module } => cmd_list(module.as_deref(), Scope::PubsOnly),
            PubCmd::Add(a) => cmd_pub_add(a),
            PubCmd::Edit(a) => cmd_pub_edit(a),
            PubCmd::Rm { name, module } => cmd_pub_rm(&name, module.as_deref()),
        },
        Cmd::Sub(s) => match s {
            SubCmd::List { module } => cmd_list(module.as_deref(), Scope::SubsOnly),
            SubCmd::Add(a) => cmd_sub_add(a),
            SubCmd::Edit { topic, module } => cmd_sub_edit(&topic, module.as_deref()),
            SubCmd::Rm { topic, module } => cmd_sub_rm(&topic, module.as_deref()),
        },
        Cmd::Service(s) => match s {
            ServiceCmd::Init { module } => cmd_service_init(module.as_deref()),
        },
        Cmd::Topic(t) => match t {
            TopicCmd::List(a) => cmd_topic_list(a),
            TopicCmd::Listen(a) => cmd_topic_listen(a),
            TopicCmd::Pub(a) => cmd_topic_pub(a),
            TopicCmd::Prune(a) => cmd_topic_prune(a),
        },
        Cmd::Up => cmd_up(),
        Cmd::Run(a) => cmd_run(&a.module, &a.args),
        Cmd::Stop(a) => cmd_stop(&a.module, &a.args),
        Cmd::Down => cmd_down(),
        Cmd::Attach => cmd_attach(),
        Cmd::Gopro(a) => cmd_gopro(a),
        Cmd::Install(a) => cmd_install(&a),
        Cmd::App(a) => match a {
            AppCmd::Init(a) => cmd_app_init(a),
            AppCmd::Install(a) => cmd_install(&a),
            AppCmd::List => cmd_app_list(),
            AppCmd::Info { name } => cmd_app_info(&name),
            AppCmd::Update { name } => cmd_app_update(name.as_deref()),
            AppCmd::Remove { name } => cmd_app_remove(&name),
        },
        Cmd::Update(a) => return cmd_update(&a),
        Cmd::External(argv) => return cmd_external(&argv),
    }?;
    Ok(ExitCode::SUCCESS)
}

// ─────────────────────────────────────────────────────────────────────
// self-update: `sb --version`, `sb update`
// ─────────────────────────────────────────────────────────────────────

/// Exit code of `sb update --check` when a newer release exists.
const EXIT_UPDATE_AVAILABLE: u8 = 10;

/// `sb --version`: `sb X.Y.Z`, then one line saying whether a newer release
/// exists. The version line is printed first, so it shows even while the
/// (short, cached, silent-on-failure) lookup runs.
fn cmd_version() -> ExitCode {
    println!("sb {}", env!("CARGO_PKG_VERSION"));
    if let Ok(home) = sb_home() {
        if let Some(notice) = sb_update::version_notice(home.root()) {
            println!("{notice}");
        }
    }
    ExitCode::SUCCESS
}

fn cmd_update(a: &UpdateArgs) -> Result<ExitCode> {
    use std::io::IsTerminal;
    let home = sb_home()?;
    let fetcher = sb_update::fetcher_from_env()?;
    if a.check {
        let out = sb_update::check(home.root(), fetcher.as_ref())?;
        let status = match out.status {
            sb_update::Status::UpdateAvailable => "update available   run `sb update`",
            sb_update::Status::UpToDate => "up to date",
            sb_update::Status::Ahead => "ahead (newer than the latest release)",
        };
        println!("installed : {}", out.installed);
        println!("latest    : {}", out.latest);
        println!("platform  : {}", out.platform);
        println!("asset     : {}", out.asset_url);
        println!("status    : {status}");
        return Ok(if out.status == sb_update::Status::UpdateAvailable {
            ExitCode::from(EXIT_UPDATE_AVAILABLE)
        } else {
            ExitCode::SUCCESS
        });
    }

    let opts = sb_update::UpdateOptions {
        version: a
            .version
            .as_deref()
            .map(sb_update::parse_version)
            .transpose()?,
        force: a.force,
    };
    let plan = sb_update::plan(home.root(), fetcher.as_ref(), &opts)?;
    let reinstall = if plan.from == plan.to {
        " (reinstall)"
    } else {
        ""
    };
    println!("sb {} -> {}{reinstall}", plan.from, plan.to);
    if plan.downgrade {
        eprintln!(
            "warning: {} is older than the running {}; this is a downgrade",
            plan.to, plan.from
        );
    }
    if !a.yes {
        if !std::io::stdin().is_terminal() {
            anyhow::bail!("not a terminal; pass --yes");
        }
        if !confirm("Proceed? [y/N] ")? {
            anyhow::bail!("cancelled");
        }
    }
    eprintln!("downloading {} ...", plan.asset_url);
    let out = sb_update::apply(home.root(), fetcher.as_ref(), &plan)?;
    println!("updated: sb {}", out.to);
    Ok(ExitCode::SUCCESS)
}

/// Ask a yes/no question on stdin; only `y` or `yes` (any case) is yes.
fn confirm(question: &str) -> Result<bool> {
    print!("{question}");
    std::io::Write::flush(&mut std::io::stdout()).ok();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .context("reading stdin")?;
    Ok(matches!(
        line.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

// ─────────────────────────────────────────────────────────────────────
// pub/sub (L3)
// ─────────────────────────────────────────────────────────────────────

/// Look up the active workspace + device, then call [`resolve_module`].
/// Centralizes the boilerplate every pub/sub verb shares.
fn resolve_for_pubsub(
    module: Option<&str>,
) -> Result<(SbCliConfig, SbHome, Vault, ModuleHandle, String, String)> {
    let cfg = load_config()?;
    let home = SbHome::from_config(&cfg)?;
    let cwd = std::env::current_dir().context("getting cwd")?;
    let handle = resolve_module(module, &cwd, &home)?;
    let workspace = active_workspace(&home)?.ok_or_else(|| {
        anyhow::anyhow!("no active workspace — run `sb ws create <n> && sb ws set <n>` first")
    })?;
    let device = cfg.device.clone().unwrap_or_else(|| "dev01".to_string());
    let vctx = vault_for(&cfg)?;
    vctx.ensure_installed()?;
    Ok((cfg, home, vctx.into_vault(), handle, workspace, device))
}

fn transport_from_flags(zenoh: bool, iox2: bool) -> Option<Transport> {
    match (zenoh, iox2) {
        (true, _) => Some(Transport::Zenoh),
        (_, true) => Some(Transport::Iceoryx2),
        _ => None,
    }
}

fn cmd_list(module: Option<&str>, scope: Scope) -> Result<()> {
    let (_, _, _, handle, _, _) = resolve_for_pubsub(module)?;
    let rows = pubsub_list(&handle, scope);
    print!("{}", render_table(&rows));
    Ok(())
}

fn cmd_pub_add(a: PubAddCli) -> Result<()> {
    let (cfg, _, vault, mut handle, ws, device) = resolve_for_pubsub(a.module.as_deref())?;
    let name = a
        .name
        .clone()
        .unwrap_or_else(|| sb_pubsub::default_name_from_topic(&a.topic));
    let colliders = find_pub_colliders(&handle.config, &name, &a.topic);
    let policy = resolve_conflict_policy(a.force, a.skip_existing, &colliders, "publisher")?;
    let args = PubAddArgs {
        topic: &a.topic,
        msg_type: &a.msg,
        name: a.name.as_deref(),
        transport: transport_from_flags(a.zenoh, a.iox2),
        policy,
    };
    let outcome = sb_pubsub::pub_add(&mut handle, &args, &cfg, &vault, &ws, &device)?;
    print_outcome(&handle, &outcome);
    Ok(())
}

fn cmd_pub_edit(a: PubEditCli) -> Result<()> {
    let (cfg, _, vault, mut handle, ws, device) = resolve_for_pubsub(a.module.as_deref())?;
    let args = PubEditArgs {
        name: &a.name,
        msg_type: a.msg.as_deref(),
        topic: a.topic.as_deref(),
    };
    let outcome = sb_pubsub::pub_edit(&mut handle, &args, &cfg, &vault, &ws, &device)?;
    print_outcome(&handle, &outcome);
    Ok(())
}

fn cmd_pub_rm(name: &str, module: Option<&str>) -> Result<()> {
    let (_, _, _, mut handle, _, _) = resolve_for_pubsub(module)?;
    let outcome = sb_pubsub::pub_rm(&mut handle, name)?;
    print_outcome(&handle, &outcome);
    Ok(())
}

fn cmd_sub_add(a: SubAddCli) -> Result<()> {
    let (cfg, _, vault, mut handle, ws, device) = resolve_for_pubsub(a.module.as_deref())?;
    let name = sb_pubsub::default_name_from_topic(&a.topic);
    let colliders = find_sub_colliders(&handle.config, &name, &a.topic);
    let policy = resolve_conflict_policy(a.force, a.skip_existing, &colliders, "subscriber")?;
    let args = SubAddArgs {
        topic: &a.topic,
        msg_type: &a.msg,
        transport: transport_from_flags(a.zenoh, a.iox2),
        policy,
    };
    let outcome = sb_pubsub::sub_add(&mut handle, &args, &cfg, &vault, &ws, &device)?;
    print_outcome(&handle, &outcome);
    Ok(())
}

fn cmd_sub_edit(topic: &str, module: Option<&str>) -> Result<()> {
    let (cfg, _, vault, mut handle, ws, device) = resolve_for_pubsub(module)?;
    let outcome = sb_pubsub::sub_edit(&mut handle, topic, &cfg, &vault, &ws, &device)?;
    print_outcome(&handle, &outcome);
    Ok(())
}

fn cmd_sub_rm(topic: &str, module: Option<&str>) -> Result<()> {
    let (_, _, _, mut handle, _, _) = resolve_for_pubsub(module)?;
    let outcome = sb_pubsub::sub_rm(&mut handle, topic)?;
    print_outcome(&handle, &outcome);
    Ok(())
}

/// `sb service init` — copy the per-module service router into the io dir,
/// with the module's namespace baked in. Independent of pub/sub state.
fn cmd_service_init(module: Option<&str>) -> Result<()> {
    let (_, _, _, handle, ws, device) = resolve_for_pubsub(module)?;
    let input = sb_codegen::ServiceAppInput {
        module: &handle.config.module,
        workspace: &ws,
        device: &device,
        language: handle.config.language,
    };
    let rendered = sb_codegen::render_service_app(&input)?;
    let abs = handle.io_dir().join(&rendered.rel_path);
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let existed = abs.exists();
    std::fs::write(&abs, &rendered.contents)
        .with_context(|| format!("writing {}", abs.display()))?;

    let (verb, file_verb) = if existed {
        ("refreshed", "updated")
    } else {
        ("wrote", "wrote")
    };
    println!("{verb} service router on module {}", handle.config.module);
    println!("  {file_verb} {}", abs.display());
    println!(
        "  serves on /{device}/{ws}/{}/zenoh/{}/**",
        handle.config.module,
        sb_codegen::SERVICE_SEGMENT,
    );
    Ok(())
}

fn print_outcome(handle: &ModuleHandle, outcome: &sb_pubsub::MutationOutcome) {
    let verb = match outcome.action {
        Action::PubAdd => "added publisher",
        Action::PubEdit => "edited publisher",
        Action::PubRm => "removed publisher",
        Action::SubAdd => "added subscriber",
        Action::SubEdit => "regenerated subscriber",
        Action::SubRm => "removed subscriber",
    };
    if outcome.skipped {
        println!(
            "{} skipped on module {} (already exists)",
            match outcome.action {
                Action::PubAdd => "publisher",
                Action::SubAdd => "subscriber",
                _ => "entry",
            },
            handle.config.module
        );
        return;
    }
    for overwritten in &outcome.overwrote {
        println!("  overwrote {overwritten}");
    }
    println!("{verb} on module {}", handle.config.module);
    if let Some(p) = &outcome.file_written {
        println!("  wrote {}", p.display());
    }
    if let Some(p) = &outcome.file_removed {
        println!("  removed {}", p.display());
    }
    if outcome.dev_yml_written {
        println!("  updated {}", handle.dev_yml_path.display());
    }
}

/// Pick a [`ConflictPolicy`] from the CLI flags + an interactive prompt
/// when stdin is a TTY. Cases:
///   - `--force` ⇒ Force regardless of colliders.
///   - `--skip-existing` ⇒ Skip regardless.
///   - No flag + no colliders ⇒ Error (irrelevant — the mutation has
///     nothing to collide with).
///   - No flag + colliders + TTY ⇒ prompt [o]/[s]/[c].
///   - No flag + colliders + non-TTY ⇒ Error (preserves CI safety;
///     the error already hints at the flags).
fn resolve_conflict_policy(
    force: bool,
    skip_existing: bool,
    colliders: &[Collider],
    kind: &str,
) -> Result<ConflictPolicy> {
    if force {
        return Ok(ConflictPolicy::Force);
    }
    if skip_existing {
        return Ok(ConflictPolicy::Skip);
    }
    if colliders.is_empty() {
        return Ok(ConflictPolicy::Error);
    }
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        // Let sb-pubsub render the canonical error.
        return Ok(ConflictPolicy::Error);
    }
    // Interactive prompt.
    println!("adding {kind} would collide with:");
    print!("{}", format_colliders(colliders));
    loop {
        print!("  [o]verwrite, [s]kip, [c]ancel? ");
        std::io::Write::flush(&mut std::io::stdout()).ok();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_err() {
            return Ok(ConflictPolicy::Error);
        }
        match line.trim().to_ascii_lowercase().as_str() {
            "o" | "overwrite" => return Ok(ConflictPolicy::Force),
            "s" | "skip" => return Ok(ConflictPolicy::Skip),
            "c" | "cancel" | "" => anyhow::bail!("cancelled"),
            other => println!("    didn't understand {other:?}; type o/s/c"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// workspaces + init (L2)
// ─────────────────────────────────────────────────────────────────────

fn sb_home() -> Result<SbHome> {
    let cfg = load_config()?;
    SbHome::from_config(&cfg)
}

fn cmd_ws_create(name: &str) -> Result<()> {
    let home = sb_home()?;
    sb_workspace::create_workspace(&home, name)?;
    println!(
        "created workspace {name} at {}",
        home.workspace_dir(name).display()
    );
    Ok(())
}

fn cmd_ws_set(name: &str) -> Result<()> {
    let home = sb_home()?;
    sb_workspace::set_active(&home, name)?;
    println!("active workspace = {name}");
    Ok(())
}

fn cmd_ws_list() -> Result<()> {
    let home = sb_home()?;
    let entries = sb_workspace::list_workspaces(&home)?;
    if entries.is_empty() {
        println!("(no workspaces — run `sb ws create <name>`)");
        return Ok(());
    }
    for e in entries {
        let marker = if e.active { "*" } else { " " };
        println!("{marker} {}", e.name);
    }
    Ok(())
}

fn cmd_ws_delete(name: &str) -> Result<()> {
    let home = sb_home()?;
    let outcome = sb_workspace::delete_workspace(&home, name)?;
    println!("deleted workspace {name}");
    if outcome.cleared_active {
        eprintln!(
            "warning: {name} was the active workspace; cleared `{}`",
            home.active_marker().display()
        );
    }
    Ok(())
}

fn cmd_init(a: InitArgs) -> Result<()> {
    let home = sb_home()?;
    let language = init_args_language(&a)?;
    let root = a.rootpath.clone().unwrap_or_else(|| PathBuf::from("."));
    let opts = InitOptions {
        root: &root,
        language,
        force: a.force,
        docker: a.docker,
    };
    let outcome = sb_workspace::init_module(&home, &opts)?;
    let lang_tag = outcome.language.map(|l| l.as_str()).unwrap_or("?");
    println!(
        "adopted module {} (language: {}, root: {})",
        outcome.module,
        lang_tag,
        outcome.root_abs.display()
    );
    if outcome.dev_yml_written {
        println!("  wrote sb.dev.yml");
    }
    if outcome.runscript_written {
        println!("  wrote runscript.bash (executable)");
    }
    if outcome.io_dir_created {
        println!("  created IO directory");
    }
    if outcome.flow_appended {
        println!("  registered in flow.yaml");
    } else {
        println!("  (already registered in flow.yaml)");
    }
    Ok(())
}

fn init_args_language(a: &InitArgs) -> Result<Option<Language>> {
    let flags = [
        (a.rust, Language::Rust),
        (a.python, Language::Python),
        (a.cpp, Language::Cpp),
        (a.flutter, Language::Flutter),
        (a.unity, Language::Unity),
    ];
    let picked: Vec<Language> = flags.iter().filter(|(b, _)| *b).map(|(_, l)| *l).collect();
    match picked.as_slice() {
        [] => Ok(None),
        [one] => Ok(Some(*one)),
        _ => anyhow::bail!("pick at most one of --rust / --python / --cpp / --flutter / --unity"),
    }
}

// ─────────────────────────────────────────────────────────────────────
// app packages: `sb install`, `sb app *`, `sb <name> ...`
// ─────────────────────────────────────────────────────────────────────

/// `sb <name> [args...]`: run an installed app and pass its exit code on.
fn cmd_external(argv: &[String]) -> Result<ExitCode> {
    let (name, args) = argv
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("missing app name"))?;
    let home = sb_home()?;
    let code = sb_apps::exec_app(home.root(), name, args)?;
    // Reached only where exec_app returns (non-Unix): codes outside 0..=255 become 1.
    Ok(ExitCode::from(u8::try_from(code).unwrap_or(1)))
}

fn cmd_install(a: &InstallArgs) -> Result<()> {
    let home = sb_home()?;
    if matches!(
        sb_apps::classify(&a.source),
        Ok(sb_apps::Source::Git { .. })
    ) {
        eprintln!("fetching {} ...", a.source);
    }
    let opts = sb_apps::InstallOptions {
        prefetch_override: a.prefetch.then_some(true),
    };
    let out = sb_apps::install(home.root(), &a.source, &opts)?;
    let verb = if out.reinstalled {
        "reinstalled"
    } else {
        "installed"
    };
    println!(
        "{verb} app {} {} (kind: {}, source: {})",
        out.name, out.version, out.kind, out.source
    );
    println!("  path: {}", out.path.display());
    if let Some(c) = &out.commit {
        println!("  commit: {}", short_commit(c));
    }
    for s in &out.steps_run {
        println!("  ran: {s}");
    }
    for n in &out.notes {
        println!("  note: {n}");
    }
    for w in &out.warnings {
        eprintln!("warning: {w}");
    }
    println!("run it: sb {} [args...]", out.name);
    Ok(())
}

fn cmd_app_init(a: AppInitArgs) -> Result<()> {
    use std::io::IsTerminal;
    let dir = a.path.clone().unwrap_or_else(|| PathBuf::from("."));
    if !dir.is_dir() {
        anyhow::bail!("{} is not an existing folder", dir.display());
    }
    // Fail before any prompt rather than after the user has answered them.
    let manifest = dir.join(sb_apps::MANIFEST_FILE);
    if manifest.exists() && !a.force {
        anyhow::bail!(
            "{} already exists; pass --force to overwrite it (scripts are never overwritten)",
            manifest.display()
        );
    }
    let tty = std::io::stdin().is_terminal();
    let mut name = a.name;
    let mut entry = a.entry;
    let mut image = a.image;
    let flag_kind = a.kind.map(AppKindArg::to_kind);
    let implied = if image.is_some() {
        sb_apps::AppKind::Docker
    } else {
        sb_apps::AppKind::None
    };
    let kind = match flag_kind {
        Some(k) => k,
        None if tty => {
            println!("sb app init: press Enter to accept the [default].");
            let n = match name.take() {
                Some(n) => n,
                None => prompt_valid(
                    "app name (run as `sb <name>`)",
                    &sb_apps::default_name(&dir)?,
                    |s| sb_apps::validate_app_name(s).map(|()| s.to_owned()),
                )?,
            };
            if entry.is_none() {
                entry = Some(prompt_valid(
                    "entry (file to run, relative to the package)",
                    &sb_apps::default_entry(&dir, &n),
                    |s| Ok(s.to_owned()),
                )?);
            }
            name = Some(n);
            prompt_valid(
                "kind (none | docker | host)",
                implied.as_str(),
                parse_app_kind,
            )?
        }
        None => implied,
    };
    if kind == sb_apps::AppKind::Docker && image.is_none() && tty {
        image = Some(prompt_valid("docker image (e.g. org/name:tag)", "", |s| {
            if s.is_empty() {
                Err("an image is required for kind docker".to_owned())
            } else {
                Ok(s.to_owned())
            }
        })?);
    }
    let opts = sb_apps::InitAppOptions {
        name,
        entry,
        kind,
        image,
        description: a.description,
        force: a.force,
    };
    let out = sb_apps::init_app(&dir, &opts)?;
    println!(
        "created app package {} (kind: {}, root: {})",
        out.name,
        out.kind,
        out.root_abs.display()
    );
    if out.manifest_written {
        println!("  wrote {}", sb_apps::MANIFEST_FILE);
    }
    if out.entry_stub_written {
        println!(
            "  wrote {} (executable stub): replace its TODO lines with your command",
            out.entry
        );
    } else {
        println!("  entry {} (existing file, left as is)", out.entry);
    }
    if out.install_stub_written {
        println!("  wrote install.bash (executable stub): add host setup steps");
    }
    println!("next:");
    println!(
        "  sb install {}   then run: sb {} [args...]",
        out.root_abs.display(),
        out.name
    );
    println!("  commit and push; others run: sb install <git-url>");
    Ok(())
}

fn parse_app_kind(s: &str) -> std::result::Result<sb_apps::AppKind, String> {
    match s.to_ascii_lowercase().as_str() {
        "none" => Ok(sb_apps::AppKind::None),
        "docker" => Ok(sb_apps::AppKind::Docker),
        "host" => Ok(sb_apps::AppKind::Host),
        other => Err(format!(
            "{other:?} is not a kind; type none, docker or host"
        )),
    }
}

/// Ask `label [default]: ` on stdin until `parse` accepts the answer. An
/// empty answer means the default; end of input takes the default if it
/// parses and fails otherwise.
fn prompt_valid<T>(
    label: &str,
    default: &str,
    parse: impl Fn(&str) -> std::result::Result<T, String>,
) -> Result<T> {
    loop {
        if default.is_empty() {
            print!("{label}: ");
        } else {
            print!("{label} [{default}]: ");
        }
        std::io::Write::flush(&mut std::io::stdout()).ok();
        let mut line = String::new();
        let read = std::io::stdin()
            .read_line(&mut line)
            .context("reading stdin")?;
        let answer = match line.trim() {
            "" => default,
            t => t,
        };
        match parse(answer) {
            Ok(v) => return Ok(v),
            Err(e) if read == 0 => anyhow::bail!("no answer for {label}: {e}"),
            Err(e) => println!("  {e}"),
        }
    }
}

fn cmd_app_list() -> Result<()> {
    let home = sb_home()?;
    let apps = sb_apps::list(home.root())?;
    if apps.is_empty() {
        println!("(no apps installed; run `sb install <url|path>`)");
        return Ok(());
    }
    let rows: Vec<[String; 5]> = apps
        .iter()
        .map(|a| {
            let kind = a
                .manifest
                .as_ref()
                .map_or("?", |m| m.kind.as_str())
                .to_owned();
            let path = if a.path_exists {
                a.record.path.display().to_string()
            } else {
                format!("{} (missing)", a.record.path.display())
            };
            [
                a.name.clone(),
                a.record.version.clone(),
                kind,
                a.record.source.as_str().to_owned(),
                path,
            ]
        })
        .collect();
    let header = ["NAME", "VERSION", "KIND", "SOURCE", "PATH"].map(str::to_owned);
    let mut widths = header.clone().map(|h| h.len());
    for row in &rows {
        for (w, cell) in widths.iter_mut().zip(row) {
            *w = (*w).max(cell.len());
        }
    }
    for row in std::iter::once(&header).chain(&rows) {
        let line = format!(
            "{:<w0$}  {:<w1$}  {:<w2$}  {:<w3$}  {}",
            row[0],
            row[1],
            row[2],
            row[3],
            row[4],
            w0 = widths[0],
            w1 = widths[1],
            w2 = widths[2],
            w3 = widths[3],
        );
        println!("{}", line.trim_end());
    }
    Ok(())
}

fn cmd_app_info(name: &str) -> Result<()> {
    let home = sb_home()?;
    let a = sb_apps::info(home.root(), name)?;
    let r = &a.record;
    println!("name:         {}", a.name);
    if let Some(d) = a.manifest.as_ref().and_then(|m| m.description.as_deref()) {
        println!("description:  {d}");
    }
    println!("version:      {} (installed {})", r.version, r.installed_at);
    let missing = if a.path_exists { "" } else { " (missing)" };
    println!("path:         {}{missing}", r.path.display());
    match (&r.url, r.source) {
        (Some(url), sb_apps::SourceKind::Git) => {
            let pin = r
                .git_ref
                .as_deref()
                .map_or(String::new(), |g| format!(" @ {g}"));
            let commit = r
                .commit
                .as_deref()
                .map_or(String::new(), |c| format!(" (commit {})", short_commit(c)));
            println!("source:       git {url}{pin}{commit}");
        }
        _ => println!("source:       local folder (installed in place)"),
    }
    if let Some(m) = &a.manifest {
        match (&m.kind, &m.docker) {
            (sb_apps::AppKind::Docker, Some(d)) => println!(
                "kind:         docker (image {}, prefetch {})",
                d.image, d.prefetch
            ),
            (sb_apps::AppKind::Host, _) => println!(
                "kind:         host (install {})",
                m.host.as_ref().map_or("?", |h| h.install.as_str())
            ),
            (k, _) => println!("kind:         {k}"),
        }
        println!("entry:        {}", m.entry);
    }
    if !a.requires.is_empty() {
        let reqs: Vec<String> = a
            .requires
            .iter()
            .map(|q| match &q.found {
                Some(p) => format!("{} ({})", q.bin, p.display()),
                None => format!("{} (NOT on PATH)", q.bin),
            })
            .collect();
        println!("requires:     {}", reqs.join(", "));
    }
    if let Some(e) = &a.manifest_error {
        println!("problem:      {e}");
    }
    println!("run:          sb {} [args...]", a.name);
    Ok(())
}

fn cmd_app_update(name: Option<&str>) -> Result<()> {
    let home = sb_home()?;
    let outs = sb_apps::update(home.root(), name)?;
    if outs.is_empty() {
        println!("(no apps installed; run `sb install <url|path>`)");
        return Ok(());
    }
    for o in outs {
        let how = match o.pull {
            Some(sb_apps::PullStatus::Pulled) if o.old_commit != o.new_commit => format!(
                "pulled {} -> {}",
                o.old_commit.as_deref().map_or("?", short_commit),
                o.new_commit.as_deref().map_or("?", short_commit)
            ),
            Some(sb_apps::PullStatus::Pulled) => "already up to date".to_owned(),
            Some(sb_apps::PullStatus::Pinned) => "pinned".to_owned(),
            None => "local folder".to_owned(),
        };
        if o.old_version == o.new_version {
            println!("updated {} {} ({how})", o.name, o.new_version);
        } else {
            println!(
                "updated {} {} -> {} ({how})",
                o.name, o.old_version, o.new_version
            );
        }
        for s in &o.steps_run {
            println!("  ran: {s}");
        }
        for n in &o.notes {
            println!("  note: {n}");
        }
        for w in &o.warnings {
            eprintln!("warning: {w}");
        }
    }
    Ok(())
}

fn cmd_app_remove(name: &str) -> Result<()> {
    let home = sb_home()?;
    let out = sb_apps::remove(home.root(), name)?;
    println!("removed app {}", out.name);
    if out.deleted_dir {
        println!("  deleted {}", out.path.display());
    } else {
        println!(
            "  left {} in place (installed from a local folder)",
            out.path.display()
        );
    }
    Ok(())
}

/// First 12 hex digits of a commit hash.
fn short_commit(c: &str) -> &str {
    c.get(..12).unwrap_or(c)
}

/// One `app` line per installed app for `sb doctor`: kind, whether its
/// folder exists, and whether each `requires` binary is on PATH. Problems
/// are `[WARN]`, never `[FAIL]`: an app is the package author's concern and
/// must not fail `sb doctor` itself.
fn doctor_app_checks(cfg: &SbCliConfig) -> sb_doctor::Report {
    use sb_doctor::{CheckResult, CheckStatus};
    let apps = sb_config::resolve_sb_home_dir(cfg).and_then(|h| sb_apps::list(&h));
    let checks = match apps {
        Err(e) => vec![CheckResult {
            name: "app",
            status: CheckStatus::Warn(format!("cannot read the app registry: {e:#}")),
        }],
        Ok(apps) => apps
            .iter()
            .map(|a| {
                let kind = a.manifest.as_ref().map_or("?", |m| m.kind.as_str());
                let path = if a.path_exists {
                    "path ok".to_owned()
                } else {
                    format!(
                        "path {} MISSING (`sb app remove {}`, then reinstall)",
                        a.record.path.display(),
                        a.name
                    )
                };
                let requires = if a.requires.is_empty() {
                    "requires nothing".to_owned()
                } else {
                    let each: Vec<String> = a
                        .requires
                        .iter()
                        .map(|q| {
                            let state = if q.found.is_some() { "ok" } else { "MISSING" };
                            format!("{} {state}", q.bin)
                        })
                        .collect();
                    format!("requires {}", each.join(", "))
                };
                let mut msg = format!("{}: kind {kind}, {path}, {requires}", a.name);
                let healthy = a.path_exists
                    && a.manifest_error.is_none()
                    && a.requires.iter().all(|q| q.found.is_some());
                if a.path_exists {
                    if let Some(e) = &a.manifest_error {
                        msg.push_str(&format!(", sb.app.yml unreadable: {e}"));
                    }
                }
                CheckResult {
                    name: "app",
                    status: if healthy {
                        CheckStatus::Ok(msg)
                    } else {
                        CheckStatus::Warn(msg)
                    },
                }
            })
            .collect(),
    };
    sb_doctor::Report { checks }
}

// ─────────────────────────────────────────────────────────────────────
// Shared helpers
// ─────────────────────────────────────────────────────────────────────

fn load_config() -> Result<SbCliConfig> {
    let inputs = LoadInputs::from_env();
    let (cfg, _srcs) = sb_config::load(&inputs).context("loading sb.config.yml")?;
    Ok(cfg)
}

/// The vault a command operates on, plus how it was located.
///
/// Carrying the provenance matters for two decisions that would otherwise
/// silently do the wrong thing in a user's own checkout: whether to install
/// the forge's `std/` bundle, and whether a bare `sb message compile` means
/// "every style under the root" or "the one I'm standing in".
struct VaultCtx {
    vault: Vault,
    /// Set when cwd IS a style directory (it has a `message_definitions/`
    /// child). A bare `sb message compile` / `list` then touches only this
    /// style rather than every sibling directory under the root.
    pinned_style: Option<String>,
    /// True when the root came from cwd rather than config.
    local: bool,
}

impl VaultCtx {
    fn vault(&self) -> &Vault {
        &self.vault
    }

    fn into_vault(self) -> Vault {
        self.vault
    }

    /// Install the forge's embedded `std/` bundle — **global vault only**.
    ///
    /// A discovered root is someone's own repo (or its parent). Installing
    /// there would scatter an unasked-for `ros2/message_definitions/std/`
    /// tree through it on the first `sb message` call.
    fn ensure_installed(&self) -> Result<sb_vault::InstallReport> {
        if self.local {
            return Ok(sb_vault::InstallReport::default());
        }
        self.vault.ensure_installed()
    }

    /// Styles this invocation should touch, honoring the cwd pin.
    fn styles(&self) -> Vec<String> {
        match &self.pinned_style {
            Some(s) => vec![s.clone()],
            None => self.vault.styles(),
        }
    }
}

/// The vault, rooted at `messages_root` and spanning every style.
///
/// A tree found by walking up from cwd wins, so `sb` run inside the forge
/// uses the forge's `messages/` rather than the installed one — and a
/// standalone message repo, cloned anywhere and registered with nothing,
/// builds in place.
fn vault_for(cfg: &SbCliConfig) -> Result<VaultCtx> {
    if std::env::var_os("SB_CONFIG").is_none() {
        if let Some(ctx) = discover_local_vault() {
            return Ok(ctx);
        }
    }
    Ok(VaultCtx {
        vault: Vault::at(sb_config::resolve_messages_root(cfg)?),
        pinned_style: None,
        local: false,
    })
}

/// Walk up from cwd looking for a style tree. The first level that matches
/// one of these shapes wins:
///
///   1. `./message_definitions/` — **cwd is itself a style**. The root is
///      cwd's parent and the style is cwd's folder name, so a standalone
///      message repo compiles into its own `message_targets/`.
///   2. `./messages/<style>/message_definitions/` — the conventional
///      layout the forge itself uses.
///   3. `./<style>/message_definitions/` — cwd holds styles directly.
///
/// Matching on the `<style>/message_definitions/` *shape* rather than the
/// folder name is what keeps an unrelated directory called "messages" from
/// being mistaken for a vault.
fn discover_local_vault() -> Option<VaultCtx> {
    discover_local_vault_from(&std::env::current_dir().ok()?)
}

/// [`discover_local_vault`] with the starting directory injected, so the
/// walk can be tested without touching process-global cwd.
fn discover_local_vault_from(start: &std::path::Path) -> Option<VaultCtx> {
    let mut cur = start.to_path_buf();
    loop {
        // 1. cwd is a style directory. Checked first and per-level, so the
        //    innermost style wins over any root further up.
        if cur.join("message_definitions").is_dir() {
            if let (Some(style), Some(root)) =
                (cur.file_name().and_then(|s| s.to_str()), cur.parent())
            {
                if sb_core::is_valid_message_style(style) {
                    return Some(VaultCtx {
                        vault: Vault::at(root.to_path_buf()),
                        pinned_style: Some(style.to_owned()),
                        local: true,
                    });
                }
            }
        }
        // 2. Conventional `messages/` subtree.
        let conventional = cur.join("messages");
        if conventional.is_dir() && looks_like_messages_root(&conventional) {
            return Some(VaultCtx {
                vault: Vault::at(conventional),
                pinned_style: None,
                local: true,
            });
        }
        // 3. cwd holds styles directly.
        if looks_like_messages_root(&cur) {
            return Some(VaultCtx {
                vault: Vault::at(cur),
                pinned_style: None,
                local: true,
            });
        }
        if !cur.pop() {
            return None;
        }
    }
}

/// True when `dir` has at least one `<style>/message_definitions/` child.
fn looks_like_messages_root(dir: &std::path::Path) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return false;
    };
    rd.flatten()
        .any(|e| e.path().join("message_definitions").is_dir())
}

/// Path of the *global* `sb.config.yml` — by definition
/// `<sb_home>/sb.config.yml`. Resolves through `sb_home_dir` rather than
/// `dirs::home_dir()` directly, which is what lets a test sandbox pin it on
/// every platform: `dirs::home_dir()` reads `$HOME` only on Unix, and on
/// Windows calls `SHGetKnownFolderPath(Profile)`, ignoring the environment.
fn global_config_path(cfg: &SbCliConfig) -> Result<PathBuf> {
    Ok(sb_config::resolve_sb_home_dir(cfg)?.join("sb.config.yml"))
}

/// Editor to spawn when `$EDITOR` is unset. `vi` is not present on a bare
/// Windows box, so fall back to something that always exists there.
fn default_editor() -> &'static str {
    #[cfg(windows)]
    {
        "notepad"
    }
    #[cfg(not(windows))]
    {
        "vi"
    }
}

// ─────────────────────────────────────────────────────────────────────
// doctor
// ─────────────────────────────────────────────────────────────────────

fn cmd_doctor() -> Result<()> {
    let cfg = load_config()?;
    // Report on the vault `sb message *` will actually use — cwd discovery
    // included. A doctor describing a different root than the next command
    // touches is worse than no doctor.
    let vctx = vault_for(&cfg)?;
    let report = sb_doctor::run_with_vault(
        &cfg,
        &sb_doctor::VaultView {
            root: vctx.vault().root(),
            pinned_style: vctx.pinned_style.as_deref(),
            local: vctx.local,
        },
    );
    use std::io::IsTerminal;
    let color = std::io::stdout().is_terminal()
        && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty());
    println!("sb {}", env!("CARGO_PKG_VERSION"));
    print!("{}", report.render(color));
    // Installed apps: warnings only, so they never change doctor's exit code.
    print!("{}", doctor_app_checks(&cfg).render(color));
    if report.all_ok() {
        Ok(())
    } else {
        let n = report.checks.iter().filter(|c| c.status.is_fail()).count();
        anyhow::bail!("{n} check(s) failed");
    }
}

// ─────────────────────────────────────────────────────────────────────
// message
// ─────────────────────────────────────────────────────────────────────

fn cmd_message_list() -> Result<()> {
    let cfg = load_config()?;
    let vctx = vault_for(&cfg)?;
    let vault = vctx.vault();
    let report = vctx.ensure_installed()?;
    if !report.copied.is_empty() {
        eprintln!(
            "installed {} std message(s) from forge bundle",
            report.copied.len()
        );
    }
    // One fully-qualified name per line, blank line between styles.
    //
    // Deliberately NOT grouped under `style/` + `namespace/` headers with
    // bare leaves underneath: that would make a line meaningful only in
    // relation to the header above it, which is the exact ambiguity these
    // names exist to remove. Every line here is directly pasteable into
    // `sb pub add -m` and directly greppable.
    // Honors the cwd pin, so standing in a standalone message repo lists
    // that repo's messages rather than every sibling directory.
    let mut names = Vec::new();
    for style in vctx.styles() {
        names.extend(vault.list_style(&style)?);
    }
    names.sort();
    let mut cur_style: Option<String> = None;
    for n in names {
        if cur_style.is_some() && cur_style.as_deref() != Some(n.style.as_str()) {
            println!();
        }
        cur_style = Some(n.style.clone());
        println!("{n}");
    }
    Ok(())
}

fn cmd_message_new(name: &str) -> Result<()> {
    let cfg = load_config()?;
    let vctx = vault_for(&cfg)?;
    vctx.ensure_installed()?;
    let parsed = MessageName::parse(name).map_err(anyhow::Error::msg)?;
    let path = vctx.vault().new_message(&parsed)?;
    println!("created {}", path.display());
    // Auto-compile the skeleton so bindings exist immediately. The skeleton
    // is empty-bodied; protoc accepts it, every backend emits an empty
    // struct. Re-runs after the user edits the file.
    compile_one_with_defaults(name)
}

fn cmd_message_edit(name: &str) -> Result<()> {
    let cfg = load_config()?;
    let vctx = vault_for(&cfg)?;
    vctx.ensure_installed()?;
    let parsed = MessageName::parse(name).map_err(anyhow::Error::msg)?;
    let path = vctx.vault().path_of(&parsed);
    if !path.exists() {
        anyhow::bail!(
            "{} not found — run `sb message new {name}` first",
            path.display()
        );
    }
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| default_editor().into());
    let status = std::process::Command::new(&editor)
        .arg(&path)
        .status()
        .with_context(|| format!("spawning {editor}"))?;
    if !status.success() {
        anyhow::bail!("{editor} exited with {status} — skipping recompile");
    }
    // Compile after a clean editor exit so the bindings reflect the new schema.
    compile_one_with_defaults(name)
}

fn cmd_message_rm(name: &str) -> Result<()> {
    let cfg = load_config()?;
    let vctx = vault_for(&cfg)?;
    let vault = vctx.vault();
    let parsed = MessageName::parse(name).map_err(anyhow::Error::msg)?;
    vault.remove(&parsed)?;
    // Drop generated artifacts under <message_targets>/{iox2,proto,fb}/
    // so removing the .proto leaves nothing dangling that still references
    // the old type. Same vault the .proto was just removed from — see
    // the `--out` derivation in compile_style_pass.
    let targets = vault.targets_dir(&parsed.style);
    let removed = cleanup_generated_for(&targets, &parsed);
    println!("removed {name}");
    if removed > 0 {
        println!("cleaned {removed} generated artifact dir(s)");
    }
    Ok(())
}

/// `sb message backends <style> [<backend>...] [--clear]`.
fn cmd_message_backends(style: &str, backends: &[String], clear: bool) -> Result<()> {
    let cfg = load_config()?;
    let vctx = vault_for(&cfg)?;
    let vault = vctx.vault();
    let path = vault.style_config_path(style);

    if !vault.styles().iter().any(|s| s == style) {
        anyhow::bail!(
            "no style {style:?} under {} (present: {})",
            vault.root().display(),
            {
                let p = vault.styles();
                if p.is_empty() {
                    "none".to_owned()
                } else {
                    p.join(", ")
                }
            }
        );
    }

    // ── clear ──
    if clear {
        if path.is_file() {
            std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
            println!("removed {}", path.display());
        } else {
            println!("no pin at {} — nothing to clear", path.display());
        }
        return Ok(());
    }

    // ── show ──
    if backends.is_empty() {
        match vault.style_config(style)?.and_then(|c| c.backends) {
            Some(b) if b.is_empty() => println!(
                "{style}: backends: []  (emits nothing automatically)\n  pinned by {}",
                path.display()
            ),
            Some(b) => {
                let list: Vec<&str> = b.iter().map(|x| x.as_str()).collect();
                println!(
                    "{style}: backends: [{}]\n  pinned by {}",
                    list.join(", "),
                    path.display()
                );
            }
            None => println!(
                "{style}: no pin — backends chosen automatically (host project, then installed tools)"
            ),
        }
        return Ok(());
    }

    // ── pin ──
    // `none` is the spelling for an empty list: an empty argv is already
    // taken by "show", so there has to be a word for "emit nothing".
    let parsed: Vec<sb_core::Backend> = if backends.len() == 1 && backends[0] == "none" {
        Vec::new()
    } else {
        backends
            .iter()
            .map(|b| match b.as_str() {
                "iox2" => Ok(sb_core::Backend::Iox2),
                "proto" => Ok(sb_core::Backend::Proto),
                "fb" => Ok(sb_core::Backend::Fb),
                other => Err(anyhow::anyhow!(
                    "unknown backend {other:?} — expected iox2, proto, fb, or none"
                )),
            })
            .collect::<Result<_>>()?
    };

    let written = vault.write_style_config(
        style,
        &sb_core::StyleConfig {
            backends: Some(parsed.clone()),
        },
    )?;
    if parsed.is_empty() {
        println!("{style}: pinned backends: []  (emits nothing automatically)");
    } else {
        let list: Vec<&str> = parsed.iter().map(|x| x.as_str()).collect();
        println!("{style}: pinned backends: [{}]", list.join(", "));
    }
    println!("wrote {}", written.display());
    Ok(())
}

/// Invoke the full compile pipeline for one message with default caps and
/// no `--out` override. Used by `new` and `edit` to keep bindings in sync.
fn compile_one_with_defaults(name: &str) -> Result<()> {
    cmd_message_compile(
        None, // out → <vault>/targets/
        false,
        false,
        false, // backend flags → "all available"
        None,
        None,
        None,
        None, // caps → built-in / config defaults
        Some(name),
        None, // style → taken from the message name itself
    )
}

/// Delete every per-message generated dir under a codegen tree root —
/// `<root>/{iox2,proto,fb}/<pkg>/<Leaf>/`. Empty parent pkg dirs are
/// pruned too. Returns the number of leaf dirs actually removed.
///
/// Callers pass `<vault>/targets` when cleaning up after a `sb message rm`;
/// the targets layer is invariant here.
fn cleanup_generated_for(targets_root: &std::path::Path, name: &MessageName) -> usize {
    let mut removed = 0;
    for backend in ["iox2", "proto", "fb"] {
        let leaf_dir = targets_root
            .join(backend)
            .join(&name.namespace)
            .join(&name.leaf);
        if leaf_dir.exists() && std::fs::remove_dir_all(&leaf_dir).is_ok() {
            removed += 1;
        }
    }
    for backend in ["iox2", "proto", "fb"] {
        let pkg_dir = targets_root.join(backend).join(&name.namespace);
        let _ = std::fs::remove_dir(&pkg_dir); // no-op unless empty
    }
    removed
}

/// Compile every style, or just the ones selected.
///
/// Styles are compiled **separately** because protoc takes exactly one
/// include root and each style is its own: an `import "std/Header.proto"`
/// has to resolve inside the style that wrote it. Compiling them together
/// would let one style's import silently bind to a same-named file in
/// another.
#[allow(clippy::too_many_arguments)]
fn cmd_message_compile(
    out: Option<&std::path::Path>,
    iox2: bool,
    proto: bool,
    fb: bool,
    string_cap: Option<u32>,
    bytes_cap: Option<u32>,
    vec_cap: Option<u32>,
    string_array_cap: Option<u32>,
    name_filter: Option<&str>,
    style_filter: Option<&str>,
) -> Result<()> {
    let cfg = load_config()?;
    let vctx = vault_for(&cfg)?;
    vctx.ensure_installed()?;
    let vault = vctx.vault();

    // A message name already names its style, so `sb message compile
    // ros2/std/Header` needs no --style and the two must not disagree.
    let styles: Vec<String> = match (name_filter, style_filter) {
        (Some(n), _) => {
            let parsed = MessageName::parse(n).map_err(anyhow::Error::msg)?;
            vec![parsed.style]
        }
        (None, Some(st)) => {
            let present = vault.styles();
            if !present.iter().any(|s| s == st) {
                anyhow::bail!(
                    "no style {st:?} under {} (present: {})",
                    vault.root().display(),
                    if present.is_empty() {
                        "none".to_owned()
                    } else {
                        present.join(", ")
                    }
                );
            }
            vec![st.to_owned()]
        }
        // Honors the cwd pin: standing in a style compiles that style only.
        (None, None) => vctx.styles(),
    };
    if styles.is_empty() {
        anyhow::bail!("no styles found under {}", vault.root().display());
    }

    let multi = styles.len() > 1;
    for st in &styles {
        if multi {
            println!("style {st}");
        }
        compile_style_pass(
            &cfg,
            vault,
            st,
            out,
            iox2,
            proto,
            fb,
            string_cap,
            bytes_cap,
            vec_cap,
            string_array_cap,
            name_filter,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn compile_style_pass(
    cfg: &SbCliConfig,
    vault: &Vault,
    style: &str,
    out: Option<&std::path::Path>,
    iox2: bool,
    proto: bool,
    fb: bool,
    string_cap: Option<u32>,
    bytes_cap: Option<u32>,
    vec_cap: Option<u32>,
    string_array_cap: Option<u32>,
    name_filter: Option<&str>,
) -> Result<()> {
    use prost::Message;
    use prost_types::FileDescriptorSet;

    let protoc = cfg
        .protoc
        .as_deref()
        .context("protoc path not set in sb.config.yml — set it or run `sb doctor`")?;

    // ── 1. Always: produce the FileDescriptorSet IR ──
    let cache_dir = vault.defs_dir(style).join(".cache");
    std::fs::create_dir_all(&cache_dir).ok();
    let descriptor_path = cache_dir.join("descriptor.bin");
    if let Some(n) = name_filter {
        let parsed = MessageName::parse(n).map_err(anyhow::Error::msg)?;
        sb_vault::compile_one(vault, &parsed, protoc, &descriptor_path)?;
    } else {
        sb_vault::compile_style(vault, style, protoc, &descriptor_path)?;
    }
    println!("wrote {}", descriptor_path.display());

    // ── 2. Codegen destination: --out override, else `message_targets` ──
    // (which itself defaults to `<message_definitions>/targets/` via the resolver).
    // Keeps source `.proto` package dirs and generated `iox2/`, `proto/`,
    // `fb/` trees in separate subtrees.
    let out_owned;
    let out: &std::path::Path = match out {
        Some(p) => p,
        None => {
            // Derived from the vault ACTUALLY in use, not from cfg's
            // `messages_root` — otherwise a locally-discovered style would
            // compile its own sources but write the bindings into the
            // global `~/.swarmbotix` tree.
            out_owned = vault.targets_dir(style);
            &out_owned
        }
    };

    // Tool availability — gates the implicit (no-flag) defaults.
    let protoc_available = cfg.protoc.as_deref().is_some_and(std::path::Path::exists);
    let flatc_available = cfg.flatc.as_deref().is_some_and(std::path::Path::exists);

    // Explicit format flag must error clearly if its tool isn't available.
    if proto && !protoc_available {
        anyhow::bail!(
            "--proto requires `protoc` to be set + present in sb.config.yml (run `sb doctor`)"
        );
    }
    if fb && !flatc_available {
        anyhow::bail!(
            "--fb requires `flatc` to be set + present in sb.config.yml (run `sb doctor`)"
        );
    }

    let selection = resolve_backends(
        vault,
        style,
        BackendFlags { iox2, proto, fb },
        ToolAvailability {
            protoc: protoc_available,
            flatc: flatc_available,
        },
    )?;
    if let Some(note) = &selection.note {
        println!("{note}");
    }
    let want_iox2 = selection.has(sb_core::Backend::Iox2);
    let want_proto = selection.has(sb_core::Backend::Proto);
    let want_fb = selection.has(sb_core::Backend::Fb);

    let bytes = std::fs::read(&descriptor_path)
        .with_context(|| format!("reading {}", descriptor_path.display()))?;
    let fds = FileDescriptorSet::decode(&*bytes).context("decoding FileDescriptorSet")?;

    std::fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;

    // ── 3. iox2 (sb-iox2-typegen) ──
    if want_iox2 {
        // Precedence per cap: CLI flag > sb.config.yml > built-in default.
        let mut codegen_cfg = sb_iox2_typegen::CodegenConfig::default();
        if let Some(c) = cfg.string_array_cap {
            codegen_cfg.string_array_capacity = c;
        }
        if let Some(map) = cfg.string_array_caps.as_ref() {
            codegen_cfg.string_array_overrides = map.clone();
        }
        if let Some(map) = cfg.bytes_caps.as_ref() {
            codegen_cfg.bytes_overrides = map.clone();
        }
        if let Some(map) = cfg.vec_caps.as_ref() {
            codegen_cfg.vec_overrides = map.clone();
        }
        if let Some(map) = cfg.iox2_variants.as_ref() {
            codegen_cfg.variants = map.clone();
        }
        codegen_cfg.style = style.to_owned();
        if let Some(c) = string_cap {
            codegen_cfg.string_capacity = c;
        }
        if let Some(c) = bytes_cap {
            codegen_cfg.bytes_capacity = c;
        }
        if let Some(c) = vec_cap {
            codegen_cfg.vec_capacity = c;
        }
        if let Some(c) = string_array_cap {
            codegen_cfg.string_array_capacity = c;
        }
        let flats = sb_iox2_typegen::flatten(&fds, &codegen_cfg).map_err(anyhow::Error::msg)?;
        let filtered: Vec<&sb_iox2_typegen::FlatStruct> = match name_filter {
            Some(n) => {
                let v: Vec<_> = flats.iter().filter(|s| s.qualified_name() == n).collect();
                if v.is_empty() {
                    anyhow::bail!("no message {n} in vault");
                }
                v
            }
            None => flats.iter().collect(),
        };
        let mut n = 0usize;
        for s in &filtered {
            let mut modules: Vec<sb_iox2_typegen::GeneratedModule> = [
                sb_iox2_typegen::rust::emit as fn(&_) -> _,
                sb_iox2_typegen::cpp::emit,
                sb_iox2_typegen::python::emit,
                sb_iox2_typegen::csharp::emit,
            ]
            .iter()
            .map(|emit| emit(s))
            .collect();
            // C# `repeated <Message>` wrappers are separate compilation units:
            // a type may be declared once, and several messages in one package
            // routinely share an element type. Writing them per-message is
            // safe because the path and contents are a pure function of
            // (element type, capacity) — see sb_iox2_typegen::csharp.
            modules.extend(sb_iox2_typegen::csharp::emit_array_wrappers(s));
            for g in modules {
                let target = out.join(&g.rel_path);
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)
                        .with_context(|| format!("creating {}", parent.display()))?;
                }
                std::fs::write(&target, g.contents)
                    .with_context(|| format!("writing {}", target.display()))?;
                n += 1;
            }
        }
        println!("iox2: wrote {n} file(s) under {}", out.display());
    }

    // Resolve the .proto source files we'll feed to protoc/flatc.
    let sources: Vec<PathBuf> = resolve_proto_sources(vault, style, name_filter)?;
    // protoc / flatc get THIS style's definitions dir as the include root —
    // the same one `compile_style` used — so an `import "std/Header.proto"`
    // resolves inside the style that wrote it.
    let defs_root = vault.defs_dir(style);

    // ── 4. proto (standard protoc native plugins: --cpp_out + --python_out) ──
    if want_proto {
        let protoc = cfg.protoc.as_deref().expect("checked above");
        emit_proto(protoc, &defs_root, &sources, out)?;
        println!(
            "proto: wrote C++/Python/C# bindings ({} source file(s)) into {}",
            sources.len(),
            out.display()
        );
    }

    // ── 5. fb (FlatBuffers via flatc: .proto → .fbs → Rust/C++/Python/C#) ──
    if want_fb {
        let flatc = cfg.flatc.as_deref().expect("checked above");
        emit_fb(flatc, &defs_root, &sources, out)?;
        println!(
            "fb: wrote Rust/C++/Python/C# FlatBuffers bindings ({} source file(s)) into {}",
            sources.len(),
            out.display()
        );
    }

    Ok(())
}

/// The `--iox2` / `--proto` / `--fb` flags as passed.
#[derive(Debug, Clone, Copy)]
struct BackendFlags {
    iox2: bool,
    proto: bool,
    fb: bool,
}

impl BackendFlags {
    fn any(self) -> bool {
        self.iox2 || self.proto || self.fb
    }

    fn selected(self) -> Vec<sb_core::Backend> {
        let mut v = Vec::new();
        if self.iox2 {
            v.push(sb_core::Backend::Iox2);
        }
        if self.proto {
            v.push(sb_core::Backend::Proto);
        }
        if self.fb {
            v.push(sb_core::Backend::Fb);
        }
        v
    }
}

/// Which external generators are actually usable on this box.
#[derive(Debug, Clone, Copy)]
struct ToolAvailability {
    protoc: bool,
    flatc: bool,
}

/// The backends one compile pass will emit, plus a line explaining any
/// non-obvious choice.
#[derive(Debug, Clone)]
struct BackendSelection {
    backends: Vec<sb_core::Backend>,
    /// Printed before compiling whenever something other than the plain
    /// tool-availability default applied. A narrowed selection must never be
    /// silent — the whole point of the `--fb` workaround being easy to forget
    /// is that nothing said anything.
    note: Option<String>,
}

impl BackendSelection {
    fn has(&self, b: sb_core::Backend) -> bool {
        self.backends.contains(&b)
    }
}

/// Decide which backends to emit for one style, highest priority first:
///
/// 1. **Explicit `--iox2` / `--proto` / `--fb`** — honored exactly. An
///    explicit flag is a direct instruction and is never second-guessed.
/// 2. **`backends:` in `<style>/sb.style.yml`** — the persisted per-style pin.
///    Travels with the style, so a clone compiles the same way.
/// 3. **Tool availability** — iox2 always, `proto` if protoc is configured,
///    `fb` if flatc is configured. The default, and deliberately NOT narrowed
///    by what the host project can consume: the generated tree feeds C++,
///    Python and Rust clients as well as the host, so dropping a backend to
///    suit one consumer silently deprives the rest. Unity compatibility is a
///    property of the emitter (it targets C# 9), not of the selection.
fn resolve_backends(
    vault: &Vault,
    style: &str,
    flags: BackendFlags,
    tools: ToolAvailability,
) -> Result<BackendSelection> {
    // 1. Explicit flags win outright.
    if flags.any() {
        return Ok(BackendSelection {
            backends: flags.selected(),
            note: None,
        });
    }

    // 2. Persisted per-style pin.
    if let Some(pinned) = vault.style_config(style)?.and_then(|c| c.backends) {
        let listed: Vec<&str> = pinned.iter().map(|b| b.as_str()).collect();
        // A pin is explicit intent, so a missing tool is an error rather than
        // a silent drop — otherwise the pin quietly stops meaning anything.
        for b in &pinned {
            match b {
                sb_core::Backend::Proto if !tools.protoc => anyhow::bail!(
                    "{} pins `proto`, but protoc is not set + present in sb.config.yml (run `sb doctor`)",
                    vault.style_config_path(style).display()
                ),
                sb_core::Backend::Fb if !tools.flatc => anyhow::bail!(
                    "{} pins `fb`, but flatc is not set + present in sb.config.yml (run `sb doctor`)",
                    vault.style_config_path(style).display()
                ),
                _ => {}
            }
        }
        let note = if listed.is_empty() {
            format!(
                "{}: backends: [] — emitting nothing (descriptor only)",
                vault.style_config_path(style).display()
            )
        } else {
            format!(
                "{}: backends pinned to [{}]",
                vault.style_config_path(style).display(),
                listed.join(", ")
            )
        };
        return Ok(BackendSelection {
            backends: pinned,
            note: Some(note),
        });
    }

    // 3. Default: whatever this box can generate.
    //
    // Deliberately NOT narrowed by the host project. An earlier build detected
    // a Unity project around the output and dropped to `fb` only; that was
    // wrong. The generated tree is consumed by more than the host — the same
    // style feeds C++/Python/Rust clients — so suppressing a backend to suit
    // one consumer silently deprives all the others. If a particular tree
    // should carry a narrower set, that is a decision to record explicitly
    // via `sb message backends`, not one to infer.
    let mut backends = vec![sb_core::Backend::Iox2];
    if tools.protoc {
        backends.push(sb_core::Backend::Proto);
    }
    if tools.flatc {
        backends.push(sb_core::Backend::Fb);
    }
    Ok(BackendSelection {
        backends,
        note: None,
    })
}

/// Collect the absolute `.proto` paths the codegen helpers should consume.
/// Honors the optional fully-qualified `<name>` filter
/// (e.g. `ros2/std/Header` → one file); otherwise every `.proto` in the
/// style being compiled.
fn resolve_proto_sources(
    vault: &Vault,
    style: &str,
    name_filter: Option<&str>,
) -> Result<Vec<PathBuf>> {
    if let Some(n) = name_filter {
        let parsed = MessageName::parse(n).map_err(anyhow::Error::msg)?;
        let p = vault.path_of(&parsed);
        if !p.exists() {
            anyhow::bail!("no message {n} in vault ({})", p.display());
        }
        return Ok(vec![p]);
    }
    let mut out = Vec::new();
    for n in vault.list_style(style)? {
        let p = vault.path_of(&n);
        if p.exists() {
            out.push(p);
        }
    }
    Ok(out)
}

/// `protoc --cpp_out=<out> --python_out=<out> <proto>` per file.
/// protoc itself preserves the relative path under `--proto_path` in the
/// output, so files land at `<out>/<vault-dir>/<Type>.pb.{h,cc}` and
/// `<out>/<vault-dir>/<Type>_pb2.py` — alongside the iox2 outputs.
fn emit_proto(
    protoc: &std::path::Path,
    vault_root: &std::path::Path,
    sources: &[PathBuf],
    out: &std::path::Path,
) -> Result<()> {
    use std::process::Command;
    // Emit into a tempdir with protoc's natural `<pkg>/<Leaf>.{pb.cc,pb.h,_pb2.py}`
    // layout, then relocate each file to `<out>/proto/<pkg>/<Leaf>/<filename>`.
    let tmp = tempfile::tempdir().context("creating tempdir for protoc output")?;
    for src in sources {
        let rel = src.strip_prefix(vault_root).unwrap_or(src);
        // protoc's `--csharp_out` dumps files flat (ignores proto subdirs), so
        // different packages with the same leaf (e.g. `std/Header` and
        // `std_msgs/Header`) clobber each other. Direct csharp into a per-pkg
        // subdir so it matches cpp/python and the relocator can find the pkg.
        let pkg_dir = rel.parent().unwrap_or_else(|| std::path::Path::new(""));
        let csharp_out = tmp.path().join(pkg_dir);
        std::fs::create_dir_all(&csharp_out)
            .with_context(|| format!("creating {}", csharp_out.display()))?;
        let status = Command::new(protoc)
            .arg(format!("--proto_path={}", vault_root.display()))
            .arg(format!("--cpp_out={}", tmp.path().display()))
            .arg(format!("--python_out={}", tmp.path().display()))
            .arg(format!("--csharp_out={}", csharp_out.display()))
            .arg(sb_vault::proto_rel_arg(rel))
            .output()
            .with_context(|| format!("spawning protoc for {}", src.display()))?;
        if !status.status.success() {
            anyhow::bail!(
                "protoc failed for {}: {}",
                src.display(),
                String::from_utf8_lossy(&status.stderr)
            );
        }
    }
    relocate_into_backend_dirs(tmp.path(), out, "proto", proto_leaf_from_filename)?;
    Ok(())
}

/// Move every regular file under `staging` into `<out>/<backend>/<pkg>/<Leaf>/<filename>`.
/// `<pkg>` is the file's parent path relative to `staging` (may be empty if the
/// generator emits flat); `<Leaf>` is derived from the filename by `leaf_of`.
fn relocate_into_backend_dirs(
    staging: &std::path::Path,
    out: &std::path::Path,
    backend: &str,
    leaf_of: fn(&str) -> Option<String>,
) -> Result<()> {
    for entry in walkdir::WalkDir::new(staging) {
        let entry = entry.context("walking codegen staging dir")?;
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry.path().strip_prefix(staging).unwrap();
        let filename = rel.file_name().and_then(|s| s.to_str()).unwrap_or_default();
        let Some(leaf) = leaf_of(filename) else {
            continue;
        };
        let pkg_dir = rel.parent().unwrap_or_else(|| std::path::Path::new(""));
        let target_dir = out.join(backend).join(pkg_dir).join(&leaf);
        std::fs::create_dir_all(&target_dir)
            .with_context(|| format!("creating {}", target_dir.display()))?;
        let target = target_dir.join(filename);
        std::fs::rename(entry.path(), &target)
            .or_else(|_| std::fs::copy(entry.path(), &target).map(|_| ()))
            .with_context(|| format!("placing {}", target.display()))?;
    }
    Ok(())
}

/// `Foo.pb.cc` / `Foo.pb.h` / `Foo_pb2.py` / `Foo.cs` → `Some("Foo")`.
fn proto_leaf_from_filename(name: &str) -> Option<String> {
    if let Some(stem) = name
        .strip_suffix(".pb.cc")
        .or_else(|| name.strip_suffix(".pb.h"))
    {
        return Some(stem.to_string());
    }
    if let Some(stem) = name.strip_suffix("_pb2.py") {
        return Some(stem.to_string());
    }
    if let Some(stem) = name.strip_suffix(".cs") {
        return Some(stem.to_string());
    }
    None
}

/// Three-step flatc:
///   1. `--proto` each source into a per-package staging subdir so leaf
///      collisions across packages (e.g. `std/Header.fbs` vs `std_msgs/Header.fbs`)
///      don't clobber each other.
///   2. Build the transitive proto-import closure for each source, then
///      compile each `.fbs` with `-I` set to *only* the packages it
///      actually imports. This prevents the namespace ambiguity where
///      flatc would otherwise resolve `include "Header.fbs"` to the wrong
///      package because two packages declare the same leaf.
///   3. Flatc emits language outputs into a per-message dir; any namespace
///      subdirs it creates inside (e.g. for Rust / Python) are flattened
///      so every file for a message sits in `fb/<pkg>/<Leaf>/`.
fn emit_fb(
    flatc: &std::path::Path,
    vault_root: &std::path::Path,
    sources: &[PathBuf],
    out: &std::path::Path,
) -> Result<()> {
    use std::collections::{BTreeSet, HashMap, HashSet};
    use std::process::Command;

    let fbs_tmp = tempfile::tempdir().context("creating tempdir for .fbs intermediate")?;

    // 1. proto → fbs, per-package staging.
    for src in sources {
        let rel = src.strip_prefix(vault_root).unwrap_or(src);
        let pkg_dir = rel.parent().unwrap_or_else(|| std::path::Path::new(""));
        let dest = fbs_tmp.path().join(pkg_dir);
        std::fs::create_dir_all(&dest)
            .with_context(|| format!("creating fbs staging dir {}", dest.display()))?;
        let status = Command::new(flatc)
            .arg("--proto")
            .arg("-I")
            .arg(vault_root)
            .arg("-o")
            .arg(&dest)
            .arg(src)
            .output()
            .with_context(|| format!("spawning flatc --proto for {}", src.display()))?;
        if !status.status.success() {
            anyhow::bail!(
                "flatc --proto failed for {}: {}",
                src.display(),
                String::from_utf8_lossy(&status.stderr)
            );
        }
    }

    // 2a. Direct imports per proto (rel-path keyed).
    let mut direct: HashMap<PathBuf, Vec<String>> = HashMap::new();
    for src in sources {
        let rel = src.strip_prefix(vault_root).unwrap_or(src).to_path_buf();
        let content =
            std::fs::read_to_string(src).with_context(|| format!("reading {}", src.display()))?;
        direct.insert(rel, parse_proto_imports(&content));
    }
    // 2b. Transitive closure.
    let transitive: HashMap<PathBuf, HashSet<String>> = direct
        .keys()
        .map(|k| {
            let mut visited: HashSet<String> = HashSet::new();
            let mut stack: Vec<PathBuf> = vec![k.clone()];
            while let Some(cur) = stack.pop() {
                if let Some(deps) = direct.get(&cur) {
                    for d in deps {
                        if visited.insert(d.clone()) {
                            stack.push(PathBuf::from(d));
                        }
                    }
                }
            }
            (k.clone(), visited)
        })
        .collect();

    // 3. Language gen per source. Output goes straight into <out>/fb/<pkg>/<Leaf>/.
    for src in sources {
        let rel = src.strip_prefix(vault_root).unwrap_or(src);
        let pkg_dir = rel.parent().unwrap_or_else(|| std::path::Path::new(""));
        let leaf = src
            .file_stem()
            .and_then(|s| s.to_str())
            .with_context(|| format!("non-utf8 file stem in {}", src.display()))?;
        let fbs_path = fbs_tmp.path().join(pkg_dir).join(format!("{leaf}.fbs"));

        let mut include_pkgs: BTreeSet<PathBuf> = BTreeSet::new();
        include_pkgs.insert(pkg_dir.to_path_buf());
        if let Some(deps) = transitive.get(rel) {
            for d in deps {
                if let Some(parent) = std::path::Path::new(d).parent() {
                    if !parent.as_os_str().is_empty() {
                        include_pkgs.insert(parent.to_path_buf());
                    }
                }
            }
        }

        let dest = out.join("fb").join(pkg_dir).join(leaf);
        std::fs::create_dir_all(&dest).with_context(|| format!("creating {}", dest.display()))?;

        let mut cmd = Command::new(flatc);
        cmd.arg("--rust")
            .arg("--cpp")
            .arg("--python")
            .arg("--csharp");
        for ipkg in &include_pkgs {
            cmd.arg("-I").arg(fbs_tmp.path().join(ipkg));
        }
        cmd.arg("-o").arg(&dest).arg(&fbs_path);
        let status = cmd
            .output()
            .with_context(|| format!("spawning flatc for {}", fbs_path.display()))?;
        if !status.status.success() {
            anyhow::bail!(
                "flatc failed for {}: {}",
                fbs_path.display(),
                String::from_utf8_lossy(&status.stderr)
            );
        }

        flatten_namespace_subdirs(&dest)
            .with_context(|| format!("flattening namespace dirs in {}", dest.display()))?;
    }
    Ok(())
}

/// Extract `import "<path>";` paths from a proto file. Strips quotes; leaves
/// the path relative to the proto include root (which the caller knows).
fn parse_proto_imports(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in content.lines() {
        let t = line.trim();
        let Some(rest) = t.strip_prefix("import ") else {
            continue;
        };
        // `"path.proto";` — strip the trailing `;`, then the surrounding quotes.
        let stripped = rest.trim_end().trim_end_matches(';').trim();
        let inner = stripped.trim_start_matches('"').trim_end_matches('"');
        if !inner.is_empty() {
            out.push(inner.to_string());
        }
    }
    out
}

/// flatc emits namespace-based subdirs for Rust and Python. Move every nested
/// file up to `dir`, skip `__init__.py` (namespace markers), then remove the
/// emptied subdirs. Cpp output is already flat; this is a no-op for it.
fn flatten_namespace_subdirs(dir: &std::path::Path) -> Result<()> {
    let mut moves: Vec<(PathBuf, PathBuf)> = Vec::new();
    for entry in walkdir::WalkDir::new(dir).min_depth(2) {
        let entry = entry.context("walking flatc output dir")?;
        if !entry.file_type().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy();
        if name == "__init__.py" {
            continue;
        }
        moves.push((entry.path().to_path_buf(), dir.join(entry.file_name())));
    }
    for (src, dst) in moves {
        // Overwrite rather than skip when the flat copy already exists.
        //
        // Skipping was not idempotent: on a re-compile the flat file is last
        // run's output, so every nested file was left in place, the subdir
        // below never emptied, and the tree accumulated a full duplicate set.
        // A Unity host then compiles both copies and fails on duplicate type
        // definitions — the same class of breakage as the C# version issue,
        // arriving only on the SECOND compile, which is what made it look
        // like the generator was fine.
        //
        // `rename` refuses an existing destination on Windows, so clear it
        // first; `copy` overwrites but leaves the source behind, so remove it
        // explicitly or the subdir stays non-empty.
        if dst.exists() {
            std::fs::remove_file(&dst).with_context(|| format!("replacing {}", dst.display()))?;
        }
        if std::fs::rename(&src, &dst).is_err() {
            std::fs::copy(&src, &dst)
                .with_context(|| format!("moving {} → {}", src.display(), dst.display()))?;
            std::fs::remove_file(&src)
                .with_context(|| format!("removing {} after copy", src.display()))?;
        }
    }
    let subdirs: Vec<PathBuf> = walkdir::WalkDir::new(dir)
        .min_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_dir())
        .map(|e| e.path().to_path_buf())
        .collect();
    for d in subdirs.into_iter().rev() {
        // Also drop any leftover __init__.py before removing the dir.
        let init = d.join("__init__.py");
        if init.exists() {
            let _ = std::fs::remove_file(init);
        }
        let _ = std::fs::remove_dir(&d);
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────
// config
// ─────────────────────────────────────────────────────────────────────

fn cmd_config_open() -> Result<()> {
    let cfg = load_config()?;
    let global = global_config_path(&cfg).context("could not resolve ~/.swarmbotix path")?;
    if !global.exists() {
        if let Some(parent) = global.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(&global, "# sb.config.yml — see requirements.md\n")
            .with_context(|| format!("creating {}", global.display()))?;
    }
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| default_editor().into());
    let status = std::process::Command::new(&editor).arg(&global).status();
    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => anyhow::bail!("{editor} exited with {s}"),
        Err(e) => anyhow::bail!("failed to spawn {editor}: {e}"),
    }
}

fn cmd_config_set(key: &str, value: &str, workspace: bool) -> Result<()> {
    if workspace {
        // L5+ — workspace concept lands then. Surface clearly.
        anyhow::bail!("--workspace requires an active workspace (L5+); not yet implemented");
    }
    let cfg = load_config()?;
    let target = global_config_path(&cfg).context("could not resolve ~/.swarmbotix path")?;
    sb_config::set_key(&target, key, value)?;
    println!("set {key} in {}", target.display());
    Ok(())
}

fn cmd_config_show(json: bool, sources: bool) -> Result<()> {
    let inputs = LoadInputs::from_env();
    let (merged, layers, _srcs) =
        sb_config::load_layers(&inputs).context("loading sb.config.yml")?;

    // The global file path is what `sb config open` / `sb config set`
    // edit — surfaces "where does this live?" without users having to
    // know the layering rules.
    let global_path = global_config_path(&merged).ok();
    let global_display = global_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "(unknown — no home dir)".to_owned());

    // Reference docs ship under <sb_home>/documents (installguide_ubuntu.md).
    // Surfaced so users can find sbcli_*.md without grep'ing the install.
    let documents_path = sb_config::resolve_sb_home_dir(&merged)
        .ok()
        .map(|h| h.join("documents"));
    let documents_display = documents_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "(unknown — no home dir)".to_owned());

    if json {
        let mut obj = serde_json::Map::new();
        obj.insert(
            "_config_file".into(),
            match &global_path {
                Some(p) => serde_json::Value::String(p.display().to_string()),
                None => serde_json::Value::Null,
            },
        );
        obj.insert(
            "_documents".into(),
            match &documents_path {
                Some(p) => serde_json::Value::String(p.display().to_string()),
                None => serde_json::Value::Null,
            },
        );
        if sources {
            for key in sb_config::KNOWN_KEYS {
                let value = sb_config::get_field(&merged, key)?;
                let source = sb_config::field_layer(&layers, key)?;
                let mut entry = serde_json::Map::new();
                entry.insert(
                    "value".into(),
                    match value {
                        Some(s) => serde_json::Value::String(s),
                        None => serde_json::Value::Null,
                    },
                );
                entry.insert(
                    "source".into(),
                    serde_json::Value::String(source.as_str().into()),
                );
                obj.insert((*key).to_owned(), serde_json::Value::Object(entry));
            }
        } else {
            let merged_val = serde_json::to_value(&merged).context("serializing config to JSON")?;
            if let serde_json::Value::Object(m) = merged_val {
                for (k, v) in m {
                    obj.insert(k, v);
                }
            }
        }
        let s = serde_json::to_string_pretty(&serde_json::Value::Object(obj))
            .context("serializing config to JSON")?;
        println!("{s}");
        return Ok(());
    }

    if sources {
        println!("config file: {global_display}");
        println!("documents:   {documents_display}");
        println!();
        // Plain-text table: key  value  source
        let mut rows: Vec<(String, String, &'static str)> = Vec::new();
        for key in sb_config::KNOWN_KEYS {
            let value = sb_config::get_field(&merged, key)?.unwrap_or_else(|| "(unset)".to_owned());
            let source = sb_config::field_layer(&layers, key)?;
            rows.push(((*key).to_owned(), value, source.as_str()));
        }
        let kw = rows.iter().map(|(k, _, _)| k.len()).max().unwrap_or(0);
        let vw = rows
            .iter()
            .map(|(_, v, _)| v.len())
            .max()
            .unwrap_or(0)
            .max(5);
        println!("{:<kw$}  {:<vw$}  source", "key", "value", kw = kw, vw = vw);
        println!("{}", "-".repeat(kw + vw + 10));
        for (k, v, s) in rows {
            println!("{:<kw$}  {:<vw$}  {}", k, v, s, kw = kw, vw = vw);
        }
    } else {
        println!("# config file: {global_display}");
        println!("# documents:   {documents_display}");
        let s = serde_yaml::to_string(&merged).context("serializing config to YAML")?;
        print!("{s}");
    }
    Ok(())
}

fn cmd_config_get(key: &str, json: bool) -> Result<()> {
    let inputs = LoadInputs::from_env();
    let (merged, layers, _srcs) =
        sb_config::load_layers(&inputs).context("loading sb.config.yml")?;
    let value = sb_config::get_field(&merged, key)?;
    let source = sb_config::field_layer(&layers, key)?;

    if json {
        let mut obj = serde_json::Map::new();
        obj.insert("key".into(), serde_json::Value::String(key.to_owned()));
        obj.insert(
            "value".into(),
            match &value {
                Some(s) => serde_json::Value::String(s.clone()),
                None => serde_json::Value::Null,
            },
        );
        obj.insert(
            "source".into(),
            serde_json::Value::String(source.as_str().into()),
        );
        let s = serde_json::to_string(&serde_json::Value::Object(obj))
            .context("serializing to JSON")?;
        println!("{s}");
        return Ok(());
    }

    match value {
        Some(v) => {
            println!("{v}");
            Ok(())
        }
        None => {
            // Exit non-zero so scripts can detect the unset case without
            // grep'ing stderr. Mirrors `git config --get`'s exit-1 behavior.
            anyhow::bail!("{key} is unset (source: {})", source.as_str())
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// topic (L4)
// ─────────────────────────────────────────────────────────────────────

fn cmd_topic_list(a: TopicListCli) -> Result<()> {
    let window = std::time::Duration::from_secs_f64(a.timeout.max(0.0));
    let report = sb_discover::discover_all(
        a.transport.map(TransportArg::to_discover),
        window,
        a.keyword.as_deref(),
        a.case_sensitive,
    )?;
    // Surface non-fatal per-backend failures (e.g. zenoh isn't running)
    // on stderr so the user knows the snapshot may be incomplete, but
    // still see whatever the other backend found on stdout.
    for e in &report.errors {
        eprintln!("warning: {e}");
    }
    let rows = report.into_merged();
    if a.json {
        print!("{}", sb_discover::render_json_lines(&rows));
    } else {
        print!("{}", sb_discover::render_table(&rows));
    }
    Ok(())
}

fn cmd_topic_listen(a: TopicListenCli) -> Result<()> {
    use sb_listen::FrameIter;
    let resolved = resolve_topic_for_runtime(&a.topic)?;
    let transport_tag = match a.transport {
        TransportArg::Zenoh => "zenoh",
        TransportArg::Iceoryx2 => "iceoryx2",
    };
    // Best-effort: build a dynamic-decode pool from the vault's
    // FileDescriptorSet so payload bodies render as decoded JSON. The
    // failure path (no protoc, vault empty, etc.) is non-fatal — we
    // just fall back to the hex-only `decode_frame`. The warning lands
    // on stderr so users know why they're getting hex instead of JSON.
    let pool = match build_descriptor_pool() {
        Ok(p) => Some(p),
        Err(e) => {
            eprintln!("warning: dynamic decode disabled ({e:#}); rendering payload as hex");
            None
        }
    };
    let mut iter: Box<dyn FrameIter> = match a.transport {
        TransportArg::Zenoh => Box::new(sb_listen::listen_zenoh(&resolved.full_topic)?),
        TransportArg::Iceoryx2 => Box::new(sb_listen::listen_iceoryx(&resolved.full_topic)?),
    };
    let mut emitted: u64 = 0;
    loop {
        if a.count != 0 && emitted >= a.count {
            return Ok(());
        }
        match iter.next_frame()? {
            None => return Ok(()),
            Some(bytes) => {
                let frame = match &pool {
                    Some(pool) => sb_listen::decode_frame_dynamic(
                        transport_tag,
                        &resolved.full_topic,
                        &bytes,
                        a.raw,
                        pool,
                    ),
                    None => {
                        sb_listen::decode_frame(transport_tag, &resolved.full_topic, &bytes, a.raw)
                    }
                };
                println!("{}", sb_listen::render_decoded_json_line(&frame));
                emitted += 1;
            }
        }
    }
}

/// Build a [`DescriptorPool`] from the vault's compiled FileDescriptorSet.
/// Compiles lazily on each call — fast enough for the one-shot `sb topic
/// listen` invocation, and keeps the cache invariant of "always reflects
/// what's in the vault NOW". Returns an error string when:
///   - `protoc` is missing from sb.config.yml,
///   - the vault is empty (no `.proto` files),
///   - `protoc` itself failed.
fn build_descriptor_pool() -> Result<prost_reflect::DescriptorPool> {
    let cfg = load_config()?;
    let vctx = vault_for(&cfg)?;
    vctx.ensure_installed()?;
    let vault = vctx.vault();
    let protoc = cfg
        .protoc
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("no protoc in sb.config.yml"))?;
    let names = vault.list()?;
    if names.is_empty() {
        anyhow::bail!("vault is empty");
    }
    let sources: Vec<PathBuf> = names.iter().map(|n| vault.path_of(n)).collect();
    let tmp = tempfile::NamedTempFile::new()?;
    sb_vault::compile_to_descriptor_set(protoc, vault.root(), &sources, tmp.path())?;
    let bytes = std::fs::read(tmp.path())?;
    let pool = prost_reflect::DescriptorPool::decode(bytes.as_slice())
        .map_err(|e| anyhow::anyhow!("DescriptorPool::decode: {e}"))?;
    Ok(pool)
}

fn cmd_topic_prune(a: TopicPruneCli) -> Result<()> {
    let report = sb_discover::prune_iceoryx()?;
    if a.json {
        // One JSON object — the natural shape for `PruneReport`. Distinct
        // from `sb topic list --json` (which is JSON-lines per topic)
        // because there's only ever one report row to emit.
        println!(
            "{}",
            serde_json::to_string(&report)
                .unwrap_or_else(|_| "{\"cleaned_pids\":[],\"failed\":[]}".to_string())
        );
        return Ok(());
    }
    if report.cleaned_pids.is_empty() && report.failed.is_empty() {
        println!("No dead iceoryx2 nodes found.");
        return Ok(());
    }
    if !report.cleaned_pids.is_empty() {
        println!(
            "Pruned {} dead iceoryx2 node(s):",
            report.cleaned_pids.len()
        );
        for pid in &report.cleaned_pids {
            println!("  - PID {pid}");
        }
    }
    for (pid, reason) in &report.failed {
        eprintln!("warning: failed to prune PID {pid}: {reason}");
    }
    if !report.failed.is_empty() {
        anyhow::bail!("{} dead node(s) could not be pruned", report.failed.len());
    }
    Ok(())
}

fn cmd_topic_pub(a: TopicPubCli) -> Result<()> {
    let resolved = resolve_topic_for_runtime(&a.topic)?;
    let bytes = sb_listen::zenoh_bytes::parse_hex(&a.bytes_hex)
        .with_context(|| format!("parsing hex {:?}", a.bytes_hex))?;
    match a.transport {
        TransportArg::Zenoh => sb_listen::publish_zenoh_raw(&resolved.full_topic, &bytes)?,
        TransportArg::Iceoryx2 => sb_listen::publish_iceoryx_raw(&resolved.full_topic, &bytes)?,
    }
    println!(
        "published {} byte(s) on {} via {:?}",
        bytes.len(),
        resolved.full_topic,
        a.transport
    );
    Ok(())
}

/// Resolve a bare-or-full topic for `sb topic listen/pub`. Builds the
/// full path against the cwd module's `sb.dev.yml` + the active
/// workspace + sb.config.yml's `device`. Fully-qualified inputs are
/// passed through unchanged (no cwd module needed).
fn resolve_topic_for_runtime(input: &str) -> Result<sb_discover::ResolveOutcome> {
    if input.starts_with('/') {
        // Fast path — `device` / `workspace` aren't needed for passthrough.
        return sb_discover::resolve_bare_topic(input, None, "", "")
            .map_err(|e| anyhow::anyhow!("{e}"));
    }
    let cfg = load_config()?;
    let home = sb_workspace::SbHome::from_config(&cfg)?;
    let cwd = std::env::current_dir().context("getting cwd")?;
    let dev_yml = cwd.join("sb.dev.yml");
    let cwd_module: Option<sb_core::ModuleDevConfig> = if dev_yml.exists() {
        let s = std::fs::read_to_string(&dev_yml)
            .with_context(|| format!("reading {}", dev_yml.display()))?;
        Some(serde_yaml::from_str(&s).with_context(|| format!("parsing {}", dev_yml.display()))?)
    } else {
        None
    };
    let workspace = sb_workspace::active_workspace(&home)?.unwrap_or_default();
    let device = cfg.device.clone().unwrap_or_else(|| "dev01".to_string());
    sb_discover::resolve_bare_topic(input, cwd_module.as_ref(), &device, &workspace)
        .map_err(|e| anyhow::anyhow!("{e}"))
}

// ─────────────────────────────────────────────────────────────────────
// launch (L5) — `sb up / run / down / attach`
// ─────────────────────────────────────────────────────────────────────

fn require_active_workspace() -> Result<(SbCliConfig, SbHome, String)> {
    let cfg = load_config()?;
    let home = SbHome::from_config(&cfg)?;
    let ws = active_workspace(&home)?.ok_or_else(|| {
        anyhow::anyhow!("no active workspace — run `sb ws create <name> && sb ws set <name>` first")
    })?;
    Ok((cfg, home, ws))
}

fn cmd_up() -> Result<()> {
    let (cfg, home, ws) = require_active_workspace()?;
    let plan = sb_launch::plan_workspace(&home, &ws)?;
    let tmux = sb_launch::resolve_tmux(cfg.tmux.as_deref())?;
    let outcome = sb_launch::up(&plan, &tmux)?;
    println!(
        "launched {} module(s) in tmux session {:?}",
        outcome.modules.len(),
        outcome.session
    );
    for m in &outcome.modules {
        println!("  - {m}");
    }
    println!("attach with: tmux attach -t {}", outcome.session);
    Ok(())
}

fn cmd_run(module: &str, args: &[String]) -> Result<()> {
    let (cfg, home, ws) = require_active_workspace()?;
    let (plan, pane) = sb_launch::plan_single(&home, &ws, module, sb_launch::Script::Run, args)?;
    let tmux = sb_launch::resolve_tmux(cfg.tmux.as_deref())?;
    let outcome = sb_launch::run_single(&plan, &pane, &tmux)?;
    let verb = if outcome.replaced {
        "restarted"
    } else {
        "launched"
    };
    if args.is_empty() {
        println!(
            "{verb} {} in tmux session {:?}",
            outcome.module, outcome.session
        );
    } else {
        println!(
            "{verb} {} in tmux session {:?} (args: {})",
            outcome.module,
            outcome.session,
            args.join(" "),
        );
    }
    Ok(())
}

fn cmd_stop(module: &str, args: &[String]) -> Result<()> {
    let (cfg, home, ws) = require_active_workspace()?;
    let (plan, pane) = sb_launch::plan_single(&home, &ws, module, sb_launch::Script::Stop, args)?;
    let tmux = sb_launch::resolve_tmux(cfg.tmux.as_deref())?;
    let outcome = sb_launch::run_single(&plan, &pane, &tmux)?;
    // `replaced` here just means the module's tmux window already held
    // another command (typically the running runscript) — that's the
    // expected case for "stop after run", so we don't surface the
    // distinction loudly.
    let verb = if outcome.replaced {
        "stopping (replaced existing window for)"
    } else {
        "stopping"
    };
    if args.is_empty() {
        println!(
            "{verb} {} in tmux session {:?}",
            outcome.module, outcome.session
        );
    } else {
        println!(
            "{verb} {} in tmux session {:?} (args: {})",
            outcome.module,
            outcome.session,
            args.join(" "),
        );
    }
    Ok(())
}

fn cmd_down() -> Result<()> {
    let (cfg, _home, ws) = require_active_workspace()?;
    let tmux = sb_launch::resolve_tmux(cfg.tmux.as_deref())?;
    let outcome = sb_launch::down(&ws, &tmux)?;
    if outcome.cleared {
        println!("killed tmux session {:?}", outcome.session);
    } else {
        println!("no tmux session {:?} — nothing to do", outcome.session);
    }
    Ok(())
}

fn cmd_attach() -> Result<()> {
    use std::io::IsTerminal;
    let (cfg, _home, ws) = require_active_workspace()?;
    let tmux = sb_launch::resolve_tmux(cfg.tmux.as_deref())?;
    let info = sb_launch::attach_info(&ws, &tmux)?;
    if std::io::stdout().is_terminal() {
        // Interactive — replace this process with `tmux attach`.
        let err = std::process::Command::new(&tmux)
            .args(["attach", "-t"])
            .arg(&info.session)
            .status();
        match err {
            Ok(s) if s.success() => return Ok(()),
            Ok(s) => anyhow::bail!("tmux attach exited with {s}"),
            Err(e) => anyhow::bail!("spawning tmux attach: {e}"),
        }
    }
    // Non-interactive — just report the session name so scripts can grep.
    println!("{}", info.session);
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────
// gopro (L5) — `sb gopro [--module <name>] [--all]`
// ─────────────────────────────────────────────────────────────────────

fn cmd_gopro(a: GoproArgs) -> Result<()> {
    if a.all {
        return cmd_gopro_all();
    }
    // Single module — resolve via the standard pub/sub path so `--module`
    // works the same way it does elsewhere.
    let (_cfg, _home, _vault, handle, _ws, _device) = resolve_for_pubsub(a.module.as_deref())?;
    let outcome = sb_gopro::gopro_module(&handle.dev_yml_path)?;
    print_gopro_outcome(&outcome);
    Ok(())
}

fn cmd_gopro_all() -> Result<()> {
    let (_cfg, home, ws) = require_active_workspace()?;
    let results = sb_gopro::gopro_all(&home, &ws)?;
    let mut ok = 0usize;
    let mut failed = 0usize;
    for r in &results {
        match &r.result {
            Ok(o) => {
                print_gopro_outcome(o);
                ok += 1;
            }
            Err(e) => {
                eprintln!("error: gopro {}: {e:#}", r.module);
                failed += 1;
            }
        }
    }
    println!("gopro --all: {ok} succeeded, {failed} failed");
    if failed > 0 {
        anyhow::bail!("{failed} module(s) failed to freeze");
    }
    Ok(())
}

fn print_gopro_outcome(o: &sb_gopro::GoproOutcome) {
    println!("froze module {} → {}", o.module, o.prd_yml_path.display());
    for m in &o.missing_files {
        eprintln!(
            "  warning: {} {} declared in sb.dev.yml but no file at {} \
             (run `sb {} edit ...` to regenerate)",
            m.role,
            m.name,
            m.expected_path.display(),
            m.role,
        );
    }
}

// ─────────────────────────────────────────────────────────────────────
// Vault discovery
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod discovery_tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// `<root>/<rel>/Thing.proto`, creating parents.
    fn touch_proto(root: &std::path::Path, rel: &str) {
        let dir = root.join(rel);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("Thing.proto"), "syntax = \"proto3\";\n").unwrap();
    }

    /// A standalone message repo cloned anywhere: `message_definitions/` sits
    /// at the repo root, so the repo IS the style. Standing in it must
    /// resolve the style from the folder name and root the vault at the
    /// repo's PARENT — otherwise `sb message compile` silently rebuilds the
    /// global vault instead (the bug this guards).
    #[test]
    fn cwd_holding_message_definitions_is_itself_the_style() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("vrobots_msgs");
        touch_proto(&repo, "message_definitions/states");

        let ctx = discover_local_vault_from(&repo).expect("repo should be discovered");
        assert_eq!(ctx.pinned_style.as_deref(), Some("vrobots_msgs"));
        assert_eq!(ctx.vault().root(), tmp.path());
        assert!(ctx.local, "a discovered root is never the global vault");
        // The pin is what keeps sibling directories out of a bare compile.
        assert_eq!(ctx.styles(), vec!["vrobots_msgs".to_string()]);
    }

    /// A discovered root must never receive the forge's embedded `std/`
    /// bundle — that would scatter `ros2/message_definitions/std/` through
    /// a user's own tree on the first `sb message` call.
    #[test]
    fn discovered_vault_never_installs_the_std_bundle() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("vrobots_msgs");
        touch_proto(&repo, "message_definitions/states");

        let ctx = discover_local_vault_from(&repo).unwrap();
        let report = ctx.ensure_installed().unwrap();
        assert!(report.copied.is_empty(), "nothing should be written");
        assert!(
            !tmp.path().join("ros2").exists(),
            "std bundle leaked into the user's tree"
        );
    }

    /// The forge's own layout: cwd is a repo containing `messages/<style>/`.
    /// Root is the `messages/` dir and no style is pinned, so every style
    /// compiles — the pre-existing behavior, which must not regress.
    #[test]
    fn conventional_messages_subtree_wins_with_no_pin() {
        let tmp = TempDir::new().unwrap();
        touch_proto(tmp.path(), "messages/ros2/message_definitions/std");
        touch_proto(tmp.path(), "messages/swarmbotix/message_definitions/header");

        let ctx = discover_local_vault_from(tmp.path()).expect("forge layout");
        assert_eq!(ctx.pinned_style, None);
        assert_eq!(ctx.vault().root(), tmp.path().join("messages"));
        assert_eq!(
            ctx.styles(),
            vec!["ros2".to_string(), "swarmbotix".to_string()]
        );
    }

    /// Standing in a namespace dir INSIDE a style still resolves that style:
    /// the walk goes up and hits `message_definitions/`'s parent.
    #[test]
    fn walks_up_from_a_namespace_dir_to_the_owning_style() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("vrobots_msgs");
        touch_proto(&repo, "message_definitions/states");

        let ctx = discover_local_vault_from(&repo.join("message_definitions").join("states"))
            .expect("should walk up");
        assert_eq!(ctx.pinned_style.as_deref(), Some("vrobots_msgs"));
        assert_eq!(ctx.vault().root(), tmp.path());
    }

    /// A directory named `messages` with no `<style>/message_definitions/`
    /// child is not a vault — matching is on shape, not on name.
    #[test]
    fn bare_directory_named_messages_is_not_a_vault() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("messages").join("inbox")).unwrap();
        assert!(discover_local_vault_from(tmp.path()).is_none());
    }
}

// ─────────────────────────────────────────────────────────────────────
// Backend selection
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod backend_tests {
    use super::*;
    use sb_core::Backend;
    use std::fs;
    use tempfile::TempDir;

    const ALL_TOOLS: ToolAvailability = ToolAvailability {
        protoc: true,
        flatc: true,
    };
    const NO_FLAGS: BackendFlags = BackendFlags {
        iox2: false,
        proto: false,
        fb: false,
    };

    /// A vault at `<tmp>/messages` with one style that has definitions.
    fn vault_with_style(tmp: &TempDir, style: &str) -> Vault {
        let d = tmp
            .path()
            .join("messages")
            .join(style)
            .join("message_definitions")
            .join("ns");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("A.proto"), "syntax=\"proto3\";").unwrap();
        Vault::at(tmp.path().join("messages"))
    }

    /// Turn `<tmp>/<name>` into something `unity_project_root` recognizes.
    fn make_unity_project(tmp: &TempDir, name: &str) -> std::path::PathBuf {
        let root = tmp.path().join(name);
        fs::create_dir_all(root.join("Assets")).unwrap();
        fs::create_dir_all(root.join("ProjectSettings")).unwrap();
        root
    }

    #[test]
    fn default_is_every_backend_the_box_can_generate() {
        let tmp = TempDir::new().unwrap();
        let v = vault_with_style(&tmp, "swarmbotix");
        let _out = v.targets_dir("swarmbotix");
        let s = resolve_backends(&v, "swarmbotix", NO_FLAGS, ALL_TOOLS).unwrap();
        assert_eq!(s.backends, vec![Backend::Iox2, Backend::Proto, Backend::Fb]);
        assert!(s.note.is_none(), "the ordinary path must stay quiet");
    }

    #[test]
    fn missing_tools_drop_their_backends() {
        let tmp = TempDir::new().unwrap();
        let v = vault_with_style(&tmp, "swarmbotix");
        let _out = v.targets_dir("swarmbotix");
        let tools = ToolAvailability {
            protoc: true,
            flatc: false,
        };
        let s = resolve_backends(&v, "swarmbotix", NO_FLAGS, tools).unwrap();
        assert_eq!(s.backends, vec![Backend::Iox2, Backend::Proto]);
    }

    /// `backends: []` is a real answer, not an empty config — it says "emit
    /// nothing automatically" and must not fall through to the default.
    /// A style inside a Unity project gets the full set. The generated C#
    /// targets C# 9, so there is nothing to protect Unity from — narrowing
    /// here would deprive the C++/Python/Rust consumers of the same style.
    #[test]
    fn unity_hosted_style_still_gets_every_backend() {
        let tmp = TempDir::new().unwrap();
        let unity = make_unity_project(&tmp, "virtual_robots");
        let scripts = unity.join("Assets").join("Scripts");
        fs::create_dir_all(scripts.join("vrobots_msgs").join("message_definitions")).unwrap();
        let v = Vault::at(&scripts);

        let s = resolve_backends(&v, "vrobots_msgs", NO_FLAGS, ALL_TOOLS).unwrap();
        assert_eq!(s.backends, vec![Backend::Iox2, Backend::Proto, Backend::Fb]);
        assert!(s.note.is_none(), "nothing was narrowed, so say nothing");
    }

    /// An explicit pin is still honored — it is a recorded decision, not an
    /// inference about the host project.
    #[test]
    fn style_pin_still_narrows() {
        let tmp = TempDir::new().unwrap();
        let v = vault_with_style(&tmp, "swarmbotix");
        v.write_style_config(
            "swarmbotix",
            &sb_core::StyleConfig {
                backends: Some(vec![Backend::Fb]),
            },
        )
        .unwrap();
        let s = resolve_backends(&v, "swarmbotix", NO_FLAGS, ALL_TOOLS).unwrap();
        assert_eq!(s.backends, vec![Backend::Fb]);
        assert!(s.note.unwrap().contains("pinned"));
    }

    /// An explicit flag still wins over a pin.
    #[test]
    fn explicit_flag_overrides_the_pin() {
        let tmp = TempDir::new().unwrap();
        let v = vault_with_style(&tmp, "swarmbotix");
        v.write_style_config(
            "swarmbotix",
            &sb_core::StyleConfig {
                backends: Some(vec![Backend::Fb]),
            },
        )
        .unwrap();
        let flags = BackendFlags {
            iox2: true,
            proto: false,
            fb: false,
        };
        let s = resolve_backends(&v, "swarmbotix", flags, ALL_TOOLS).unwrap();
        assert_eq!(s.backends, vec![Backend::Iox2]);
    }

    #[test]
    fn empty_pin_emits_nothing() {
        let tmp = TempDir::new().unwrap();
        let v = vault_with_style(&tmp, "swarmbotix");
        v.write_style_config(
            "swarmbotix",
            &sb_core::StyleConfig {
                backends: Some(vec![]),
            },
        )
        .unwrap();
        let _out = v.targets_dir("swarmbotix");
        let s = resolve_backends(&v, "swarmbotix", NO_FLAGS, ALL_TOOLS).unwrap();
        assert!(s.backends.is_empty());
    }

    /// A pin naming a backend whose tool is missing is an error, not a silent
    /// drop — otherwise the pin quietly stops meaning what it says.
    #[test]
    fn pin_naming_an_unavailable_tool_errors() {
        let tmp = TempDir::new().unwrap();
        let v = vault_with_style(&tmp, "swarmbotix");
        v.write_style_config(
            "swarmbotix",
            &sb_core::StyleConfig {
                backends: Some(vec![Backend::Fb]),
            },
        )
        .unwrap();
        let _out = v.targets_dir("swarmbotix");
        let tools = ToolAvailability {
            protoc: true,
            flatc: false,
        };
        let err = resolve_backends(&v, "swarmbotix", NO_FLAGS, tools).unwrap_err();
        assert!(err.to_string().contains("flatc"), "{err}");
    }

    /// flatc emits namespace subdirs for Rust/Python; they get flattened up
    /// into the per-message dir. The flattening must be IDEMPOTENT: the old
    /// code skipped the move whenever the flat copy already existed, so a
    /// second compile left the whole nested tree behind as a duplicate — and
    /// Unity then failed on duplicate type definitions.
    #[test]
    fn flattening_namespace_subdirs_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let msg_dir = tmp.path().join("StatesMsg");

        // Simulate one flatc run: nested namespace dir + an __init__.py marker.
        let seed = || {
            let nested = msg_dir.join("swarmbotix").join("states");
            fs::create_dir_all(&nested).unwrap();
            fs::write(nested.join("StatesMsg.py"), "# generated").unwrap();
            fs::write(nested.join("StatesMsg.rs"), "// generated").unwrap();
            fs::write(nested.join("__init__.py"), "").unwrap();
        };

        for run in 1..=3 {
            seed();
            flatten_namespace_subdirs(&msg_dir).unwrap();

            let files: Vec<String> = fs::read_dir(&msg_dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_file())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            let mut sorted = files.clone();
            sorted.sort();
            assert_eq!(
                sorted,
                vec!["StatesMsg.py".to_string(), "StatesMsg.rs".to_string()],
                "run {run}: files must be flat and not accumulate"
            );
            assert!(
                !msg_dir.join("swarmbotix").exists(),
                "run {run}: the emptied namespace dir must be removed"
            );
        }
    }

    #[test]
    fn unity_root_needs_both_markers() {
        let tmp = TempDir::new().unwrap();
        // Assets/ alone is far too generic to treat as a Unity project.
        let only_assets = tmp.path().join("not_unity");
        fs::create_dir_all(only_assets.join("Assets").join("deep")).unwrap();
        assert!(sb_vault::unity_project_root(&only_assets.join("Assets").join("deep")).is_none());

        let unity = make_unity_project(&tmp, "real_unity");
        let deep = unity
            .join("Assets")
            .join("Scripts")
            .join("x")
            .join("message_targets");
        assert_eq!(
            sb_vault::unity_project_root(&deep).as_deref(),
            Some(unity.as_path())
        );
    }
}

// ─────────────────────────────────────────────────────────────────────
// App dispatch
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod app_dispatch_tests {
    use super::*;
    use clap::CommandFactory;

    /// `sb_apps::BUILTIN_NAMES` decides which app names are refused. It must
    /// list exactly the top-level subcommands (plus aliases and `help`), or an
    /// app could shadow a new built-in or be refused for a name nothing uses.
    #[test]
    fn builtin_names_match_the_command_tree() {
        let cli = Cli::command();
        let mut names: Vec<String> = cli
            .get_subcommands()
            .flat_map(|c| {
                std::iter::once(c.get_name().to_owned())
                    .chain(c.get_all_aliases().map(str::to_owned))
            })
            .collect();
        names.push("help".to_owned());
        for n in &names {
            assert!(
                sb_apps::is_builtin(n),
                "`sb {n}` is missing from sb_apps::BUILTIN_NAMES"
            );
        }
        for b in sb_apps::BUILTIN_NAMES {
            assert!(
                names.iter().any(|n| n == b),
                "sb_apps::BUILTIN_NAMES lists `{b}`, which is not an sb subcommand"
            );
        }
    }

    #[test]
    fn unknown_subcommand_parses_as_an_app_with_raw_args() {
        let cli = Cli::try_parse_from(["sb", "camcalib", "--help", "-x", "a b"]).unwrap();
        match cli.cmd {
            Some(Cmd::External(argv)) => assert_eq!(argv, ["camcalib", "--help", "-x", "a b"]),
            _ => panic!("expected an external app subcommand"),
        }
    }

    #[test]
    fn app_kind_answers_parse() {
        assert_eq!(parse_app_kind("Docker"), Ok(sb_apps::AppKind::Docker));
        assert_eq!(parse_app_kind("none"), Ok(sb_apps::AppKind::None));
        assert!(parse_app_kind("vm").is_err());
    }
}
