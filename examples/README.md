# swarmbotix examples

Real example apps demonstrating the (language, transport) cells sb
ships. These are **not** part of the test crate: they're working
projects you can run directly. The heavy-gate tests
(`crates/sb-cli/tests/level3_e2e_*.rs`, `level4_e2e_rust_stamped.rs`,
`level5_e2e_hello_world.rs`) drive these same examples to keep them
honest.

Layout:

```
examples/
  rust_python_zenoh/       : TDD #9: Rust pubber, Python subber, Zenoh
    pubber_rust/
    subber_python/
    run.sh                 : workspace setup → scaffold → build → spawn → assert
  rust_rust_iceoryx/       : TDD #10: Rust pubber + Rust subber, iceoryx2
    pubber/
    subber/
    run.sh
  python_python_iceoryx/   : Python pubber + Python subber, iceoryx2 (typed ctypes)
    pubber/
    subber/
    run.sh
  rust_zenoh_stamped/      : L4: Rust pubber on Zenoh, discovered by `sb topic list`
    pubber/
    run.sh
  level5_hello_world/      : L5: Rust pubber + Rust subber launched by `sb up`
    pubber/
    subber/
    run.sh
```

Each module folder holds its own sources, an `sb.module.yaml` (the module
manifest, sb 0.6.0+), and the generated `swarmbotix_io/`. There are no
launch scripts. The Python modules declare nothing about launching: their
entry point is `main.py`, which the Python default (`{python} main.py
{args}`) already starts. Each Rust module declares one line, because its
binary is not named after the module:

```yaml
scripts:
  start: "target/release/hello_pubber{exe} {args}"
```

Each example's `run.sh` is self-contained: it points `SB_CONFIG` at a
tempdir, runs `sb ws create` / `sb init` / `sb pub add` / `sb sub add`
inside that tempdir (the workspace's `sb.flow.yaml` is written there, not
here), then runs the two ends and asserts they exchanged the expected
payload. `level5_hello_world/run.sh` launches both ends with `sb up`,
which builds each module with `cargo build --release` and starts the
binary named in its manifest, then reads the subber's tmux window back
with `tmux capture-pane`.

## Prerequisites

| Example                 | Needs                                                        |
| ----------------------- | ------------------------------------------------------------ |
| rust_python_zenoh       | `cargo` + conda base with `eclipse-zenoh` installed          |
| rust_rust_iceoryx       | `cargo` + a host that allows POSIX shared memory             |
| python_python_iceoryx   | conda base with `iceoryx2` installed                          |
| rust_zenoh_stamped      | `cargo` + `protoc`                                           |
| level5_hello_world      | `cargo` + `protoc` + `tmux`                                  |

Install once into conda base:

```bash
source $(conda info --base)/etc/profile.d/conda.sh && conda activate base
pip install eclipse-zenoh iceoryx2
```

## Running

```bash
# Build sb first.
cargo build --bin sb

# Run any example end-to-end:
./examples/rust_python_zenoh/run.sh
./examples/rust_rust_iceoryx/run.sh
./examples/python_python_iceoryx/run.sh
./examples/rust_zenoh_stamped/run.sh
./examples/level5_hello_world/run.sh
```

To launch an example's modules by hand instead, adopt them into a workspace
and use the launch verbs; nothing in the module folders needs editing:

```bash
cd /tmp/demo && sb ws create demo
sb init --rust ~/path/to/sbcli/examples/level5_hello_world/pubber
sb init --rust ~/path/to/sbcli/examples/level5_hello_world/subber
sb up            # builds both, one tmux window each
sb attach
sb down
```
