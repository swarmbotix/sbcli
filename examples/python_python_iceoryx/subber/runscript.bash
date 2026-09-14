#!/usr/bin/env bash
# /home/el/Dropbox/Projects/swarmbotix/examples/python_python_iceoryx/subber/runscript.bash
# Invoked by `sb run` / `sb up`, or directly from any cwd. The `cd`
# below pins the working dir to this script's directory (= module
# root) so the commands below always run from the right place.
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"

# Standalone tmux wrap: when this script is run directly (not from
# `sb up`, which already runs us inside its own tmux pane), re-exec
# inside a per-instance tmux session so the app is detachable and its
# output persists. `-A` attaches if the session is already running.
#
# Session name is `subber-dev` for the no-id case, and
# `subber-<id>-dev` when `--id <id>` is in "$@". This is what
# makes `bash runscript.bash --id 1` and `bash runscript.bash --id 2`
# launch as TWO independent processes on two distinct wire paths
# (`/.../subber-1/...` and `/.../subber-2/...`) rather than
# the second call silently reattaching to the first's session — which
# would discard the new tmux command and only ever run one binary.
# See documents/sbcli_pubsub_consuming.md §10 for the swarm pattern.
#
# After the launch command exits we pause for a keypress so build
# errors / crash output stay readable — tmux otherwise closes the
# pane the instant the inner bash exits and you'd never see why.
# The outer wrapper traps SIGINT so Ctrl+C inside the pane stops the
# app but does NOT kill the wrapper itself (otherwise the pane closes
# before you can read the exit message). The inner subshell resets
# the trap so the app receives Ctrl+C normally.
# Set `SB_NO_TMUX=1` to skip the wrap (CI, scripts, IDE runners).
# If tmux isn't on PATH, we fall through to inline.
if [[ -z "${TMUX:-}" ]] && [[ "${SB_NO_TMUX:-0}" != "1" ]] && command -v tmux >/dev/null 2>&1; then
  __sb_self="$PWD/$(basename "${BASH_SOURCE[0]}")"
  __sb_args=""
  for __arg in "$@"; do __sb_args="$__sb_args $(printf %q "$__arg")"; done
  __sb_id_suffix=""
  __sb_prev=""
  for __arg in "$@"; do
    if [[ "$__sb_prev" == "--id" ]]; then __sb_id_suffix="-$__arg"; break; fi
    __sb_prev="$__arg"
  done
  __sb_session="subber${__sb_id_suffix}-dev"
  exec tmux new -A -s "${__sb_session}" \
    "trap '' INT; (trap - INT; bash $(printf %q "${__sb_self}")${__sb_args}); ec=\$?; echo; echo \"[app exited \$ec — press any key to close pane]\"; read -n 1"
fi

# All "$@" args passed to this script are forwarded to the inner app.
# Use this to thread per-instance overrides through to your `main()` —
# e.g., `bash runscript.bash --id 2` lets your main parse `--id 2` and
# construct the generated publisher with `module = "<MODULE>-2"`, which
# shifts the wire path to `<device>/<ws>/<MODULE>-2/<transport>/<topic>`.
# The tmux-wrap above already extracts the same `--id` value so each
# instance lands in its own tmux session (`subber-2-dev`) — that
# way N invocations from N terminals truly run N processes side by
# side. See documents/sbcli_pubsub_consuming.md for per-language sketches.

# Replace the TODO block at the bottom with the command(s) that build
# and start this module. Uncomment one of the examples below — each is
# a runnable bash line — or write your own. End with `exec <cmd> "$@"`
# so per-instance flags reach your `main()`.

# cargo run --release -- "$@"                       # Rust
# python main.py "$@"                               # Python
# cmake --build build && exec ./build/app "$@"      # C++
# flutter run -d linux                              # Flutter
# : 'no command — Unity launches from the editor'   # Unity

echo "TODO: edit /home/el/Dropbox/Projects/swarmbotix/examples/python_python_iceoryx/subber/runscript.bash to launch this module" >&2
exit 1
