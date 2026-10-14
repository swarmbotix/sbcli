# `sb up` / `run` / `stop` / `down` / `attach`: Launch Reference

Official reference for the launch verbs: how a workspace's modules are
built, started, restarted, stopped, torn down, and reattached under
tmux.

This document is normative for behavior shipped at L5, extended by the
native launch of `sb 0.6.0+` (§1 onward). `sb 0.7.0+` launches every
module natively (§1.4).

The canonical spec is `requirements.md` in the sbcli **source
repository**, a maintainer document that is not part of your install;
when it and this page disagree, it wins.

Related: [sbcli_module.md](sbcli_module.md) (the `sb.module.yaml` these
verbs build and start from), [sbcli_ws.md](sbcli_ws.md) (the
`sb.flow.yaml` registry, instances, `up: false`),
[sbcli_init.md](sbcli_init.md) (which writes the manifest).

---

## 1. Conceptual model

`sb` does not supervise your processes. It owns a build step and a tmux
session: it builds every module it is about to start, then starts each
instance of each module in its own window, and closes those windows
again on `sb stop` or `sb down`.

```
sb up
 1. build   cam: cargo build --release          (inline, in your terminal)
            viz: cmake -S . -B build ... then cmake --build build ...
 2. launch  tmux session  sb-<active_workspace>
            ├── window "cam-left"    ← target/release/cam --id left
            ├── window "cam-right"   ← target/release/cam --id right
            ├── window "detector"    ← .venv/bin/python main.py
            └── window "viz"         ← build/viz
```

Five facts follow from that picture (sb 0.6.0+), and most surprises
come from forgetting one of them:

1. **The session name is `sb-<workspace>`**, derived from the active
   workspace marker. Switching workspaces (`sb ws set`) points every
   launch verb at a different session. Two workspaces can be up at once
   without colliding.
2. **Every module is built first, inline.** `sb up` and `sb run` run
   each module's `build` in your terminal, in order, before the first
   tmux call. The first failure stops everything, so **a failed build
   leaves no session behind**.
3. **One window per instance.** A module with instances in
   `sb.flow.yaml` gets one window per instance, named `<module>-<id>`;
   a module without gets one window named `<module>` (§1.1).
4. **The launched command is the module's `start` command, run
   directly**, with no shell in between: the program and its arguments,
   in the module root, with the manifest's environment
   ([sbcli_module.md](sbcli_module.md) §9). An instance's command ends
   in `--id <id>`.
5. **A window stays open after its process exits** (tmux
   `remain-on-exit`), so a crash leaves its last output on screen.
   `sb stop` and `sb down` close it.

Windows are used rather than panes so each instance's output is
independently scrollable.

### 1.1 Window naming

| Module | Instances in `sb.flow.yaml` | Windows |
|---|---|---|
| `cam` | `[left, right]` | `cam-left`, `cam-right` |
| `detector` | none | `detector` |

`<module>-<id>` is also the `<module>` segment a per-instance publisher
uses on the wire ([sbcli_pubsub_consuming.md](sbcli_pubsub_consuming.md)
§10), so a window and the topics it publishes carry the same name.

Targets are exact (sb 0.6.0+). `sb` addresses its session as
`=sb-<workspace>` and each window by its tmux window id, so `sb-demo`
never matches `sb-demo2`, and `sb stop cam` closes `cam` and `cam-*`
but never `camera`. Module names cannot contain `-`, so `cam-*` can only
be an instance of `cam`.

### 1.2 What runs in a window

| Aspect | Value |
|---|---|
| program and arguments | the expanded `scripts.start`, with `{args}` replaced by the instance's `args` and any passthrough args, then `--id <id>` for an instance |
| working directory | the module root (tmux `-c`) |
| environment | the merged `env:` of the manifest, the instance's `env`, then `SB_WORKSPACE`, `SB_MODULE`, `SB_MODULE_ROOT`, and, for an instance, `SB_INSTANCE_ID` (tmux `-e`) |

The details, including the language defaults that apply when a manifest
declares no `scripts:`, are in [sbcli_module.md](sbcli_module.md) §3.2
and §9.

### 1.3 Preconditions common to every verb

Checked in this order, before anything is built or tmux is touched:

1. **An active workspace exists.** Otherwise:
   ``no active workspace; run `sb ws create <name> && sb ws set <name>` first``
2. **The module resolves through the installed `sb.flow.yaml`.** Module
   paths come from the registry (the workspace's installed copy, see
   [sbcli_ws.md](sbcli_ws.md) §4.3), never from cwd, so every launch
   verb works from any directory.
