"""Check that .rvr files agree with themselves.

    rowver validate <file.rvr> [more.rvr ...] [--parent parent.rvr]
"""
import hashlib
import re
import struct
import zlib

import numpy as np

from . import rvr


def check(what, ok):
    """Print one verdict."""
    print(f"  {'ok  ' if ok else 'FAIL'} {what}")
    return bool(ok)


def decoded(section, record, kind, count):
    """One array's uncompressed bytes, or None when they do not decompress to the length the header declares."""
    try:
        raw = rvr.decode(section, record, rvr.dtype(kind).itemsize)
    except (zlib.error, ValueError):
        return None
    return raw if len(raw) == count * rvr.dtype(kind).itemsize else None


def tiled(data, section):
    """Whether the arrays fill the section end to end, in file order, with nothing between them."""
    at = 0
    for record, *_ in rvr.blocks(data):
        if record["at"] != at or record["size"] <= 0:
            return False
        at += record["size"]
    return at == len(section)


def sampled(label, record, array, bounds, steps=False):
    """Check an array's by_sample: one sha256 per sample, each of that sample's values."""
    return check(f"{label}: by_sample has one sha256 per sample, each matching its rows",
                 record.get("by_sample") == rvr.by_sample(array, bounds, steps))


def sha(value):
    """Whether a value is a sha256 as the header writes one."""
    return isinstance(value, str) and re.fullmatch("[0-9a-f]{64}", value) is not None


def shaped(derived):
    """Whether a derived object has every field, each of its kind, and exactly one parent."""
    if not isinstance(derived, dict):
        return False
    texts = all(isinstance(derived.get(key), str) for key in ("built", "by", "source", "runtime"))
    columns, parents = derived.get("columns"), derived.get("parents")
    if not (texts and sha(derived.get("sha256")) and isinstance(derived.get("parameters"), dict)
            and isinstance(columns, dict) and columns and all(sha(mark) for mark in columns.values())
            and isinstance(parents, list) and len(parents) == 1 and isinstance(parents[0], dict)):
        return False
    parent = parents[0]
    return (sha(parent.get("data_sha256")) and sha(parent.get("made_sha256")) and "derived_sha256" in parent
            and (parent["derived_sha256"] is None or sha(parent["derived_sha256"]))
            and type(parent.get("rows")) is int and isinstance(parent.get("path"), str)
            and isinstance(parent.get("read"), dict) and all(sha(v) for v in parent["read"].values()))


def derived_checks(header, parent):
    """The checks of a derived file: its derived object, and its parent when one is named."""
    data, made, derived = header["data"], header["made"], header["derived"]
    if not check("derived has sha256, built, by, source, columns, parameters, runtime and one parent",
                 shaped(derived)):
        return False
    recorded = derived["parents"][0]
    ok = check("derived columns name exactly the columns, in order",
               list(derived["columns"]) == [c["name"] for c in data["cols"]])
    ok &= check("made and rows are the ones derived records of the parent",
                made.get("sha256") == recorded["made_sha256"] and data["rows"] == recorded["rows"])
    ok &= check("derived sha256 matches the complete derived object", derived["sha256"] == rvr.made_checksum(derived))
    if parent is None:
        print("  --   the parent was not checked (name it with --parent)")
        return ok
    try:
        above = rvr.read_header(parent)
    except (OSError, ValueError, KeyError, struct.error) as error:
        return check(f"parent {parent} can be read: {error}", False)
    try:
        return ok & compared(above, data, made, derived, recorded)
    except (KeyError, TypeError, AttributeError) as error:
        return check(f"parent {parent} has the parts of a rowver file: missing {error}", False)


def compared(above, data, made, derived, recorded):
    """Check a derived file's record of its parent against the parent's header."""
    ok = True
    theirs = above["data"]
    ok &= check("parent: data sha256 is the one recorded", theirs.get("sha256") == recorded["data_sha256"])
    ok &= check("parent: made is this file's made, its sha256 the one recorded",
                above["made"] == made and above["made"].get("sha256") == recorded["made_sha256"])
    ok &= check("parent: derived sha256 is the one recorded, none for a parent that is not derived",
                above.get("derived", {}).get("sha256") == recorded["derived_sha256"])
    ok &= check("parent: rows, samples and files are this file's",
                (theirs["rows"], theirs["samples"], theirs["files"])
                == (recorded["rows"], data["samples"], data["files"]))
    ok &= check("parent: sample and entry arrays have this file's sha256s",
                (theirs["sample"]["sha256"], theirs["entry"]["sha256"])
                == (data["sample"]["sha256"], data["entry"]["sha256"]))
    held = {c["name"]: rvr.column_checksum(c) for c in theirs["cols"]}
    ok &= check("parent: every column read has the sha256 recorded",
                all(held.get(name) == mark for name, mark in recorded["read"].items()))
    ok &= check("parent: holds no column of this file's names", not set(held) & set(derived["columns"]))
    return ok


