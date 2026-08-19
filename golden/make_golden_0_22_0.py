"""Build the sixth frozen golden: the keys 0.19.0–0.22.0 added, all in one file.

Run ONCE at 0.22.0, committed, then never re-run — the same rule as its two predecessors.
A golden is frozen bytes with a sha256 pin in smoke.py, so rebuilding one would break the
guard it exists to be. To add a NEW capability, cut a new golden beside this one.

Derived from golden_0.13.0_tabs.rvr for the same reason make_golden_regions.py is: the
`demo_higgs` study the 0.13.0 four were built from is gone, and these are the same CMS
Open Data H->ZZ->4l events (CC0), already public, already frozen, so the input cannot
drift underneath the pins.

WHAT IT PINS that the five before it cannot — every one an OPTIONAL key whose absence has
to keep meaning what it meant, and whose presence has to survive a reader that has never
seen it:

    regions.grid        the DECLARATION a partition was built from: ordered variables,
                        each with cut points or value groups. Here channel by groups and
                        M4L by cut points, non-exhaustive — so the tails are leftover
                        leaves rather than declared regions, which is the shape that
                        actually reaches a datacard
    regions.axes/logr   the bubble layout's two switches
    regions.floorsrcs   the floor watching NAMED samples instead of the region total,
    regions.floormode   with `all` rather than `any`, since the two differ only when more
                        than one sample is watched — a golden that pinned one would pin
                        nothing
    lumis               per-group luminosity: `signal` at one, `ZZ` at another, so the
                        yields cannot be reproduced by any single factor. This is the one
                        that would silently pass if a reader ignored the key, so the pin
                        is a yield ratio and not a count
    style               the second channel beside colour, page-global

The counts printed at the end are computed here in numpy, straight from the columns, and
are what smoke.py pins. The page computes them again in JavaScript and the two must agree
exactly — two implementations, not one checking itself.
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
OUT = HERE / "golden_0.22.0_grid.rvr"

SIG = "MC · H(125)→ZZ→4l"
ZZ4MU, ZZ4E, ZZ2E2MU = "MC · ZZ→4mu", "MC · ZZ→4e", "MC · ZZ→2e2mu"
GROUPS = {"signal": [SIG], "ZZ": [ZZ4MU, ZZ4E]}
LABELS = {"channel": {0: "4mu", 1: "4e", 2: "2e2mu"}}

TAB_CUTS = {"MZ1": [76, 106], "MZ2": [20, 120], "M4L": [70, 180]}

#: the dict the partition is declared from. Ordered: channel is the outermost split.
GRID = {"vars": [{"col": "channel", "bands": [[0], [1]]},
                 {"col": "M4L", "bands": [110, 140, 170]}],
        "exhaustive": False}

#: per-group luminosity. Deliberately unequal and neither equal to the tab's own, so a
#: reader that drops the key cannot land on the right yields by accident.
LUMIS = {"signal": 3.0, "ZZ": 7.0}
TAB_LUMI = 1

#: the floor watching two named sources, mode `all` — a region is only set aside when
#: BOTH are thin, which is the case `any` gets wrong
FLOORSRCS = ["signal", "ZZ"]

STYLE = {"signal": "dash", "ZZ": "cross", ZZ2E2MU: "dots"}


def chain(cuts: list[dict]) -> dict | None:
    """Several cuts on one column as one nested spec, each taking what the rest did not."""
    spec = None
    for cut in reversed(cuts):
        spec = {"cut": cut, "yes": None, "no": spec}
    return spec


def cuts_of(var: dict) -> list[dict]:
    bands = var["bands"]
    if isinstance(bands[0], list):
        return [{"col": var["col"], "in": sorted(b)} for b in bands]
    return [{"col": var["col"], "lt": e} for e in sorted(bands)]


def interior(var: dict) -> set[int]:
    """Which bands are DECLARED: the bounded ones, or the named groups."""
    n = len(var["bands"]) + 1
    if isinstance(var["bands"][0], list):
        return set(range(n - 1))
    return set(range(1, n - 1))


def slice_slots(spec: dict) -> list[tuple[dict, str]]:
    out, cur = [], spec
    while cur is not None:
        out.append((cur, "yes"))
        if cur["no"] is None:
            out.append((cur, "no"))
            break
        cur = cur["no"]
    return out


def to_tree(grid: dict):
    """The dict as the map's root — only declared bands are split by the next variable."""
    vs = grid["vars"]

    def build(depth: int):
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

REGIONS = {"at": 0, "view": "bubble", "floor": 1,
           "sig": "signal", "bkg": ["ZZ", ZZ2E2MU],
           "grid": GRID, "axes": True, "logr": False,
           "floorsrcs": FLOORSRCS, "floormode": "all",
           "root": TREE}


def source() -> tuple[dict, dict, np.ndarray]:
    raw = gzip.decompress(SOURCE.read_bytes())
    length = int.from_bytes(raw[:4], "little")
    meta = json.loads(raw[4:4 + length])
    buf = raw[4 + length:]
    _, columns = page.read(SOURCE)
    dataset = np.frombuffer(buf, np.int32, meta["n"], meta["ds_offset"])
    return meta, columns, dataset


def per_sample(meta: dict, columns: dict, dataset: np.ndarray) -> dict:
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


def walk(node, columns: dict, here: np.ndarray) -> list[int]:
    """Every leaf's raw count, depth first, the predicate side before its complement."""
    if node is None:
        return [int(here.sum())]
    cut = node["cut"]
    values = columns[cut["col"]]
    took = here & (np.isin(values, cut["in"]) if "in" in cut else values < cut["lt"])
    return walk(node["yes"], columns, took) + walk(node["no"], columns, here & ~took)


def main() -> None:
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

    tabs = [{"name": "plots", "state": plots_tab}, {"name": "grid", "state": map_tab}]
    meta, buf = page.build(columns, labels=LABELS, weights=weights, tabs=tabs,
                           extra={"tab": 0, "style": STYLE})
    meta["tabs"], meta["tab"], meta["title"] = tabs, 0, OUT.name
    meta["style"] = STYLE
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
    raw = gzip.decompress(OUT.read_bytes())
    length = int.from_bytes(raw[:4], "little")
    dsi = np.frombuffer(raw[4 + length:], np.int32, back["n"], back["ds_offset"])
    keep = kept(back, cols_back, TAB_CUTS)
    counts = walk(TREE, cols_back, keep)
    print(f"leaf counts {counts}")
    print(f"they sum to {sum(counts):,} and the tab's cuts keep {int(keep.sum()):,} — "
          f"{'exact' if sum(counts) == int(keep.sum()) else 'MISMATCH'}")

    # the per-group luminosity, as a yield that no single factor could produce
    names = np.array(back["datasets"])
    w = cols_back[weights]
    per = {}
    for g, members in GROUPS.items():
        rows = keep & np.isin(names, members)[dsi]
        per[g] = float(w[rows].sum()) * LUMIS[g]
    loose = keep & np.isin(names, [ZZ2E2MU])[dsi]
    per[ZZ2E2MU] = float(w[loose].sum()) * TAB_LUMI
    print("\nper-group yields at their own luminosities:")
    for g, y in per.items():
        print(f"  {g:24s} {y:.6f}")
    print(f"  {'TOTAL':24s} {sum(per.values()):.6f}")
    print(f"\nregions declared by the grid: "
          f"{np.prod([len(interior(v)) for v in GRID['vars']])}")


if __name__ == "__main__":
    main()
