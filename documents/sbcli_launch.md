# `sb up` / `run` / `stop` / `down` / `attach` — Launch Reference

Official reference for the launch verbs: how a workspace's modules are
started, restarted, torn down, and reattached under tmux.

This document is normative for behavior shipped at L5.

The canonical spec is `requirements.md` in the sbcli **source
repository** — a maintainer document that is not part of your install;
when it and this page disagree, it wins.

Related: [sbcli_ws.md](sbcli_ws.md) (the `flow.yaml` registry these verbs
read), [sbcli_init.md](sbcli_init.md) (which writes the `runscript.bash`
stub), [sbcli_docker_runscript.md](sbcli_docker_runscript.md) (the
docker-flavored runscript and its `--id` dispatch).

---

## 1. Conceptual model

`sb` does not supervise your processes. It owns exactly one thing: a
tmux session, and which script runs in which window of it.

```
tmux session  sb-<active_workspace>
├── window "alpha"   ← cd <alpha_root> && ./runscript.bash [args...]
├── window "beta"    ← cd <beta_root>  && ./runscript.bash
└── window "gamma"   ← cd <gamma_root> && ./runscript.bash
```

Four facts follow from that picture, and most surprises come from
forgetting one of them:

1. **The session name is `sb-<workspace>`**, derived from the active
   workspace marker. Switching workspaces (`sb ws set`) points every
   launch verb at a different session. Two workspaces can be up at once
   without colliding.
2. **One window per module**, named after the module. That is the unit
   of idempotency — `sb run` and `sb stop` both target the module's
   window, and both replace whatever is running in it.
3. **The launched command is a script, not a binary.** `sb` runs
   `cd <module_root> && ./<script> [args...]`, so the script's own
   `set -euo pipefail` and relative paths behave the same whether tmux
   spawned it or you ran it by hand.
4. **A window closes when its command exits.** A runscript that returns
   immediately leaves no window behind, which reads exactly like "the
   launch did nothing". Use `tmux set remain-on-exit on` while debugging.

Windows are used rather than panes so each module's output is
independently scrollable.

### 1.1 The two lifecycle scripts

| Script | Run by | Written by |
|---|---|---|
| `runscript.bash` | `sb up`, `sb run` | `sb init` writes a non-zero-exit stub; you fill it in |
| `stopscript.bash` | `sb stop` | **you** — `sb init` writes no stub |

Both live at the module root, next to `sb.dev.yml`. Both must exist and
be executable before the verb that needs them will run.

### 1.2 Preconditions common to every verb

Checked in this order, before tmux is touched:

1. **An active workspace exists.** Otherwise:
   `no active workspace — run \`sb ws create <name> && sb ws set <name>\` first`
2. **The module resolves through `flow.yaml`.** Module paths come from
   the registry, never from cwd, so every launch verb works from any
   directory.
3. **The required script exists** at the module root.
4. **tmux is resolvable** — the `tmux:` path in `sb.config.yml` if set,
   otherwise `tmux` on `PATH`. On Windows, `psmux` ships as the `tmux`
   command; nothing here is platform-specific. `sb doctor` validates it.

`sb up` is **plan-then-execute**: every module's preconditions are
checked before the first tmux call, so a workspace where one module is
missing its runscript launches *nothing* rather than a partial session.

---

## 2. `sb up`

```
sb up
```

Launch every module in the active workspace's `flow.yaml`, one window
each, in a detached session. No arguments, and no per-module passthrough
args — that is `sb run`'s job.

```console
$ sb up
launched 2 module(s) in tmux session "sb-docdemo"
  - alpha
  - beta
attach with: tmux attach -t sb-docdemo
```

**`sb up` refuses to touch an existing session.** Re-launching a
workspace is explicitly `sb down && sb up`:

```console
$ sb up
error: tmux session "sb-docdemo" already exists — `sb down` first, or use `sb run <module>` to restart a single pane
```

Other refusals:

```console
$ sb up
error: workspace "emptyws" has no modules in flow.yaml — run `sb init` to adopt one

$ sb up
error: module "beta": missing runscript at /path/beta/runscript.bash — `sb init` writes a stub; fill it in (or re-run `sb init --force`) before `sb up`
```

---

## 3. `sb run <module> [args...]`

