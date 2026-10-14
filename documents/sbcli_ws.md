# `sb ws`: Workspace Management Reference

Official reference for workspaces: the repository `sb.flow.yaml` that
defines one, the installed copy `sb` derives from it, the active
marker, the `sb ws` subcommands, and the resolution rules that
downstream verbs (`sb init`, `sb io`, `sb up`) consult.

This document is normative for behavior shipped at L2, extended by the
repository-flow model of `sb 0.5.0+` (§4 onward).

The canonical spec is `requirements.md` in the sbcli **source
repository**, a maintainer document that is not part of your install;
when it and this page disagree, it wins.

Related: [sbcli_init.md](sbcli_init.md) (how a module joins a
workspace), [sbcli_app.md](sbcli_app.md) (`sb install` also installs
app packages), [sbcli_launch.md](sbcli_launch.md) (what `sb up` does
with the registry), [sbcli_pubsub.md](sbcli_pubsub.md) §14 (`sb io`).

---

## 1. Conceptual model

A **workspace** is an isolated project scope, analogous to a Python
virtual environment: a named set of modules that `sb up` launches
together and whose topics share the `<workspace>` segment of every wire
key.

`sb 0.5.0+`: a workspace is defined by an **`sb.flow.yaml` at the root
of a repository**, committed with the code (the `sb 0.6.0+` name; since
0.7.0 the pre-0.6.0 name is no longer read, §4.8). That file is the
source of truth. `sb install` (or `sb ws install`) derives an **installed copy**
from it at `<sb_home>/workspaces/<name>/sb.flow.yaml`, and every other
verb reads the installed copy.

| Copy | Location | Written by | Paths | Edit it? |
|---|---|---|---|---|
| repo copy | `<repo>/sb.flow.yaml` | you, `sb ws create`, `sb init` | relative to the file | yes; commit it |
| installed copy | `<sb_home>/workspaces/<name>/sb.flow.yaml` | `sb install` / `sb ws install` / `sb ws update` | absolute, plus a `source:` block | never; reinstall instead |

The split keeps the committed file free of host-specific paths, so one
repository installs on any machine, while the launch and codegen verbs
still read a single absolute registry without searching the filesystem.

`sb` records **which workspace is active** in a one-line marker file at
`<sb_home>/active`. Every downstream verb (`sb init`, `sb io`,
`sb up`, `sb topic *`) consults that marker to decide
which installed `sb.flow.yaml` to read. Installing a workspace makes it the
active one.

A workspace made before 0.5.0 (by the old `sb ws create`, which wrote
only the installed copy) keeps working, provided that copy has been
brought to the 0.6.0 form (§4.5; required since 0.7.0): it has no
`source:` block, a
module adopted outside any repository flow is registered directly into
it, and the repository commands (`sb ws update`, the stale check) do
not apply to it.

Throughout this doc, `<sb_home>` refers to the resolved per-user
swarmbotix root (§2).

### 1.1 What a workspace consists of

```
<repo>/                              # your repository: the source of truth
├─ sb.flow.yaml                      # repo copy, see §4.1
├─ modules/<module>/sb.module.yaml   # one per module, wherever sb.flow.yaml points
└─ messages/<dir>/*.proto            # optional; imported through `messages:`

<sb_home>/workspaces/<name>/
├─ sb.flow.yaml                      # installed copy, see §4.3
└─ sb.config.yml                     # OPTIONAL per-workspace config override (L1 stub; wired by `sb config --workspace` at L5)

<sb_home>/repos/<name>/              # the clone, when `sb install <git-url>` used the default location (§5.2)
```

Workspaces do **not** own the modules they register. A module's source
tree lives in the repository (or wherever the user keeps it); the
installed copy records the absolute path of that tree's `sb.module.yaml`.

### 1.2 Workspace identifiers

Workspace names use the same identifier rule as module names (enforced
by `sb_core::validate_identifier`):

- Non-empty.
- First character: ASCII letter or `_`.
- Remaining characters: ASCII letters, digits, or `_`.

This is stricter than the prose hint in requirements.md ("no spaces or
slashes") on purpose, so names like `demo-app`, `123demo`, or `my ws`
are rejected up front rather than producing surprises in
filesystem-path or YAML-key contexts.

Valid: `demo`, `robot1`, `_scratch`, `nav_stack2`.
Invalid (rejected at parse time): `demo-app`, `123demo`, `my ws`,
`nav/stack`, `` (empty).

The rule applies to the name a repository flow installs under: its
`name:`, or the repository folder name when `name:` is absent. A
repository folder such as `ws-ips` therefore needs an explicit
`name: ws_ips` (sb 0.5.0+).

---

## 2. `<sb_home>` resolution

`<sb_home>` is the per-user swarmbotix tree that holds the active
marker, every workspace's directory, and the global `sb.config.yml`.
It resolves in this order (highest priority first):

1. `$SB_CONFIG` env override, then its `sb_home_dir` field (tilde-expanded).
2. Active workspace's `sb.config.yml`, then its `sb_home_dir` field.
3. `~/.swarmbotix/sb.config.yml`, then its `sb_home_dir` field.
4. Built-in fallback: `~/.swarmbotix/` (via `dirs::home_dir()`).

The resolver is `sb_config::resolve_sb_home_dir` and is shared with
every other verb that needs to read `<sb_home>`. `$SB_CONFIG` is the
only override the test suite uses to pin a tempdir as the root.

The parts of `<sb_home>/` that concern workspaces:

```
<sb_home>/
├─ sb.config.yml          # global config (optional, see sbcli_config.md)
├─ active                 # one-line marker file (see §3)
├─ workspaces/            # one subdir per workspace (see §1.1)
│  └─ <name>/
│     ├─ sb.flow.yaml     # installed copy
│     └─ sb.config.yml    # optional per-workspace override
├─ repos/                 # sb 0.5.0+: clones `sb install <git-url>` makes (§5.2)
│  └─ <name>/
└─ messages/              # message styles (see sbcli_messages.md)
   └─ <style>/
      ├─ message_definitions/   # .proto vault
      └─ message_targets/       # codegen output root
```

`workspaces/` is auto-created by the first install, and `repos/` by the
first install from a git URL. The `messages/` style tree is managed by
`sb message *`; workspaces touch it only when `sb install` imports a
repository's `messages:` (§5.2).

---

## 3. The active marker: `<sb_home>/active`

A single-line file holding the active workspace's name (no trailing
slash, no path, just the bare identifier). Examples:

