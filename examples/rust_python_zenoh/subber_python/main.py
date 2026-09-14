"""Example subber driven by examples/rust_python_zenoh/run.sh.

Reads the subscriber class scaffolded by `sb sub add` (lives under
swarmbotix_io/subscribers/hello.py), polls for samples, prints the
payload, and exits 0 on first hit.
"""
import os
import sys
import time

import zenoh

# `swarmbotix_io/subscribers/hello.py` is the generated subscriber.
THIS_DIR = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(THIS_DIR, "swarmbotix_io"))

from subscribers.hello import HelloSubscriber  # noqa: E402


def main() -> int:
    with zenoh.open(zenoh.Config()) as session:
        sub = HelloSubscriber(session)
        # Listen for up to 15 seconds — Zenoh discovery + the pubber's
        # 5-second publish loop fits comfortably in that window.
        deadline = time.time() + 15.0
        while time.time() < deadline:
            payload = sub.try_recv_latest()
            if payload is not None:
                sys.stdout.write(payload.decode("utf-8", "replace"))
                sys.stdout.flush()
                return 0
            time.sleep(0.05)
    sys.stderr.write("timeout waiting for sample\n")
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
