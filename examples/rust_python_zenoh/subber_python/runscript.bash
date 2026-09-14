#!/usr/bin/env bash
# /home/el/Dropbox/Projects/swarmbotix/examples/rust_python_zenoh/subber_python/runscript.bash
# Invoked by `sb run` / `sb up`, or directly from any cwd. The `cd`
# below pins the working dir to this script's directory (= module
# root) so the commands below always run from the right place.
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"

# Replace the TODO block at the bottom with the command(s) that build
# and start this module. Uncomment one of the examples below — each is
# a runnable bash line — or write your own.

# cargo run --release                                # Rust
# python main.py                                    # Python
# cmake --build build && ./build/app                # C++
# flutter run -d linux                              # Flutter
# : 'no command — Unity launches from the editor'   # Unity

echo "TODO: edit /home/el/Dropbox/Projects/swarmbotix/examples/rust_python_zenoh/subber_python/runscript.bash to launch this module" >&2
exit 1