```
demo
```

Or empty / missing (no workspace active):

```
(file absent)
```

or

```
(empty)
```

`sb_workspace::active_workspace(home)` returns `Ok(None)` when the
file is absent **or** empty (whitespace-only). This is a valid
non-erroneous state; it only means no verb can resolve a workspace
until you run `sb ws set <name>` or install one.

The file is written by `sb ws set <name>` and, `sb 0.5.0+`, by every
install (`sb install`, `sb ws install`, `sb ws create`, `sb ws update`,
and the reinstall `sb init` performs under a repository flow). It is
cleared (overwritten with empty content, never deleted) by
`sb ws delete` / `sb ws uninstall` of the active workspace.

---

## 4. `sb.flow.yaml` schema

### 4.1 The repo copy (sb 0.5.0+)

Lives at the root of your repository and is committed with it. Written
by hand, by `sb ws create` (§5.1), and by `sb init` (§7.2); never by an
install. `sb 0.6.0+` names it `sb.flow.yaml`, and `sb 0.7.0+` reads no
other name (§4.8).

```yaml
name: ws_ips                       # optional; default = the repo folder name
modules:
  ips_track: modules/ips_track                      # short form: path relative to this file, a module dir or its manifest
  ips_calib: { path: modules/ips_calib, up: false } # registered, rendered, but skipped by sb up
  ips_detect:                                       # long form with instances (sb 0.6.0+, §4.6)
    path: modules/ips_detect
    instances: [sb00, sb01, { id: sb02, args: [--debug] }]
messages:
  - { style: ros2, path: messages/custom, backends: [proto, iox2] }   # imported into the vault and compiled by sb install; backends optional
wires:                                              # sb 0.6.0+, §4.7
  ips_track.markers: "ips_detect[*].markers"
```

| Key | Required | Meaning |
|---|---|---|
| `name` | no | Workspace name. Default: the name of the directory holding this `sb.flow.yaml`. Must satisfy §1.2. |
| `modules` | no | Module name to module location. The value is a path relative to this file, naming either the module directory (its `sb.module.yaml` is looked up in it) or the manifest file itself. A path that does not exist yet is taken as a directory unless it ends in `.yml` / `.yaml`, so a module can be listed before `sb init` has run in it. The map form `{ path, up, instances }` adds `up: false` (§4.4) and, `sb 0.6.0+`, the module's instances (§4.6); `up` defaults to `true`. Module names follow §1.2 and are unique within the workspace. |
| `messages` | no | List of `{ style, path, backends }`. Each `path` (relative to this file) is a directory of `.proto` files that `sb install` imports into the vault under `style` and then compiles. `backends` is optional, any of `proto`, `fb`, `iox2`. See [sbcli_messages.md](sbcli_messages.md) §4.7. |
| `wires` | no | `sb 0.6.0+`. Which publisher feeds which subscriber: `<module>.<entry>: <module>[<selector>].<entry>` (§4.7). Validated by `sb ws check`; not yet read by code generation. |
| `instances` | not accepted | `sb 0.7.0+` refuses the pre-0.6.0 top-level form, a map of module name to instances (§4.8). Write `instances` under the module entry (§4.6). |

Load rules:

- `deny_unknown_fields` applies to the file, to the map form of a
  module entry, to each instance map, and to each `messages:` entry, so
  a typo such as `upp: false` is reported by name. It also means an `sb`
  older than the release that introduced a key cannot read a file that
  uses it: 0.5.0 for `name`, `messages`, and the `{ path, up }` form;
  0.6.0 for `instances` under a module entry and `wires`.
- `source:` belongs to the installed copy only (§4.3); do not write it
  in a repo copy.
- The validation errors (bad `name:`, empty module path, bad `messages:`
  entry, duplicate instance id, a top-level `instances:` map, a
  malformed wire) are listed in §8. They fail the install before anything is
  written.

`sb.flow.yaml` stores modules, their locations, their instances, and,
`sb 0.6.0+`, optionally the edges between them as `wires:`. Without
`wires:`, edges (which publisher feeds which subscriber) are
**derived** by matching wire keys across the modules' own manifests, as
before; with them, `sb ws check` confirms that each declared edge
resolves (§4.7). This is load-bearing for L6's swarmctl UI.

### 4.2 Paths

Every relative path in a repo copy resolves against the directory that
holds the `sb.flow.yaml`. `sb init` writes `/`-separated paths, so a
committed file reads the same on every OS. A path may climb out of the
repository (`../shared/module`), but then the repository no longer
installs on its own; keep modules inside it.

### 4.3 The installed copy

Derived by every install from the repo copy and written in full each
time:

```yaml
# <sb_home>/workspaces/ws_ips/sb.flow.yaml, written by sb install
name: ws_ips
modules:
  ips_calib:
    path: /home/el/ws_ips/modules/ips_calib/sb.module.yaml
    up: false
  ips_detect:
    path: /home/el/ws_ips/modules/ips_detect/sb.module.yaml
    instances:
    - sb00
    - sb01
    - id: sb02
      args:
      - --debug
  ips_track: /home/el/ws_ips/modules/ips_track/sb.module.yaml
messages:
- style: ros2
  path: /home/el/ws_ips/messages/custom
  backends:
  - proto
  - iox2
wires:
  ips_track.markers: ips_detect[*].markers
source:
  repo_flow: /home/el/ws_ips/sb.flow.yaml
  url: https://github.com/acme/ws_ips.git
  installed_at: 2026-10-13T08:35:23Z
```

