"""Compatibility shim: the reference implementation lives in rowver/rvr6.py.

``python rvr6.py check|show file.rvr`` keeps working; so does ``import rvr6``
from scripts that put this repository root on sys.path.
"""
from rowver.rvr6 import *            # noqa: F401,F403
from rowver.rvr6 import _main        # noqa: F401

if __name__ == "__main__":
    raise SystemExit(_main())
