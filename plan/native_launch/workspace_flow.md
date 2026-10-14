# 0.8.0: `sb.workspace.yaml` + `*.flow.yaml`, name-keyed ports, no docker

Status: implemented 2026-10-14, bumped to 0.8.0, not pushed, not installed. Verification: `cargo fmt --all --check`, clippy with `-D warnings`, `cargo test --workspace` (145 tests) and `level5_e2e_hello_world` all green. Departures from this contract: `sb stop` reads `sb.workspace.yaml` only (no flow); a flow's `name:` must equal its file stem or `sb install` refuses it; the installed default flow keeps the name `flows/sb.flow.yaml`; `sb init` registers a module but never adds a node, so a flow written by hand follows `ws create` + `init`.

Rollback point: sbcli `28700da`, sbcli_forge `058b9d6`.

Execution: one write-only wave, three `coder` agents (`core`, `ws-launch-cli`,
`docs`), disjoint files, no cargo. Main thread: `cargo fmt --all`, `cargo
check`, errors routed back, clippy `-D warnings`, build, one `cargo test
--workspace`, `level5_e2e_hello_world`, bump 0.8.0, report, no push.

Every name in §3 is fixed. YAML house style everywhere (docs, templates,
fixtures): never `{ }`; records keyed by name; short plain lists inline
(`[a, b]` or one string for `args`); no bare keys with empty values.

---

## 1. File shapes

### sb.module.yaml

```yaml
module: ips_detect
language: rust
version: 0.3.0                      # optional
sb: ">=0.8"                         # optional
description: ...                    # optional
scripts:                            # optional, language defaults
  build: cargo build --release
  start: "{bin} {args}"
  test: cargo test
  stop: [echo stopping, rm -f /tmp/x.lock]
env:
  RUST_LOG: info
requires: [cargo]
platforms:
  linux:
    env:
      LIBCLANG_PATH: ${LIBCLANG_PATH:-/usr/lib/llvm-18/lib}
    requires: [cmake]
  windows:
    env:
      LIBCLANG_PATH: ${LIBCLANG_PATH:-C:/Program Files/LLVM/bin}
io_dir: swarmbotix_io
publishers:                         # map keyed by port name
  markers:
    type: ros2/custom/MarkerDetections
    transport: zenoh
  status:
    topic: health                   # optional, defaults to the key
    type: ros2/std/StringStamped
    transport: zenoh
    encoding: raw
    mime: text/plain
subscribers:
  image:
    type: ros2/sensor_msgs/Image1280x1024
    transport: iceoryx2
  commands:
    key: dev01/ops/console/zenoh/cmd
    type: ros2/std/StringStamped
    transport: zenoh
services: [cam_param]               # list of names; map form `cam_param:` + options also read
```

Read rules: `publishers` / `subscribers` accept the map above or the 0.7
dash list (`- name: ...`). In the map form an inner `name:` is allowed only
if equal to the key. `services` accepts `[a, b]`, a map keyed by name, or the
0.7 list of `{name}` maps. Duplicate keys are a YAML error (serde_yaml
reports them); duplicate names across the list form stay the existing
validation error. Write rules: always the map form; `services` as `[..]`.

### sb.workspace.yaml (repo root)

```yaml
name: ws_ips
sb: ">=0.8"                         # optional
modules:
  ips_cam_gcam_pub: modules/ips_cam_gcam_pub
  ips_detect: modules/ips_detect
  ips_track: modules/ips_track
  ips_viz: modules/ips_viz
  ips_calib: modules/ips_calib
messages:                           # map keyed by a label
  custom:
    style: ros2
    path: messages/custom
    backends: [proto, iox2]
```

