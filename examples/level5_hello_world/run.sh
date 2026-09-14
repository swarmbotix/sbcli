#!/usr/bin/env bash
# L5 end-to-end gate (level5.html TDD #9): proof-of-life for the full
# L1..L5 stack. Two-module workspace (Rust pubber + Rust subber over
# Zenoh) is created, scaffolded, launched via `sb up`, observed via
# `sb topic list`, frozen via `sb gopro --all`, torn down via `sb down`.
#
# Hermetic SB home + workspace. Exits 0 only when the subber's stdout
# (captured from tmux pipe-pane) contains the publisher's payload.
set -euo pipefail

cd "$(dirname "$0")"
EXAMPLE_DIR="$PWD"
ROOT="$(cd "$EXAMPLE_DIR/../.." && pwd)"
SB="$ROOT/target/debug/sb"

[ -x "$SB" ] || { echo "build sb first: cargo build --bin sb" >&2; exit 1; }
command -v tmux >/dev/null 2>&1 || { echo "SKIP: tmux not on PATH" >&2; exit 0; }

SB_HOME="$(mktemp -d)"
PUB_LOG="$SB_HOME/pubber.log"
SUB_LOG="$SB_HOME/subber.log"

cleanup() {
  # Kill the tmux session first so child processes get SIGTERM.
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

# protoc — both pubber + subber build.rs needs it on PATH (or PROTOC env).
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

echo "--- L2: workspace + adopt both modules ---"
"$SB" ws create demo
"$SB" ws set demo
"$SB" init --rust "$PUBBER" --force >/dev/null
"$SB" init --rust "$SUBBER" --force >/dev/null

# Replace the `sb init` runscript stubs (which exit 1) with real cargo run
# lines so `sb up` brings the processes alive.
cat > "$PUBBER/runscript.bash" <<EOF
#!/usr/bin/env bash
set -euo pipefail
cd -- "\$(dirname -- "\${BASH_SOURCE[0]}")"
exec cargo run --quiet --bin hello_pubber 2>&1 | tee "$PUB_LOG"
EOF
chmod +x "$PUBBER/runscript.bash"

cat > "$SUBBER/runscript.bash" <<EOF
#!/usr/bin/env bash
set -euo pipefail
cd -- "\$(dirname -- "\${BASH_SOURCE[0]}")"
exec cargo run --quiet --bin hello_subber 2>&1 | tee "$SUB_LOG"
EOF
chmod +x "$SUBBER/runscript.bash"

echo "--- L3: scaffold pub + sub on Zenoh ---"
( cd "$PUBBER" && "$SB" pub add -m std/StringStamped hello --zenoh --force )
( cd "$SUBBER" && "$SB" sub add -m std/StringStamped /dev01/demo/pubber/hello --zenoh --force )

echo "--- pre-build to make `sb up` fast ---"
( cd "$PUBBER" && cargo build --quiet --bin hello_pubber )
( cd "$SUBBER" && cargo build --quiet --bin hello_subber )

echo "--- L5: sb up ---"
"$SB" up
sleep 3

echo "--- L4: sb topic list (sanity check) ---"
LIST_JSON="$SB_HOME/list.json"
"$SB" topic list --transport zenoh --json -t 1.0 > "$LIST_JSON" 2>/dev/null || true
if ! grep -F '"topic":"dev01/demo/pubber/hello"' "$LIST_JSON" >/dev/null; then
  echo "FAIL: pubber not visible in sb topic list after sb up"
  echo "--- sb topic list ---"
  cat "$LIST_JSON"
  echo "--- pubber log ---"
  cat "$PUB_LOG" 2>/dev/null || echo "(no pubber log yet)"
  exit 1
fi

echo "--- wait up to 12 s for subber to receive a frame ---"
DEADLINE=$(( $(date +%s) + 12 ))
SAW=""
while [ $(date +%s) -lt $DEADLINE ]; do
  if grep -F "RX_HELLO data=HELLO_FROM_L5" "$SUB_LOG" >/dev/null 2>&1; then
    SAW=1
    break
  fi
  sleep 0.5
done
if [ -z "$SAW" ]; then
  echo "FAIL: subber never logged RX_HELLO"
  echo "--- subber log ---"
  cat "$SUB_LOG" 2>/dev/null || echo "(no subber log)"
  echo "--- pubber log ---"
  cat "$PUB_LOG" 2>/dev/null || echo "(no pubber log)"
  exit 1
fi

echo "--- L5: sb down ---"
"$SB" down
sleep 0.5
if tmux has-session -t sb-demo 2>/dev/null; then
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
