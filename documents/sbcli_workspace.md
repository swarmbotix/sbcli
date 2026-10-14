# `sb.workspace.yaml`: Workspace Inventory Reference

Official reference for `sb.workspace.yaml`, the file at the root of a
workspace repository that lists what the workspace holds: its modules
and its message definitions. Which modules run, how many times, and
wired to what, is the job of the flow files beside it
([sbcli_flow.md](sbcli_flow.md)).

This document is normative for behavior shipped in `sb 0.8.0+`.

The canonical spec is `requirements.md` in the sbcli **source
repository**, a maintainer document that is not part of your install;
when it and this page disagree, it wins.

Related: [sbcli_flow.md](sbcli_flow.md) (the flow files: nodes,
instances, wires), [sbcli_module.md](sbcli_module.md) (one module's
`sb.module.yaml`), [sbcli_ws.md](sbcli_ws.md) (the `sb ws` commands and
`sb install`), [sbcli_init.md](sbcli_init.md) (how a module is
registered), [sbcli_launch.md](sbcli_launch.md) (`sb up [FLOW]`).

**Added in sb 0.8.0.** Before 0.8.0 one `sb.flow.yaml` held both the
inventory and the launch diagram. §7 splits a 0.7 file in two. `sb 0.8.0`
still reads the 0.7 shape, with a deprecation warning; `sb 0.8.1` will
not.

---

## 1. Three file kinds

`sb 0.8.0+` reads three kinds of committed file:

| File | Where | One per | Holds | Reference |
|---|---|---|---|---|
| `sb.module.yaml` | each module root | module | identity, build and start, environment, platforms, ports | [sbcli_module.md](sbcli_module.md) |
| `sb.workspace.yaml` | the repository root | workspace | the modules the workspace holds and the message definitions it imports | this document |
| `sb.flow.yaml`, `<name>.flow.yaml` | the repository root | way to run the workspace | which modules run (nodes), their instances, per-instance settings, wires | [sbcli_flow.md](sbcli_flow.md) |

The split follows what changes together. A module joins a workspace
once and stays: that is inventory. Which modules run together, how many
cameras today, which detector feeds which tracker, changes from one
session to the next and can have several answers at once (a runtime
setup, a calibration setup): that is a diagram, and a workspace may
hold several.

```
ws_ips/                                  a workspace repository (committed)
├── sb.workspace.yaml                    inventory: modules and messages
├── sb.flow.yaml                         the flow named "default"
├── calib.flow.yaml                      the flow named "calib" (optional; any number)
├── messages/custom/*.proto              imported through messages:
└── modules/
    ├── ips_cam_gcam_pub/sb.module.yaml  one manifest per module
    ├── ips_detect/sb.module.yaml
    └── ...
```

A module listed in `sb.workspace.yaml` is **registered**: `sb install`
regenerates its io, `sb io --module <name>` finds it, and `sb ws check`
checks it. It **runs** only when a flow lists it under `nodes:`
([sbcli_flow.md](sbcli_flow.md) §3).

---

## 2. Schema

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

| Key | Required | Meaning |
|---|---|---|
| `name` | no | Workspace name. Default: the name of the directory holding the file. It follows the identifier rule ([sbcli_ws.md](sbcli_ws.md) §1.2), so a folder such as `ws-ips` needs `name: ws_ips`. It is the `<workspace>` segment of every owned wire key and the `sb-<workspace>` tmux session name. |
| `sb` | no | The `sb` version the workspace needs, such as `">=0.8"`. Free text; not enforced. |
| `modules` | no | Module name to module location (§2.1). |
| `messages` | no | Message definitions to import into the vault, keyed by a label (§2.2). |

Load rules:

- `deny_unknown_fields` applies to the file and to each `messages`
  entry, so a typo such as `modlues:` is reported by name. An `sb` older
  than 0.8.0 does not look for this file at all.
- A `modules:` or `messages:` key with nothing under it reads as empty,
  not as an error.
- A name listed twice under `modules:` or `messages:` is refused as
  `<path>: duplicate entry with key "<k>"`.
