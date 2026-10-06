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


def print_cart(source, name="cart.py"):
    """The recipe, or the text that derived the columns, as Python text."""
    print(f"##### begin {name} #####")
    print(source, end="" if source.endswith("\n") else "\n")
    print(f"##### end {name} #####")


def print_columns(header, section):
    """The compressed size, one line per array, then one line per sample column."""
    data = header["data"]
    print(f"arrays {len(section)} bytes compressed")
    show("sample", rvr.array(section, data["sample"], "i4", data["rows"]))
    show("entry", rvr.array(section, data["entry"], "i8", data["rows"]))
    for record in data["cols"]:
        show(record["name"], rvr.column(header, section, record["name"]))
    names = [sample["name"] for sample in data["samples"]]
    for record in data.get("sample_cols", []):
        print(f"{record['name']:14} {record['kind']} by sample[{len(names)}]  "
              + ", ".join(f"{name}: {json.dumps(value, ensure_ascii=False)}"
                          for name, value in zip(names, record["values"])))


def main(path, fileset=False, cart=False, columns=False):
    """Print the parts asked for, or everything: header, recipe, arrays."""
    header, section = rvr.read(path)
    source = header["made"]["cart"]["source"]
    if not (fileset or cart or columns):
        header["made"]["cart"]["source"] = "[printed below as cart.py]"
        derived = header["derived"]["source"] if "derived" in header else None
        if derived is not None:
            header["derived"]["source"] = "[printed below as derived source]"
        print(json.dumps(header, indent=2))
        print()
        print_cart(source)
        print()
        if derived is not None:
            print_cart(derived, "derived source")
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