- `name:` is always present.
- Every path is absolute (canonical when it exists), and every module
  path names the module's `sb.module.yaml`.
- `sb 0.6.0+`: `instances` and `wires` are copied as written.
- `source.repo_flow` is the repo copy that was installed;
  `source.url` is the git URL it was cloned from (absent for a local
  path install); `source.installed_at` is the RFC 3339 UTC time of the
  install, to the second.
- **Never edit it by hand.** The next install overwrites it, and the
  stale check (§5.3, [sbcli_launch.md](sbcli_launch.md) §2) compares
  its modification time with the repo copy's.
- An installed copy is refused as an install source (§8).

### 4.4 `up: false`

A module with `up: false` is registered, its io is regenerated by
`sb install` / `sb io update`, and `sb run <module>` / `sb stop
<module>` work on it. Only the whole-workspace `sb up` skips it,
neither building nor starting it. The typical cases are a tool started
by hand, such as a calibration module, and a Unity module, which has no
start command. See [sbcli_launch.md](sbcli_launch.md) §2.

### 4.5 Pre-0.5.0 installed copies

An installed copy written before 0.5.0 holds only `modules:` (absolute
paths) and optionally a top-level `instances:` map. A bare path is a
module with `up: true`, and a missing `source:` marks a workspace that
was not installed from a repository. `sb 0.7.0+` refuses such a copy
when it has the top-level map or is still stored under its pre-0.6.0
file name (§4.8). There is no repository to reinstall from, so edit the
file by hand as [sbcli_module.md](sbcli_module.md) §11 describes:
rename it, and move each instance list under its module entry. A module
path that still names a pre-0.6.0 manifest is read as the
`sb.module.yaml` in the same directory, so renaming the manifests
themselves (§11.1 there) is enough.

### 4.6 Instances (sb 0.6.0+)

An instance is one process of a module: the same code, its own id, its
own tmux window, its own wire paths. Instances sit under the module
entry:

```yaml
modules:
  ips_cam_gcam_pub: { path: modules/ips_cam_gcam_pub, instances: [sb00, sb01, sb02, sb03] }
  ips_detect:
    path: modules/ips_detect
    instances:
      - { id: sb00 }
      - { id: sb01 }
      - { id: sb02, args: [--debug] }
      - { id: sb03, env: { RUST_LOG: debug } }
```

An instance is a bare id (`sb00`, or a number such as `1`, read as the
string `"1"`) or a map:

| Field | Required | Meaning |
|---|---|---|
| `id` | yes | Matches `[A-Za-z0-9_-]+` and is unique within the module. It becomes the window name `<module>-<id>`, the `--id <id>` appended to the start command, `SB_INSTANCE_ID`, and, through the generated `TopicOverrides`, the `<module>-<id>` segment of the instance's wire keys ([sbcli_pubsub_consuming.md](sbcli_pubsub_consuming.md) §10). |
| `label` | no | Display string for the L6 graph editor; not used at runtime. |
| `args` | no | Extra arguments for this instance, placed where the start command has `{args}`, before any passthrough args of `sb run` ([sbcli_module.md](sbcli_module.md) §9). |
| `env` | no | Environment variables for this instance, over the manifest's `env:` ([sbcli_module.md](sbcli_module.md) §8). |
| `docker` | no | Container settings `{ image, env, devices, mounts, gpus, network }`. Validated, and checked by `sb init --docker`. `sb` does not start containers yet: `sb up` skips a module whose instances declare `docker:`, and `sb run` refuses it ([sbcli_module.md](sbcli_module.md) §11.6). |

A module without instances runs as one process, in a window named after
the module, with no `--id`. `sb up` and `sb run <module>` start every
instance; `sb run <module>:<id>` starts one
([sbcli_launch.md](sbcli_launch.md) §3).

`sb` writes the bare form back whenever an instance has only an `id`,
and the short module form `<name>: <path>` only when the module has
`up: true` and no instances.

### 4.7 Wires (sb 0.6.0+)

`wires:` declares which publisher feeds which subscriber, one line per
subscriber entry:

```yaml
wires:                                           # subscriber <- publisher
  ips_detect.image:   "ips_cam_gcam_pub[{id}].image"   # same instance id
  ips_track.markers:  "ips_detect[*].markers"          # every instance (zenoh only)
  ips_viz.cube_poses: ips_track.cube_poses             # singleton
```

The key is `<module>.<entry>`, a subscriber entry of a module in this
workspace. The value is `<module>[<selector>].<entry>`, a publisher
entry, where the selector picks the publisher's instances:

| Selector | Meaning |
|---|---|
| none (`ips_track.cube_poses`) | the publisher module has no instances |
| `[{id}]` | the publisher instance with the same id as the subscriber instance |
| `[*]` | every instance of the publisher; Zenoh only |
| `[<id>]`, such as `[sb02]` | that one instance |

Values containing `[` or `{` must be quoted, as YAML requires.

**What `sb` does with them today** (0.6.0 and 0.7.0). `sb install` copies them into the
installed copy, a malformed one fails the install (§8), and
`sb ws check` / `sb doctor` validate each against the modules'
manifests in the `ws wires` row (§5.3): the subscriber and publisher
modules and entries must exist, transports and types must agree, `[*]`
is refused on iceoryx2, and `{id}` needs a publisher with instances.
**Code generation does not read them yet.** Until a later release
generates subscriber keys from `wires:`, subscribers still pair through
`key:` entries ([sbcli_pubsub.md](sbcli_pubsub.md) §10.4) or the
`TopicOverrides` a module's `main()` builds. Declaring wires now costs
nothing and documents the graph.

### 4.8 Legacy names and top-level `instances:` (removed in 0.7.0)

