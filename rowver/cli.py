"""The rowver command: run, dump, validate."""
import argparse
import os
import sys

from . import __version__, dump, run, validate


def main():
    """Parse the command line and hand off to run, dump, or validate."""
    parser = argparse.ArgumentParser(prog="rowver")
    parser.add_argument("--version", action="version", version=f"rowver {__version__}")
    commands = parser.add_subparsers(dest="command", required=True)
    press = commands.add_parser("run", help="turn <study>/cart.py into <study>/<study>.rvr")
    press.add_argument("study")
    press.add_argument("--out", metavar="PATH", help="write the .rvr file here instead of <study>/<study>.rvr")
    press.add_argument("--parallel", nargs="?", const=os.cpu_count(), default=1, type=int, metavar="N")
    press.add_argument("--progress", choices=("table", "json"), default="table",
                       help="table for people (the default); json for one JSON object per line")
    dump_parser = commands.add_parser("dump", help="print the inside of an .rvr file, or only the parts named")
    dump_parser.add_argument("file")
    dump_parser.add_argument("--fileset", action="store_true", help="only the files that were read")
    dump_parser.add_argument("--cart", action="store_true", help="only the recipe")
    dump_parser.add_argument("--columns", action="store_true", help="only the column values")
    commands.add_parser("validate", help="check .rvr files").add_argument("files", nargs="+")
    args = parser.parse_args()
    if args.command == "run":
        sys.exit(run.main(args.study, args.parallel, args.out, args.progress == "json"))
    elif args.command == "dump":
        dump.main(args.file, args.fileset, args.cart, args.columns)
    else:
        sys.exit(0 if all([validate.validate(path) for path in args.files]) else 1)
