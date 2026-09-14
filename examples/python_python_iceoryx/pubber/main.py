"""Python iceoryx2 publisher example. Sends a known `ImageStamped`
payload a few times so the subscriber has a chance to attach.

Post-`<T>` API: the generated module re-exports the iox2-flat
`ImageStamped` class loaded from the absolute path baked in by
`sb pub add`. No shared payload module is needed.
"""
import os
import sys
import time

THIS_DIR = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(THIS_DIR, "swarmbotix_io"))

from publishers.frame import FramePublisher, ImageStamped, open_node  # noqa: E402


def main() -> int:
    node = open_node()
    pub = FramePublisher(node)
    for seq in range(40):
        msg = ImageStamped()
        msg.header_timestamp_ns = 1_700_000_000_000_000_000 + seq
        pub.publish(msg)
        time.sleep(0.05)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