- `source:` belongs to the installed copy only (§6); do not write it in
  the repository file.
- Nothing in this file says what runs. Flows name modules from it, by
  name ([sbcli_flow.md](sbcli_flow.md)).

### 2.1 `modules`

Each value is a path, and only a path: relative to the directory that
holds `sb.workspace.yaml`, naming either the module directory (its
`sb.module.yaml` is looked up in it) or the manifest file itself. A path
that does not exist yet is taken as a directory unless it ends in
`.yml` or `.yaml`, so a module can be listed before `sb init` has run in
it.

- Module names follow the identifier rule ([sbcli_ws.md](sbcli_ws.md)
  §1.2). A name listed twice is refused (a duplicate key, §2).
- `sb init` writes `/`-separated paths, so the file reads the same on
  every OS.
- A path may climb out of the repository (`../shared/module`), and an
  absolute path is used as it is, but either ties the repository to one
  machine's layout. Keep modules inside the repository.

The 0.7 map form of a module entry (`path`, `up`, `instances`) is gone:
whether a module runs, and how many times, is said by a flow (§7).

### 2.2 `messages`

Each entry is keyed by a label and names a directory of `.proto` files
that `sb install` imports into the vault and then compiles:

| Field | Required | Meaning |
|---|---|---|
| `style` | yes | The vault style to import into, `[A-Za-z0-9_]+`. A new name creates the style. |
| `path` | yes | Directory of `.proto` files, relative to `sb.workspace.yaml`. |
| `backends` | no | Any of `proto`, `fb`, `iox2`. Absent: the normal backend selection. |

The label only names the entry; it is neither the style nor a path, and
any identifier will do (`custom` above). An inner `label:` is accepted
only when it equals the key (otherwise
`messages.<label>: label <x> does not match the key`). The 0.7 list form of `messages:` is still read,
without a warning: each entry's label is then the last component of its
`path`, with `_2`, `_3`, ... appended when two entries would get the
same label. The import and compile rules, conflicts and `--force`
included, are in [sbcli_messages.md](sbcli_messages.md) §4.7.

---

## 3. `sb install` with a workspace

`sb install <git-url | path>` (and `sb ws install`) installs a
workspace when the root of the source holds `sb.workspace.yaml`
([sbcli_ws.md](sbcli_ws.md) §5.2 has every step). For the files on
this page:

- `sb.workspace.yaml` is read and validated, and the installed copy is
  written from it (§6).
- Every `*.flow.yaml` at the repository root is validated and copied
  into the installed copy; a flow whose `name:` does not match its file
  name is refused ([sbcli_flow.md](sbcli_flow.md) §2). **No flow is
  required**: a workspace without one installs (`-- flows: none`), and
  `sb up` then has nothing to launch.
- A root that holds only a `sb.flow.yaml` in the 0.7 shape installs as
  a workspace too, converted as §7.7 describes, with a deprecation
  warning on stderr.

The install prints one line naming the flows it installed, after the
modules line:

```
-- workspace ws_ips: created, active
-- modules: 5 registered
-- flows: default, calib
```

After editing either kind of file by hand, run `sb install .` so the
installed copy follows; `sb up` and `sb run` read only the installed
copy.

---

## 4. `sb ws create`

```bash
cd ~/ws_ips
sb ws create ws_ips
```

`sb 0.8.0+` writes two files into the current directory and installs
them: `sb.workspace.yaml` with `name: ws_ips` and an empty `modules:`
(no `messages:`), and `sb.flow.yaml` with `nodes: []`. It prints

```
wrote sb.workspace.yaml and sb.flow.yaml
```

and then the install lines of §3. The empty default flow is valid, but
`sb up` refuses it (`flow default has no nodes`) until a module is
listed under `nodes:`. Refusals and the cleanup on a failed install are
in [sbcli_ws.md](sbcli_ws.md) §5.1.

---

## 5. `sb init` registration

`sb init` registers a module in a workspace file, never in a flow
([sbcli_init.md](sbcli_init.md) §1.2):

