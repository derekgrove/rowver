"""The `rvr` command, as `pip install rowver` installs it."""

import os
import signal
import sys

from ._rowver import quiet_on_closed_pipe, rvr


def main():
    """Run rvr with this process's arguments and exit with its status."""
    # rvr runs in Rust, out of Python's reach: let Ctrl-C end it at once, as it ends the binary.
    signal.signal(signal.SIGINT, signal.SIG_DFL)
    quiet_on_closed_pipe()
    # As the system gave them: a name that is not UTF-8 is refused by rvr, as by the binary.
    sys.exit(rvr([os.fsencode(arg) for arg in sys.argv[1:]]))
