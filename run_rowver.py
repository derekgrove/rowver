#!/usr/bin/env python
"""Compatibility shim for ``rowver run``."""
import sys

from rowver.cli import main

if __name__ == "__main__":
    raise SystemExit(main(["run", *sys.argv[1:]]))