`modules` values are paths only (string). `messages` also accepts the 0.7
list form (label defaults to the path's last component). A `modules:` or
`messages:` key with a null value means empty.

### `<name>.flow.yaml` (repo root; `sb.flow.yaml` is the flow named `default`)

```yaml
name: runtime                       # optional; defaults to the file stem, "default" for sb.flow.yaml
nodes: [ips_cam_gcam_pub, ips_detect, ips_track, ips_viz]
instances:                          # optional; a node absent here is a singleton
  ips_cam_gcam_pub: [sb00, sb01, sb02, sb03]
  ips_detect: [sb00, sb01, sb02, sb03]
instance_settings:                  # optional; node -> id -> settings
  ips_detect:
    sb02:
      label: East wall
      args: --debug --fps 30        # string (split like scripts) or [..]
      env:
        RUST_LOG: debug
wires:                              # publisher on the left, one subscriber or [subscribers]
  ips_cam_gcam_pub[{id}].image: ips_detect.image
  ips_detect[*].markers: [ips_track.markers, ips_calib.markers]
  ips_track.cube_poses: ips_viz.cube_poses
```

Validation (sb-core, no manifests needed): nodes are identifiers and
unique; every `instances` and `instance_settings` key is a node; ids pass
`validate_instance_id` and are unique per node; every settings id is in that
node's instances; every wire module (both sides) is a node; selector grammar
`[{id}]` `[*]` `[<id>]` or none, on the publisher side only; `{id}` requires
the subscriber node to have instances. Port existence, transport and type
checks stay in sb-doctor.

### Legacy, read for one release (0.8.0 only)

A `sb.flow.yaml` in the 0.7 shape (top-level `modules:` whose values are
paths or `{path, up, instances}` maps) is read as workspace + default flow:
modules and messages into the workspace; nodes = modules with `up != false`;
inline instances into `instances`; per-instance `args`/`env`/`label` into
`instance_settings`; `docker:` dropped; wires flipped to publisher-first.
A `Deprecation` is pushed:
`item: "sb.flow.yaml in the 0.7 shape"`,
`hint: "split it into sb.workspace.yaml and sb.flow.yaml; see sbcli_workspace.md §Migrating"`.
Removed in 0.8.1. The 0.7 list forms of ports/services/messages are read
silently and are not deprecated.

## 2. Behaviour

| command | 0.8.0 |
|---|---|
| `sb install .` | reads `sb.workspace.yaml` (or the legacy flow), copies the workspace and every `*.flow.yaml` into the installed copy; no flow required |
| `sb ws create <ws>` | writes `sb.workspace.yaml` (`name:`, `modules:` empty, no messages) and `sb.flow.yaml` (`nodes: []`) |
| `sb init` | registers the module in `sb.workspace.yaml` only; `--docker` removed |
| `sb up [FLOW]` | flow by name, default `default`; nodes of that flow only; records it as the active flow; refuses if the session exists |
| `sb run <module>[:<id>]` | instances from the active flow, else `default`; a module that is not a node of that flow is refused |
| `sb stop`, `sb down`, `sb attach` | unchanged |
| `sb ws check`, `sb doctor` | row `ws flows` replaces `ws instances` and `ws wires`: one line per flow, `<flow>: <n> node(s), <i> instance(s), <w> wire(s) resolve` OK, or FAIL lines `<flow>: <wire or item>: <reason>`; row `deprecated` lists a legacy-shape flow |
| `sb io *` | unchanged semantics; writes the map form |

## 3. Rust surface

### sb-core (`crates/sb-core/src/config.rs`, `topic.rs`, `lib.rs`)

```rust
pub const WORKSPACE_FILE: &str = "sb.workspace.yaml";
pub const DEFAULT_FLOW_FILE: &str = "sb.flow.yaml";
pub const FLOW_SUFFIX: &str = ".flow.yaml";
pub const DEFAULT_FLOW_NAME: &str = "default";
/// "sb.flow.yaml" -> "default"; "calib.flow.yaml" -> "calib"; None if not a flow file name.
pub fn flow_name_of(file_name: &str) -> Option<String>;
/// "default" -> "sb.flow.yaml"; "calib" -> "calib.flow.yaml".
pub fn flow_file_name(flow: &str) -> String;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workspace {
    pub name: Option<String>,
    pub sb: Option<String>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub modules: BTreeMap<String, PathBuf>,
    #[serde(default, deserialize_with = "messages_from_map_or_list", serialize_with = "messages_to_map", skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<MessageSource>,
    pub source: Option<WorkspaceSource>,         // was FlowSource; same fields, repo_workspace instead of repo_flow
}
impl Workspace { pub fn validate(&self) -> Result<(), FlowValidationError>; }

pub struct MessageSource { pub label: String, pub style: String, pub path: PathBuf, pub backends: Vec<String> }

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Flow {
    pub name: Option<String>,
    #[serde(default)] pub nodes: Vec<String>,
    #[serde(default, deserialize_with = "null_as_default", skip_serializing_if = "BTreeMap::is_empty")]
    pub instances: BTreeMap<String, Vec<String>>,
    #[serde(default, deserialize_with = "null_as_default", skip_serializing_if = "BTreeMap::is_empty")]
    pub instance_settings: BTreeMap<String, BTreeMap<String, InstanceSettings>>,
    #[serde(default, deserialize_with = "wires_from_map", serialize_with = "wires_to_map", skip_serializing_if = "Vec::is_empty")]
    pub wires: Vec<Wire>,
}
impl Flow {
    pub fn validate(&self) -> Result<(), FlowValidationError>;
    pub fn is_node(&self, module: &str) -> bool;
    pub fn instances_for(&self, node: &str) -> &[String];
    pub fn settings_for(&self, node: &str, id: &str) -> Option<&InstanceSettings>;
    /// Convenience for launch: (id, settings-or-default) per instance; empty for a singleton.
    pub fn resolved_instances(&self, node: &str) -> Vec<ModuleInstance>;
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceSettings {
    pub label: Option<String>,
    #[serde(default, deserialize_with = "argv_from_str_or_list", serialize_with = "argv_to_str", skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}
/// Kept as the launch-side value type. No docker field.
pub struct ModuleInstance { pub id: String, pub label: Option<String>, pub args: Vec<String>, pub env: BTreeMap<String, String> }

pub struct Wire { pub source: WireSource, pub sinks: Vec<WireEnd> }
pub struct WireSource { pub module: String, pub selector: WireSelector, pub entry: String }
pub struct WireEnd { pub module: String, pub entry: String }
pub enum WireSelector { None, SameId, All, Id(String) }
/// lhs "<module>[<sel>].<entry>" (selector optional), rhs one "<module>.<entry>" or several.
pub fn parse_wire(lhs: &str, rhs: &[String]) -> Result<Wire, FlowValidationError>;

/// Detects a 0.7-shape flow (top-level `modules:` with path values). Pure; no IO.
pub fn probe_legacy_flow_shape(yaml: &str) -> bool;
/// Converts a 0.7-shape flow document into (Workspace, Flow). Pure.
pub fn convert_legacy_flow(yaml: &str) -> Result<(Workspace, Flow), FlowValidationError>;
```

Deleted: `WorkspaceFlow`, `ModuleRef`, `ModuleRefRaw`, `DockerSpec`,
`FlowSource` (renamed), `probe_legacy_flow_keys`, `LegacyTopLevelInstances`.
`FlowValidationError` gains `UnknownNode(flow_item, name)`, `DuplicateNode`,
`SettingsForUnknownInstance(node, id)`, `BadWire(lhs, reason)`,
`LegacyFlowShape`. `args` string splitting reuses `Script::parse_line`.

Ports: `ModuleDevConfig.publishers/subscribers` keep type `Vec<PubSpec>` /
`Vec<SubSpec>` with `deserialize_with = "ports_from_map_or_list"`,
`serialize_with = "ports_to_map"`; `services: Vec<ServiceSpec>` with
`services_from_any` / `services_to_list`. `RawSpec.name` becomes `Option`;
map entries get the key as name. Error paths read `publishers.markers: ...`
in the map form and keep `publishers[0]: ...` in the list form.

### sb-workspace (`crates/sb-workspace/src/lib.rs`)

```rust
pub use sb_core::{WORKSPACE_FILE, DEFAULT_FLOW_FILE, FLOW_SUFFIX, DEFAULT_FLOW_NAME};
pub fn find_workspace_file(dir: &Path) -> Option<PathBuf>;              // sb.workspace.yaml
pub fn find_flow_files(dir: &Path) -> Vec<(String, PathBuf)>;           // every *.flow.yaml, sorted, default first
pub fn find_workspace_upwards(start: &Path) -> Result<Option<PathBuf>>; // replaces find_flow_upwards
// installed layout: <home>/workspaces/<ws>/sb.workspace.yaml, <home>/workspaces/<ws>/flows/<name>.flow.yaml, <home>/workspaces/<ws>/active_flow
impl SbHome {
    pub fn workspace_file(&self, ws: &str) -> PathBuf;
    pub fn flow_file(&self, ws: &str, flow: &str) -> PathBuf;
    pub fn flows_dir(&self, ws: &str) -> PathBuf;
}
pub fn read_workspace(home: &SbHome, ws: &str) -> Result<Workspace>;
pub fn read_flow(home: &SbHome, ws: &str, flow: &str) -> Result<Flow>;   // error: `no flow named {flow} in workspace {ws} (flows: a, b)`
pub fn list_flows(home: &SbHome, ws: &str) -> Result<Vec<String>>;
pub fn active_flow(home: &SbHome, ws: &str) -> Result<Option<String>>;
pub fn set_active_flow(home: &SbHome, ws: &str, flow: &str) -> Result<()>;
pub fn install_workspace(home: &SbHome, repo_file: &Path, url: Option<&str>) -> Result<InstallReport>; // replaces install_flow; repo_file is sb.workspace.yaml or a legacy sb.flow.yaml
pub struct InstallReport { pub name, pub created, pub installed_workspace: PathBuf, pub flows: Vec<String>, pub modules: Vec<InstalledModule>, pub messages, pub deprecations: Vec<Deprecation> }
pub struct InitOptions { pub root, pub language, pub force }               // docker removed
```

`register_in_repo_flow` becomes `register_in_workspace` (writes
`modules.<name>: <relative path>`). `create_workspace` writes both files.
`resolve_module_yaml` unchanged in spirit (directory -> `sb.module.yaml`).

### sb-launch

```rust
pub fn plan_up(home: &SbHome, ws: &str, flow: Option<&str>, host: &Host) -> Result<LaunchPlan>;  // replaces plan_workspace
pub fn plan_run(home: &SbHome, ws: &str, target: &str, args: &[String], host: &Host) -> Result<LaunchPlan>; // flow = active_flow or default
pub fn plan_stop(..) // unchanged signature
pub struct LaunchPlan { ..., pub flow: String }   // new field
pub fn up(plan, tmux) // also calls set_active_flow(plan.flow)
```

Errors: `module {m} is not a node of flow {f}`; `flow {f} has no nodes`.

### sb-doctor

`WorkspaceView { name, workspace: Workspace, flows: Vec<(String, Flow)>, vault_root, stale_repo, host }`.
Rows: `ws modules` (from workspace), `ws types`, `ws encoding`, `ws io files`,
`ws requires`, `ws platforms`, `ws flows` (new, replaces `ws instances` and
`ws wires`; per-flow lines; reasons as in the 0.6 contract §8 plus
`unknown node <n>`), `deprecated`, `ws source`.

### sb-cli

`Cmd::Up { flow: Option<String> }` (positional `[FLOW]`). `cmd_up` prints
`flow {name}: launched {n} window(s) in tmux session "{s}"` then the window
lines. `sb install` prints `-- flows: default, calib` after the modules line.
`InitArgs` loses `--docker`. `sb ws check` unchanged entry point.
`sb ws create` prints `wrote sb.workspace.yaml and sb.flow.yaml`.

## 4. Tests (minimal)

Fix: `level2_workspace_lifecycle` (two files written), `level2_ws_install`
(fixture becomes workspace + flow), `level2_init_*` (registration target),
`level2_legacy_manifest` (now also covers a 0.7 flow installing with the
deprecation warning), `level3_pub_add_mutation` (written manifest lines in
map form), `level5_native_*`, `level5_run_stop_args`,
`level5_launch_two_modules`, `l5_sandbox` (flow fixtures), sb-core unit
tests for the new structs. Add one: `sb up calib` launches only that flow's
nodes and `sb run` afterwards reads the active flow.

## 5. Docs (`docs` agent)

New `documents/sbcli_workspace.md` (inventory file; Migrating from 0.7
section with the before/after split, docker removal, compose pattern) and
`documents/sbcli_flow.md` (diagram file: nodes, instances, instance_settings,
wires publisher-first with selectors and fan-out, multiple flows, `sb up
<flow>`, active flow). `sbcli_module.md` reshaped to the map form, services
list, compose section. `sbcli_ws.md`, `sbcli_launch.md`, `sbcli_init.md`,
`sbcli_doctor.md`, `sbcli.md` (layout, command table, deprecations: 0.7 flow
shape and `--docker`), `sbcli_pubsub.md` and `_consuming.md` examples,
`requirements.md`, README. Every example in house style. Marker `sb 0.8.0+`.
Examples: nine `sb.module.yaml` to map form; each example's `sb.flow.yaml`
becomes `sb.workspace.yaml` + `sb.flow.yaml`; `run.sh` and READMEs follow.
