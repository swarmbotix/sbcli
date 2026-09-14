"""Python iceoryx2 subscriber example. Polls the service for samples,
prints the first one that arrives, exits 0.

Post-`<T>` API: the generated module re-exports the iox2-flat
`ImageStamped` class loaded from the absolute path baked in by
`sb sub add`. No shared payload module is needed.
"""
import os
import sys
import time

THIS_DIR = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(THIS_DIR, "swarmbotix_io"))

from subscribers.frame import FrameSubscriber, open_node  # noqa: E402


def main() -> int:
    node = open_node()
    sub = FrameSubscriber(node)
    deadline = time.time() + 8.0
    while time.time() < deadline:
        msg = sub.try_recv_latest()
        if msg is not None:
            sys.stdout.write(f"FRAME ts_ns={msg.header_timestamp_ns}\n")
            sys.stdout.flush()
            return 0
        time.sleep(0.05)
    sys.stderr.write("timeout waiting for sample\n")
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