3. **The module's manifest reads and resolves for this host**: it
   parses, the host is among its `platforms` (§2 for how `sb up`
   treats an unsupported module), it has a start command, and its
   placeholders expand ([sbcli_module.md](sbcli_module.md) §12).
4. **tmux is resolvable**: the `tmux:` path in `sb.config.yml` if set,
   otherwise `tmux` on `PATH`, and on Windows `psmux` on `PATH` as a
   last resort (§11). `sb doctor` validates it.

`sb up` is **plan-then-execute**: every module's preconditions are
checked, then every build runs, and only then is tmux called. A
workspace where one module has a broken manifest or a failing build
launches *nothing* rather than a partial session.

### 1.4 Every module is native (sb 0.7.0+)

Every module is launched as this document describes, from its
`sb.module.yaml`: its own `scripts:` where it declares them, its
language defaults ([sbcli_module.md](sbcli_module.md) §3.2) where it
does not. `sb 0.7.0+` never runs a per-module launch or stop script of
the kind releases before 0.6.0 generated, even when one is still present
at the module root. `sb ws check` and `sb doctor` list such leftovers in
their `deprecated` row ([sbcli_doctor.md](sbcli_doctor.md) §4.11), and
[sbcli_module.md](sbcli_module.md) §11.3 and §11.4 translate them.

---

## 2. `sb up`

```
sb up
```

Build and launch every module in the active workspace's `sb.flow.yaml`,
one window per instance, in a detached session. No arguments, and no
passthrough args; that is `sb run`'s job. An instance's own `args` and
`env` from `sb.flow.yaml` do apply.

In order:

1. **Plan.** Read the installed copy, resolve every module's manifest
   for this host (§1.3). Modules with `up: false` are left out. Three
   kinds of module are skipped, each with a `skipped <module>: <reason>`
   line on stdout, printed before any build: a module whose `platforms:`
   do not list this host, a Unity module (it has no start command), and
   a module whose instances declare `docker:` (`sb` does not start
   containers yet; [sbcli_module.md](sbcli_module.md) §11.6). The
   session check also happens here.
2. **Build.** Run each planned module's `build`, inline, printing
   `building <module>: <command>` before each command. The first failure
   stops `sb up` with a non-zero exit, before any session exists.
3. **Launch.** Create the `sb-<workspace>` session and one window per
   instance.

```console
$ sb up
building cam: cargo build --release
    Finished `release` profile [optimized] target(s) in 0.21s
launched 3 window(s) in tmux session "sb-docdemo"
  - cam-left
  - cam-right
  - detector
attach with: tmux attach -t sb-docdemo
```

On a host a module does not support, its `skipped` line comes first:

```console
$ sb up
skipped cam_gige_ht: not supported on windows-x86_64 (declares: linux)
building viz: cmake -S . -B build -DCMAKE_BUILD_TYPE=Release
building viz: cmake --build build --config Release
launched 2 window(s) in tmux session "sb-docdemo"
  - detector
  - viz
attach with: tmux attach -t sb-docdemo
```

The other two reasons read
`unity modules have no start command (launch them from their editor)`
and `instance <id> declares docker:, which native launch does not run yet`
([sbcli_module.md](sbcli_module.md) §11.6 has the way forward).

Warnings, such as the stale-copy warning below, go to stderr and never
change these lines.

**`up: false` modules are skipped** (sb 0.5.0+). A module registered as
`{ path: ..., up: false }` ([sbcli_ws.md](sbcli_ws.md) §4.4) is neither
built nor given a window by `sb up`; start it on its own with
`sb run <module>`.

**A stale installed copy warns** (sb 0.5.0+). When the workspace was
installed from a repository and the repo `sb.flow.yaml` is newer than
the installed copy (you edited it, or pulled, without reinstalling),
`sb up` prints, on stderr,

```
warning: sb.flow.yaml at /home/el/ws_ips is newer than the installed copy; run sb install
```

and launches from the installed copy anyway. Run `sb install <repo>`
(or `sb ws update <name>`) to bring the installed copy up to date.
`sb run` gives the same warning.

**`sb up` refuses to touch an existing session.** Re-launching a
workspace is explicitly `sb down && sb up`:

```console
$ sb up
error: tmux session "sb-docdemo" already exists; run `sb down` first, or `sb run <module>` to restart one module
```

The session check comes before the builds, so a refused `sb up` costs
no build time. Other refusals:

```console
$ sb up
error: workspace "emptyws" has no modules in its flow; run `sb init` to adopt one

$ sb up
error: workspace "tools" has no modules to launch: every module in its flow has `up: false` (use `sb run <module>` to start one)

$ sb up
error: workspace "rig" has nothing to launch on windows-x86_64: cam_gige_ht: not supported on windows-x86_64 (declares: linux)
```

