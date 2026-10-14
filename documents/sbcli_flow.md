# `*.flow.yaml`: Flow Reference

Official reference for flow files, `sb.flow.yaml` and
`<name>.flow.yaml` at the root of a workspace repository. A flow is the
diagram of one way to run the workspace: which modules run (nodes), how
many processes each one gets (instances), what each instance receives
(instance settings), and which publisher feeds which subscriber
(wires).

This document is normative for behavior shipped in `sb 0.8.0+`.

The canonical spec is `requirements.md` in the sbcli **source
repository**, a maintainer document that is not part of your install;
when it and this page disagree, it wins.

Related: [sbcli_workspace.md](sbcli_workspace.md) (`sb.workspace.yaml`,
the modules a flow can name), [sbcli_launch.md](sbcli_launch.md)
(`sb up [FLOW]`, `sb run`), [sbcli_module.md](sbcli_module.md) (what a
started process sees), [sbcli_doctor.md](sbcli_doctor.md) §4.11 (the
`ws flows` row).

**Added in sb 0.8.0.** A flow file holds what the 0.7 `sb.flow.yaml`
said about running (instances, `up:`, wires), and its wires now read
publisher first. [sbcli_workspace.md](sbcli_workspace.md) §7 migrates a
0.7 file.

---

## 1. Conceptual model

A flow names modules that the workspace already holds
([sbcli_workspace.md](sbcli_workspace.md) §2.1); it never holds a path.
`sb up <flow>` builds and launches exactly that flow's nodes, one tmux
window per instance, and nothing else.

A workspace may hold several flows side by side, all at the repository
root:

| File | Flow name | Launched by |
|---|---|---|
| `sb.flow.yaml` | `default` | `sb up` |
| `calib.flow.yaml` | `calib` | `sb up calib` |
| `<name>.flow.yaml` | `<name>` | `sb up <name>` |

Flows are about running, not about code. `sb install` regenerates the
io of every registered module whether or not any flow uses it, and
`sb io` works on every registered module.

---

## 2. Schema

```yaml
name: calib                         # optional; defaults to the file stem, "default" for sb.flow.yaml
nodes: [ips_cam_gcam_pub, ips_detect, ips_track, ips_calib]
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
```

| Key | Required | Meaning |
|---|---|---|
| `name` | no | The flow's name. Default: the file stem, and `default` for `sb.flow.yaml`. A flow is addressed by its file name (`sb up calib` reads `calib.flow.yaml`): a `name:` that does not match the file name makes `sb install` refuse the flow, so leave it out or keep it equal to the stem. |
| `nodes` | no, but `sb up` refuses a flow without any | The modules this flow runs, by name (§3). |
| `instances` | no | Node to list of instance ids (§4). |
| `instance_settings` | no | Node to instance id to `label`, `args`, `env` (§5). |
| `wires` | no | Publisher entry to one or more subscriber entries (§6). |

Load rules:

- `deny_unknown_fields` applies to the file and to each settings map,
  so a typo such as `instance_setings:` or `evn:` is reported by name.
- The structural rules of §8.1 are checked whenever the flow is read,
  with no module manifest needed: a flow that breaks one fails the
  install before anything is written.
- Whether the ports a wire names exist, and agree, needs the manifests;
  `sb ws check` and `sb doctor` check that in the `ws flows` row (§8.2).

---

## 3. `nodes`

```yaml
nodes: [ips_cam_gcam_pub, ips_detect, ips_track, ips_viz]
```

- Each node is the name of a module in `sb.workspace.yaml`; it follows
  the identifier rule, and a name may appear only once.
- `sb up <flow>` builds and starts the nodes, and only the nodes, of
  that flow.
- A registered module that is not a node of a flow is neither built
  nor started by `sb up <flow>`, and `sb run` refuses it while that flow
  is active (§7.4). This replaces the per-module `up` flag of earlier
  releases.
- Every other key of the flow (`instances`, `instance_settings`, both
  sides of every wire) may only name nodes.

`sb ws create` writes `nodes: []`, and `sb init` registers a module
without touching any flow, so a new module is added here by hand:

```bash
sb init --python modules/ips_track     # registered in sb.workspace.yaml
# edit sb.flow.yaml:  nodes: [ips_cam_gcam_pub, ips_detect, ips_track]
sb install .
```

---

## 4. `instances`

```yaml
instances:
  ips_cam_gcam_pub: [sb00, sb01, sb02, sb03]
  ips_detect: [sb00, sb01, sb02, sb03]
```

An instance is one process of a module: the same code, its own id, its
own tmux window, its own wire paths.

- Each key is a node; each value is that node's list of ids.
- A node absent here is a **singleton**: one window named after the
  module, no `--id`, no `SB_INSTANCE_ID`.
- An id matches `[A-Za-z0-9_-]+` and is unique within its node. A
  number such as `1` is read as the id `"1"`.