- **Under a repository**: the nearest `sb.workspace.yaml` at or above
  the module root gets `<module>: <path relative to sb.workspace.yaml>`,
  appended as one line under `modules:` so comments and layout survive.
  The repository is then reinstalled, so the installed copy follows.
- **Without one**: the module goes into the active workspace's
  installed `sb.workspace.yaml`, with an absolute path. Nothing is
  reinstalled, and nothing in a repository changes.

A newly registered module is not a node of any flow, so `sb up` does
not start it yet. Add it where it should run, then reinstall:

```bash
sb init --rust modules/ips_detect      # registers ips_detect in ./sb.workspace.yaml, reinstalls
# edit sb.flow.yaml:  nodes: [ips_cam_gcam_pub, ips_detect]
sb install .                           # the installed flow follows the edit
sb up                                  # builds and starts the nodes of the default flow
```

A module registered without a repository exists only in the installed
`sb.workspace.yaml`, and a reinstall from the repository would drop it
again. For such a workspace (one made with `sb ws create` in a scratch
folder, say, and filled with `sb init` calls on modules elsewhere), the
installed default flow is where its nodes go:

```bash
cat > ~/.swarmbotix/workspaces/demo/flows/sb.flow.yaml <<'EOT'
nodes: [pubber, subber]
EOT
sb up
```

Listing the module paths in the repository's `sb.workspace.yaml`
instead (§2.1) keeps everything in files you commit, and is the better
choice for anything you keep.

---

## 6. The installed layout

`sb install` derives an installed copy under `<sb_home>` (`~/.swarmbotix`
unless configured otherwise, [sbcli_ws.md](sbcli_ws.md) §2). Every
other verb reads it, never the repository files directly:

```
<sb_home>/workspaces/ws_ips/
├── sb.workspace.yaml        installed copy: absolute paths plus a source: block
├── flows/
│   ├── sb.flow.yaml         the default flow
│   └── calib.flow.yaml      every other flow, under its own file name
├── active_flow              one line: the flow the last `sb up` launched
└── sb.config.yml            optional per-workspace config override
```

The installed `sb.workspace.yaml` has absolute paths, and every module
path names the module's `sb.module.yaml`:

```yaml
# <sb_home>/workspaces/ws_ips/sb.workspace.yaml, written by sb install
name: ws_ips
modules:
  ips_calib: /home/el/ws_ips/modules/ips_calib/sb.module.yaml
  ips_cam_gcam_pub: /home/el/ws_ips/modules/ips_cam_gcam_pub/sb.module.yaml
  ips_detect: /home/el/ws_ips/modules/ips_detect/sb.module.yaml
  ips_track: /home/el/ws_ips/modules/ips_track/sb.module.yaml
  ips_viz: /home/el/ws_ips/modules/ips_viz/sb.module.yaml
messages:
  custom:
    style: ros2
    path: /home/el/ws_ips/messages/custom
    backends: [proto, iox2]
source:
  repo_workspace: /home/el/ws_ips/sb.workspace.yaml
  url: https://github.com/acme/ws_ips.git
  installed_at: 2026-10-13T08:35:23Z
```

(`sb` writes lists in block style; the content is what matters.)

- `name:` is always present.
- `source.repo_workspace` is the repository file that was installed:
  `sb.workspace.yaml`, or the 0.7-shape `sb.flow.yaml` while that is
  still read (§7.7). `source.url` is the git URL it was cloned from
  (absent for a local path install); `source.installed_at` is the
  RFC 3339 UTC time of the install, to the second.
- `flows/` holds every flow of the repository, each under its
  repository file name (the default flow as `flows/sb.flow.yaml`). A
  flow carries no path, so its content is installed unchanged; `sb`
  writes it back with block-style lists.
- `active_flow` names the flow `sb up` launched last. `sb run` reads its
  instances from that flow, or from `default` when the file is absent
  ([sbcli_flow.md](sbcli_flow.md) §7.4).
- **Never edit the installed copy by hand** when the workspace has a
  repository: the next install overwrites it; edit the repository files
  and run `sb install .`. (The exception is a workspace registered
  without a repository, §5.)
