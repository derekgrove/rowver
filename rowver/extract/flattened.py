"""Dump per-(bin, process) yields from a .rvr as a combine framework's flattened.json.

    rowver extract flattened file.rvr bins.json      [options] > flattened.json
    rowver extract flattened file.rvr --from-map     [options] > flattened.json

Options: [--tab 0] [--w2 COLUMN] [--tab-cuts] [--allow-unset-lumi] [-o out.json]

Bins come from ONE of two places:
- bins.json maps bin name -> list of cuts, each ["column", op, value] with op
  one of < <= > >= == in ("in" takes a list). A bin's cuts are ANDed. This is
  the cross-check path: the combine framework's own bin definitions, joined by NAME.
- --from-map derives the bins from the cartridge's saved region map: the first
  `map` stage on the chosen tab, its `grid` walked into half-open windows
  ([threshold, next) per variable, first window open below, last open above).
  Bin names join per-variable cut labels: Bin_MET_ge_200_MET_lt_300_nJet_lt_2.

Output: {bin: {process: {"nominal": [raw_count, weighted, error]}}} with
4-space indent and lexically sorted keys — the shape the combine framework's merge
step writes (semantics at BFI_condor.cpp:846). A process is a GROUP from the
file's own view.groups; ungrouped samples stand as themselves.

TWO RULES THIS FILE MUST NOT GET WRONG, because both are silent when they are:

  A SAMPLE THAT DECLARES NO WEIGHTS IS REAL DATA. Its rows are counts, and no
  luminosity ever multiplies them. `data.samples[].weighted` says so per
  sample; it is read rather than inferred, because a sample whose weights all
  happen to be 1.0 is still MC and must still be scaled.

  THE OUTERMOST LUMINOSITY WINS — group, then subgroup, then sample.
  `rvr6.lumi_of` is the one implementation; no second copy of the ladder here.

A weighted sample that resolves to no luminosity anywhere falls back to 1 — a
silent default of exactly the kind that produced the "x1 tab in a x400 study"
trap. Such a sample is REFUSED unless --allow-unset-lumi is passed.

--tab-cuts additionally applies the chosen tab's saved rail cuts before any bin.
NaN fails every comparison on both numpy and RDataFrame, so the two frameworks
agree on NaN rows by construction.
"""
import argparse
import json
import math
import sys
from pathlib import Path

import numpy as np

from . import ExtractError
from .. import rvr6

OPS = {"<": np.less, "<=": np.less_equal, ">": np.greater,
       ">=": np.greater_equal, "==": np.equal}


def _require(condition: object, message: str) -> None:
    if not condition:
        raise ExtractError(message)


def cut_mask(columns: dict, rows: int, cuts: dict) -> np.ndarray:
    """A tab's saved rail cuts, in the format's own grammar: [lo, hi] or {"in": [...]}."""
    keep = np.ones(rows, bool)
    for name, spec in (cuts or {}).items():
        _require(name in columns, f"the cut names {name!r}, which this file has no column for")
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
        _require(col in columns, f"no column {col!r} — have: {sorted(columns)}")
        if op == "in":
            m = np.isin(columns[col], value)
        else:
            _require(op in OPS, f"unknown op {op!r} — have: {sorted(OPS)} and 'in'")
            m = OPS[op](columns[col], value)
        keep = m if keep is None else keep & m
    return keep if keep is not None else np.ones(len(next(iter(columns.values()))), bool)


def _num(value: float) -> str:
    return format(value, "g")


