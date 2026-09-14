# `sb ws` — Workspace Management Reference

Official reference for the `sb ws` subcommand surface: workspace
lifecycle, the active marker, `flow.yaml`, and the resolution rules
that downstream verbs (`sb init`, `sb pub/sub *`, `sb up`) consult.

This document is normative for behavior shipped at L2.

The canonical spec is `requirements.md` in the sbcli **source
repository** — a maintainer document that is not part of your install;
when it and this page disagree, it wins.

---

## 1. Conceptual model

A **workspace** is an isolated project scope, analogous to a Python
virtual environment. Each workspace is a real directory on disk at
`<sb_home>/workspaces/<name>/` and holds exactly one file: its
`flow.yaml` module registry (see §4).

`sb` mediates **which workspace is active** through a one-line marker
file at `<sb_home>/active`. Every downstream verb (`sb init`,
`sb pub/sub *`, `sb up`, `sb topic *`) consults that marker to decide
which workspace's `flow.yaml` to read or mutate.

Throughout this doc, `<sb_home>` refers to the resolved per-user
swarmbotix root — see §2.

### 1.1 What a workspace contains

```
<sb_home>/workspaces/<name>/
├─ flow.yaml                  # module registry — see §4
└─ sb.config.yml              # OPTIONAL per-workspace config override (L1 stub; wired by `sb config --workspace` at L5)
```

Workspaces do **not** own the modules they register. A module's source
tree lives wherever the user keeps it (cargo crates, Python projects,
Unity trees, etc.); the workspace just records an absolute path to
that tree's `sb.dev.yml` in `flow.yaml`.

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

---

## 2. `<sb_home>` resolution

`<sb_home>` is the per-user swarmbotix tree that holds the active
marker, every workspace's directory, and the global `sb.config.yml`.
It resolves in this order (highest priority first):

1. `$SB_CONFIG` env override → its `sb_home_dir` field (tilde-expanded).
2. Active workspace's `sb.config.yml` → its `sb_home_dir` field.
3. `~/.swarmbotix/sb.config.yml` → its `sb_home_dir` field.
4. Built-in fallback: `~/.swarmbotix/` (via `dirs::home_dir()`).

The resolver is `sb_config::resolve_sb_home_dir` and is shared with
every other verb that needs to read `<sb_home>`. `$SB_CONFIG` is the
only override the test suite uses to pin a tempdir as the root.

The `<sb_home>/` tree only ever contains:

```
<sb_home>/
├─ sb.config.yml          # global config (optional — see sbcli_config.md)
├─ active                 # one-line marker file (see §3)
├─ workspaces/            # one subdir per workspace (see §1.1)
│  └─ <name>/
│     ├─ flow.yaml
│     └─ sb.config.yml    # optional per-workspace override
└─ messages/             # message styles (see sbcli_messages.md)
   └─ <style>/
      ├─ message_definitions/   # .proto vault
      └─ message_targets/       # codegen output root
```

`workspaces/` is auto-created on the first `sb ws create`. The
`messages/` style tree is managed by `sb message *` and unrelated to
workspaces — it's listed only to clarify the layout.

---

## 3. The active marker — `<sb_home>/active`

A single-line file holding the active workspace's name (no trailing
slash, no path — just the bare identifier). Examples:

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
non-erroneous state — it just means no verb can resolve a workspace
until you run `sb ws set <name>`.

The file is written by `sb ws set <name>` and cleared (overwritten with
empty content, never deleted) by `sb ws delete <active-name>`.

---

## 4. `flow.yaml` schema

Lives at `<sb_home>/workspaces/<name>/flow.yaml`. Empty after
`sb ws create <name>`; populated incrementally by `sb init`,
`sb pub add`, and the swarmctl UI.

```yaml
modules:
  camera:   /home/user/projects/camera/sb.dev.yml
  detector: /home/user/projects/detector/sb.dev.yml
  tablet:   /home/user/projects/tablet/sb.prd.yml
```

Schema notes:

- The top-level key is `modules:` and only `modules:`. Anything else
  is rejected at parse time (`deny_unknown_fields` on `WorkspaceFlow`).
- Values are absolute paths to either an `sb.dev.yml` (dev-time) or
  `sb.prd.yml` (production) file. `sb init` only ever writes
  `sb.dev.yml` paths; production paths are reserved for when a
  binary ships with its baked-in interface.
- Module names follow the identifier rule from §1.2.
- The file is rewritten in full on every mutation — there's no
  partial-update semantics. Re-running `sb init` on the same path is
  a no-op.

`flow.yaml` only stores `module → path` pairs. Edges between modules
(which publisher feeds which subscriber) are **derived** by matching
topic names across the modules' own `sb.dev.yml` declarations — never
by listing edges in `flow.yaml` directly. This is intentional and
load-bearing for L6's swarmctl UI.

---

## 5. Subcommands

