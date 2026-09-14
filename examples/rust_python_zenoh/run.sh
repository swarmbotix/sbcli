#!/usr/bin/env bash
# Drive a full Rust↔Python Zenoh round-trip via sb. Exits 0 only if the
# Python subscriber actually observes the Rust pubber's payload on the wire.
set -euo pipefail

cd "$(dirname "$0")"
EXAMPLE_DIR="$PWD"
ROOT="$(cd "$EXAMPLE_DIR/../.." && pwd)"
SB="$ROOT/target/debug/sb"

[ -x "$SB" ] || { echo "build sb first: cargo build --bin sb" >&2; exit 1; }

# Activate conda base so `python` resolves to the env that has
# `eclipse-zenoh` installed. The cargo test runner spawns bash directly
# without inheriting the user's interactive conda activation.
if command -v conda >/dev/null 2>&1; then
  source "$(conda info --base)/etc/profile.d/conda.sh"
  conda activate base
fi

# Hermetic SB home for this example so it can't collide with the user's
# real ~/.swarmbotix tree.
SB_HOME="$(mktemp -d)"
trap 'rm -rf "$SB_HOME"' EXIT
cat > "$SB_HOME/sb.config.yml" <<EOF
sb_home_dir: $SB_HOME
device: dev01
transport_on_device: iceoryx2
transport_cross_device: zenoh
EOF
export SB_CONFIG="$SB_HOME/sb.config.yml"

PUBBER="$EXAMPLE_DIR/pubber_rust"
SUBBER="$EXAMPLE_DIR/subber_python"

# 1. Workspace + adopt both modules.
"$SB" ws create demo
"$SB" ws set demo
"$SB" init --rust   "$PUBBER" --force >/dev/null
"$SB" init --python "$SUBBER" --force >/dev/null

# 2. Add the publisher / subscriber. Use --force so the example is
#    re-runnable without manual cleanup.
#
# Cross-module pub/sub needs an aligned topic. By default `sb pub add hello`
# scopes the topic under the publisher's module (`/dev01/demo/pubber_rust/hello`).
# The Python subscriber lives in a different module, so we pass the same
# fully-qualified topic to `sb sub add` — `sb` honours leading `/` verbatim.
TOPIC_FULL="/dev01/demo/pubber_rust/hello"
( cd "$PUBBER" && "$SB" pub add -m std/StringStamped hello --zenoh --force )
( cd "$SUBBER" && "$SB" sub add -m std/StringStamped "$TOPIC_FULL" --zenoh --force )

# 3. Spawn the Python subscriber.
PY_LOG="$SB_HOME/subber.log"
python "$SUBBER/main.py" >"$PY_LOG" 2>&1 &
SUB_PID=$!
trap 'kill $SUB_PID 2>/dev/null || true; rm -rf "$SB_HOME"' EXIT
# Zenoh scouting + declare can take 1–2 seconds on first session open.
# Wait long enough that the subscriber is on the wire before pubber starts.
sleep 2

# 4. Run the Rust publisher.
( cd "$PUBBER" && cargo run --quiet --bin pub_hello )

# 5. Wait for the subscriber to print the payload (or time out).
DEADLINE=$(( $(date +%s) + 16 ))
GOT=""
while [ $(date +%s) -lt $DEADLINE ]; do
  if grep -q "HELLO_FROM_RUST" "$PY_LOG" 2>/dev/null; then
    GOT="ok"
    break
  fi
  sleep 0.2
done
kill $SUB_PID 2>/dev/null || true
wait $SUB_PID 2>/dev/null || true

if [ "$GOT" = "ok" ]; then
  echo "PASS: Python subscriber observed Rust pubber's payload"
  exit 0
else
  echo "FAIL: timeout waiting for HELLO_FROM_RUST"
  echo "--- subber log ---"
  cat "$PY_LOG"
  exit 1
fi
