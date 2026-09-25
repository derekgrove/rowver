"""Check that .rvr files agree with themselves.

    rowver validate <file.rvr> [more.rvr ...]
"""
import hashlib

import awkward as ak
import numpy as np

from . import rvr


def check(what, ok):
    """Print one verdict."""
    print(f"  {'ok  ' if ok else 'FAIL'} {what}")
    return bool(ok)


def spans(data):
    """Every (offset, byte length) the header declares."""
    rows = data["rows"]
    out = [(data["sample_at"], 4 * rows)]
    for c in data["cols"]:
        if c["kind"] == "jagged":
            out += [(c["at"], 8 * (rows + 1)), (c["content"]["at"], int(c["content"]["kind"][1:]) * c["content"]["count"])]
        else:
            out.append((c["at"], int(c["kind"][1:]) * rows))
    return out


def validate(path):
    """Run every check on one file; return whether all passed."""
    print(path)
    header, buffer = rvr.read(path)
    data, made = header["data"], header["made"]
    rows = data["rows"]
    sample = rvr.span(buffer, data["sample_at"], "i4", rows)
    ok = check("rowver, data, made at the top level", header["rowver"] == rvr.VERSION and data and made)
    ok &= check("data sha256 matches the complete buffer",
                data.get("sha256") == hashlib.sha256(buffer).hexdigest())
    ok &= check("sample rows sum to rows", sum(s["rows"] for s in data["samples"]) == rows)
    ok &= check("sample index counts match", all((sample == i).sum() == s["rows"] for i, s in enumerate(data["samples"])))
    ok &= check("every span 8-aligned and inside the buffer",
                all(at % 8 == 0 and at + n <= len(buffer) for at, n in spans(data)))
    for c in data["cols"]:
        array = rvr.column(header, buffer, c["name"])
        if c["kind"] == "jagged":
            offsets = rvr.span(buffer, c["at"], "i8", rows + 1)
            ok &= check(f"{c['name']}: offsets start at 0, never decrease, end at count",
                        offsets[0] == 0 and (np.diff(offsets) >= 0).all() and offsets[-1] == c["content"]["count"])
            continue
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
    ok &= check("made file samples match data samples", set(made["files"]) == {s["name"] for s in data["samples"]})
    ok &= check("made files have paths, trees, UUIDs, and entry counts",
                all(path and record["tree"] and record["uuid"] and record["entries"] >= 0
                    for sample in made["files"].values() for path, record in sample["files"].items()))
    ok &= check("cart sha256 matches its source",
                made["cart"]["sha256"] == hashlib.sha256(made["cart"]["source"].encode()).hexdigest())
    ok &= check("made sha256 matches the complete snapshot", made.get("sha256") == rvr.made_checksum(made))
    print("  valid" if ok else "  INVALID")
    return ok