All `sb ws *` subcommands resolve `<sb_home>` once at startup (§2),
read/write only under `<sb_home>/workspaces/` and the active marker,
and never touch a module's source tree.

### 5.1 `sb ws create <name>`

```bash
sb ws create demo
```

**Effects (in order)**:

1. Validates `<name>` against the identifier rule (§1.2). Rejects with
   `"invalid workspace name <name>: <reason>"` on mismatch.
2. Errors with `"workspace <name> already exists at <path>"` if
   `<sb_home>/workspaces/<name>/` already exists.
3. Creates `<sb_home>/workspaces/<name>/` (and the parent
   `<sb_home>/workspaces/` if missing — first-run install).
4. Writes an empty `flow.yaml` containing `{}` for the modules map.

`create` does **not** set the workspace active — that requires a
separate `sb ws set <name>`. Rationale: in a multi-workspace
session, you may want to create N workspaces ahead of time and then
switch among them.

Output:

```
created workspace demo at /home/el/.swarmbotix/workspaces/demo
```

### 5.2 `sb ws set <name>`

```bash
sb ws set demo
```

**Effects (in order)**:

1. Validates `<name>` against the identifier rule.
2. Errors with `"workspace <name> does not exist (run `sb ws create
   <name>` first)"` if `<sb_home>/workspaces/<name>/` is missing.
3. Creates `<sb_home>/` if missing (defensive — should already exist).
4. Writes `<name>\n` to `<sb_home>/active`, replacing any prior content.

The active marker is the **only** thing this command writes. The
workspace dir itself is untouched.

Output:

```
active workspace = demo
```

### 5.3 `sb ws list`

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

Sample output:

```
* demo
  experimental
  nav_stack
```

If `<sb_home>/workspaces/` doesn't exist (you've never run
`sb ws create`), `list` prints:

```
(no workspaces — run `sb ws create <name>`)
```

`list` never modifies state. It works without an active workspace.

### 5.4 `sb ws delete <name>`

```bash
sb ws delete demo
```

**Effects (in order)**:

1. Errors with `"workspace <name> does not exist"` if
   `<sb_home>/workspaces/<name>/` is missing.
2. `rm -rf <sb_home>/workspaces/<name>/` (removes the directory and
   its `flow.yaml`).
3. If `<sb_home>/active` named the deleted workspace, overwrites it
   with empty content (the file remains on disk for forward
   compatibility but `active_workspace()` reads it as `None`).

If the deleted workspace was active, the CLI emits a warning to
**stderr** so scripts piping stdout don't have to parse around it:

```
deleted workspace demo
warning: demo was the active workspace; cleared `/home/el/.swarmbotix/active`
```

If the deleted workspace was **not** active, no warning prints.

`delete` does **not** touch the module source trees that the deleted
workspace's `flow.yaml` referenced. Those projects remain on disk and
keep their own `sb.dev.yml` files; they're just no longer registered
in any workspace until you `sb init` them again.

---

## 6. End-to-end workflow

Start fresh:

```bash
sb ws create demo                       # mkdir workspaces/demo/, init empty flow.yaml
sb ws set demo                          # active = demo

sb init --rust /path/to/my_rust_crate   # adopts crate, appends to flow.yaml
sb init --python /path/to/my_py_app     # adopts app, appends to flow.yaml

sb ws list
# * demo

cat ~/.swarmbotix/workspaces/demo/flow.yaml
# modules:
#   my_rust_crate: /path/to/my_rust_crate/sb.dev.yml
#   my_py_app:     /path/to/my_py_app/sb.dev.yml
```

Multiple workspaces, switch between them:

```bash
sb ws create nav
sb ws create perception
sb ws set perception
# `sb init` from here lands modules in perception/flow.yaml only.

sb ws set nav
# `sb init` from here lands modules in nav/flow.yaml only.

sb ws list
#   demo
#   nav
# * perception
```

Tear down a workspace cleanly:

```bash
sb ws delete demo
# deleted workspace demo
# (no warning — demo wasn't active)

# Module source trees untouched. Re-adopt them later with `sb init` if needed.
```

---

## 7. Interaction with other commands

| Command | Behavior when no workspace is active |
|---|---|
| `sb init` | **Errors** — needs a workspace to register the module in. Message: `"no active workspace — run \`sb ws create <name> && sb ws set <name>\` first"`. |
| `sb pub/sub add/edit/rm` | Only errors when `--module <name>` is passed and resolution falls back to `flow.yaml` (see [sbcli_pubsub.md](sbcli_pubsub.md) §3). Bare invocation in a module dir still works without an active workspace. |
| `sb list` / `sb pub list` / `sb sub list` | Same as above. |
| `sb up` / `sb run` / `sb down` | (L5) Errors — need a workspace for the tmux session name (`sb-<workspace>`). |
| `sb ui` | (L6) Errors — swarmctl is workspace-scoped. |
| `sb message *` | Unaffected. The message vault is global (`<sb_home>/messages/`), not per-workspace. A workspace's `sb.config.yml` can override `messages_root` to point at a different tree entirely. |
| `sb doctor` | Unaffected. Validates host tooling only. |
| `sb config *` | Unaffected. Edits the global `sb.config.yml`. The `--workspace` flag (L5) will target the active workspace's override file. |