def bins_from_map(meta: dict, tab: int) -> dict:
    """Walk the tab's map grid into {bin name: [[col, op, value], ...]}.

    ponytail: grids only — a `root` region tree or a non-exhaustive categorical
    grid is refused loudly; use an explicit bins.json for those.
    """
    tabs = meta.get("tabs") or []
    _require(tabs, "this file has no tabs — --from-map needs a saved region map")
    stage = (tabs[tab].get("stage") or [])
    maps = [s for s in stage if s.get("kind") == "map"]
    _require(maps, f"tab {tab} has no map stage — kinds present: "
                   f"{[s.get('kind') for s in stage]}")
    grid = maps[0].get("grid")
    _require(grid, "the map has no grid (a root region tree is not supported here — use bins.json)")

    per_var = []  # one list of (label, cuts) windows per variable
    for var in grid["vars"]:
        col, bands = var["col"], var["bands"]
        if any(isinstance(b, list) for b in bands):
            _require(grid.get("exhaustive"),
                     f"categorical bands on {col!r} without exhaustive — the remainder "
                     "region is not expressible as cuts; use bins.json")
            per_var.append([(f"{col}_in_" + "-".join(map(str, b)), [[col, "in", b]])
                            for b in bands])
            continue
        windows = [(f"{col}_lt_{_num(bands[0])}", [[col, "<", bands[0]]])]
        for lo, hi in zip(bands, bands[1:]):
            windows.append((f"{col}_ge_{_num(lo)}_{col}_lt_{_num(hi)}",
                            [[col, ">=", lo], [col, "<", hi]]))
        windows.append((f"{col}_ge_{_num(bands[-1])}", [[col, ">=", bands[-1]]]))
        per_var.append(windows)

    bins = {"": []}
    for windows in per_var:
        bins = {f"{name}_{label}": cuts + more
                for name, cuts in bins.items() for label, more in windows}
    return {f"Bin{name}": cuts for name, cuts in bins.items()}


def dump(rvr: str | Path, bins: dict, tab: int = 0, w2: str | None = None,
         tab_cuts: bool = False, allow_unset_lumi: bool = False) -> dict:
    meta, buf = rvr6.read(rvr)
    data, view = meta["data"], meta.get("view") or {}
    columns = rvr6.columns(meta, buf)
    names = [s["name"] for s in data["samples"]]
    weighted = {s["name"]: s["weighted"] for s in data["samples"]}
    rows = data["rows"]

    weights_name = data.get("weight_col")
    _require(weights_name, "this file has no weights column — nothing to scale")
    ds_of = np.frombuffer(buf, np.int32, rows, data["sample_at"])

    # an UNWEIGHTED sample is data: its rows are counts and its luminosity is 1,
    # whatever any group it sits in says. That is not a fallback, it is the rule.
    lumi = {n: (rvr6.lumi_of(n, view) if weighted[n] else 1.0) for n in names}
    unset = [n for n in names
             if weighted[n] and rvr6.lumi_of(n, view) == 1.0 and not _named(n, view)]
    _require(allow_unset_lumi or not unset,
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


def add_arguments(ap: argparse.ArgumentParser) -> None:
    ap.add_argument("rvr")
    ap.add_argument("bins", nargs="?", default=None,
                    help="json: {bin name: [[column, op, value], ...]}")
    ap.add_argument("--from-map", action="store_true",
                    help="derive the bins from the cartridge's saved region-map grid")
    ap.add_argument("--tab", type=int, default=0)
    ap.add_argument("--w2", default=None,
                    help="per-event weight^2 column for the error term")
    ap.add_argument("--tab-cuts", action="store_true",
                    help="apply the tab's saved rail cuts before every bin")
    ap.add_argument("--allow-unset-lumi", action="store_true")
    ap.add_argument("-o", "--out", default=None, help="write here instead of stdout")


def run(a: argparse.Namespace) -> int:
    _require((a.bins is None) != (not a.from_map),
             "give exactly one bins source: a bins.json path or --from-map")
    if a.from_map:
        meta, _ = rvr6.read(a.rvr)
        bins = bins_from_map(meta, a.tab)
    else:
        bins = json.load(open(a.bins))
    out = dump(a.rvr, bins, tab=a.tab, w2=a.w2,
               tab_cuts=a.tab_cuts, allow_unset_lumi=a.allow_unset_lumi)
    handle = open(a.out, "w") if a.out else sys.stdout
    json.dump(out, handle, indent=4, sort_keys=True)
    handle.write("\n")
    return 0


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    add_arguments(ap)
    try:
        return run(ap.parse_args(argv))
    except (ExtractError, rvr6.FormatError, OSError) as err:
        print(err, file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
