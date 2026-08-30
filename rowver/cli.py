"""The ``rowver`` command: new / run / check / show / extract / render."""
from __future__ import annotations

import argparse
import contextlib
import importlib.resources
import shutil
import time
from pathlib import Path

from .press import Cart, CartError, study_names
from .rvr6 import FormatError, _main as rvr6_main


def _new(args: argparse.Namespace) -> int:
    dest = Path(args.study).expanduser()
    if dest.exists():
        raise SystemExit(f"error: {dest} already exists")
    template = importlib.resources.files("rowver") / "studies" / "template"
    with importlib.resources.as_file(template) as source:
        shutil.copytree(source, dest)
    print(f"new study at {dest} — edit {dest / 'cart.py'}, then: rowver run {dest}")
    return 0


def _run(args: argparse.Namespace) -> int:
    if args.list:
        print("\n".join(study_names()))
        return 0
    if not args.study:
        raise SystemExit("choose a study, or use --list")
    step: int | str = int(args.step_size) if str(args.step_size).isdigit() else args.step_size
    board = None
    try:
        # --path is a compatibility alias: Cart.load now takes paths directly
        cart = Cart.load(args.study)
        print(cart.summary())
        if args.live:
            from .dashboard import Dashboard
            board = Dashboard(cart)
            print("indexing files…")
            board.index(cart)          # totals for the bars and the ETA
            board.t0 = time.monotonic()
        with board.live() if board else contextlib.nullcontext():
            result = cart.build(args.out, step_size=step,
                                progress=board.chunk if board else None)
    except (CartError, FormatError) as exc:
        raise SystemExit(f"error: {exc}") from exc
    if board:
        board.draw(final=True)
    print(f"wrote {result['file']} — {result['rows']:,} rows, "
          f"{result['compressed'] / 1024 / 1024:.1f} MiB")
    return 0


def _extract(args: argparse.Namespace, extra: list[str]) -> int:
    from .extract import cutflow, flattened, plot1d
    kinds = {"flattened": flattened.main, "cutflow": cutflow.main,
             "plot1d": plot1d.main}
    if args.kind not in kinds:
        raise SystemExit(f"error: unknown extract kind {args.kind!r} — have: {sorted(kinds)}")
    return kinds[args.kind](extra)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="rowver", description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    new = sub.add_parser("new", help="scaffold a study directory from the template")
    new.add_argument("study", help="directory to create")

    run = sub.add_parser("run", help="run a study, produce the .rvr")
    run.add_argument("study", nargs="?",
                     help="a study directory path, or a name under ./studies/")
    run.add_argument("--path", action="store_true",
                     help=argparse.SUPPRESS)  # compatibility: paths now work directly
    run.add_argument("--list", action="store_true", help="list studies under ./studies/ and exit")
    run.add_argument("--out", type=Path, help="output .rvr path or directory")
    run.add_argument("--step-size", default="100 MB",
                     help="Uproot chunk size, for example '100 MB' or an entry count")
    run.add_argument("--live", action="store_true",
                     help="draw a live press dashboard while building")

    for name, blurb in (("check", "format validation"), ("show", "print the full v6 header")):
        p = sub.add_parser(name, help=blurb)
        p.add_argument("file", type=Path)

    extract = sub.add_parser("extract", help="flattened / cutflow / plots / tables / regions")
    extract.add_argument("kind", help="what to extract, e.g. flattened")

    sub.add_parser("render",
                   help="render extracted plot records to png/pdf",
                   add_help=False)

    args, extra = parser.parse_known_args(argv)
    if extra and args.command not in ("extract", "render"):
        parser.error(f"unrecognized arguments: {' '.join(extra)}")
    if args.command == "new":
        return _new(args)
    if args.command == "run":
        return _run(args)
    if args.command in ("check", "show"):
        return rvr6_main([args.command, str(args.file)])
    if args.command == "render":
        from .extract import render
        return render.main(extra)
    return _extract(args, extra)


if __name__ == "__main__":
    raise SystemExit(main())