Not read since 0.7.0: the pre-0.6.0 flow file name and a top-level
`instances:` map are refused with an error that names the fix (§8);
[sbcli_module.md](sbcli_module.md) §11.2 and §11.5 migrate them.

---

## 5. Subcommands

```
sb install <git-url | path> [--dir <path>] [--force]        # workspace or app, by manifest (§5.2)
sb ws create <name>                                          # write ./sb.flow.yaml, then install it
sb ws install [<git-url | path>] [--dir <path>] [--force]    # workspace-only form of sb install
sb ws check [<name>]                                         # validate a workspace (default: the active one)
sb ws update <name> [--force]                                # git pull --ff-only, then reinstall
sb ws uninstall <name> [--clean]                             # remove the installed copy
sb ws set <name>
sb ws list
sb ws delete <name>
```

Every subcommand resolves `<sb_home>` once at startup (§2). None of
them writes a module's own source files: an install regenerates the
generated io files (§5.2), and `sb ws uninstall --clean` deletes the
generated io directories and the clone `sb` made (§5.5).

### 5.1 `sb ws create <name>`

```bash
cd ~/ws_ips
sb ws create ws_ips
```

**Effects (in order)** (sb 0.5.0+):

1. Validates `<name>` against the identifier rule (§1.2).
2. Refuses when `./sb.flow.yaml` already exists
   (``<path> already exists; install it with `sb install .`, or create the workspace in another folder``)
   or a workspace of that name is already installed
   (``workspace "<name>" already exists at <dir>; pick another name, or remove it with `sb ws uninstall <name>` ``).
3. Writes `./sb.flow.yaml` as `name: <name>` and `modules: {}`, and prints
   `wrote <path>`.
4. Installs it exactly as `sb ws install .` would (§5.2), which creates
   `<sb_home>/workspaces/<name>/`, writes the installed copy, and makes
   the workspace active. If that install fails, `./sb.flow.yaml` is removed
   again, so no half-made workspace is left behind.

The first `sb init` below the folder adds the module to `modules:`
(§7.2).

Before 0.5.0, `create` wrote only an empty installed copy and did not
change the active workspace. It now activates the workspace because it
installs it.

### 5.2 `sb install` and `sb ws install` (sb 0.5.0+)

```bash
sb install https://github.com/acme/ws_ips.git                  # clone into <sb_home>/repos/ws_ips/, then install
sb install https://github.com/acme/ws_ips.git --dir ~/ws_ips   # clone into ~/ws_ips instead
cd ~/ws_ips && sb install .                                    # install a folder in place
sb ws install                                                  # same, workspace only, from cwd
```

`sb install <git-url | path>` looks at the manifest at the root of the
source:

| Root holds | Installs as |
|---|---|
| `sb.app.yml` | an app package, as before ([sbcli_app.md](sbcli_app.md) §4) |
| `sb.flow.yaml` | a workspace (this section) |
| both | error |
| neither | error |
| only the pre-0.6.0 flow file name (sb 0.7.0+) | error: `sb.flow.yaml not found at <dir>; flow.yaml is no longer read since 0.7.0, rename it (see sbcli_module.md §Migrating)` |

Source detection (git URL or local folder, `@<ref>` pinning) follows
[sbcli_app.md](sbcli_app.md) §4.1. For a workspace:

| Argument / flag | Effect |
|---|---|
| `<git-url>` | Shallow-cloned (`@<ref>` pins a branch or tag) into `--dir`, else `<sb_home>/repos/<name>/`, `<name>` being the flow's `name:` or else the repository folder name in the URL. Installing the same URL again fast-forwards that clone in place and reinstalls (as `sb ws update`). |
| `<path>` | Used in place, never copied. `sb install .` inside a clone is the common author form. |
| `--dir <path>` | Git URLs only (a local folder is refused: it is installed where it is). The folder must be new or empty, or already a clone of the same URL, which is then fast-forwarded. |
| `--force` | Overwrite vault messages that differ from the repository's copies (§5.2 step 3). The bundled `ros2/std/` set is never overwritten. |

`sb ws install [<git-url | path>]` is the workspace-only form (default
`.`), with the same `--dir` and `--force`: given an app package, it
refuses and points at `sb install`.

**Steps, in order**:

1. **Resolve the source**: clone the URL (stderr: `fetching <url> ...`,
   or `updating <url> in <dir> ...` for a URL already installed), or
   canonicalise the path.
2. **Install the flow.** Read and validate the repo copy, derive the
   name (`name:`, else the folder name), and write the installed copy
   (§4.3), creating `<sb_home>/workspaces/<name>/` when missing. The
   workspace becomes active. Installing again updates the installed
   copy in place. The repo copy is never written.
3. **Import and compile `messages:`** into the configured vault
   (`messages_root`; never a vault discovered from the current
   directory). Each entry's directory is copied into
   `<style>/message_definitions/`: a file already there with identical
   content is unchanged, one with different content is a conflict and
   is left alone unless `--force`, and `ros2/std/` is never overwritten.
   An entry with no conflict is then compiled with its `backends`; an
   entry with a conflict is **not compiled**. Details:
   [sbcli_messages.md](sbcli_messages.md) §4.7.
4. **Regenerate every module's io** from its manifest, as
   `sb io update` does ([sbcli_pubsub.md](sbcli_pubsub.md) §14.6).
   Service routers are rendered only where the file does not exist yet
   (sb 0.6.0+), so a fresh clone gets every router its manifests
   declare while routes you wrote are never overwritten. A module whose
   manifest does not exist yet (listed in `sb.flow.yaml` before
   `sb init` ran in it) is skipped with a warning; an entry that does
   not render is a warning, which `sb ws check` also reports.
5. Print `done. next: sb up` (`done. next: sb init <module dir>` when
   the workspace has no modules yet).

Output, one line per step:

