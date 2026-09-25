"""Look inside one ROOT file without a recipe: `rowver probe <file> [--tree T] [--json]`.

Lists the file's trees with their entries, and each branch's type, whether it holds one value
per event (flat), a list per event (jagged) or something the viewer cannot plot (unsupported),
and its first three values.
"""
import math
import sys
import time

import awkward as ak
import numpy as np
import uproot

from . import terminal
from .run import describe

FIRST = 3
TREES = ("TTree", "TNtuple", "TNtupleD", "ROOT::RNTuple")
NUMBERS = ("bool", "int8", "uint8", "int16", "uint16", "int32", "uint32", "int64", "uint64", "float32", "float64")


class NoTree(Exception):
    """The file opened but has no tree of the name asked for."""


def kind(array):
    """flat, jagged or unsupported, and the type as text, for a branch read or a column a recipe made."""
    if isinstance(array, np.ndarray):
        name = array.dtype.name
        return ("flat" if array.ndim == 1 and name in NUMBERS else "unsupported"), \
            (name if array.ndim == 1 else f"{name} {array.shape[1:]}")
    if not isinstance(array, ak.Array):
        return "unsupported", type(array).__name__
    kind_of = ak.type(array).content
    text = str(kind_of)
    if isinstance(kind_of, ak.types.NumpyType) and kind_of.primitive in NUMBERS:
        return "flat", text
    if (isinstance(kind_of, (ak.types.ListType, ak.types.RegularType)) and kind_of.parameter("__array__") is None
            and isinstance(kind_of.content, ak.types.NumpyType) and kind_of.content.primitive in NUMBERS):
        return "jagged", text
    return "unsupported", text


def plain(value, short):
    """A value JSON can carry: NaN and infinities as text, float32 at its own precision."""
    if isinstance(value, list):
        return [plain(item, short) for item in value]
    if isinstance(value, float):
        if not math.isfinite(value):
            return str(value)
        return float(str(np.float32(value))) if short else value
    return value


def first(array, count=FIRST):
    """The first values of an array as plain JSON values."""
    head = array[:count]
    values = head.tolist() if isinstance(head, np.ndarray) else ak.to_list(head)
    primitive = head.dtype.name if isinstance(head, np.ndarray) else str(ak.type(head).content).split(" * ")[-1]
    return plain(values, primitive == "float32")


def branches(tree):
    """[{name, type, kind, first}] for every branch, read in one go where the file allows."""
    names = tree.keys()
    typenames = tree.typenames()
    unknown = uproot.interpretation.identify.UnknownInterpretation
    readable = [name for name in names if not isinstance(getattr(tree[name], "interpretation", None), unknown)]
    try:
        arrays = tree.arrays(readable, entry_stop=FIRST, how=dict)
    except Exception:
        arrays = {}
        for name in readable:
            try:
                arrays[name] = tree[name].array(entry_stop=FIRST)
            except Exception:
                pass
    out = []
    for name in names:
        entry = {"name": name, "type": typenames.get(name, ""), "kind": "unsupported", "first": []}
        if name in arrays:
            entry["kind"] = kind(arrays[name])[0]
            if entry["kind"] != "unsupported":
                entry["first"] = first(arrays[name])
        out.append(entry)
    return out


def probe(path, only=None):
    """The file's uuid and its trees, each with entries and branches."""
    started = time.monotonic()
    handle = uproot.open(path)
    found = [name for name, cls in handle.classnames(cycle=False).items() if cls in TREES]
    if only is not None:
        if only not in found:
            there = ", ".join(found) or "none"
            raise NoTree(f"This file has no tree named {only}. Its trees: {there}.")
        found = [only]
    trees = []
    for name in found:
        tree = handle[name]
        trees.append({"name": name, "entries": int(tree.num_entries), "branches": branches(tree)})
    return {"path": path, "uuid": str(handle.file.uuid), "trees": trees,
            "seconds": round(time.monotonic() - started, 3)}


def row(start, values):
    """One table row: the values cut at the end, with an ellipsis, to fit the terminal."""
    room = terminal.width() - len(start)
    if len(values) > room:
        values = values[:max(room - 1, 0)] + "…"
    return (start + values).rstrip()


def show(found):
    """The probe as a table for people."""
    print(f"{found['path']}\nuuid {found['uuid']}")
    if not found["trees"]:
        print("\nThis file has no trees.")
    for tree in found["trees"]:
        print(f"\n{tree['name']}: {tree['entries']:,} entries, {len(tree['branches']):,} branches")
        if not tree["branches"]:
            continue
        name_width = max(len(branch["name"]) for branch in tree["branches"])
        type_width = max(len(branch["type"]) for branch in tree["branches"])
        for branch in tree["branches"]:
            print(row(f"  {branch['name']:{name_width}}  {branch['type']:{type_width}}  {branch['kind']:11}  ",
                      ", ".join(map(str, branch["first"]))))


def main(path, tree=None, json=False):
    """Probe one file, opening it a second time if the first open fails; exit 1 when it cannot be read."""
    found, reason = None, None
    for _ in range(2):
        try:
            found = probe(path, tree)
            break
        except NoTree as error:
            reason = str(error)
            break
        except Exception as error:
            reason = describe(error)
    if found is None:
        if json:
            terminal.error("input", reason, "")
        else:
            print(reason, file=sys.stderr)
        return 1
    if json:
        terminal.emit({"event": "probe", **found})
    else:
        show(found)
    return 0
