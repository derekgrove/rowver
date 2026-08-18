"""Build the fifth frozen golden: a .rvr carrying a region map's partition.

Run ONCE at 0.18.0, committed, then never re-run — the same rule as
make_goldens_0_13_0.py. A golden is frozen bytes with a sha256 pin in smoke.py, so
rebuilding one would break the guard it exists to be. Read it to see what the file
contains; to add a NEW capability, cut a new golden beside this one.

WHY IT IS DERIVED FROM ANOTHER GOLDEN. The four 0.13.0 files were built from a
`demo_higgs` study that this project no longer carries, so their maker cannot be
followed. Rather than invent a study, this reads the columns straight back out of
golden_0.13.0_tabs.rvr — the same CMS Open Data H->ZZ->4l events (CC0), already public,
already in the repo, and already frozen, so the input can never drift underneath it.

WHAT IT PINS that the other four cannot:

    a chain on a CATEGORICAL column   channel: 4mu | 4e | 2e2mu, from two cuts
    a chain on a CONTINUOUS column    M4L under 4mu: <110 | 110-140 | >=140
    a NON-UNIFORM tree                2e2mu is one region while 4mu is cut three ways
    depth 3                           MZ2 inside the M4L peak inside 4mu
    an EMPTY region                   `M4L < 0` takes nothing, on purpose: a cut dragged
                                      past the data is a real state, and a zero-area
                                      child is exactly where a layout breaks
    a map and a BOARD on one tab      two blocks in one stack, at 0 and at 1
    the boot tab is an ORDINARY tab   smoke's boot probe needs PLOTS[0], so the map rides
                                      on tab 2 and the probe switches to it

There is no leftover bin to pin, and that is the point. Every cut is a predicate and its
complement, so the two sides are mutually exclusive and exhaustive whatever the data
holds. What replaces the leftover as the guard is stronger: the leaf regions must sum to
the root exactly, on raw integer counts.

The counts printed at the end are computed here in numpy, straight from the columns, and
are what smoke.py pins. The page computes them again in JavaScript and the two must
agree exactly — two implementations, not one checking itself.
"""
import gzip
import hashlib
import json
import sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import page

HERE = Path(__file__).parent
SOURCE = HERE / "golden_0.13.0_tabs.rvr"
OUT = HERE / "golden_0.18.0_regions.rvr"

SIG = "MC · H(125)→ZZ→4l"
ZZ4MU, ZZ4E, ZZ2E2MU = "MC · ZZ→4mu", "MC · ZZ→4e", "MC · ZZ→2e2mu"
GROUPS = {"signal": [SIG], "ZZ": [ZZ4MU, ZZ4E]}      # ZZ2E2MU stays loose, as it was
LABELS = {"channel": {0: "4mu", 1: "4e", 2: "2e2mu"}}

#: the map tab's cuts. NOT a chip cut on `channel` — that would empty the 4e camp and
#: silently stop the categorical chain from being pinned at all.
MAP_CUTS = {"MZ1": [76, 106], "MZ2": [20, 120], "M4L": [70, 180]}
PLOT_CUTS = {"MZ1": [76, 106], "MZ2": [20, 120], "M4L": [70, 180], "channel": [0, 1, 2]}

#: The partition. `yes` satisfies the cut, `no` is everything else; null is a leaf.
TREE = {
    "cut": {"col": "channel", "in": [0]},                       # 4mu, then everything else
    "yes": {
        "cut": {"col": "M4L", "lt": 110},                       # a chain of two on M4L
        "yes": None,
        "no": {
            "cut": {"col": "M4L", "lt": 140},
            "yes": {"cut": {"col": "MZ2", "lt": 30}, "yes": None, "no": None},
            "no": None,
        },
    },
    "no": {
        "cut": {"col": "channel", "in": [1]},                   # 4e, then 2e2mu
        # a cut that takes NOTHING: the yes side is an empty region, on purpose
        "yes": {"cut": {"col": "M4L", "lt": 0}, "yes": None, "no": None},
        "no": None,
    },
}

#: The map carries its OWN preselection — the same selection the tab's cuts describe, but
#: owned by the map, because a region map does not read the rail. `srcs` is deliberately
#: ABSENT: that must go on meaning "every source", or a file written before the sample tray
#: existed would quietly start showing nothing.
REGIONS = {"at": 1, "view": "stack", "sig": "signal", "bkg": ["ZZ", ZZ2E2MU],
           "pre": {"cuts": dict(MAP_CUTS), "chips": {}, "lumi": 1},
           "root": TREE}

BOARD = {"rows": 2, "cols": 2,
         "cells": ["signal", "ZZ", ZZ2E2MU, None],
         "bkgs": ["ZZ", ZZ2E2MU],
         "plots": [], "channel": "metrics", "err": "poisson_neff",
         "met": ["sob", "sig"], "at": 0}


def source() -> tuple[dict, dict, np.ndarray]:
    """The frozen golden's META, its columns, and the dataset index behind them."""
    raw = gzip.decompress(SOURCE.read_bytes())
    length = int.from_bytes(raw[:4], "little")
    meta = json.loads(raw[4:4 + length])
    buf = raw[4 + length:]
    _, columns = page.read(SOURCE)
    dataset = np.frombuffer(buf, np.int32, meta["n"], meta["ds_offset"])
    return meta, columns, dataset


