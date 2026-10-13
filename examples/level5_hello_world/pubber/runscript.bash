#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"
exec cargo run --quiet --bin hello_pubber 2>&1 | tee "/tmp/tmp.K8BrG70ViS/pubber.log"
