"""Hermetic release gate for the RowVer cartridge builder."""
from __future__ import annotations

import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent


def main() -> int:
    production = ["rowver/__init__.py", "rowver/press.py", "rowver/rvr6.py", "rowver/cli.py",
                  "rowver/builder_spec.py", "rowver/builder.py", "rowver/dashboard.py",
                  "rowver/extract/__init__.py", "rowver/extract/flattened.py",
                  "rowver/extract/core.py", "rowver/extract/cutflow.py",
                  "press.py", "cart.py", "rvr6.py", "run_rowver.py"]
    missing = [name for name in production if not (ROOT / name).is_file()]
    if missing:
        print(f"missing production files: {', '.join(missing)}", file=sys.stderr)
        return 1
    for name in production:
        compile((ROOT / name).read_text(encoding="utf-8"), name, "exec")
    # the default live path must IMPORT, not merely compile — a missing runtime
    # dependency in the dashboard would otherwise pass the gate
    import importlib
    sys.path.insert(0, str(ROOT))
    for name in ("rowver.press", "rowver.dashboard", "rowver.cli",
                 "rowver.extract.flattened", "rowver.extract.core",
                 "rowver.extract.cutflow"):
        importlib.import_module(name)
    print(f"syntax: {len(production)} production modules")

    suite = unittest.defaultTestLoader.discover(str(ROOT / "tests"))
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    if not result.wasSuccessful():
        return 1
    print(f"\nrelease gate: {result.testsRun} tests green")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