- Each id becomes the window name `<module>-<id>`, the `--id <id>`
  appended to the start command, the `SB_INSTANCE_ID` variable, and,
  through the generated `TopicOverrides`, the `<module>-<id>` segment
  of the instance's wire keys
  ([sbcli_pubsub_consuming.md](sbcli_pubsub_consuming.md) §10).
- Instances belong to the flow, not to the module: the same module can
  run four instances in one flow and one in another.

`sb up` and `sb run <module>` start every instance of a node;
`sb run <module>:<id>` starts one ([sbcli_launch.md](sbcli_launch.md)
§3).

---

## 5. `instance_settings`

```yaml
instance_settings:
  ips_detect:
    sb02:
      label: East wall
      args: --debug --fps 30
      env:
        RUST_LOG: debug
```

Keyed by node, then by instance id. Only the instances that need
something appear; an instance without an entry runs with none.

| Field | Meaning |
|---|---|
| `label` | Display string for the L6 graph editor. Not used at runtime. |
| `args` | Extra arguments for this instance, placed where the start command has `{args}`, before any passthrough args of `sb run` ([sbcli_module.md](sbcli_module.md) §9). A string is split like a script command: whitespace separates, `"..."` and `'...'` group ([sbcli_module.md](sbcli_module.md) §3.1). A list such as `[--name, front cam]` is taken element by element. |
| `env` | Environment variables for this instance, over the manifest's `env:` ([sbcli_module.md](sbcli_module.md) §8). |

- Each node key must be a node of the flow, and each id must be in that
  node's `instances`.
- The instance settings are the last step of the merge order
  ([sbcli_module.md](sbcli_module.md) §8): they override the
  manifest's environment for that one process.
- Where `sb` writes a flow (the installed copy), `args` comes out as
  one string, or as a list when one of its arguments holds whitespace
  or quotes; `nodes` and multi-subscriber wires come out as block
  lists. Files you write may use either form.

---

## 6. `wires`

```yaml
wires:                              # publisher on the left, one subscriber or [subscribers]
  ips_cam_gcam_pub[{id}].image: ips_detect.image
  ips_detect[*].markers: [ips_track.markers, ips_calib.markers]
  ips_track.cube_poses: ips_viz.cube_poses
```

### 6.1 Grammar

```
<publisher module>[<selector>].<publisher entry>: <subscriber module>.<subscriber entry>
<publisher module>[<selector>].<publisher entry>: [<module>.<entry>, <module>.<entry>, ...]
```

- The key is a **publisher** entry of a node; the selector, optional,
  picks which of the publisher's instances (§6.2).
- The value is one **subscriber** entry of a node, or a list of them
  (§6.3). A subscriber never carries a selector.
- `<entry>` is the entry's name in the module's `sb.module.yaml`
  (the key under `publishers:` or `subscribers:`).
- Both modules must be nodes of this flow.

### 6.2 Selectors

| Selector | Example | Meaning |
|---|---|---|
| none | `ips_track.cube_poses` | the publisher node is a singleton |
| `[{id}]` | `ips_cam_gcam_pub[{id}].image` | each subscriber instance takes the publisher instance with its own id; the subscriber node must have instances |
| `[*]` | `ips_detect[*].markers` | every instance of the publisher feeds the subscriber (fan-in); Zenoh only |
| `[<id>]` | `ips_cam_gcam_pub[sb02].image` | that one instance of the publisher |

### 6.3 Fan-out

One publisher entry may feed several subscribers. List them on the
right:

```yaml
wires:
  ips_detect[*].markers: [ips_track.markers, ips_calib.markers]
```

A key appears only once in YAML, so all subscribers of one publisher
entry with one selector go in one list; a second line with the same key
is a duplicate-key error. The same publisher entry with a different
selector (`cam[sb00].image` beside `cam[{id}].image`) is a different
key.

### 6.4 Quoting

None of the forms above needs quotes. In a YAML block mapping a key or
value that starts with a letter may contain `[`, `]`, `{`, `}`, and `*`.
Quoting them anyway (`"ips_detect[*].markers"`) is accepted.

### 6.5 What `sb` does with wires

- **On load** (`sb install`, `sb up`, `sb run`): the grammar, the
  selector rules, and "both sides are nodes" are checked (§8.1).
- **In `sb ws check` and `sb doctor`**: each wire is resolved against
  the manifests in the `ws flows` row (§8.2): both entries exist,
  transports and types agree, `[*]` is not on iceoryx2, `{id}` meets
  instances.
- **Code generation does not read them yet.** Until a later release
  derives subscriber keys from `wires:`, subscribers still pair through
  `key:` entries ([sbcli_pubsub.md](sbcli_pubsub.md) §10.4) or the
  `TopicOverrides` a module's `main()` builds from its id
  ([sbcli_pubsub_consuming.md](sbcli_pubsub_consuming.md) §10.4).
  Declaring wires now costs nothing and documents the graph.

