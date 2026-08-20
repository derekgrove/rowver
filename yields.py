"""Dump per-(bin, process) yields from a .rvr in a combine-style flattened.json shape.

    python yields.py file.rvr bins.json [--tab 0] [--w2 COLUMN] [--tab-cuts]
                                        [--allow-tab-lumi]

bins.json maps bin name -> list of cuts, each ["column", op, value] with op one of
< <= > >= == in ("in" takes a list). A bin's cuts are ANDed; NaN fails every
comparison on both numpy and RDataFrame, so the two agree on NaN by construction.

Output (stdout): {bin: {process: {"nominal": [raw_count, weighted, error]}}} with
4-space indent and lexically sorted keys — the shape the C++ fitting framework's
merge step writes. Bins are iterated in plain sorted() order over their names. A process is a
GROUP from the file's own grouping; weighted = sum of weight x that dataset's
luminosity, resolved subgroup > group > tab exactly as the page resolves it; error =
sqrt(sum of w2-column x luminosity^2) when --w2 names a per-event weight^2 column,
else sqrt taken over the squared weights themselves.

The tab `lumi` is a trap, not a fallback (the "x1 tab in a x400 study" lesson): any
dataset that fails to resolve through the `lumis` map is REFUSED unless
--allow-tab-lumi is passed explicitly.

--tab-cuts additionally applies the chosen tab's saved rail cuts before any bin
(inclusive handles, a handle at its stop is open — the page's own rule).
"""
import argparse
import gzip
import json
import math
import sys
from pathlib import Path

import numpy as np

import page

OPS = {"<": np.less, "<=": np.less_equal, ">": np.greater,
       ">=": np.greater_equal, "==": np.equal}


def dataset_index(path: str | Path, meta: dict) -> np.ndarray:
    """The per-event dataset index — the one array page.read does not hand back."""
    raw = gzip.decompress(Path(path).read_bytes())
    buf = raw[4 + int.from_bytes(raw[:4], "little"):]
    return np.frombuffer(buf, np.int32, meta["n"], meta["ds_offset"])


def lumi_of(meta: dict, st: dict) -> dict[str, tuple[float, str]]:
    """dataset -> (luminosity, how it resolved): subgroup > group > tab."""
    grouping = meta.get("grouping") or {}
    sub, grp = grouping.get("sub") or {}, grouping.get("group") or {}
    lumis = st.get("lumis") or {}
    out = {}
    for ds in meta["datasets"]:
        s, g = sub.get(ds), grp.get(ds)
        if s in lumis:
            out[ds] = (float(lumis[s]), "subgroup")
        elif g in lumis:
            out[ds] = (float(lumis[g]), "group")
        else:
            out[ds] = (float(st.get("lumi") or 1), "tab")
    return out


def tab_cut_mask(meta: dict, columns: dict, st: dict) -> np.ndarray:
    """The tab's saved rail cuts, the page's rule: a handle at its stop is open."""
    keep = np.ones(meta["n"], bool)
    for i, span in (st.get("cuts") or {}).items():
        col = meta["cols"][int(i)]
        stop_lo, stop_hi = col["nice"] or col["lim"]
        v = columns[col["name"]]
        if span[0] > stop_lo:
            keep &= v >= span[0]
        if span[1] < stop_hi:
            keep &= v <= span[1]
    for i, vals in (st.get("chips") or {}).items():
        col = meta["cols"][int(i)]
        if len(vals) < len(col["vals"] or ()):
            keep &= np.isin(columns[col["name"]], vals)
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
         tab_cuts: bool = False, allow_tab_lumi: bool = False) -> dict:
    meta, columns = page.read(rvr)
    weights_name = meta.get("weights")
    assert weights_name, "this file has no weights column — nothing to scale"
    st = (meta.get("tabs") or [{"state": {}}])[tab].get("state") or {}

    lumi = lumi_of(meta, st)
    on_tab = [ds for ds, (_, how) in lumi.items() if how == "tab"]
    assert allow_tab_lumi or not on_tab, (
        f"{len(on_tab)} dataset(s) resolve to the TAB luminosity ({st.get('lumi')}), "
        f"e.g. {on_tab[:3]} — a silent default is the x1-tab trap. Fix the `lumis` "
        f"map or pass --allow-tab-lumi to accept it deliberately.")

    grouping = meta.get("grouping") or {}
    grp = grouping.get("group") or {}
    ds_of = dataset_index(rvr, meta)
    w = columns[weights_name]
    w2col = columns[w2] if w2 else w * w
    lumi_row = np.array([lumi[ds][0] for ds in meta["datasets"]])[ds_of]
    wl, w2l = w * lumi_row, w2col * lumi_row * lumi_row
    group_row = np.array([grp.get(ds, ds) for ds in meta["datasets"]], dtype=object)[ds_of]

    base = tab_cut_mask(meta, columns, st) if tab_cuts else np.ones(meta["n"], bool)
    groups = sorted(set(grp.get(ds, ds) for ds in meta["datasets"]))
    out = {}
    for name in sorted(bins):
        m = base & bin_mask(columns, bins[name])
        entry = {}
        for g in groups:
            gm = m & (group_row == g)
            entry[g] = {"nominal": [float(int(gm.sum())), float(wl[gm].sum()),
                                    math.sqrt(float(w2l[gm].sum()))]}
        out[name] = entry
    return out


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
    ap.add_argument("--allow-tab-lumi", action="store_true")
    a = ap.parse_args()
    out = dump(a.rvr, json.load(open(a.bins)), tab=a.tab, w2=a.w2,
               tab_cuts=a.tab_cuts, allow_tab_lumi=a.allow_tab_lumi)
    json.dump(out, sys.stdout, indent=4, sort_keys=True)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
