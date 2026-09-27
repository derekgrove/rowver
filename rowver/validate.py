"""Check that .rvr files agree with themselves.

    rowver validate <file.rvr> [more.rvr ...]
"""
import hashlib
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


def validate(path):
    """Run every check on one file; return whether all passed."""
    print(path)
    with open(path, "rb") as f:
        raw = f.read()
    try:
        header, start = rvr.header_of(raw, path)
    except (ValueError, UnicodeDecodeError) as error:
        check(f"a rowver {rvr.VERSION} file: {error}", False)
        print("  INVALID")
        return False
    section = memoryview(raw)[start:]
    data, made = header["data"], header["made"]
    rows = data["rows"]
    ok = check(f"rowver {rvr.VERSION}, data, made at the top level",
               header["rowver"] == rvr.VERSION and data and made)
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
            offsets, _ = next(arrays), next(arrays)
            ok &= check(f"{c['name']}: offsets start at 0, never decrease, end at count",
                        offsets[0] == 0 and (np.diff(offsets) >= 0).all() and offsets[-1] == c["content"]["count"])
            continue
        array = next(arrays)
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
    ok &= check("made files have paths, trees, UUIDs, and entry counts",
                all(path and record["tree"] and record["uuid"] and record["entries"] >= 0
                    for sample in made["files"].values() for path, record in sample["files"].items()))
    ok &= check("cart sha256 matches its source",
                made["cart"]["sha256"] == hashlib.sha256(made["cart"]["source"].encode()).hexdigest())
    for path, module in made["cart"].get("modules", {}).items():
        ok &= check(f"module {path}: sha256 matches its source",
                    module["sha256"] == hashlib.sha256(module["source"].encode()).hexdigest())
    ok &= check("made sha256 matches the complete snapshot", made.get("sha256") == rvr.made_checksum(made))
    print("  valid" if ok else "  INVALID")
    return ok