```
-- workspace ws_ips: created, active
-- modules: 2 registered, 1 with up: false (ips_calib)
-- messages: ros2 <- messages/custom: 1 imported, 0 unchanged, 0 conflicts; compiled [proto, iox2]
-- io: ips_calib 1 written, 0 unchanged
-- io: ips_track 2 written, 0 unchanged
  rendered services/cam_param for ips_track
done. next: sb up
```

`created` becomes `updated` on a reinstall. The `rendered` line
(sb 0.6.0+) appears once per service router that did not exist yet. A
module listed before `sb init` ran in it shows
`-- io: <module> skipped (no module manifest)`. `sb 0.7.0+` prints no
deprecation warnings here: a pre-0.6.0 file name or a top-level
`instances:` map fails the install instead (§4.8). **A message conflict fails
the install**, after every step has run: the conflicting entry's line
ends `; not compiled`, each conflict is named on stderr
(`conflict: <style>/<ns>/<Leaf> differs from the vault copy at <path>`),
and the command exits non-zero with
`<n> message(s) differ from the vault copies and were not imported (their styles were not compiled); re-run with --force to overwrite the vault copies`.
The workspace itself is installed and active by then.

The install is **idempotent**: re-run it after `git pull`, or let
`sb ws update` (§5.4) do both. It refuses to replace a workspace of the
same name that was installed from a **different** repository flow that
still exists (§8, §9.7).

### 5.3 `sb ws check [<name>]` (sb 0.5.0+)

```bash
sb ws check            # the active workspace
sb ws check ws_ips     # a named one
```

Validates one installed workspace and prints its rows in the
`sb doctor` format ([sbcli_doctor.md](sbcli_doctor.md) §3), in this
order:

| Row | `[OK]` when | Otherwise | Fix |
|---|---|---|---|
| `workspace` | the installed copy reads; shows the module count and the source (URL or repo `sb.flow.yaml`) | `[FAIL]` (`cannot read <path>`) | reinstall |
| `ws modules` | every module's manifest exists and parses | `[FAIL]` per module | `sb init` in the module; fix the YAML; rename a pre-0.6.0 manifest ([sbcli_module.md](sbcli_module.md) §11.1) |
| `ws types` | every `type:` of a proto / fb entry is in the vault (or is an `iox2_variants` name) | `[FAIL]` per entry | `sb message list`; reinstall to import `messages:` |
| `ws instances` | no instance id appears twice for one module | `[FAIL]` per duplicate | edit `instances:` |
| `ws encoding` | all publishers and subscribers on one wire key agree on `encoding`, across modules | `[FAIL]` per key, listing who uses what | `sb io edit <name> --proto\|--fb\|--raw` |
| `ws io files` | every entry's generated file exists under its module's io dir | `[WARN]` per module, naming the missing files | `sb io update <module>` |
| `ws requires` (sb 0.6.0+) | every executable named by a module's `requires:` is on `PATH` (`all required tools present`) | `[FAIL]`: `<module>: missing <bin>[, <bin>]`, modules joined by `; ` | install the tool, or fix `PATH` |
| `ws platforms` (sb 0.6.0+) | every module supports this host (`all modules support this host`) | `[WARN]`: `<module>: not supported on <os>-<arch> (declares: linux, windows)` | nothing, if the module does not target this host; else add it to `platforms:` |
| `ws wires` (sb 0.6.0+) | every wire resolves (`<n> wire(s) resolve`, or `no wires`) | `[FAIL]`: `<lhs>: <reason>` per wire (§4.7) | fix `wires:` or the manifests |
| `deprecated` (sb 0.6.0+) | nothing deprecated in use (`nothing deprecated in use`) | `[WARN]`, one line per leftover pre-0.6.0 file, each ending `is ignored since 0.7.0 (see sbcli_module.md §Migrating)` (sb 0.7.0+) | [sbcli_module.md](sbcli_module.md) §11 |
| `ws source` | the installed copy is current with the repo `sb.flow.yaml` | `[WARN]` when the repo copy is newer; `[SKIP]` with no `source:` | `sb install <repo>` |

The four `sb 0.6.0+` rows, their exact texts, and the reasons
`ws wires` can give are in [sbcli_doctor.md](sbcli_doctor.md) §4.11.
`ws requires` skips a module that does not support this host.

Modules that fail `ws modules` are left out of the rows below it. Any
`[FAIL]` makes the command exit non-zero (`<n> check(s) failed`). With
no `<name>` and no active workspace it errors with
``no active workspace; pass a name or run `sb ws set <name>` ``. The same
rows, for the active workspace, are part of `sb doctor`
([sbcli_doctor.md](sbcli_doctor.md) §4.11).

### 5.4 `sb ws update <name> [--force]` (sb 0.5.0+)

```bash
sb ws update ws_ips
```

**Effects (in order)**:

1. Reads the installed copy's `source:`. A workspace with no `source:`
   (not installed from a repository) is an error, and so is one
   installed, or last reinstalled, from a local path: it has no URL, so
   `git pull` the checkout yourself and run `sb install .` in it.
2. Runs `git -C <repo dir> pull --ff-only` in the directory holding
   the repo copy. A non-fast-forward or any git failure aborts with
   git's own message; nothing is reinstalled.
3. Installs the pulled repository again, exactly as §5.2 steps 2 to 5,
   keeping the recorded URL. `--force` has the same meaning as for
   `sb install`.

If the pulled `sb.flow.yaml` changed its `name:`, the workspace is
installed under the new name; the old installed copy stays until you
`sb ws uninstall` it.

### 5.5 `sb ws uninstall <name> [--clean]` (sb 0.5.0+)

```bash
sb ws uninstall ws_ips            # remove the installed copy
sb ws uninstall ws_ips --clean    # also delete every module's generated io dir
```

**Effects (in order)**:

1. Validates `<name>` (§1.2). Errors with
   `workspace "<name>" does not exist` when there is no such workspace.