---

## 7. Multiple flows and the active flow

### 7.1 Naming

A flow file is any `*.flow.yaml` at the repository root. Its name is
the file stem: `calib.flow.yaml` is `calib`, and `sb.flow.yaml` is
`default`. The default flow is always `sb.flow.yaml`: a file named
`default.flow.yaml` is not a flow file. Flow files in subdirectories
are not read.

### 7.2 Installing

`sb install .` copies every flow file into the installed copy
(`<sb_home>/workspaces/<ws>/flows/`,
[sbcli_workspace.md](sbcli_workspace.md) §6) and names them on one
line, the default flow first (`-- flows: none` for a workspace
without any):

```
-- flows: default, calib
```

A flow added, renamed, or edited later reaches `sb up` only after the
next `sb install .`.

### 7.3 `sb up [FLOW]`

```bash
sb up                 # the flow named default (sb.flow.yaml)
sb up calib           # calib.flow.yaml
```

`sb up` builds and launches the nodes of one flow and records it as the
workspace's **active flow**. Its launch line names the flow:

```
flow calib: launched 10 window(s) in tmux session "sb-ws_ips"
```

The tmux session is per workspace, not per flow, and `sb up` refuses a
session that already exists. Switching flows is therefore
`sb down && sb up <other>`. An unknown flow is refused with the list of
installed ones:

```
error: no flow named calibration in workspace ws_ips (flows: default, calib)
```

(`(flows: none)` when the workspace has no flow installed.)

The whole `sb up` sequence (plan, build, launch) is in
[sbcli_launch.md](sbcli_launch.md) §2.

### 7.4 The active flow

The name of the flow the last `sb up` launched is kept in
`<sb_home>/workspaces/<ws>/active_flow`.

- `sb run <module>[:<id>]` takes the module's instances and instance
  settings from the active flow, or from `default` when no flow has
  been launched yet. A module that is not a node of that flow is
  refused:

  ```
  error: module ips_calib is not a node of flow default
  ```

- `sb stop` reads only `sb.workspace.yaml`, and `sb down` and
  `sb attach` only the session; none of them reads a flow. `sb down` leaves the active flow recorded,
  so a later `sb run` still uses it.

---

## 8. Validation

### 8.1 On load

Checked every time a flow is read, without any module manifest. Each
rule, and the error it gives:

| Rule | Error |
|---|---|
| every node is an identifier, listed once | `nodes: <n> is listed more than once` |
| every key of `instances` is a node | `instances: unknown node <n>` |
| every key of `instance_settings` is a node | `instance_settings: unknown node <n>` |
| every id under `instance_settings` is one of that node's instances | `instance_settings.<node>: <id> is not an instance of <node>` |
| every instance id matches `[A-Za-z0-9_-]+` and is unique within its node | the offending id is named |
| every module named by a wire, on either side, is a node | `wires.<publisher key>: unknown node <n>` |
| a wire has at least one subscriber | `wires.<publisher key>: no subscriber on the right side` |
| a selector sits on the publisher (left) side only | ``wires.<publisher key>: subscriber "<s>" takes no selector; selectors go on the publisher (left) side`` |
| a selector is `[{id}]`, `[*]`, `[<id>]`, or absent | `wires.<publisher key>: <reason>` |
| `[{id}]` requires the subscriber node to have instances | `wires.<publisher key>: {id} but <subscriber> has no instances` |
| a key appears once per map (YAML) | `<path>: duplicate entry with key "<k>"` |

A failure stops `sb install` before anything is written, and stops
`sb up` and `sb run` before anything is built.

Whether each node is a module of `sb.workspace.yaml` is a workspace
question; `sb ws check` reports it (§8.2), and `sb up` stops on it
before building anything.

### 8.2 `ws flows` in `sb ws check` and `sb doctor`

The `ws flows` row (sb 0.8.0+, replacing `ws instances` and
`ws wires`) gives one line per installed flow, the default flow first:

```
[OK]   ws flows     default: 4 node(s), 8 instance(s), 3 wire(s) resolve
                    calib: 4 node(s), 8 instance(s), 2 wire(s) resolve
```

`<i> instance(s)` counts the ids under `instances:`; a singleton
contributes none. A problem turns the row into `[FAIL]` with one line
per problem, each its own `[FAIL] ws flows` row,
`<flow>: <wire or item>: <reason>`. A problem at the publisher end
names the wire by its key; a problem at one subscriber of the wire
names it as `<key> -> <module>.<entry>`; an unknown node has the item
`nodes`:

```
[FAIL] ws flows     calib: ips_detect[*].debug_image: [*] on iceoryx2 is not supported
[FAIL] ws flows     calib: ips_detect[*].markers -> ips_calib.markers: no subscriber markers in ips_calib
```

