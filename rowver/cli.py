"""The rowver command: run, check, probe, recipe, dump, validate."""
import argparse
import os
import sys

from . import __version__, check, dump, probe, recipe, run, validate


def main():
    """Parse the command line and hand off to the command named."""
    parser = argparse.ArgumentParser(prog="rowver")
    parser.add_argument("--version", action="version", version=f"rowver {__version__}")
    commands = parser.add_subparsers(dest="command", required=True)
    press = commands.add_parser("run", help="turn <study>/cart.py into <study>/<study>.rvr")
    press.add_argument("study")
    press.add_argument("--out", metavar="PATH", help="write the .rvr file here instead of <study>/<study>.rvr")
    press.add_argument("--parallel", nargs="?", const=os.cpu_count(), default=1, type=int, metavar="N")
    press.add_argument("--progress", choices=("table", "json"), default="table",
                       help="table for people (the default); json for one JSON object per line")
    press.add_argument("--onto", metavar="FILE", help="add the recipe's new columns to this cartridge, already "
                       "pressed from the study, instead of making every column again")
    press.add_argument("--plan", action="store_true", help="with --onto: only say which columns would be "
                       "added; read no sample file and write nothing")
    trying = commands.add_parser("check", help="try <study>/cart.py on the first 1,000 events of a sample's first file")
    trying.add_argument("study")
    which = trying.add_mutually_exclusive_group()
    which.add_argument("--sample", metavar="NAME", help="the sample to try (default: the first)")
    which.add_argument("--all", action="store_true", help="try every sample: one file open each")
    trying.add_argument("--parallel", nargs="?", const=os.cpu_count(), default=1, type=int, metavar="N")
    trying.add_argument("--json", action="store_true", help="one JSON object per line")
    looking = commands.add_parser("probe", help="list a ROOT file's trees and branches with their first values")
    looking.add_argument("file")
    looking.add_argument("--tree", metavar="T", help="only this tree")
    looking.add_argument("--json", action="store_true", help="one JSON object")
    reading = commands.add_parser("recipe", help="read <study>/cart.py for the app's recipe form, without running it")
    reading.add_argument("study")
    reading.add_argument("--json", action="store_true", help="one JSON object")
    dump_parser = commands.add_parser("dump", help="print the inside of an .rvr file, or only the parts named")
    dump_parser.add_argument("file")
    dump_parser.add_argument("--fileset", action="store_true", help="only the files that were read")
    dump_parser.add_argument("--cart", action="store_true", help="only the recipe")
    dump_parser.add_argument("--columns", action="store_true", help="only the column values")
    commands.add_parser("validate", help="check .rvr files").add_argument("files", nargs="+")
    args = parser.parse_args()
    if args.command == "run":
        sys.exit(run.main(args.study, args.parallel, args.out, args.progress == "json", args.onto, args.plan))
    elif args.command == "check":
        sys.exit(check.main(args.study, args.sample, args.all, args.parallel, args.json))
    elif args.command == "probe":
        sys.exit(probe.main(args.file, args.tree, args.json))
    elif args.command == "recipe":
        sys.exit(recipe.main(args.study, args.json))
    elif args.command == "dump":
        dump.main(args.file, args.fileset, args.cart, args.columns)
    else:
        sys.exit(0 if all([validate.validate(path) for path in args.files]) else 1)