2. With `--clean`: for every module in the installed copy, deletes the
   generated io directory (`io_dir:` of its manifest, default
   `swarmbotix_io`) from the module root. An `io_dir` that is absolute
   or climbs out of the module root is never deleted, and a module whose
   manifest is missing is not touched; both are reported as skipped
   with the reason.
3. Removes `<sb_home>/workspaces/<name>/` and, if it was the active
   workspace, clears the marker (§3) with a warning on stderr.
4. A clone `sb install <git-url>` made under `<sb_home>/repos/` is
   deleted with `--clean`, and otherwise kept and named. A `--dir` clone
   and a local folder belong to you and are never deleted.

```
uninstalled workspace ws_ips (removed /home/el/.swarmbotix/workspaces/ws_ips)
  removed /home/el/.swarmbotix/repos/ws_ips/modules/ips_track/swarmbotix_io
  left ips_calib: io_dir /tmp/io is not a directory inside the module root
  removed clone /home/el/.swarmbotix/repos/ws_ips
```

Without `--clean` the clone line reads
`left clone <path> (pass --clean to delete it)`. The repository's
`sb.flow.yaml` and every module's `sb.module.yaml` and own source are left
alone, except that a clone `sb` made is removed whole under `--clean`;
push your work first. Messages imported into the vault stay there;
remove them with `sb message rm` if you no longer want them.

### 5.6 `sb ws set <name>`

```bash
sb ws set demo
```

**Effects (in order)**:

1. Validates `<name>` against the identifier rule.
2. Errors with ``workspace "<name>" does not exist (run `sb ws create
   <name>` first)`` if `<sb_home>/workspaces/<name>/` is missing.
3. Creates `<sb_home>/` if missing (defensive; it should already exist).
4. Writes `<name>\n` to `<sb_home>/active`, replacing any prior content.

The active marker is the **only** thing this command writes. The
workspace dir itself is untouched.

Output:

```
active workspace = demo
```

### 5.7 `sb ws list`

```bash
sb ws list
```

**Effects (in order)**:

1. Reads `<sb_home>/active` (treats missing/empty as "no active").
2. Walks `<sb_home>/workspaces/` and collects every immediate
   subdirectory.
3. Sorts the entries alphabetically.
4. Prints one per line, prefixing the active one with `*` (other
   lines start with a space to keep the column alignment).

`sb 0.5.0+`: each row also shows the workspace's source (its URL, or
for a local install the repository folder) and `[stale]` when that
repository's `sb.flow.yaml` is newer than the installed copy. A workspace
with no `source:` shows none:

```
* ws_ips  https://github.com/acme/ws_ips.git
  nav     /home/el/nav  [stale]
  demo
[stale]: the repo flow changed after install; run `sb install <source>`
```

The footer line appears only when a row is stale. If no workspace
exists, `list` prints:

```
(no workspaces; run `sb ws create <name>` or `sb install <repo>`)
```

`list` never modifies state. It works without an active workspace.

### 5.8 `sb ws delete <name>`

```bash
sb ws delete demo
```

**Effects (in order)**:

1. Errors with `workspace "<name>" does not exist` if
   `<sb_home>/workspaces/<name>/` is missing.
2. `rm -rf <sb_home>/workspaces/<name>/` (the installed copy and any
   per-workspace `sb.config.yml`; a clone under `<sb_home>/repos/` is
   kept).
3. If `<sb_home>/active` named the deleted workspace, overwrites it
   with empty content (the file remains on disk for forward
   compatibility but `active_workspace()` reads it as `None`).

`sb ws delete` removes what `sb ws uninstall` without `--clean` removes.
If the deleted workspace was active, the CLI emits a warning to
**stderr** so scripts piping stdout don't have to parse around it:

```
deleted workspace demo
warning: demo was the active workspace; cleared `/home/el/.swarmbotix/active`
```

If the deleted workspace was **not** active, no warning prints.

`delete` does **not** touch the repository or the module source trees
the workspace referenced. Reinstall the repository (`sb install <repo>`)
to get the workspace back.

---

## 6. Workflows

### 6.1 Consumer: install a published workspace

```bash
sb config set device <name>
sb install https://github.com/<org>/<workspace>.git --dir ~/<workspace>   # or: cd clone && sb install .
sb up
```

`sb install` clones the repository (into `--dir`, or
`<sb_home>/repos/<workspace>/` without it), installs its `sb.flow.yaml`,
imports and compiles its `messages:`, regenerates every module's
`swarmbotix_io/`, and makes the workspace active. To pick up upstream
changes later:

```bash
sb ws update <workspace>                   # git pull --ff-only, then reinstall
# or: sb install <the same git-url>        # also pulls the clone and reinstalls
# or, in a checkout installed from a path:
git pull && sb install .
```

### 6.2 Author: build a workspace repository

```bash
mkdir ws_ips && cd ws_ips && git init
sb ws create ws_ips                        # writes ./sb.flow.yaml, installs it, active = ws_ips

mkdir -p modules/ips_track
sb init --python modules/ips_track         # registers ips_track in ./sb.flow.yaml, reinstalls
cd modules/ips_track
sb io add pub pose -m ros2/std/StringStamped --zenoh
cd ../..

git add sb.flow.yaml modules && git commit -m "ws_ips: first module"
```

Commit `sb.flow.yaml` and each module's `sb.module.yaml`. The generated
`swarmbotix_io/` may be committed or ignored: every install regenerates
it. After editing `sb.flow.yaml` by hand, run `sb install .` so the
installed copy follows; `sb up` and `sb run` warn while it is stale.

### 6.3 Multiple workspaces

Each repository installs under its own name. Switch between them with
`sb ws set`:

```bash
sb install ~/nav                           # active = nav
sb install ~/perception                    # active = perception
sb ws set nav                              # back to nav

sb ws list
# * nav ...
#   perception ...
```

### 6.4 Tear down

