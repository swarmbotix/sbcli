#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"
exec cargo run --quiet --bin hello_subber 2>&1 | tee "/tmp/tmp.Irin23RByM/subber.log"
