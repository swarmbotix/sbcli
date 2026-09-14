#!/usr/bin/env bash
# Drive a full Python ↔ Python iceoryx2 round-trip via sb. Both ends use
# a shared `Frame` ctypes.Structure (stand-in for what `sb message compile
# --iox2` would emit). Exits 0 only if the subscriber actually receives
# the publisher's payload.
set -euo pipefail

cd "$(dirname "$0")"
EXAMPLE_DIR="$PWD"
ROOT="$(cd "$EXAMPLE_DIR/../.." && pwd)"
SB="$ROOT/target/debug/sb"

[ -x "$SB" ] || { echo "build sb first: cargo build --bin sb" >&2; exit 1; }

# Activate conda base so `python` resolves to the env that has
# `iceoryx2` installed. The test runner spawns bash directly without
# inheriting the user's interactive conda activation, so we do it here.
if command -v conda >/dev/null 2>&1; then
  source "$(conda info --base)/etc/profile.d/conda.sh"
  conda activate base
fi

SB_HOME="$(mktemp -d)"
trap 'rm -rf "$SB_HOME"' EXIT
cat > "$SB_HOME/sb.config.yml" <<EOF
sb_home_dir: $SB_HOME
messages_root: $SB_HOME/messages
message_style: ros2

device: dev01
transport_on_device: iceoryx2
transport_cross_device: zenoh
EOF
export SB_CONFIG="$SB_HOME/sb.config.yml"

PUBBER="$EXAMPLE_DIR/pubber"
SUBBER="$EXAMPLE_DIR/subber"

"$SB" ws create demo
"$SB" ws set demo

# Compile the iox2-flat std vault so the generated pub/sub files can
# importlib-load `ImageStamped.py` from the absolute path they bake in.
"$SB" message compile --iox2 std/ImageStamped

"$SB" init --python "$PUBBER" --force >/dev/null
"$SB" init --python "$SUBBER" --force >/dev/null

# Same trick as the Zenoh example: subscriber points at the publisher's
# fully-qualified topic so both ends land on the same iceoryx2 service.
TOPIC_FULL="/dev01/demo/pubber/iox2/frame"
( cd "$PUBBER" && "$SB" pub add -m std/ImageStamped frame --iox2 --force )
( cd "$SUBBER" && "$SB" sub add -m std/ImageStamped "$TOPIC_FULL" --iox2 --force )

# Spawn subber first so the service is open before pubber starts spraying.
SUB_LOG="$SB_HOME/subber.log"
python "$SUBBER/main.py" >"$SUB_LOG" 2>&1 &
SUB_PID=$!
trap 'kill $SUB_PID 2>/dev/null || true; rm -rf "$SB_HOME"' EXIT
sleep 0.5

python "$PUBBER/main.py"

# Wait for subber output (or its timeout).
DEADLINE=$(( $(date +%s) + 10 ))
while [ $(date +%s) -lt $DEADLINE ]; do
  if grep -q "FRAME ts_ns=1700000000" "$SUB_LOG" 2>/dev/null; then
    kill $SUB_PID 2>/dev/null || true
    echo "PASS: subscriber received the publisher's ImageStamped"
    echo "--- subber output ---"
    cat "$SUB_LOG"
    exit 0
  fi
  if ! kill -0 $SUB_PID 2>/dev/null; then
    break
  fi
  sleep 0.2
done
kill $SUB_PID 2>/dev/null || true
wait $SUB_PID 2>/dev/null || true

echo "FAIL: subscriber did not observe the publisher's Frame"
echo "--- subber log ---"
cat "$SUB_LOG"
exit 1