The last one is printed when every module that is not `up: false` was
skipped. A manifest that does not resolve (a bad `platforms` key, a
`start` with two commands, an unknown placeholder) stops `sb up` in
step 1 with the error from [sbcli_module.md](sbcli_module.md) §12; a
failing build stops it in step 2 with the build's own output above the
error.

---

## 3. `sb run <module>[:<id>] [args...]`

```
sb run [--fg] <MODULE>[:<ID>] [ARGS]...
```

Build one module, then start it in the `sb-<workspace>` session,
creating the session if it does not exist yet.

| Argument | Required | Meaning |
|---|---|---|
| `<MODULE>` | yes | Module name as registered in the active workspace's `sb.flow.yaml`. Starts **every** instance of the module (or its one window, without instances). A module marked `up: false` can be run (sb 0.5.0+); the flag only keeps it out of `sb up`. |
| `<MODULE>:<ID>` | instead of `<MODULE>` | One instance only (sb 0.6.0+). `<ID>` must be one of the module's instances. |
| `--fg` | no | Run in this terminal instead of tmux (§3.1, sb 0.6.0+). |
| `[ARGS]...` | no | Passthrough args, placed where the start command has `{args}` (§7) |

**Idempotent:** a window that already exists is respawned in place, so
there is never a duplicate window for one instance. After starting,
`sb run <module>` also closes windows named `<module>` or `<module>-*`
that are no longer in the plan, such as an instance you removed from
`sb.flow.yaml`.

Output, one line per window:

```console
$ sb run detector
launched detector in tmux session "sb-docdemo"

$ sb run cam
building cam: cargo build --release
restarted cam-left in tmux session "sb-docdemo"
restarted cam-right in tmux session "sb-docdemo"

$ sb run cam:right --exposure 4000
building cam: cargo build --release
restarted cam-right in tmux session "sb-docdemo" (args: --exposure 4000)
```

The verb distinguishes the two cases: `launched` for a new window,
`restarted` when an existing one was replaced.

Refusals specific to `sb run`:

| Situation | Error |
|---|---|
| unknown instance | `module "cam" has no instance "top" (instances: left, right)` |
| `:<id>` on a module without instances | ``module "detector" has no instances; run it as `sb run detector` `` |
| this host not in `platforms:` | `module "cam_gige_ht" is not supported on windows-x86_64 (declares: linux)` |
| an instance declares `docker:` | `module "cam_gige_ht": instance left declares docker:, ...` (as in §2) |
| malformed target | `bad target "cam:left:x": expected <module> or <module>:<id>` | Under the hood the
replace path is `tmux respawn-window -k`, not `kill-window` +
`new-window`: killing the last window in a session also kills the
session (and, on default config, the tmux server), which would break
the immediately following create.

### 3.1 `sb run --fg`

```bash
sb run --fg viz              # a module without instances
sb run --fg cam:left         # one instance of an instanced module
```

Builds, then runs the start command in the current terminal, with no
tmux at all: no banner, the program's stdin, stdout, and stderr are
your terminal's, and `sb` exits with the program's exit code. Ctrl+C
reaches the program. The command, working directory, and environment
are exactly what the window would have had (§1.2).

`--fg` starts exactly one process: an instanced module needs its
`:<id>` (otherwise:
``module "cam" has 2 instances and `sb run --fg` runs one; pick it with cam:<id>``).

---

## 4. `sb stop <module> [args...]`

```
sb stop <MODULE> [ARGS]...
```

Stop a module: close every window named `<module>` or `<module>-<id>`,
then run its `scripts.stop` in your terminal if the manifest declares
one (sb 0.6.0+).

| Argument | Required | Meaning |
|---|---|---|
| `<MODULE>` | yes | Module name as registered in the active workspace's `sb.flow.yaml`. Every instance's window is closed. |
| `[ARGS]...` | no | Passthrough args for `scripts.stop`, where it has `{args}` |

```console
$ sb stop cam
stopped cam: closed 2 window(s) [cam-left, cam-right]

$ sb stop cam_gige_ht
stopped cam_gige_ht: closed 2 window(s) [cam_gige_ht-left, cam_gige_ht-right]
ran scripts.stop for cam_gige_ht
```

The second line appears only when `scripts.stop` ran. If closing the
windows leaves the session empty, tmux ends the session, exactly as
`sb down` would.