- A workspace installed by `sb` 0.7 or earlier has the old layout and
  is refused with
  ``workspace "<ws>" was installed by an older sb (<path> holds its modules and wiring in one file); install it again from its repository with `sb install <repo folder>` ``.

---

## 7. Migrating from 0.7

`sb 0.8.0` reads a `sb.flow.yaml` in the 0.7 shape, and warns on
stderr. The 0.7 shape is recognised as a `sb.flow.yaml` that has no
`nodes:` but has any of `modules:`, `messages:`, or `source:` (its
`modules:` maps names to paths or to `{path, up, instances}` entries):

```
warning: sb.flow.yaml in the 0.7 shape is deprecated since 0.8.0 and will be removed in 0.8.1; split it into sb.workspace.yaml and sb.flow.yaml; see sbcli_workspace.md §Migrating
```

This section is where that hint points. `sb 0.8.1` will not read the
0.7 shape. There is no migration command: the split below is a few
minutes of editing, and none of it needs an older `sb`.

### 7.1 What moved where

| 0.7 `sb.flow.yaml` | `sb 0.8.0+` |
|---|---|
| `name` | `sb.workspace.yaml` `name` |
| `modules.<m>` as a path | `sb.workspace.yaml` `modules.<m>`, the same path |
| `modules.<m>.path` | `sb.workspace.yaml` `modules.<m>` |
| a module with `up` absent or `true` | listed in a flow's `nodes:` |
| `modules.<m>.up: false` | not a node of the default flow; put it in a flow of its own (§7.4) |
| `modules.<m>.instances`, the ids | the flow's `instances.<m>` |
| an instance's `label`, `args`, `env` | the flow's `instance_settings.<m>.<id>` |
| an instance's `docker:` | removed; the module's own scripts call `docker compose` (§7.6) |
| `messages:` (list) | `sb.workspace.yaml` `messages:`, keyed by a label; the list form is still read |
| `wires:`, subscriber first | the flow's `wires:`, **publisher first** (§7.3) |

`sb init --docker` is removed in 0.8.0 as well (§7.6).

### 7.2 Before and after

Before, one file:

```yaml
# sb.flow.yaml, 0.7 shape
name: ws_ips
modules:
  ips_cam_gcam_pub:
    path: modules/ips_cam_gcam_pub
    instances: [sb00, sb01, sb02, sb03]
  ips_detect:
    path: modules/ips_detect
    instances:
      - sb00
      - sb01
      - id: sb02
        label: East wall
        args: [--debug, --fps, "30"]
        env:
          RUST_LOG: debug
      - sb03
  ips_track: modules/ips_track
  ips_viz: modules/ips_viz
  ips_calib:
    path: modules/ips_calib
    up: false
messages:
  - style: ros2
    path: messages/custom
    backends: [proto, iox2]
wires:
  ips_detect.image: "ips_cam_gcam_pub[{id}].image"
  ips_track.markers: "ips_detect[*].markers"
  ips_calib.markers: "ips_detect[*].markers"
  ips_viz.cube_poses: ips_track.cube_poses
```

After, the inventory:

```yaml
# sb.workspace.yaml
name: ws_ips
modules:
  ips_cam_gcam_pub: modules/ips_cam_gcam_pub
  ips_detect: modules/ips_detect
  ips_track: modules/ips_track
  ips_viz: modules/ips_viz
  ips_calib: modules/ips_calib
messages:
  custom:
    style: ros2
    path: messages/custom
    backends: [proto, iox2]
```

the default flow, which is everything `sb up` used to start:

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

and a second flow for the module that was `up: false` (§7.4):

```yaml
# calib.flow.yaml
nodes: [ips_cam_gcam_pub, ips_detect, ips_track, ips_calib]
instances:
  ips_cam_gcam_pub: [sb00, sb01, sb02, sb03]
  ips_detect: [sb00, sb01, sb02, sb03]
wires:
  ips_cam_gcam_pub[{id}].image: ips_detect.image
  ips_detect[*].markers: [ips_track.markers, ips_calib.markers]
```

