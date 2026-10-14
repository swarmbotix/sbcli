#!/usr/bin/env bash
# L5 end-to-end gate (level5.html TDD #9): proof-of-life for the full
# L1..L5 stack. Two-module workspace (Rust pubber + Rust subber over
# Zenoh) is created, scaffolded, built and launched via `sb up`, observed
# via `sb topic list`, frozen via `sb gopro --all`, torn down via `sb down`.
#
# Native launch (sb 0.6.0+): there are no launch scripts. Each module's
# committed sb.module.yaml names its binary in scripts.start (the binary
# is not named after the module, so the Rust default does not apply);
# `sb up` runs the default `cargo build --release` for both modules, then
# starts each binary in its own tmux window.
#
# Hermetic SB home + workspace. Exits 0 only when the subber's window
# (read back with tmux capture-pane) shows the publisher's payload.
set -euo pipefail

cd "$(dirname "$0")"
EXAMPLE_DIR="$PWD"
ROOT="$(cd "$EXAMPLE_DIR/../.." && pwd)"
SB="$ROOT/target/debug/sb"

[ -x "$SB" ] || { echo "build sb first: cargo build --bin sb" >&2; exit 1; }
command -v tmux >/dev/null 2>&1 || { echo "SKIP: tmux not on PATH" >&2; exit 0; }

SB_HOME="$(mktemp -d)"

cleanup() {
  # Kill the tmux session first so child processes get SIGHUP.
  "$SB" down >/dev/null 2>&1 || true
  rm -rf "$SB_HOME"
}
trap cleanup EXIT

cat > "$SB_HOME/sb.config.yml" <<EOF
sb_home_dir: $SB_HOME
device: dev01
transport_on_device: iceoryx2
transport_cross_device: zenoh
tmux: $(command -v tmux)
EOF
export SB_CONFIG="$SB_HOME/sb.config.yml"

# protoc: both modules' build.rs need it on PATH (or PROTOC env). `sb up`
# runs the builds inline, in this environment, so exporting it here is enough.
if [ -x /opt/protobuffer/protoc-35.0-linux-x86_64/bin/protoc ]; then
  export PROTOC=/opt/protobuffer/protoc-35.0-linux-x86_64/bin/protoc
elif command -v protoc >/dev/null 2>&1; then
  export PROTOC="$(command -v protoc)"
else
  echo "FAIL: protoc not found on PATH and dev-box default missing" >&2
  exit 2
fi

PUBBER="$EXAMPLE_DIR/pubber"
SUBBER="$EXAMPLE_DIR/subber"

# What a module's window shows. Native windows stay open after their
# process exits, so a crash on startup is readable here too.
window_text() {
  tmux capture-pane -p -J -S - -t "=sb-demo:=$1" 2>/dev/null || true
}

echo "--- L2: workspace + adopt both modules ---"
# `sb ws create` writes ./sb.flow.yaml; keep it in the scratch home, not here.
mkdir -p "$SB_HOME/ws"
( cd "$SB_HOME/ws" && "$SB" ws create demo )
"$SB" ws set demo
# No --force: each committed sb.module.yaml (with its scripts.start) is
# adopted as is; --force would rewrite it without scripts.
"$SB" init --rust "$PUBBER" >/dev/null
"$SB" init --rust "$SUBBER" >/dev/null

echo "--- L3: scaffold pub + sub on Zenoh ---"
( cd "$PUBBER" && "$SB" io add pub -m ros2/std/StringStamped hello --zenoh --force )
( cd "$SUBBER" && "$SB" io add sub -m ros2/std/StringStamped --key dev01/demo/pubber/zenoh/hello --zenoh --force )

echo "--- L5: sb up (builds both modules, then launches them) ---"
"$SB" up
sleep 3

echo "--- L4: sb topic list (sanity check) ---"
LIST_JSON="$SB_HOME/list.json"
"$SB" topic list --transport zenoh --json -t 1.0 > "$LIST_JSON" 2>/dev/null || true
# `sb topic list` reports the raw wire key, no leading `/` (sb 0.5.0+).
if ! grep -F '"topic":"dev01/demo/pubber/zenoh/hello"' "$LIST_JSON" >/dev/null; then
  echo "FAIL: pubber not visible in sb topic list after sb up"
  echo "--- sb topic list ---"
  cat "$LIST_JSON"
  echo "--- pubber window ---"
  window_text pubber
  exit 1
fi

echo "--- wait up to 12 s for subber to receive a frame ---"
DEADLINE=$(( $(date +%s) + 12 ))
SAW=""
while [ $(date +%s) -lt $DEADLINE ]; do
  if window_text subber | grep -F "RX_HELLO data=HELLO_FROM_L5" >/dev/null 2>&1; then
    SAW=1
    break
  fi
  sleep 0.5
done
if [ -z "$SAW" ]; then
  echo "FAIL: subber never logged RX_HELLO"
  echo "--- subber window ---"
  window_text subber
  echo "--- pubber window ---"
  window_text pubber
  exit 1
fi

echo "--- L5: sb down ---"
"$SB" down
sleep 0.5
if tmux has-session -t "=sb-demo" 2>/dev/null; then
  echo "FAIL: sb-demo session still alive after sb down"
  exit 1
fi

echo "--- L5: sb gopro --all ---"
"$SB" gopro --all
for m in pubber subber; do
  if [ ! -f "$EXAMPLE_DIR/$m/sb.prd.yml" ]; then
    echo "FAIL: $m/sb.prd.yml not written"
    exit 1
  fi
  if grep -E "/(home|Users)/" "$EXAMPLE_DIR/$m/sb.prd.yml" >/dev/null; then
    echo "FAIL: $m/sb.prd.yml leaks host paths"
    cat "$EXAMPLE_DIR/$m/sb.prd.yml"
    exit 1
  fi
done

# Leave the prd.yml files in tree as a side-effect users can inspect.
echo "PASS: L1..L5 end-to-end alive (pub ↔ sub via Zenoh + sb up/down + sb gopro --all)"
exit 0