`sb stop` takes a module, not an instance
(``error: `sb stop` takes a module name and closes every instance of it; drop the :<id> from "cam:left"``).
To restart a single instance, use `sb run <module>:<id>`. On a host the
module does not support, `sb stop` still closes its windows and skips
`scripts.stop`.

### 4.1 What `sb stop` actually does

**The processes are ended first, by closing their windows; then
`scripts.stop` runs.** tmux sends each window's process a hang-up
signal (`SIGHUP`) when the window is killed. `scripts.stop` therefore
runs after the module's processes are gone, and is **not** a graceful
signal delivered to a still-running module.

The practical consequences:

- A program that needs to flush or release something on shutdown should
  handle `SIGHUP` (and `SIGTERM`, for `sb down` and for runs outside
  `sb`).
- If the module's real work runs somewhere else (a docker container, a
  systemd unit, a remote box), `scripts.stop` is exactly the right tool:
  the closed window held only the launcher.
- `scripts.stop` runs once per module, in the module root, with the
  module's `env:`. It has no `{id}`
  ([sbcli_module.md](sbcli_module.md) §5).

### 4.2 Do I need a stop script?

**No.** `sb stop` closes the windows itself, and `sb init` writes no
stop script. Declare `scripts.stop` only for teardown outside the
processes `sb` started: removing containers, unmounting devices,
releasing hardware, deleting lock files.

```yaml
# sb.module.yaml
scripts:
  stop: docker rm -f cam_gige_ht-left cam_gige_ht-right
```

### 4.3 Two wrinkles worth knowing

- **A module that is not running is not an error.** With none of its
  windows open, `sb stop` closes nothing and still runs `scripts.stop`,
  so a teardown against an already-stopped workspace is harmless.
- **Closing the last window ends the session.** After `sb stop` of the
  only running module, `tmux ls` no longer lists `sb-<workspace>`, and
  the next `sb run` creates it again.

---

## 5. `sb down`

```
sb down
```

Kill the `sb-<workspace>` session and every window in it. No arguments.

```console
$ sb down
killed tmux session "sb-docdemo"

$ sb down
no tmux session "sb-docdemo"; nothing to do
```

Missing session is **not** an error: `sb down` exits 0 either way, so
it is safe in cleanup paths and CI teardown.

`sb down` is coarser than `sb stop`: it tears down the whole session in
one call and **runs no `scripts.stop` at all**. If your modules need
teardown (containers, hardware, lock files), `sb stop` each of them
first, or accept that the cleanup does not happen.

---

## 6. `sb attach`

```
sb attach
```

Reattach to the workspace's session. No arguments. Behavior depends on
whether stdout is a TTY:

| Context | Behavior |
|---|---|
| Interactive terminal | Runs `tmux attach -t sb-<workspace>`, replacing the `sb` process |
| Non-interactive (pipe, script, CI) | Prints the session name to stdout and exits 0, so callers can scrape it |

```console
$ sb attach | cat
sb-docdemo

$ sb attach
error: no tmux session "sb-ws1"; run `sb up` first
```

Unlike `sb down`, a missing session **is** an error here.

---

## 7. Passthrough arguments

`sb run` forwards everything after `<module>[:<id>]` into the start
command, verbatim and in order, where the command has `{args}`: after
the instance's own `args` from `sb.flow.yaml`, and before the
`--id <id>` that `sb` appends for an instance. The command is never
parsed by a shell, so a value with spaces stays one argument without
any quoting tricks.

With the default Rust start command (`{bin} {args}`; `{bin}` expands to
the absolute path, shortened here):

```bash
sb run cam                        # target/release/cam                     (no instances)
sb run cam --name "front cam"     # target/release/cam --name "front cam"  (two arguments)
sb run cam:left --exposure 4000   # target/release/cam --exposure 4000 --id left
sb run cam -- --help              # target/release/cam --help
```

Use `--` when an argument would otherwise be parsed as one of `sb`'s own
flags. `--help` and `--version` are the ones that actually bite, and
they bite quietly: `sb run cam --help` prints *`sb run`'s* help and exits
0 without launching anything. `sb run cam -- --help` launches the module
and passes `--help` through. The `--` itself is consumed and never
reaches the program.

The confirmation line joins the forwarded args with spaces for
readability, so `(args: --name front cam)` is one flag and one
two-word value, not three arguments.

A custom `start` command without `{args}` drops passthrough args (an
instance still gets its `--id`). `sb up` passes none. `sb stop` forwards
its args to `scripts.stop` the same way.

How the program turns `--id` (or `SB_INSTANCE_ID`) into a per-instance
wire key is in [sbcli_pubsub_consuming.md](sbcli_pubsub_consuming.md)
§3 and §10.