Then:

```bash
git add sb.workspace.yaml sb.flow.yaml calib.flow.yaml
sb install .
sb ws check          # ws flows: one OK line per flow
```

### 7.3 Wires flip to publisher first

A 0.7 wire was keyed by the subscriber; an 0.8 wire is keyed by the
publisher, and its value is one subscriber or a list of them. The
selector stays on the publisher side and keeps its meaning:

| 0.7 (subscriber: publisher) | `sb 0.8.0+` (publisher: subscriber) |
|---|---|
| `ips_detect.image: "ips_cam_gcam_pub[{id}].image"` | `ips_cam_gcam_pub[{id}].image: ips_detect.image` |
| `ips_viz.cube_poses: ips_track.cube_poses` | `ips_track.cube_poses: ips_viz.cube_poses` |
| `ips_track.markers: "ips_detect[*].markers"` and `ips_calib.markers: "ips_detect[*].markers"` | `ips_detect[*].markers: [ips_track.markers, ips_calib.markers]` |

Two 0.7 lines that named the same publisher entry with the same
selector become one 0.8 line with a list: a key may appear only once in
YAML. The quotes the 0.7 values needed are no longer necessary
([sbcli_flow.md](sbcli_flow.md) §6.4).

Every module a wire names must be a node of that flow. A wire to a
module that used to be `up: false` therefore moves to the flow that
runs it, as `ips_calib.markers` moved to `calib.flow.yaml` above. The
automatic conversion of §7.7 cannot do that for you: it drops such
wire ends.

### 7.4 `up: false` becomes "not a node"

`up: false` kept a registered module out of `sb up` while
`sb run <module>` could still start it. In `sb 0.8.0+` a module is in
the run or out of it per flow: it is registered by `sb.workspace.yaml`
and runs only where a flow lists it in `nodes:`.

- A tool started now and then, such as a calibrator, goes into a flow
  of its own (`calib.flow.yaml` above) together with the modules it
  needs: `sb down && sb up calib`.
- A Unity module, which launches from its editor, is simply left out of
  every flow's `nodes:`.
- `sb run <module>` refuses a module that is not a node of the active
  flow (`module ips_calib is not a node of flow default`), so the 0.7
  habit of `sb run ips_calib` beside a running `sb up` needs the module
  in that flow's `nodes:` instead.

### 7.5 Instances and their settings

Ids move to `instances:`, keyed by node, as a plain list. What an
instance map carried besides its id moves to `instance_settings:`,
keyed by node, then by id, where only the instances that have settings
appear. `args` may now be one string, split like a script command
([sbcli_module.md](sbcli_module.md) §3.1), so `[--debug, --fps, "30"]`
becomes `--debug --fps 30`; the list form is read too
([sbcli_flow.md](sbcli_flow.md) §5).

### 7.6 `docker:` and `sb init --docker` are removed

`sb 0.8.0` removes the instance `docker:` block and the
`sb init --docker` flag. `sb` never started containers itself: since 0.6.0 it
skipped a module whose instances declared `docker:`. Now a module that
runs in a container says so in its own manifest, and Docker Compose
does the rest:

- the module's `scripts:` call `docker compose`, from the module root,
  where its `compose.yaml` lives;
