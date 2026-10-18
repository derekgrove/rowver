"""Add the recipe's new columns to a cartridge already pressed: `rowver run <study> --onto FILE`.

The columns the cartridge holds are not made again and their bytes are not touched. prepare and
keep run on the same files, and every file must give exactly the entry numbers the cartridge
holds for it, so the new columns sit on the same rows.
"""
import ast
import functools
import hashlib
import importlib.util
import inspect
import json
from pathlib import Path

import numpy as np

from . import rvr

AGAIN = "Press again to make the cartridge anew."


class Differs(Exception):
    """The recipe or the files are no longer what the cartridge was pressed from, so nothing can be added."""


class Nothing(Exception):
    """The recipe has no column the cartridge lacks."""


SIMPLE = (str, bytes, int, float, complex, bool, type(None))


def node_of(code):
    """The def or lambda a code object was compiled from, in its file's syntax tree; None when the
    file cannot be read or the function is not found in it."""
    try:
        tree = ast.parse(importlib.util.decode_source(Path(code.co_filename).read_bytes()))
    except (OSError, SyntaxError, ValueError):
        return None
    if code.co_name != "<lambda>":
        # a decorated function's first line is its first decorator's
        return next((node for node in ast.walk(tree)
                     if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and node.name == code.co_name
                     and code.co_firstlineno in [node.lineno, *(d.lineno for d in node.decorator_list)]), None)
    # the first instruction with a place in the text; the one before it stands at column 0 of the line
    line, column = next(((p[0], p[2]) for p in code.co_positions() if p[0] and p[3]), (code.co_firstlineno, 0))
    around = [node for node in ast.walk(tree) if isinstance(node, ast.Lambda) and node.lineno == code.co_firstlineno
              and (node.lineno, node.col_offset) <= (line, column) <= (node.end_lineno, node.end_col_offset)]
    # of lambdas one inside another, the outermost: it holds the others
    return min(around, key=lambda node: node.col_offset) if around else None


def names(code):
    """Every global name a code object reads, the functions written inside it included."""
    found = set(code.co_names)
    for inner in code.co_consts:
        if hasattr(inner, "co_names"):
            found |= names(inner)
    return found


def text(spec, seen):
    """What a hook is, as text: for a function, its code as Python writes it back from its syntax tree
    (so its comments, its layout and the line a lambda shares with other code are not part of it),
    then every plain value and every function of the study it names, and what it closes over."""
    if isinstance(spec, str):
        return f"branch {spec}"
    if isinstance(spec, functools.partial):
        return f"partial {text(spec.func, seen)} {spec.args!r} {sorted(spec.keywords.items())!r}"
    function = inspect.unwrap(spec) if callable(spec) else spec
    code = getattr(function, "__code__", None)
    if code is None:  # an object that is called: its class says what it is, and nothing says how
        kind = type(function)
        return f"{kind.__module__}.{kind.__qualname__}"
    if code in seen:
        return f"again {code.co_name}"
    seen.add(code)
    node = node_of(code)
    parts = [ast.unparse(node) if node is not None else f"{function.__module__}.{function.__qualname__}"]
    used = [(name, function.__globals__[name]) for name in sorted(names(code)) if name in function.__globals__]
    used += [(name, cell.cell_contents) for name, cell in zip(code.co_freevars, function.__closure__ or ())]
    for name, value in used:
        if isinstance(value, SIMPLE) or (isinstance(value, (tuple, list)) and all(isinstance(v, SIMPLE) for v in value)):
            parts.append(f"{name} = {value!r}")
        elif isinstance(value, dict) and all(isinstance(v, SIMPLE) for pair in value.items() for v in pair):
            parts.append(f"{name} = {sorted(value.items(), key=repr)!r}")
        elif own(value):
            parts.append(f"{name}: {text(value, seen)}")
    return "\n".join(parts)


def own(value):
    """Whether a value is a function of the study itself: written in a file, and not an installed one."""
    code = getattr(inspect.unwrap(value) if callable(value) else value, "__code__", None)
    if isinstance(value, functools.partial):
        return True
    return code is not None and "site-packages" not in Path(code.co_filename).parts and Path(code.co_filename).is_file()


def mark(spec):
    """What makes a column, or a hook, as a sha256: a branch by its name, a function by its text."""
    return hashlib.sha256(text(spec, set()).encode()).hexdigest()


def marks(cart):
    """made.cart.hooks: the mark of keep, of prepare when there is one, of the samples' metadata, which
    every hook is given, and of every column."""
    out = {"keep": mark(cart.keep)}
    if cart.prepare is not None:
        out["prepare"] = mark(cart.prepare)
    metadata = {sample: entry["metadata"] for sample, entry in cart.fileset.items()}
    out["metadata"] = hashlib.sha256(json.dumps(metadata, sort_keys=True, default=repr).encode()).hexdigest()
    out["columns"] = {name: mark(spec) for name, spec in cart.columns.items()}
    return out