```
sb run <MODULE> [ARGS]...
```

Launch a single module in the `sb-<workspace>` session, creating the
session if it does not exist yet. **Idempotent:** if a window already
exists for `<module>`, the command running in it is killed and the
runscript is respawned in the same window. There is never a duplicate
window for one module.

| Argument | Required | Meaning |
|---|---|---|
| `<MODULE>` | yes | Module name as registered in the active workspace's `flow.yaml` |
| `[ARGS]...` | no | Appended verbatim to `runscript.bash`'s argv |

```console
$ sb run alpha
launched alpha in tmux session "sb-docdemo"

$ sb run alpha --id sb02
restarted alpha in tmux session "sb-docdemo" (args: --id sb02)
```

The verb in the output distinguishes the two cases: `launched` for a new
window, `restarted` when an existing one was replaced.

Under the hood the replace path is `tmux respawn-window -k`, not
`kill-window` + `new-window` — killing the last window in a session also
kills the session (and, on default config, the tmux server), which would
break the immediately following create.

---

## 4. `sb stop <module> [args...]`

```
sb stop <MODULE> [ARGS]...
```

Stop a single module by running its `stopscript.bash`. `sb stop` is the
symmetric counterpart of `sb run`: same module resolution through
`flow.yaml`, same `sb-<workspace>` session, same window, same
idempotent respawn — only the script filename differs.

| Argument | Required | Meaning |
|---|---|---|
| `<MODULE>` | yes | Module name as registered in the active workspace's `flow.yaml` |
| `[ARGS]...` | no | Appended verbatim to `stopscript.bash`'s argv |

```console
$ sb stop alpha
stopping alpha in tmux session "sb-docdemo"

$ sb stop alpha --graceful
stopping (replaced existing window for) alpha in tmux session "sb-docdemo" (args: --graceful)
```

`(replaced existing window for)` means the module's window already held
a command — normally the runscript you are stopping. That is the
expected case for "stop after run", so it is reported without alarm.

### 4.1 What `sb stop` actually does — read this before writing a stopscript

**The running process is killed first, then the stopscript runs.** `sb
stop` reuses `sb run`'s respawn path, so `tmux respawn-window -k` tears
down whatever occupied the module's window and starts `stopscript.bash`
in its place. The stopscript is **not** a graceful signal delivered to a
still-running module.

The practical consequences:

- A stopscript cannot cleanly shut down the process `sb run` started —
  by the time it executes, that process is already gone. Write it to
  clean up what the kill left behind: stop containers, unmount devices,
  release hardware, remove PID/lock files, drain queues.
- If the module's real work runs somewhere else (a docker container, a
  systemd unit, a remote box), the stopscript is exactly the right tool
  — the killed thing was only the launcher.
- If you need in-process graceful shutdown, have the runscript trap
  signals itself; `sb stop` will not deliver one for you.

### 4.2 The stopscript is required, and `sb init` does not write one

```console
$ sb stop beta
error: module "beta": missing stopscript at /path/beta/stopscript.bash — `sb stop` requires a stopscript.bash next to the module's sb.dev.yml (mirror your runscript's launch with a teardown)
```

`sb init` writes a `runscript.bash` stub but deliberately no
`stopscript.bash` — there is no generic teardown to stub. Author one
yourself, `chmod +x` it, and put it at the module root:

```bash
#!/usr/bin/env bash
# stopscript.bash — teardown counterpart of runscript.bash.
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"

# Mirror whatever runscript.bash started outside this shell.
docker rm -f "sb_${1:-alpha}" 2>/dev/null || true
rm -f ./run/*.pid
echo "alpha stopped"
```

### 4.3 Two wrinkles worth knowing

- **`sb stop` will create the session if none exists.** It is a normal
  launch, so with no `sb-<workspace>` session running it makes one, runs
  the stopscript in it, and the session disappears again the moment the
  stopscript exits. The command succeeds and prints `stopping <module>
  …`; nothing is left behind. Running a teardown against an
  already-stopped workspace is therefore harmless, not an error.
