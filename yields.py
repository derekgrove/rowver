"""Dump per-(bin, process) yields from a .rvr in a combine-style flattened.json shape.

    python yields.py file.rvr bins.json [--tab 0] [--w2 COLUMN] [--tab-cuts]
                                        [--allow-unset-lumi]

bins.json maps bin name -> list of cuts, each ["column", op, value] with op one of
< <= > >= == in ("in" takes a list). A bin's cuts are ANDed; NaN fails every
comparison on both numpy and RDataFrame, so the two agree on NaN by construction.

Output (stdout): {bin: {process: {"nominal": [raw_count, weighted, error]}}} with
4-space indent and lexically sorted keys — the shape the C++ fitting framework's
merge step writes. A process is a GROUP from the file's own `view.groups`.

TWO RULES THIS FILE MUST NOT GET WRONG, because both are silent when they are:

  A SAMPLE THAT DECLARES NO WEIGHTS IS REAL DATA. Its rows are counts, and no
  luminosity ever multiplies them. `data.samples[].weighted` says so per sample; it is
  read here rather than inferred, because a sample whose weights all happen to be 1.0 is
  still MC and must still be scaled.

  THE OUTERMOST LUMINOSITY WINS — group, then subgroup, then sample. `rvr6.lumi_of` is
  the one implementation; this file does not carry a second copy of the ladder.

A weighted sample that resolves to no luminosity anywhere falls back to 1, which is a
silent default of exactly the kind that produced the "x1 tab in a x400 study" trap. Such
a sample is REFUSED unless --allow-unset-lumi is passed deliberately.

--tab-cuts additionally applies the chosen tab's saved rail cuts before any bin.
"""
import argparse
import json
import math
import sys
from pathlib import Path

import numpy as np

import rvr6

OPS = {"<": np.less, "<=": np.less_equal, ">": np.greater,
       ">=": np.greater_equal, "==": np.equal}


def cut_mask(columns: dict, rows: int, cuts: dict) -> np.ndarray:
    """A tab's saved rail cuts, in the format's own grammar: [lo, hi] or {"in": [...]}."""
    keep = np.ones(rows, bool)
    for name, spec in (cuts or {}).items():
        assert name in columns, f"the cut names {name!r}, which this file has no column for"
        v = columns[name]
        if isinstance(spec, dict):
            keep &= np.isin(v, spec["in"])
        else:
            lo, hi = spec
            if lo is not None:
                keep &= v >= lo
            if hi is not None:
                keep &= v <= hi
    return keep


def bin_mask(columns: dict, cuts: list) -> np.ndarray:
    keep = None
    for col, op, value in cuts:
        assert col in columns, f"no column {col!r} — have: {sorted(columns)}"
        if op == "in":
            m = np.isin(columns[col], value)
        else:
            assert op in OPS, f"unknown op {op!r} — have: {sorted(OPS)} and 'in'"
            m = OPS[op](columns[col], value)
        keep = m if keep is None else keep & m
    return keep if keep is not None else np.ones(len(next(iter(columns.values()))), bool)


def dump(rvr: str | Path, bins: dict, tab: int = 0, w2: str | None = None,
         tab_cuts: bool = False, allow_unset_lumi: bool = False) -> dict:
    meta, buf = rvr6.read(rvr)
    data, view = meta["data"], meta.get("view") or {}
    columns = rvr6.columns(meta, buf)
    names = [s["name"] for s in data["samples"]]
    weighted = {s["name"]: s["weighted"] for s in data["samples"]}
    rows = data["rows"]

    weights_name = data.get("weight_col")
    assert weights_name, "this file has no weights column — nothing to scale"
    ds_of = np.frombuffer(buf, np.int32, rows, data["sample_at"])

    # an UNWEIGHTED sample is data: its rows are counts and its luminosity is 1, whatever
    # any group it sits in says. That is not a fallback, it is the rule.
    lumi = {n: (rvr6.lumi_of(n, view) if weighted[n] else 1.0) for n in names}
    unset = [n for n in names
             if weighted[n] and rvr6.lumi_of(n, view) == 1.0 and not _named(n, view)]
    assert allow_unset_lumi or not unset, (
        f"{len(unset)} weighted sample(s) resolve to no luminosity anywhere and fall back "
        f"to 1, e.g. {unset[:3]} — a silent default is the x1-in-a-x400-study trap. Give "
        f"their group an entry in view.lumis, or pass --allow-unset-lumi to accept it.")

    of = (view.get("groups") or {}).get("of") or {}
    w = columns[weights_name]
    w2col = columns[w2] if w2 else w * w
    lumi_row = np.array([lumi[n] for n in names])[ds_of]
    wl, w2l = w * lumi_row, w2col * lumi_row * lumi_row
    group_row = np.array([of.get(n, n) for n in names], dtype=object)[ds_of]

    tab_entry = (meta.get("tabs") or [{}])[tab] if (meta.get("tabs") or []) else {}
    base = (cut_mask(columns, rows, rvr6.effective(tab_entry, view, "cuts"))
            if tab_cuts else np.ones(rows, bool))
    groups = sorted(set(of.get(n, n) for n in names))
    out = {}
    for name in sorted(bins):
        m = base & bin_mask(columns, bins[name])
        out[name] = {g: {"nominal": [float(int((m & (group_row == g)).sum())),
                                     float(wl[m & (group_row == g)].sum()),
                                     math.sqrt(float(w2l[m & (group_row == g)].sum()))]}
                     for g in groups}
    return out


def _named(sample: str, view: dict) -> bool:
    """Whether any tier actually names a luminosity for this sample, rather than defaulting."""
    lumis, groups = view.get("lumis") or {}, view.get("groups") or {}
    return any((lumis.get(tier) or {}).get(key) is not None
               for tier, key in (("group", (groups.get("of") or {}).get(sample)),
                                 ("sub", (groups.get("sub") or {}).get(sample)),
                                 ("sample", sample)))


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("rvr")
    ap.add_argument("bins", help="json: {bin name: [[column, op, value], ...]}")
    ap.add_argument("--tab", type=int, default=0)
    ap.add_argument("--w2", default=None,
                    help="per-event weight^2 column for the error term")
    ap.add_argument("--tab-cuts", action="store_true",
                    help="apply the tab's saved rail cuts before every bin")
    ap.add_argument("--allow-unset-lumi", action="store_true")
    a = ap.parse_args()
    out = dump(a.rvr, json.load(open(a.bins)), tab=a.tab, w2=a.w2,
               tab_cuts=a.tab_cuts, allow_unset_lumi=a.allow_unset_lumi)
    json.dump(out, sys.stdout, indent=4, sort_keys=True)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