def per_sample(meta: dict, columns: dict, dataset: np.ndarray) -> dict:
    """The flat columns split back into one dict per sample, in the file's own order."""
    out = {}
    for i, name in enumerate(meta["datasets"]):
        rows = dataset == i
        out[name] = {c: v[rows].copy() for c, v in columns.items()}
    return out


def kept(meta: dict, columns: dict, cuts: dict) -> np.ndarray:
    """The page's mask, in numpy: inclusive both ends, a handle at its stop is no bound."""
    keep = np.ones(meta["n"], bool)
    by_name = {c["name"]: c for c in meta["cols"]}
    for name, span in cuts.items():
        col = by_name[name]
        values = columns[name]
        if col["vals"] is not None:
            if len(span) < len(col["vals"]):
                keep &= np.isin(values, span)
            continue
        stop_lo, stop_hi = col["nice"] or col["lim"]
        if span[0] > stop_lo:
            keep &= values >= span[0]
        if span[1] < stop_hi:
            keep &= values <= span[1]
    return keep


def side(cut: dict, columns: dict) -> np.ndarray:
    """One cut as a boolean per event. Its complement is simply ~this."""
    values = columns[cut["col"]]
    if "in" in cut:
        return np.isin(values, cut["in"])
    return values < cut["lt"]


def label(cut: dict, meta: dict, yes: bool) -> str:
    """How a slice reads: the value names it admits, or the half-line it takes."""
    col = {c["name"]: c for c in meta["cols"]}[cut["col"]]
    if "in" in cut:
        names = col["names"] or [str(v) for v in col["vals"]]
        picked = [names[col["vals"].index(v)] for v in cut["in"]]
        return f"{cut['col']} {'is' if yes else 'is not'} {'/'.join(picked)}"
    return f"{cut['col']} {'<' if yes else '>='} {cut['lt']:g}"


def regions(node, meta: dict, columns: dict, here: np.ndarray, path: tuple) -> dict:
    """Every leaf region of the tree as {path: raw count}, walked in numpy."""
    if node is None:
        return {path: int(here.sum())}
    took = here & side(node["cut"], columns)
    rest = here & ~took
    out = regions(node["yes"], meta, columns, took, path + (label(node["cut"], meta, True),))
    out |= regions(node["no"], meta, columns, rest, path + (label(node["cut"], meta, False),))
    return out


def main() -> None:
    meta0, columns0, dataset0 = source()
    columns = per_sample(meta0, columns0, dataset0)
    weights = meta0["weights"]
    assert weights == "weight", f"the source golden lost its weights: {weights!r}"

    packed, _ = page.pack(columns, LABELS, weights)
    plots_tab = page.state(
        packed, cuts=PLOT_CUTS, groups=GROUPS, fom=("signal", "ZZ"), shown=["*"], lumi=1,
        cms="off", rlabel="CMS open data",
        plots=[{"varx": "M4L", "nb": 40, "xauto": False, "xmin": 70, "xmax": 180,
                "cmt": "the boot tab — the map is on tab 2, because smoke's boot probe "
                       "needs a plot to poke at"}])
    map_tab = page.state(
        packed, cuts=MAP_CUTS, groups=GROUPS, fom=("signal", "ZZ"), shown=["*"], lumi=1,
        regions=REGIONS, table=BOARD,
        plots=[{"varx": "M4L", "nb": 30, "xauto": False, "xmin": 70, "xmax": 180}])

    tabs = [{"name": "plots", "state": plots_tab}, {"name": "map", "state": map_tab}]
    meta, buf = page.build(columns, labels=LABELS, weights=weights, tabs=tabs,
                           extra={"tab": 0})
    meta["tabs"], meta["tab"], meta["title"] = tabs, 0, OUT.name
    header = json.dumps(meta).encode()
    OUT.write_bytes(gzip.compress(
        len(header).to_bytes(4, "little") + header + buf, 6, mtime=0))

    sha = hashlib.sha256(OUT.read_bytes()).hexdigest()
    print(f"{OUT.name}  {OUT.stat().st_size/1e6:.2f} MB  n={meta['n']:,}  "
          f"samples={len(meta['datasets'])}  tabs={len(meta['tabs'])}  "
          f"made_by={meta['made_by']}")
    print(f"sha256: {sha}\n")

    # the pins, computed here in numpy and nowhere near the page's own arithmetic
    back, cols_back = page.read(OUT)
    keep = kept(back, cols_back, REGIONS["pre"]["cuts"])
    found = regions(TREE, back, cols_back, keep, ())
    total = int(keep.sum())
    print(f"map tab keeps {total:,} of {back['n']:,} events\n")
    print("REGIONS = {")
    for path, n in found.items():
        print(f'    {list(path)!r}: {n},')
    print("}")
    leaves = sum(found.values())
    print(f"\nleaves sum to {leaves:,} — root is {total:,} — "
          f"{'EXACT' if leaves == total else 'MISMATCH'}")
    empty = [p for p, n in found.items() if n == 0]
    print(f"empty regions: {len(empty)} {[p[-1] for p in empty]}")


if __name__ == "__main__":
    main()