```bash
sb ws uninstall perception                 # installed copy gone; repo untouched
sb ws uninstall nav --clean                # also deletes each module's swarmbotix_io/, and a clone sb made
```

---

## 7. Interaction with other commands

| Command | Uses |
|---|---|
| `sb install` / `sb ws install` / `sb ws create` / `sb ws update` | write the installed copy and the active marker (§5) |
| `sb init` | registers the module in the nearest repo `sb.flow.yaml` and reinstalls; without one, in the active workspace's installed copy; **errors** when there is neither (§7.2) |
| `sb io` | the active installed copy, only when `--module <name>` falls through to it (§7.1); a bare invocation in a module dir needs no workspace (`sb io update` excepted: it always needs one) |
| `sb up` / `sb run` / `sb stop` / `sb down` | the active installed copy; `sb up` skips `up: false` modules, and `sb up` / `sb run` warn when the repo copy is newer ([sbcli_launch.md](sbcli_launch.md) §2) |
| `sb topic listen` / `sb topic list` | the active installed copy, to type frames from a module's manifest ([sbcli_messages.md](sbcli_messages.md) §1.1) |
| `sb doctor` | the active installed copy, for the `topics` row and the `sb ws check` rows ([sbcli_doctor.md](sbcli_doctor.md) §4.10, §4.11); a `[FAIL]` there fails doctor |
| `sb ui` | (L6) workspace-scoped |
| `sb message *` | unaffected: the vault is global (`<sb_home>/messages/`). Only an install writes to it, by importing `messages:` |
| `sb config *` | unaffected: edits the global `sb.config.yml`. The `--workspace` flag (L5) will target the active workspace's override file |

### 7.1 Module resolution and `sb.flow.yaml`

`sb io` resolves `--module <name>` in this order
([sbcli_pubsub.md](sbcli_pubsub.md) §3):

1. `./<name>/sb.module.yaml` under the current directory;
2. `<name>` in the active workspace's installed `sb.flow.yaml`;
3. with no `--module`, `./sb.module.yaml` in the current directory.

`sb 0.7.0+` looks for no other manifest name; a directory that holds
only the pre-0.6.0 one stops the lookup with the error in
[sbcli_module.md](sbcli_module.md) §12.

If the installed copy lists `<name>` but its path no longer exists on
disk, `sb` errors with every path it searched (including the dangling
one). Fix the path in the repo `sb.flow.yaml` and reinstall, or re-run
`sb init` at the module's new location.

### 7.2 What `sb init` writes to `sb.flow.yaml`

`sb init <rootpath> --<lang>` (see [sbcli_init.md](sbcli_init.md))
registers the module in one of two places.

**Under a repository flow** (sb 0.5.0+): the nearest `sb.flow.yaml` at or
above the module root (never an installed copy, and never one directly
in `$HOME` unless the search starts there).

- The entry is `<module>: <path relative to sb.flow.yaml>`, appended as
  one line under `modules:` so your comments and layout survive. When
  `modules:` is written in a form a one-line append cannot handle (a
  flow mapping such as `modules: {}`), the file is rewritten from its
  parsed form instead.
- An entry of the same name that already points at this module is left
  alone. One that points at **another existing** module is an error:
  module names are unique within a workspace. One that points at a
  module that no longer exists is repointed, keeping its `up:`.
- The repository is then reinstalled (§5.2 step 2), keeping the URL a
  previous install recorded, so the installed copy and the active
  marker follow.

**Without a repository flow**: the module is inserted into the active
workspace's installed copy with its absolute `sb.module.yaml` path, as
before 0.5.0. A same-named entry is silently repointed (keeping its
`up:`), because there is only one slot per module name. With no active
workspace either, `sb init` errors (§8).

Module names come from `basename(rootpath)`, so give two module
directories distinct basenames.

---

## 8. Error catalogue

Strings here are the canonical wording from the `sb_workspace::*` and
`sb_core` functions; the CLI prints them after `error:`.

