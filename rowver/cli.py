"""The ``rowver`` command: new / run / check / show / extract / render."""
from __future__ import annotations

import argparse
import contextlib
import importlib.resources
import os
import shutil
from pathlib import Path

from .press import Cart, CartError, study_names
from .rvr6 import FormatError, _main as rvr6_main


def _positive_int(value: str) -> int | str:
    if value in ("auto", "max"):
        return value
    try:
        number = int(value)
    except ValueError as exc:
        raise argparse.ArgumentTypeError("must be a positive integer, 'max', or 'auto'") from exc
    if number < 1:
        raise argparse.ArgumentTypeError("must be a positive integer")
    return number


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
        parallel = getattr(args, "parallel", None)
        cores = max(1, os.cpu_count() or 1)
        if parallel is None:
            effective = 1
        else:
            runnable = sum(len(entry["files"]) for entry in cart.fileset.values())
            if parallel == "auto":
                parallel = min(runnable, max(1, cores - 2))
            elif parallel == "max":
                parallel = cores
            effective = min(parallel, runnable)
        message = (f"using {effective} {'worker' if effective == 1 else 'workers'} "
                   f"({cores} {'core' if cores == 1 else 'cores'} available)")
        if parallel is None:
            message += "; try --parallel for faster processing"
        elif effective == 1:
            message += "; running sequentially"
        print(message)
        workers = None if parallel is None else effective
        if args.progress != "plain":
            from .dashboard import Dashboard
            board = Dashboard(cart, robust=args.progress == "robust")
        with board.live() if board else contextlib.nullcontext():
            if board and board.robust:
                board.index(cart)      # totals for the bars and the ETA
                board.begin_skim()
            result = cart.build(args.out, step_size=step,
                                progress=board, workers=workers)
    except (CartError, FormatError) as exc:
        raise SystemExit(f"error: {exc}") from exc
    if board:
        board.draw(final=True)
    print(f"wrote {result['file']} — {result['rows']:,} rows, "
          f"{result['compressed'] / 1024 / 1024:.1f} MiB")
    return 0


def _extract(args: argparse.Namespace, extra: list[str]) -> int:
    from .extract import board, cutflow, flattened, plot1d, plot2d, regions
    kinds = {"flattened": flattened.main, "cutflow": cutflow.main,
             "plot1d": plot1d.main, "plot2d": plot2d.main,
             "plot3d": plot2d.main3d, "board": board.main,
             "regions": regions.main}
    if args.kind not in kinds:
        raise SystemExit(f"error: unknown extract kind {args.kind!r} — have: {sorted(kinds)}")
    return kinds[args.kind](extra)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="rowver", description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    new = sub.add_parser("new", help="scaffold a study directory from the template")
    new.add_argument("study", help="directory to create")

    run = sub.add_parser(
        "run", help="run a study, produce the .rvr",
        description=("Run a study. By default progress is live and one-pass; "
                     "sources are opened only by the real skim."),
        allow_abbrev=False,
    )
    run.add_argument("study", nargs="?",
                     help="a study directory path, or a name under ./studies/")
    run.add_argument("--path", action="store_true",
                     help=argparse.SUPPRESS)  # compatibility: paths now work directly
    run.add_argument("--list", action="store_true", help="list studies under ./studies/ and exit")
    run.add_argument("--out", type=Path, help="output .rvr path or directory")
    run.add_argument("--step-size", default="100 MB",
                     help=("chunk bound: uncompressed bytes of the branches the recipe "
                           "touches, for example '100 MB', or an entry count"))
    run.add_argument(
        "--parallel", nargs="?", const="auto", type=_positive_int, metavar="N|max",
        help=("process sources with N workers; max uses all logical CPUs; without a value "
              "uses all but two; capped by runnable sources"),
    )
    progress = run.add_mutually_exclusive_group()
    progress.add_argument(
        "--no-live", dest="progress", action="store_const", const="plain",
        help="plain one-pass execution with no dashboard",
    )
    progress.add_argument(
        "--live-robust", dest="progress", action="store_const", const="robust",
        help=("indexed live display with exact totals and ETA (opens sources twice; "
              "with --parallel, indexing stays sequential)"),
    )
    run.set_defaults(progress="fast")

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