def validate(path, parent=None):
    """Run every check on one file; return whether all passed. A derived file is compared with
    its parent when that file is named."""
    print(path)
    try:
        with open(path, "rb") as f:
            raw = f.read()
        header, start = rvr.header_of(raw, path)
    except (OSError, ValueError, UnicodeDecodeError, struct.error) as error:
        check(f"a rowver {rvr.VERSION} file: {error}", False)
        print("  INVALID")
        return False
    section = memoryview(raw)[start:]
    data, made = header["data"], header["made"]
    rows = data["rows"]
    ok = check(f"rowver {rvr.VERSION}, data, made at the top level",
               header["rowver"] == rvr.VERSION and data and made)
    bounds = rvr.edges(data)
    ok &= check("arrays fill the file in order, end to end", tiled(data, section))
    arrays = []
    for record, kind, count, label in rvr.blocks(data):
        values = decoded(section, record, kind, count)
        ok &= check(f"{label}: decompresses to {count:,} values, sha256 matches",
                    values is not None and hashlib.sha256(values).hexdigest() == record["sha256"])
        arrays.append(np.frombuffer(values, rvr.dtype(kind), count) if values is not None else None)
    ok &= check("data sha256 matches the arrays' sha256s", data.get("sha256") == rvr.data_checksum(data))
    if any(a is None for a in arrays):
        print("  INVALID")
        return False
    sample, entry, arrays = arrays[0], arrays[1], iter(arrays[2:])
    ok &= check("sample rows sum to rows", sum(s["rows"] for s in data["samples"]) == rows)
    ok &= check("sample index runs through the samples in order",
                (np.diff(sample) >= 0).all() and all((sample == i).sum() == s["rows"]
                                                     for i, s in enumerate(data["samples"])))
    ok &= check("file rows sum to rows", sum(f["rows"] for f in data["files"]) == rows)
    ok &= sampled("entry", data["entry"], entry, bounds)
    names = [s["name"] for s in data["samples"]]
    start, files_ok = 0, True
    for f in data["files"]:
        record = made["files"].get(f["sample"], {}).get("files", {}).get(f["path"])
        span, entries = slice(start, start + f["rows"]), entry[start:start + f["rows"]]
        read = None if record is None else min(record["entries"], record.get("stop", record["entries"]))
        files_ok &= (record is not None and f["sample"] in names and (sample[span] == names.index(f["sample"])).all()
                     and len(np.unique(entries)) == len(entries)
                     and (not len(entries) or 0 <= entries.min() and entries.max() < read))
        start += f["rows"]
    ok &= check("each file's rows are its sample's, its entry numbers distinct and inside the entries read", files_ok)
    ok &= check("files match made's files",
                sorted((f["sample"], f["path"]) for f in data["files"])
                == sorted((s, p) for s, e in made["files"].items() for p in e["files"]))
    for c in data["cols"]:
        if c["kind"] == "jagged":
            offsets, content = next(arrays), next(arrays)
            ok &= check(f"{c['name']}: offsets start at 0, never decrease, end at count",
                        offsets[0] == 0 and (np.diff(offsets) >= 0).all() and offsets[-1] == c["content"]["count"])
            ok &= sampled(f"{c['name']} offsets", c["offsets"], offsets, np.minimum(bounds, rows), True)
            ok &= sampled(f"{c['name']} content", c["content"], content, offsets[np.minimum(bounds, rows)])
            continue
        array = next(arrays)
        ok &= sampled(c["name"], c, array, bounds)
        if array.dtype.kind == "f":
            ok &= check(f"{c['name']}: finite", np.isfinite(array).all())
        if rows:
            ok &= check(f"{c['name']}: lim is the observed min and max",
                        c["lim"] == [array.min().item(), array.max().item()])
        ok &= check(f"{c['name']}: nice encloses lim", c["nice"][0] <= c["lim"][0] and c["lim"][1] <= c["nice"][1])
        if "vals" in c:
            ok &= check(f"{c['name']}: vals are the distinct values", c["vals"] == [int(v) for v in np.unique(array)])
        if "names" in c:
            ok &= check(f"{c['name']}: names are members of vals", all(int(k) in c["vals"] for k in c["names"]))
    ok &= check("made has study, built, by, cart, files", all(k in made for k in ("study", "built", "by", "cart", "files")))
    ok &= check("made file samples match data samples", set(made["files"]) == set(names))
    ok &= check("made files carry each sample's metadata as plain values",
                all(isinstance(s.get("metadata"), dict) and rvr.plain(s["metadata"])
                    and all(isinstance(k, str) for k in s.get("metadata_omitted", []))
                    for s in made["files"].values()))
    ok &= check("made files have paths, trees, UUIDs, and entry counts",
                all(path and record["tree"] and record["uuid"] and record["entries"] >= 0
                    for sample in made["files"].values() for path, record in sample["files"].items()))
    ok &= check("cart sha256 matches its source",
                made["cart"]["sha256"] == hashlib.sha256(made["cart"]["source"].encode()).hexdigest())
    for path, module in made["cart"].get("modules", {}).items():
        ok &= check(f"module {path}: sha256 matches its source",
                    module["sha256"] == hashlib.sha256(module["source"].encode()).hexdigest())
    if "derived" in header:
        ok &= derived_checks(header, parent)
    elif "hooks" in made["cart"]:
        ok &= check("cart hooks mark keep and exactly the columns",
                    "keep" in made["cart"]["hooks"]
                    and list(made["cart"]["hooks"].get("columns", {})) == [c["name"] for c in data["cols"]])
    ok &= check("made sha256 matches the complete snapshot", made.get("sha256") == rvr.made_checksum(made))
    print("  valid" if ok else "  INVALID")
    return ok