| Code path | Message |
|---|---|
| `validate_identifier` | `invalid workspace name "<n>": <reason from IdentError>` |
| `set_active` (missing) | ``workspace "<n>" does not exist (run `sb ws create <n>` first)`` |
| `delete_workspace` / `uninstall_workspace` (missing) | `workspace "<n>" does not exist` |
| reading an `sb.flow.yaml` | `parsing <path>` (chained from serde_yaml), or `validating <path>` followed by one of the rows below |
| `name:` invalid (sb 0.5.0+) | `workspace name "<n>" is invalid: <reason>` |
| module path empty (sb 0.5.0+) | `module "<m>" has an empty path` |
| `messages:` style invalid (sb 0.5.0+) | `messages[<i>]: style "<s>" is invalid (allowed: [A-Za-z0-9_])` |
| `messages:` path empty (sb 0.5.0+) | `messages[<i>]: path is empty` |
| `messages:` backend unknown (sb 0.5.0+) | `messages[<i>]: unknown backend "<b>" (expected one of: proto, fb, iox2)` |
| duplicate instance id | `module "<m>" declares instance id "<id>" more than once` |
| install: folder name not an identifier (sb 0.5.0+) | ``workspace name "<n>" (from <flow>) is invalid: <reason>; set `name:` in that file`` |
| install: no usable folder name (sb 0.5.0+) | ``cannot derive a workspace name from <flow>; set `name:` in it`` |
| install: source is an installed copy (sb 0.5.0+) | `<path> is an installed workspace copy; install from the repository's sb.flow.yaml instead` |
| install: name taken by another live repository (sb 0.5.0+) | ``workspace "<n>" is already installed from <flow>; uninstall it first, or give <other flow> a different `name:` `` |
| `sb ws update`: no `source:` (sb 0.5.0+) | `workspace "<n>" was not installed from a repository flow` |
| `sb ws update`: local path install (sb 0.5.0+) | `workspace "<n>" was installed from a local path (<flow>) with no URL; update that checkout yourself and install it again` |
| `sb ws update`: pull failed (sb 0.5.0+) | `git -C <repo dir> pull --ff-only failed: <git stderr>` |
| `sb ws create`: `./sb.flow.yaml` exists (sb 0.5.0+) | ``<path> already exists; install it with `sb install .`, or create the workspace in another folder`` |
| `sb ws create`: name taken (sb 0.5.0+) | ``workspace "<n>" already exists at <dir>; pick another name, or remove it with `sb ws uninstall <n>` `` |
| `sb install`: both manifests (sb 0.5.0+) | `<source> has both sb.app.yml and sb.flow.yaml; a package is one or the other` |
| `sb install`: no manifest (sb 0.5.0+) | `<source> has neither sb.app.yml (an app package) nor sb.flow.yaml (a workspace package) at its root` |
| `sb install`: message conflicts (sb 0.5.0+) | `<n> message(s) differ from the vault copies and were not imported (their styles were not compiled); re-run with --force to overwrite the vault copies` |
| `sb install --dir` on a local folder (sb 0.5.0+) | `--dir only applies to a git URL; <dir> is a local folder and is installed where it is` |
| `sb install --dir` into a used folder (sb 0.5.0+) | `<dir> already exists and is not empty; pass a new or empty --dir` |
| `sb ws install` given an app (sb 0.5.0+) | ``<source> is an app package (sb.app.yml), not a workspace; install it with `sb install <source>` `` |
| `sb ws update`: no git (sb 0.5.0+) | `running git (is it on PATH?)` |
| `sb init`: duplicate module name in a repo flow (sb 0.5.0+) | `module "<m>" is already registered in <flow> for <other manifest>; module names must be unique within a workspace` |
| `sb init`: nowhere to register (sb 0.5.0+) | ``no active workspace and no sb.flow.yaml above <root>; run `sb ws create <name> && sb ws set <name>` first, or add a sb.flow.yaml at the repository root`` |
| a top-level `instances:` map (sb 0.7.0+) | `top-level instances: is no longer read since 0.7.0; move each list under modules.<name>.instances (see sbcli_module.md §Migrating)` |
| a directory with the pre-0.6.0 flow file name only (sb 0.7.0+) | `sb.flow.yaml not found at <dir>; flow.yaml is no longer read since 0.7.0, rename it (see sbcli_module.md §Migrating)` |
| malformed wire (sb 0.6.0+) | `wires.<lhs>: <reason>` |

These are stable wordings; integration tests in `crates/sb-cli/tests`
grep for substrings of them.

---

## 9. Edge cases and gotchas

### 9.1 Active workspace deleted out from under the CLI

If you `rm -rf <sb_home>/workspaces/<active>/` manually instead of
through `sb ws delete`, the `active` marker still names the gone
workspace. Verbs that read it then find an empty registry, and a write
into it fails because the directory is gone. Fix by reinstalling the
repository (`sb install <repo>`), or by `sb ws set <other>`.

### 9.2 `sb.flow.yaml` corruption

A malformed repo copy fails the next install with `parsing <path>` or
`validating <path>`; fix the file and install again. Nothing is written
until it loads.

A malformed installed copy makes every verb that reads it fail the same
way. `sb` does **not** auto-rewrite it on read. Reinstall the repository
(`sb install <repo>`), which overwrites it. A pre-0.5.0 workspace with
no repository: hand-fix the YAML, or delete the file (an absent
installed copy reads as an empty registry) and re-run `sb init` for
each module.

### 9.3 Workspace-scoped `sb.config.yml` not yet wired

`<sb_home>/workspaces/<name>/sb.config.yml` is documented in the
config layering chain ([sbcli_config.md](sbcli_config.md) §3) and
read by `SbCliConfig::merge`, but `sb config --workspace` currently
errors with `"--workspace requires an active workspace (L5+); not yet
implemented"`. Hand-write the file if you need a workspace override
before L5 ships. Reinstalling never touches it.

### 9.4 Symlinks in `<sb_home>` and in the repository

`SbHome::root()` is the resolved string; symlinks are not
canonicalized. If `~/.swarmbotix` is a symlink, `sb` follows it
transparently on every operation.

Module and message paths in an installed copy are canonicalized when
they exist, so they hold symlink-resolved locations. If the repository
sits behind a symlink, the installed copy names the real directory.

### 9.5 Active marker race

`sb` reads `active`, then performs work, then writes back. If another
process changes the marker between the read and the write (you
`sb ws set other` in another terminal while a long-running command is
mid-flight), the in-flight command operates on the workspace it read at
the start. There is no locking; workspaces are designed for interactive
single-user workflows.

### 9.6 Renaming a workspace

There is no `sb ws rename`. For a repository workspace (sb 0.5.0+):

```bash
# set `name: new_name` in the repo sb.flow.yaml, then:
sb install .                     # installs as new_name, active = new_name
sb ws uninstall old_name
```

Module manifests do not store the workspace name. Generated files do
(the `<workspace>` segment of every owned wire key), and the reinstall
regenerates them under the new name.

For a pre-0.5.0 workspace with no repository: create the new directory,
move the installed `sb.flow.yaml` into it (migrated first, §4.5),
`sb ws set new_name`, then `sb ws delete old_name`.

### 9.7 Two repositories, one workspace name

Two repositories whose flows install under the same name would take
turns overwriting one installed copy, so the second install is refused
while the first repository's `sb.flow.yaml` still exists (§8). Give one of
them a distinct `name:`, or `sb ws uninstall` the first. A previous
source that no longer exists (the repository was moved or deleted) is
replaced without a prompt, so moving a clone and running `sb install .`
in its new location just works.

### 9.8 An `sb.flow.yaml` in `$HOME`

`sb init` searches upwards from the module root for a repository flow
but never examines `$HOME` itself (unless the search starts there). A
stray `~/sb.flow.yaml` would otherwise capture every project under the home
directory.