---

## 8. Exit codes: what they do and do not tell you

Every launch verb exits non-zero only for **its own** failures:
unresolvable workspace or module, a manifest that does not resolve, a
failed build, unusable tmux, a refused `sb up`. A zero exit means "the
builds passed and tmux was driven successfully". `sb run --fg` is the
exception: it exits with the program's own code.

It does **not** mean your module started or is healthy. Windows run
detached, and `sb` never waits on them. To check what actually
happened, look in the window:

```bash
sb attach                                                  # interactive
tmux capture-pane -p -t '=sb-<workspace>:=<window>'        # scriptable
```

A native window stays open after its program exits (§1), so a module
that crashed on startup still shows its window and its last output.

---

## 9. Failure modes

| Symptom | Cause | Fix |
|---|---|---|
| `no active workspace` | no `active` marker set | `sb ws create <name> && sb ws set <name>` |
| `module "x" not registered in workspace "y"` | module absent from `sb.flow.yaml` | `sb init` from the module root |
| `has no modules to launch` | every module is marked `up: false` (sb 0.5.0+) | `sb run <module>`, or set `up: true` in the repo `sb.flow.yaml` and reinstall |
| `warning: sb.flow.yaml at … is newer than the installed copy` | edited or pulled without reinstalling (sb 0.5.0+) | `sb install <repo>` or `sb ws update <name>` |
| a module listed in the repo `sb.flow.yaml` is not launched | the installed copy predates the edit, or the module has `up: false` | reinstall; check `up:` |
| `skipped <module>: …` on `sb up` | the manifest's `platforms:` does not list this host | expected; add the platform if the module does run here ([sbcli_module.md](sbcli_module.md) §6) |
| `sb up` stopped after a `building` line, no session | that module's build failed | fix the build; its output is above the error |
| `skipped <m>: unity modules have no start command …` | Unity modules launch from the editor | expected; `up: false` keeps the line away |
| `skipped <m>: instance <id> declares docker: …` | native launch does not start containers yet | [sbcli_module.md](sbcli_module.md) §11.6 |
| `has nothing to launch on <os>-<arch>` | every module that is not `up: false` was skipped | read the reasons in the message |
| `unknown placeholder {…}` | a typo in `scripts:` or `env:` | [sbcli_module.md](sbcli_module.md) §5 |
| a window shows `No such file or directory` | the start command names a binary that was not built under that name | override `scripts.start` ([sbcli_module.md](sbcli_module.md) §3.2) |
| the program never sees passthrough args | the custom `start` has no `{args}` | add `{args}` |
| `tmux session … already exists` on `sb up` | workspace is already up | `sb down` first, or `sb run <module>` |
| `tmux not found on PATH` | tmux/psmux not installed or not configured | install it, or set `tmux:` in `sb.config.yml`; `sb doctor` checks it |
| `sb stop` "worked" but the process is still alive | the real work is not the window's process | `scripts.stop` must reach it (container, unit, remote); see §4.1 |
| `scripts.stop` never ran during teardown | `sb down` runs none by design | `sb stop <module>` per module first (§5) |
| a module that used to start from its own launch script now starts the wrong program, or none | `sb 0.7.0+` never runs per-module launch scripts; the module starts from its manifest and the language defaults (§1.4) | declare `scripts:` ([sbcli_module.md](sbcli_module.md) §11.3) |
| `[WARN] deprecated` in `sb ws check` / `sb doctor` | files from before 0.6.0 are still in the workspace; they are ignored | [sbcli_module.md](sbcli_module.md) §11 |
| Two workspaces fighting over one session | they are not: sessions are per workspace | `tmux ls` shows `sb-<workspace>` for each |

---

## 10. Legacy modules (removed in 0.7.0)

Removed in 0.7.0: every module launches natively (§1.4); migrate a
module that still carries launch or stop scripts with
[sbcli_module.md](sbcli_module.md) §11.3 and §11.4.

---

## 11. Windows

`sb 0.6.0+` launches native modules on Windows. tmux's Windows
counterpart, psmux, is installed as the `tmux` command, and `sb` also
finds it as `psmux` on `PATH` when no `tmux` resolves. A native window
command is a program and its arguments, not a POSIX command line, which
is what psmux needs: the same `sb.module.yaml` launches on both
platforms, and per-OS differences go under `platforms.windows`
([sbcli_module.md](sbcli_module.md) §6).

Language defaults pick the Windows forms automatically: `{exe}` is `.exe`, a C++ binary
is looked up under `build/Release/`, and `{python}` falls back to
`python`.