- the compose file reads `${SB_INSTANCE_ID}` (and any variable from the
  instance's `env`), which `sb` puts in the environment of every
  instance's window.

The full pattern, with the `--id` that `sb` appends and the teardown,
is [sbcli_module.md](sbcli_module.md) §14. What a 0.7 `docker:` block
held maps like this:

| 0.7 `docker:` field | With compose |
|---|---|
| `image` | `image:` (and `build:`) of the compose service |
| `env` | `environment:` of the service, values from `${...}` set in the instance's `env` |
| `devices`, `mounts`, `network` | `devices:`, `volumes:`, `network_mode:` of the service |
| `gpus` | the service's device reservations (`deploy.resources.reservations.devices`) |

`sb init --docker` is now an unknown flag, so clap rejects it before
`sb` does anything. Run `sb init` without it.

### 7.7 What `sb 0.8.0` does with a 0.7 file

While the 0.7 shape is still read (0.8.0 only), `sb install`, reading
the repository's `sb.flow.yaml`, converts it in memory into a workspace
and the default flow:

1. `modules` and `messages` go into the workspace, paths unchanged.
2. `nodes` are the modules whose `up` is not `false`.
3. Inline instance ids go into `instances`; each instance's `args`,
   `env`, and `label` go into `instance_settings`.
4. `docker:` blocks are dropped.
5. `wires` are flipped to publisher first, and wire ends on modules
   that were `up: false` are dropped (they are not nodes).

A 0.7-shape file that does not parse stops the install with
`sb.flow.yaml in the 0.7 shape does not parse: <error>`.

The installed copy is written in the 0.8 layout (§6), with
`source.repo_workspace` naming the 0.7 file, and the warning above is
printed once. `sb ws check` and `sb doctor` list the file in their
`deprecated` row until it is split
([sbcli_doctor.md](sbcli_doctor.md) §4.11):

```
[WARN] deprecated   sb.flow.yaml at <path> is in the 0.7 shape, which 0.8.1 no longer reads (split it into sb.workspace.yaml and sb.flow.yaml; see sbcli_workspace.md §Migrating)
```

The 0.7 shape is read only while no `sb.workspace.yaml` sits beside it.
Once you add one, a flow file still in the 0.7 shape is refused with
`<path> is a flow in the 0.7 shape (top-level modules:), which is read only when no sb.workspace.yaml sits beside it; move modules: and messages: into sb.workspace.yaml and keep nodes, instances and wires in the flow (see sbcli_workspace.md §Migrating)`,
so split the whole file in one go (§7.8). The repository file itself
is never rewritten: the split is yours to commit.

The 0.7 list forms inside a module manifest (`publishers:` /
`subscribers:` as `- name:` lists, `services:` as a list of maps) and
the 0.7 list form of `messages:` are a different matter: they are read
without a warning and are not scheduled for removal
([sbcli_module.md](sbcli_module.md) §11.10).

### 7.8 Checklist

1. Write `sb.workspace.yaml` from `name`, the module paths, and
   `messages` (§7.2).
2. Rewrite `sb.flow.yaml` as `nodes`, `instances`, `instance_settings`,
   and publisher-first `wires`, leaving out the `up: false` modules.
3. Give each `up: false` module that you still launch a flow of its own
   (§7.4).
4. Move each `docker:` block into a `compose.yaml` and the module's
   `scripts:` (§7.6); drop `--docker` from any `sb init` in your
   scripts.
5. `sb install .`, then `sb ws check` until `ws flows` passes and the
   `deprecated` row reads `nothing deprecated in use`.

---

## 8. Errors

The `sb ws` and `sb install` errors, including the ones that name
`sb.workspace.yaml`, are catalogued in [sbcli_ws.md](sbcli_ws.md) §8;
the flow errors in [sbcli_flow.md](sbcli_flow.md) §10. The ones most
often met while writing this file:

| Cause | Message |
|---|---|
| a typo in a key | `unknown field` from the YAML parser, naming the key and listing the expected ones |
| a module name or a `messages` label listed twice | `<path>: duplicate entry with key "<k>"` |
| an inner `label:` that differs from the key | `messages.<label>: label <x> does not match the key` |
| a 0.7-shape `sb.flow.yaml` that does not parse | `sb.flow.yaml in the 0.7 shape does not parse: <error>` |
| a workspace installed by sb 0.7 or earlier | ``workspace "<ws>" was installed by an older sb (<path> holds its modules and wiring in one file); install it again from its repository with `sb install <repo folder>` `` |
| `name:` not an identifier | `workspace name "<n>" is invalid: <reason>` |
| an empty module path | `module "<m>" has an empty path` |
| a bad `messages` entry | the entry, then `style "<s>" is invalid (allowed: [A-Za-z0-9_])`, `path is empty`, or `unknown backend "<b>" (expected one of: proto, fb, iox2)` |
