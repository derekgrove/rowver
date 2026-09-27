"""Print the inside of an .rvr file: the header in full, then every array as text.

    rowver dump <file.rvr> [--fileset] [--cart] [--columns]
"""
import json

import awkward as ak
import numpy as np

from . import rvr


def first(array, n):
    """The first n values as a list, ending in ... when there are more."""
    text = str(array[:n].tolist())
    return text[:-1] + ", ...]" if len(array) > n else text


def show(name, array):
    """One line: name, kind, length, first values."""
    if isinstance(array, np.ndarray):
        print(f"{name:14} {rvr.kind(array.dtype)}[{len(array)}]  {first(array, 5)}")
    else:
        print(f"{name:14} jagged[{len(array)}]  {ak.type(array)}  {first(array, 3)}")


def print_cart(source):
    """The recipe as Python text."""
    print("##### begin cart.py #####")
    print(source, end="" if source.endswith("\n") else "\n")
    print("##### end cart.py #####")


def print_columns(header, section):
    """The compressed size, then one line per array."""
    data = header["data"]
    print(f"arrays {len(section)} bytes compressed")
    show("sample", rvr.array(section, data["sample"], "i4", data["rows"]))
    show("entry", rvr.array(section, data["entry"], "i8", data["rows"]))
    for record in data["cols"]:
        show(record["name"], rvr.column(header, section, record["name"]))


def main(path, fileset=False, cart=False, columns=False):
    """Print the parts asked for, or everything: header, recipe, arrays."""
    header, section = rvr.read(path)
    source = header["made"]["cart"]["source"]
    if not (fileset or cart or columns):
        header["made"]["cart"]["source"] = "[printed below as cart.py]"
        print(json.dumps(header, indent=2))
        print()
        print_cart(source)
        print()
        print_columns(header, section)
        return
    parts = []
    if fileset:
        parts.append(lambda: print(json.dumps(header["made"]["files"], indent=2)))
    if cart:
        parts.append(lambda: print_cart(source))
    if columns:
        parts.append(lambda: print_columns(header, section))
    for i, part in enumerate(parts):
        if i:
            print()
        part()