### 7.1 Module resolution and `flow.yaml`

`sb pub/sub *` resolve `--module <name>` against the active
workspace's `flow.yaml` as rule 2 of their three-rule lookup chain. See
[sbcli_pubsub.md](sbcli_pubsub.md) §3 for the full order.

The relevant guarantee: if `flow.yaml` lists `<name>: <path>` and
`<path>` no longer exists on disk, `sb` errors with all paths it
searched (including the dangling one). Use `sb init <new_path>
--force` to re-register, or hand-edit `flow.yaml` to point elsewhere.

### 7.2 What `sb init` writes to `flow.yaml`

`sb init <rootpath> --<lang>` (see [sbcli_init.md](sbcli_init.md))
appends one entry per call to the active workspace's `flow.yaml`. The
write is idempotent: re-running with the same rootpath does not
duplicate the entry; it just updates the value if the path changed.

Module names come from `basename(rootpath)`, so two projects under the
same parent directory with the same name (e.g.
`/projects/camera/` and `/other_projects/camera/`) will collide in
`flow.yaml`. The second `sb init` silently overwrites the first
because there's only one slot per module name. Avoid by giving the
project dirs distinct basenames.

---

## 8. Error catalogue

Strings here are the canonical wording from the
`sb_workspace::*` functions; the CLI surfaces them with an `error:`
prefix from clap's exit path.

| Code path | Message |
|---|---|
| `validate_identifier` | `invalid workspace name "<n>": <reason from IdentError>` |
| `create_workspace` (dup) | `workspace "<n>" already exists at <path>` |
| `set_active` (missing) | `workspace "<n>" does not exist (run \`sb ws create <n>\` first)` |
| `delete_workspace` (missing) | `workspace "<n>" does not exist` |
| `read_flow` (parse) | `parsing <path>` (chained from serde_yaml) |

These are stable wordings — integration tests grep for substrings in
`crates/sb-cli/tests` and the conflict-policy tests in
`crates/sb-cli/tests` depend on them.

---

## 9. Edge cases and gotchas

### 9.1 Active workspace deleted out from under the CLI

If you `rm -rf <sb_home>/workspaces/<active>/` manually instead of
through `sb ws delete`, the `active` marker still names the gone
workspace. Subsequent `sb init` runs error with `"flow.yaml not
found"` (technically: `flow.yaml` reads as empty, then `write_flow`
fails because the parent dir is gone). Fix by `sb ws set <other>` or
`sb ws create <active>` to re-create the dir.

### 9.2 `flow.yaml` corruption

If a user (or a swarmctl bug) writes malformed YAML to `flow.yaml`,
every subsequent `sb init` / `sb pub *` / `sb sub *` errors with
`parsing <path>` from serde. `sb` does **not** auto-rewrite
malformed `flow.yaml` files. Recover by deleting the file (next `sb
init` reinitializes an empty one) or by hand-fixing the YAML.

### 9.3 Workspace-scoped `sb.config.yml` not yet wired

`<sb_home>/workspaces/<name>/sb.config.yml` is documented in the
config layering chain ([sbcli_config.md](sbcli_config.md) §3) and
read by `SbCliConfig::merge`, but `sb config --workspace` currently
errors with `"--workspace requires an active workspace (L5+); not yet
implemented"`. Hand-write the file if you need a workspace override
before L5 ships.

### 9.4 Symlinks in `<sb_home>`

`SbHome::root()` is the resolved string — symlinks are not
canonicalized. If `~/.swarmbotix` is a symlink, `sb` follows it
transparently on every operation; the path that ends up in
`flow.yaml` is whatever path the resolver returned, symlinks and all.
This is fine for typical setups (e.g. a project-local sb_home in a
shared volume) but can confuse `realpath`-based tooling.

### 9.5 Active marker race

`sb` reads `active`, then performs work, then writes back. If another
process changes the marker between the read and the write (you
`sb ws set other` in another terminal while a long-running `sb init`
is mid-flight), the in-flight command operates on the workspace it
read at the start. There is no locking — workspaces are designed for
interactive single-user workflows.

### 9.6 Renaming a workspace

There is no `sb ws rename`. To rename:

```bash
sb ws create new_name
mv <sb_home>/workspaces/old_name/flow.yaml <sb_home>/workspaces/new_name/flow.yaml
sb ws set new_name
sb ws delete old_name           # warns if old_name was active; otherwise silent
```

The active marker won't automatically follow the rename — set it
explicitly. Existing `sb.dev.yml` files on disk are unaffected; they
don't store the workspace name (only `flow.yaml` does).