A workspace with no flow at all gets
``[WARN] ws flows     no flows installed; `sb up` needs one (add sb.flow.yaml beside sb.workspace.yaml)``.

| Reason | Meaning |
|---|---|
| `unknown node <n>` | a node of the flow is not a module of `sb.workspace.yaml` |
| `no publisher <entry> in <module>` | the wire's key names a publisher entry the module does not have |
| `no subscriber <entry> in <module>` | the wire names a subscriber entry the module does not have |
| `[*] on iceoryx2 is not supported` | fan-in from every instance needs Zenoh |
| `{id} but <module> has no instances` | `[{id}]` on a wire where that node has no instances in this flow |
| `no instance <id> in <module>` | `[<id>]`, or an id `[{id}]` pairs, names no instance of the publisher |
| `transport mismatch (<a> vs <b>)` | publisher and subscriber entries use different transports |
| `type mismatch (<a> vs <b>)` | publisher and subscriber entries use different message types |

A `[FAIL]` here fails `sb doctor` and `sb ws check`. The other rows of
those commands are in [sbcli_doctor.md](sbcli_doctor.md) §4.11.

---

## 9. Worked example: `ws_ips` with two flows

Four cameras, four ArUco detectors, a tracker, a visualiser, and a
calibrator that runs only during calibration. The inventory is
[sbcli_workspace.md](sbcli_workspace.md) §2; the module manifests are
[sbcli_module.md](sbcli_module.md) §10. The default flow:

```yaml
# sb.flow.yaml
nodes: [ips_cam_gcam_pub, ips_detect, ips_track, ips_viz]
instances:
  ips_cam_gcam_pub: [sb00, sb01, sb02, sb03]
  ips_detect: [sb00, sb01, sb02, sb03]
instance_settings:
  ips_detect:
    sb02:
      label: East wall
      args: --debug --fps 30
      env:
        RUST_LOG: debug
wires:
  ips_cam_gcam_pub[{id}].image: ips_detect.image
  ips_detect[*].markers: ips_track.markers
  ips_track.cube_poses: ips_viz.cube_poses
```

and the calibration flow of §2, `calib.flow.yaml`. Day to day:

```console
$ sb up
building ips_cam_gcam_pub: cargo build --release
building ips_detect: cargo build --release
building ips_track: cargo build --release
building ips_viz: cargo build --release
flow default: launched 10 window(s) in tmux session "sb-ws_ips"
  - ips_cam_gcam_pub-sb00
  - ips_cam_gcam_pub-sb01
  - ips_cam_gcam_pub-sb02
  - ips_cam_gcam_pub-sb03
  - ips_detect-sb00
  - ips_detect-sb01
  - ips_detect-sb02
  - ips_detect-sb03
  - ips_track
  - ips_viz
attach with: tmux attach -t sb-ws_ips

$ sb run ips_calib
error: module ips_calib is not a node of flow default

$ sb down && sb up calib
killed tmux session "sb-ws_ips"
building ips_cam_gcam_pub: cargo build --release
building ips_detect: cargo build --release
building ips_track: cargo build --release
building ips_calib: cargo build --release
flow calib: launched 10 window(s) in tmux session "sb-ws_ips"
  - ips_cam_gcam_pub-sb00
  ...
  - ips_track
  - ips_calib
attach with: tmux attach -t sb-ws_ips
```

(Cargo's own output appears between the `building` lines.) The
`ips_detect-sb02` window starts with `--debug --fps 30` before its
`--id sb02`, and with `RUST_LOG=debug` in its environment.

---

## 10. Error catalogue

| Cause | Message |
|---|---|
| `sb up <flow>` names no installed flow | `no flow named <f> in workspace <ws> (flows: <a>, <b>)`, or `(flows: none)` |
| `sb up` on a flow whose `nodes` is empty | `flow <f> has no nodes` |
| `sb run` on a module that is not a node of the active flow | `module <m> is not a node of flow <f>` |
| a flow's `name:` differs from its file name | `sb install` refuses the flow |
| a rule of §8.1 broken | the error in the §8.1 table |
| a malformed wire | `wires.<publisher key>: <reason>` |
| a typo in a key | `unknown field` from the YAML parser, naming the key |
| a wire key listed twice | `<path>: duplicate entry with key "<k>"` |
| a flow problem only the manifests reveal | a `[FAIL] ws flows` line (§8.2) |
| a flow file in the 0.7 shape beside a `sb.workspace.yaml` | `<path> is a flow in the 0.7 shape (top-level modules:), which is read only when no sb.workspace.yaml sits beside it; move modules: and messages: into sb.workspace.yaml and keep nodes, instances and wires in the flow (see sbcli_workspace.md §Migrating)` |