- **The window disappears when the stopscript exits.** After a
  successful `sb stop alpha`, `tmux list-windows -t sb-<workspace>` no
  longer lists `alpha`. That is the window-closes-on-exit rule from §1,
  not a failure.

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
no tmux session "sb-docdemo" — nothing to do
```

Missing session is **not** an error — `sb down` exits 0 either way, so
it is safe in cleanup paths and CI teardown.

`sb down` is coarser than `sb stop`: it tears down the whole session in
one call and **runs no stopscripts at all**. If your modules need
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
error: no tmux session "sb-ws1" — run `sb up` first
```

Unlike `sb down`, a missing session **is** an error here.

---

## 7. Passthrough arguments

`sb run` and `sb stop` forward everything after `<module>` to the
script's argv, verbatim and in order. Each argument is shell-quoted
independently before it reaches tmux, so values containing spaces
survive both layers of shell parsing.

```bash
sb run cam_gige_ht                    # ./runscript.bash
sb run cam_gige_ht --id sb02          # ./runscript.bash --id sb02
sb run cam_gige_ht --name "front cam" # ./runscript.bash --name 'front cam'
sb run cam_gige_ht -- --help          # ./runscript.bash --help
```

Use `--` when an argument would otherwise be parsed as one of `sb`'s own
flags. `--help` and `--version` are the ones that actually bite, and they
bite quietly: `sb run alpha --help` prints *`sb run`'s* help and exits 0
without launching anything. `sb run alpha -- --help` launches the module
and passes `--help` through. The `--` itself is consumed and never
reaches the script.

The confirmation line joins the forwarded args with spaces for
readability, so `(args: --name front cam)` above is one flag and one
two-word value, not three arguments.

For this to be useful, the runscript must forward `"$@"` to the program
it launches:

```bash
exec ./target/release/<module> "$@"      # ← "$@" forwards --id sb02
```

`sb up` passes **no** args. A module that needs per-instance arguments
is launched with `sb run`, or reads its identity from the environment.
See [sbcli_docker_runscript.md](sbcli_docker_runscript.md) for the
`--id`-dispatch pattern that generated docker runscripts use, and
[sbcli_pubsub_consuming.md](sbcli_pubsub_consuming.md) §3 for how that
`--id` becomes a per-instance topic override.

---

## 8. Exit codes — what they do and do not tell you

Every launch verb exits non-zero only for **its own** failures:
unresolvable workspace or module, missing script, unusable tmux, a
refused `sb up`. A zero exit means "tmux was driven successfully".

It does **not** mean your module started, is healthy, or that the
stopscript succeeded. Those run detached inside tmux, and `sb` never
waits on them. To check what actually happened, look in the window:

```bash
sb attach                                  # interactive
tmux capture-pane -p -t sb-<workspace>:<module>   # scriptable
```

A module that crashes on startup takes its window with it, so an empty
`tmux list-windows` right after a successful `sb up` means the
runscripts exited, not that the launch failed.

---

## 9. Failure modes

| Symptom | Cause | Fix |
|---|---|---|
| `no active workspace` | no `active` marker set | `sb ws create <name> && sb ws set <name>` |
| `module "x" not registered in workspace "y"` | module absent from `flow.yaml` | `sb init` from the module root |
| `sb.dev.yml not found … did the module dir get moved?` | `flow.yaml` points at a stale path | re-`sb init` at the new location |
| `missing runscript … sb init writes a stub` | stub deleted, or never filled in | restore with `sb init --force`, then edit it |
| `missing stopscript … sb stop requires a stopscript.bash` | none was authored | write one (§4.2) — `sb init` never stubs it |
| `tmux session … already exists` on `sb up` | workspace is already up | `sb down` first, or `sb run <module>` for one pane |
| `tmux not found on PATH` | tmux/psmux not installed or not configured | install it, or set `tmux:` in `sb.config.yml`; `sb doctor` checks it |
| Windows vanish right after `sb up` | runscripts exited immediately | `tmux set remain-on-exit on`, then re-run to read the error |
| `sb stop` "worked" but the process is still alive | the real work is not the runscript's child | the stopscript must reach it (container, unit, remote); see §4.1 |
| Stopscripts never ran during teardown | `sb down` runs none by design | `sb stop <module>` per module first (§5) |
| Two workspaces fighting over one session | they are not — sessions are per workspace | `tmux ls` shows `sb-<workspace>` for each |
