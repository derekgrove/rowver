"""Build the eighth frozen golden: the 0.27.0 keys — the bubble grid and the heat map.

Run ONCE at 0.27.0, committed, then never re-run — the same rule as every golden before
it. Derived from golden_0.13.0_tabs.rvr like its predecessors: the same CMS Open Data
H->ZZ->4l events (CC0), already public, already frozen, so the input cannot drift.

WHAT IT PINS that the seven before it cannot — every one an OPTIONAL key whose absence
has to keep meaning what it meant:

    regions.bgrid       the bubble grid: {x, y} naming map variables. Deliberately set
                        to the REVERSE of the declared grid's axis order (M4L across,
                        channel down) so the pin also fixes PRECEDENCE — this file
                        carries grid+axes too, and if the declared-axes layout ever
                        outranked the bubble grid again, ncol flips from 5 to 2.
                        The 2e2mu leaf has no M4L cut, so it also pins the trailing
                        "any M4L" band.
    regions.pages.heat  the two-gradient heat map. Windows are computed here in numpy
                        (per category, log10, 4-decade floor, per-group lumis folded)
                        and must match the page's own model.
    regions.pages.cats  one explicit override: the loose 2e2mu sample flipped from its
                        RGN.bkg default (red) to signal (blue) — so both the default
                        inference and the override path are exercised.

The counts and windows printed at the end are computed here in numpy, straight from the
columns, and are what smoke.py pins. The page computes them again in JavaScript and the
two must agree — two implementations, not one checking itself. (The two rendered color
strings in the pin are regression pins on the formula, read off the page once, not an
independent claim.)
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
OUT = HERE / "golden_0.27.0_bgrid.rvr"

SIG = "MC · H(125)→ZZ→4l"
ZZ4MU, ZZ4E, ZZ2E2MU = "MC · ZZ→4mu", "MC · ZZ→4e", "MC · ZZ→2e2mu"
GROUPS = {"signal": [SIG], "ZZ": [ZZ4MU, ZZ4E]}
LABELS = {"channel": {0: "4mu", 1: "4e", 2: "2e2mu"}}

TAB_CUTS = {"MZ1": [76, 106], "MZ2": [20, 120], "M4L": [70, 180]}

#: same declaration as the 0.22 golden — kept, WITH axes on, so the file proves the
#: bubble grid outranks the declared-axes layout
GRID = {"vars": [{"col": "channel", "bands": [[0], [1]]},
                 {"col": "M4L", "bands": [110, 140, 170]}],
        "exhaustive": False}

LUMIS = {"signal": 3.0, "ZZ": 7.0}
TAB_LUMI = 1

#: axes REVERSED against the declared order, on purpose (see module docstring)
BGRID = {"x": "M4L", "y": "channel"}
#: heat on, with the one override; floor 0 so every leaf is live and the cell pins are
#: about the grid, not the floor (the 0.22 golden already pins the floor)
PAGES = {"on": True, "heat": True, "cats": {ZZ2E2MU: "sig"}}

M4L_EDGES = [110.0, 140.0, 170.0]


def chain(cuts):
    spec = None
    for cut in reversed(cuts):
        spec = {"cut": cut, "yes": None, "no": spec}
    return spec


def cuts_of(var):
    bands = var["bands"]
    if isinstance(bands[0], list):
        return [{"col": var["col"], "in": sorted(b)} for b in bands]
    return [{"col": var["col"], "lt": e} for e in sorted(bands)]


def interior(var):
    n = len(var["bands"]) + 1
    if isinstance(var["bands"][0], list):
        return set(range(n - 1))
    return set(range(1, n - 1))


def slice_slots(spec):
    out, cur = [], spec
    while cur is not None:
        out.append((cur, "yes"))
        if cur["no"] is None:
            out.append((cur, "no"))
            break
        cur = cur["no"]
    return out


def to_tree(grid):
    vs = grid["vars"]

    def build(depth):
        if depth == len(vs):
            return None
        root = chain(cuts_of(vs[depth]))
        inner = interior(vs[depth])
        for i, (holder, key) in enumerate(slice_slots(root)):
            if grid["exhaustive"] or i in inner:
                holder[key] = build(depth + 1)
        return root

    return build(0)


TREE = to_tree(GRID)

REGIONS = {"at": 0, "view": "bubble", "floor": 0,
           "sig": "signal", "bkg": ["ZZ", ZZ2E2MU],
           "grid": GRID, "axes": True, "logr": False,
           "bgrid": BGRID, "pages": PAGES,
           "root": TREE}


def source():
    raw = gzip.decompress(SOURCE.read_bytes())
    length = int.from_bytes(raw[:4], "little")
    meta = json.loads(raw[4:4 + length])
    buf = raw[4 + length:]
    _, columns = page.read(SOURCE)
    dataset = np.frombuffer(buf, np.int32, meta["n"], meta["ds_offset"])
    return meta, columns, dataset


def per_sample(meta, columns, dataset):
    out = {}
    for i, name in enumerate(meta["datasets"]):
        rows = dataset == i
        out[name] = {c: v[rows].copy() for c, v in columns.items()}
    return out


def kept(meta, columns, cuts):
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


def m4l_col(values):
    """The M4L window index the bubble grid's x axis uses: the four bands in window
    order, plus 4 = the trailing "any M4L" band a leaf without the cut lands in."""
    out = np.full(len(values), 3, dtype=int)
    out[values < M4L_EDGES[2]] = 2
    out[values < M4L_EDGES[1]] = 1
    out[values < M4L_EDGES[0]] = 0
    return out


def main():
    meta0, columns0, dataset0 = source()
    columns = per_sample(meta0, columns0, dataset0)
    weights = meta0["weights"]
    assert weights == "weight", f"the source golden lost its weights: {weights!r}"

    packed, _ = page.pack(columns, LABELS, weights)
    plots_tab = page.state(
        packed, cuts=TAB_CUTS, groups=GROUPS, fom=("signal", "ZZ"), shown=["*"],
        lumi=TAB_LUMI, cms="off", rlabel="CMS open data",
        plots=[{"varx": "M4L", "nb": 40, "xauto": False, "xmin": 70, "xmax": 180,
                "cmt": "the boot tab — smoke's boot probe needs a plot to poke at"}])
    map_tab = page.state(
        packed, cuts=TAB_CUTS, groups=GROUPS, fom=("signal", "ZZ"), shown=["*"],
        lumi=TAB_LUMI, regions=REGIONS, lumis=LUMIS,
        plots=[{"varx": "M4L", "nb": 30, "xauto": False, "xmin": 70, "xmax": 180}])

    tabs = [{"name": "plots", "state": plots_tab}, {"name": "bgrid", "state": map_tab}]
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

    # ---- the pins, computed in numpy and nowhere near the page's own arithmetic ----
    back, cols_back = page.read(OUT)
    raw = gzip.decompress(OUT.read_bytes())
    length = int.from_bytes(raw[:4], "little")
    dsi = np.frombuffer(raw[4 + length:], np.int32, back["n"], back["ds_offset"])
    keep = kept(back, cols_back, TAB_CUTS)
    ch, m4l = cols_back["channel"], cols_back["M4L"]

    # cells keyed "row,col": rows are channel bands (4mu, 4e, then the 2e2mu complement),
    # cols the four M4L windows plus the trailing "any" band the 2e2mu leaf lands in
    m4l_idx = m4l_col(m4l)
    cells = {}
    for row, csel in ((0, ch == 0), (1, ch == 1)):
        for col in range(4):
            cells[f"{row},{col}"] = [int((keep & csel & (m4l_idx == col)).sum())]
    cells["2,4"] = [int((keep & ~np.isin(ch, [0, 1])).sum())]
    print("cells (row,col -> leaf raw counts):", json.dumps(cells, sort_keys=True))
    total = sum(v[0] for v in cells.values())
    print(f"they sum to {total:,} and the tab keeps {int(keep.sum()):,} — "
          f"{'exact' if total == int(keep.sum()) else 'MISMATCH'}")

    # heat: per-(leaf, source) yields at their own luminosities, then the per-category
    # log10 windows. srcs in the page-model order: sorted source names.
    names = np.array(back["datasets"])
    w = cols_back[weights]
    srcs = {"signal": (GROUPS["signal"], LUMIS["signal"]),
            "ZZ": (GROUPS["ZZ"], LUMIS["ZZ"]),
            ZZ2E2MU: ([ZZ2E2MU], TAB_LUMI)}
    leaf_masks = ([keep & (ch == c) & (m4l_idx == m) for c in (0, 1) for m in range(4)]
                  + [keep & ~np.isin(ch, [0, 1])])
    cats = {"signal": "sig", "ZZ": "bkg", ZZ2E2MU: PAGES["cats"].get(ZZ2E2MU, "bkg")}
    per_cat = {"sig": [], "bkg": []}
    for s, (members, lumi) in srcs.items():
        rows = np.isin(names, members)[dsi]
        for m in leaf_masks:
            y = float(w[m & rows].sum()) * lumi
            if y > 0:
                per_cat[cats[s]].append(y)
    win = {}
    for k, ys in per_cat.items():
        if not ys:
            win[k] = None
            continue
        top = float(np.log10(max(ys)))
        win[k] = [round(max(float(np.log10(min(ys))), top - 4), 6), round(top, 6)]
    print("cats in sorted-src order:",
          json.dumps({s: cats[s] for s in sorted(srcs)}, sort_keys=False))
    print("heat windows:", json.dumps(win, sort_keys=True))


if __name__ == "__main__":
    main()
