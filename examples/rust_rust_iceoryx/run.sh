#!/usr/bin/env bash
# Drive a full Rust ↔ Rust iceoryx2 round-trip via sb. Pubber writes a
# known `std/ImageStamped` to shared memory; subber prints it; we assert.
#
# Under the post-`<T>` API the codegen bakes the iox2-flat type
# (`swarmbotix_std::ImageStamped`, re-exported from the generated module)
# into both ends, so the user no longer ships a custom shared-payload
# crate; both binaries reference the codegen-emitted type directly.
set -euo pipefail

cd "$(dirname "$0")"
EXAMPLE_DIR="$PWD"
ROOT="$(cd "$EXAMPLE_DIR/../.." && pwd)"
SB="$ROOT/target/debug/sb"

[ -x "$SB" ] || { echo "build sb first: cargo build --bin sb" >&2; exit 1; }

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

# Compile the iox2-flat std vault into this sandbox's message_targets.
# The generated pub/sub files include! the resulting ImageStamped.rs via
# absolute path; if we skip this step the include! fails at compile time.
"$SB" message compile --iox2 std/ImageStamped

"$SB" init --rust "$PUBBER" --force >/dev/null
"$SB" init --rust "$SUBBER" --force >/dev/null

TOPIC_FULL="/dev01/demo/pubber/iox2/frame"
( cd "$PUBBER" && "$SB" pub add -m std/ImageStamped frame --iox2 --force )
( cd "$SUBBER" && "$SB" sub add -m std/ImageStamped "$TOPIC_FULL" --iox2 --force )

# Build both before we race them.
( cd "$PUBBER" && cargo build --quiet --bin pub_frame )
( cd "$SUBBER" && cargo build --quiet --bin sub_frame )

SUB_LOG="$SB_HOME/subber.log"
( cd "$SUBBER" && cargo run --quiet --bin sub_frame ) >"$SUB_LOG" 2>&1 &
SUB_PID=$!
trap 'kill $SUB_PID 2>/dev/null || true; rm -rf "$SB_HOME"' EXIT
sleep 0.5

( cd "$PUBBER" && cargo run --quiet --bin pub_frame )

DEADLINE=$(( $(date +%s) + 10 ))
while [ $(date +%s) -lt $DEADLINE ]; do
  if grep -q "FRAME ts_ns=1700000000" "$SUB_LOG" 2>/dev/null; then
    kill $SUB_PID 2>/dev/null || true
    echo "PASS: Rust subscriber received the publisher's ImageStamped"
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

echo "FAIL: Rust subscriber did not observe the publisher's ImageStamped"
echo "--- subber log ---"
cat "$SUB_LOG"
exit 1