def listed(names):
    """a, b and c"""
    names = list(names)
    return names[0] if len(names) == 1 else f"{', '.join(names[:-1])} and {names[-1]}"


def plan(cart, header):
    """The columns to add, the files to read as (sample, path, spec) in the cartridge's row order, and
    the recipe's files the cartridge was not pressed from; Differs when nothing can be added."""
    data, old = header["data"], header["made"]
    if "derived" in header:
        raise Differs("The cartridge holds columns derived from another cartridge, not pressed from a recipe, "
                      "so nothing can be added to it.")
    held = [c["name"] for c in data["cols"]]
    gone = [name for name in held if name not in cart.columns]
    if gone:
        raise Differs(f"The recipe no longer makes {listed(gone)}, which the cartridge holds. {AGAIN}")
    new = [name for name in cart.columns if name not in held]
    if not new:
        raise Nothing("The cartridge already holds every column of the recipe.")
    if "sample_cols" not in data:
        raise Differs(f"The cartridge has no sample columns: it was pressed before they existed. {AGAIN}")
    clash = [name for name in new if name in {c["name"] for c in data["sample_cols"]}]
    if clash:
        raise Differs(f"The cartridge holds {listed(clash)} as a sample column, made from the samples' metadata, "
                      f"and the recipe now makes a column of that name. {AGAIN}")
    before, now = old["cart"].get("hooks"), marks(cart)
    if before:
        changed = [hook for hook in ("keep", "prepare") if before.get(hook) != now.get(hook)]
        if before.get("metadata") != now["metadata"]:
            changed.append("the samples' metadata")
        changed += [f"the column {name}" for name in held if before["columns"].get(name) != now["columns"][name]]
        if changed:
            raise Differs(f"{listed(changed)} changed since the cartridge was pressed. {AGAIN}")
    jobs, skipped = [], []
    for sample, entry in cart.fileset.items():
        records = old["files"].get(sample, {}).get("files", {})
        for path, spec in entry["files"].items():
            if path not in records:
                skipped.append((sample, path))
                continue
            tree, stop = (spec, None) if isinstance(spec, str) else (spec["tree"], spec["stop"])
            if (tree, stop) != (records[path]["tree"], records[path].get("stop")):
                raise Differs(f"The recipe reads {path} differently now (its tree, or where it stops). {AGAIN}")
            jobs.append((sample, path, spec))
    if [job[:2] for job in jobs] != [(f["sample"], f["path"]) for f in data["files"]]:
        raise Differs(f"The recipe no longer reads the cartridge's files in the order it was pressed from. {AGAIN}")
    return new, jobs, skipped


def proven(header, section, pieces):
    """Refuse unless every file is the file the cartridge was pressed from and gave the same rows."""
    data, made = header["data"], header["made"]
    entry, at = rvr.array(section, data["entry"], "i8", data["rows"]), 0
    read = iter([(path, record, entries) for sample in pieces for _, files, entries in pieces[sample]
                 for path, record in files.items()])
    for file in data["files"]:
        path, record, entries = next(read)
        if record["uuid"] != made["files"][file["sample"]]["files"][path]["uuid"]:
            raise Differs(f"{path} is not the file the cartridge was pressed from. {AGAIN}")
        if not np.array_equal(entries, entry[at:at + file["rows"]]):
            raise Differs(f"keep and prepare now choose other events in {path}: {len(entries):,} rows, where "
                          f"the cartridge holds {file['rows']:,}, or the same number in another order. {AGAIN}")
        at += file["rows"]


def merge(header, section, order, labels, fresh, arrays):
    """The data section and the arrays of the cartridge with the fresh columns among its own, in the
    recipe's order. An array already held moves as the bytes it is; names follow the recipe's labels."""
    fresh_section = b"".join(arrays)
    source = {c["name"]: (c, section) for c in header["data"]["cols"]}
    source.update({c["name"]: (c, fresh_section) for c in fresh["cols"]})
    data, out, end = dict(header["data"]), [], [0]

    def move(record, bytes_from):
        """One array after the ones before it; its record with the new place."""
        out.append(bytes(bytes_from[record["at"]:record["at"] + record["size"]]))
        end[0] += record["size"]
        return {**record, "at": end[0] - record["size"]}

    data["sample"], data["entry"] = move(data["sample"], section), move(data["entry"], section)
    data["cols"] = []
    for name in order:
        column, held = source[name]
        if column["kind"] == "jagged":
            column = {**column, "offsets": move(column["offsets"], held), "content": move(column["content"], held)}
        else:
            column = move(column, held)
            column.pop("names", None)
            if "vals" in column and labels.get(name):
                column["names"] = {str(v): labels[name][v] for v in column["vals"] if v in labels[name]}
        data["cols"].append(column)
    data["sha256"] = rvr.data_checksum(data)
    return data, out
