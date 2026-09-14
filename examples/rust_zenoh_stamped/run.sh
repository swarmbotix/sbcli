#!/usr/bin/env bash
# L4 end-to-end gate (TDD #3 + #9): a Rust pubber scaffolded by L3
# emits real protobuf `std/StringStamped` over Zenoh; `sb topic list`
# discovers it with schema + rate populated from the Header.
#
# Hermetic SB home + workspace; exits 0 only when the discovery output
# names the publisher's topic AND reports `std/StringStamped` + 10.0 Hz.
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
device: dev01
transport_on_device: iceoryx2
transport_cross_device: zenoh
EOF
export SB_CONFIG="$SB_HOME/sb.config.yml"

PUBBER="$EXAMPLE_DIR/pubber"

# 1. Workspace + adopt the pubber module.
"$SB" ws create demo
"$SB" ws set demo
"$SB" init --rust "$PUBBER" --force >/dev/null

# 2. Scaffold the L3 publisher. The example's main.rs `#[path]`-mounts
#    `swarmbotix_io/publishers/chatter.rs` produced by this command.
( cd "$PUBBER" && "$SB" pub add -m std/StringStamped chatter --zenoh --force )

# 3. Build the example so the run is fast. prost-build needs `protoc`
#    on PATH — surface the dev box's path explicitly so the build script
#    finds the same version `sb message compile` uses.
if [ -x /opt/protobuffer/protoc-35.0-linux-x86_64/bin/protoc ]; then
  export PROTOC=/opt/protobuffer/protoc-35.0-linux-x86_64/bin/protoc
elif command -v protoc >/dev/null 2>&1; then
  export PROTOC="$(command -v protoc)"
else
  echo "FAIL: protoc not found on PATH and dev-box default missing" >&2
  exit 2
fi
( cd "$PUBBER" && cargo build --quiet --bin pub_stamped )

# 4. Spawn the publisher.
PUB_LOG="$SB_HOME/pubber.log"
( cd "$PUBBER" && cargo run --quiet --bin pub_stamped ) >"$PUB_LOG" 2>&1 &
PUB_PID=$!
trap 'kill $PUB_PID 2>/dev/null || true; rm -rf "$SB_HOME"' EXIT

# Give Zenoh a moment to scout + the publisher's first put.
sleep 1.5

# 5. Run `sb topic list` and grep for the schema + rate columns. The
#    JSON form gives a stable key=value lookup; assert on both.
LIST_JSON="$SB_HOME/list.json"
"$SB" topic list --transport zenoh --json -t 1.0 > "$LIST_JSON" 2>/dev/null || true

# Zenoh key expressions don't carry a leading `/` — discovery reports
# `dev01/demo/pubber/chatter`, not `/dev01/...`. Both forms address the
# same wire topic.
TOPIC="dev01/demo/pubber/chatter"
ROW="$(grep -F "\"topic\":\"$TOPIC\"" "$LIST_JSON" || true)"

kill $PUB_PID 2>/dev/null || true
wait $PUB_PID 2>/dev/null || true

if [ -z "$ROW" ]; then
  echo "FAIL: $TOPIC missing from sb topic list output"
  echo "--- sb topic list --json ---"
  cat "$LIST_JSON"
  echo "--- pubber log ---"
  cat "$PUB_LOG"
  exit 1
fi
if ! echo "$ROW" | grep -q '"schema":"std/StringStamped"'; then
  echo "FAIL: schema column not populated from Header"
  echo "row: $ROW"
  exit 1
fi
if ! echo "$ROW" | grep -q '"rate_hz":10'; then
  echo "FAIL: rate column not 10.0 Hz from Header.msg_freq_desired"
  echo "row: $ROW"
  exit 1
fi
echo "PASS: sb topic list reports $TOPIC schema=std/StringStamped rate=10.0"
exit 0
