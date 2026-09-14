# swarmbotix examples

Real example apps demonstrating every (language, transport) cell L3
ships. These are **not** part of the test crate — they're working
projects you can run directly. The L3 heavy-gate tests
(`level3_e2e_*.rs`) drive these same examples to keep them honest.

Layout:

```
examples/
  rust_python_zenoh/       — TDD #9: Rust pubber, Python subber, Zenoh
    pubber_rust/
    subber_python/
    run.sh                 — workspace setup → scaffold → build → spawn → assert
  rust_rust_iceoryx/       — TDD #10: Rust pubber + Rust subber, iceoryx2
    pubber/
    subber/
    run.sh
  python_python_iceoryx/   — Python pubber + Python subber, iceoryx2 (typed ctypes)
    pubber/
    subber/
    run.sh
```

Each example's `run.sh` is self-contained: it points `SB_CONFIG` at a
tempdir, runs `sb ws create` / `sb init` / `sb pub add` / `sb sub add`
inside that tempdir, then runs the two ends and asserts they
exchanged the expected payload.

## Prerequisites

| Example                 | Needs                                                        |
| ----------------------- | ------------------------------------------------------------ |
| rust_python_zenoh       | `cargo` + conda base with `eclipse-zenoh` installed          |
| rust_rust_iceoryx       | `cargo` + a host that allows POSIX shared memory             |
| python_python_iceoryx   | conda base with `iceoryx2` installed                          |

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
```
