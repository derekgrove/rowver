#!/usr/bin/env python
"""Build one RowVer 6 cartridge from ``studies/<name>/cart.py``."""
from __future__ import annotations

import argparse
from pathlib import Path

from cart import Cart, CartError, study_names
from rvr6 import FormatError


def parse(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("study", nargs="?", help="directory name under studies/")
    parser.add_argument("--list", action="store_true", help="list studies and exit")
    parser.add_argument("--out", type=Path, help="output .rvr path or directory")
    parser.add_argument("--step-size", default="100 MB",
                        help="Uproot chunk size, for example '100 MB' or an entry count")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse(argv)
    if args.list:
        print("\n".join(study_names()))
        return 0
    if not args.study:
        raise SystemExit("choose a study, or use --list")
    step: int | str = int(args.step_size) if str(args.step_size).isdigit() else args.step_size
    try:
        cart = Cart.load(args.study)
        print(cart.summary())
        result = cart.build(args.out, step_size=step)
    except (CartError, FormatError) as exc:
        raise SystemExit(f"error: {exc}") from exc
    print(f"wrote {result['file']} — {result['rows']:,} rows, "
          f"{result['compressed'] / 1024 / 1024:.1f} MiB")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
