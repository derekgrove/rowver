"""Dump per-(bin, process) yields from a .rvr as a combine framework's flattened.json.

    rowver extract flattened file.rvr bins.json      [options] > flattened.json
    rowver extract flattened file.rvr --from-map     [options] > flattened.json

Options: [--tab 0] [--w2 COLUMN] [--no-cuts] [--allow-unset-lumi] [-o out.json]

Bins come from ONE of two places:
- bins.json maps bin name -> list of cuts, each ["column", op, value] with op
  one of < <= > >= == in ("in" takes a list). A bin's cuts are ANDed. This is
  the cross-check path: the combine framework's own bin definitions, joined by NAME.
- --from-map uses the tab's effective region map: its saved `root` tree when
  present, else a tree derived from the declared `grid` (this derivation is an
  explicit bins REQUEST — page parity counts only a saved root, since a grid
  with no root is a recipe not yet applied and the page shows one undivided
  region). Bin names are the viewer's own `regionBinName` output, operators
  transliterated: Bin_MET_ge_200_MET_lt_300_nJet_lt_2. NOTE: these names
  changed when the walk replaced the old Cartesian grid conversion.

The tab's EFFECTIVE state applies by default, exactly as the page resolves it:
its cuts (locks honored; a NaN value PASSES a range cut and FAILS a chip cut —
this differs from the old --tab-cuts mask, which dropped NaN rows at range
cuts, so numbers can move for NaN-bearing columns) and its `shown` visibility
(hidden samples are never counted). --no-cuts disables the cuts; visibility
always applies. With -o, the cuts actually applied are written alongside as
<out>.cuts.txt.

A file with NO weights column is real data everywhere: every row counts 1,
yield equals the raw count, and the error is sqrt(n) — regionFlat's own
arithmetic (sum of 1² per row), the same numbers the viewer shows.

Output: {bin: {process: {"nominal": [raw_count, weighted, error]}}} with
4-space indent and lexically sorted keys — the shape the combine framework's merge
step writes (semantics at BFI_condor.cpp:846). Processes are the viewer's
sources: group names, then ungrouped samples standing as themselves.

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
"""
import argparse
import json
import math
import sys
from pathlib import Path

import numpy as np

from . import ExtractError, core
from .. import rvr6

OPS = {"<": np.less, "<=": np.less_equal, ">": np.greater,
       ">=": np.greater_equal, "==": np.equal}


def _require(condition: object, message: str) -> None:
    if not condition:
        raise ExtractError(message)


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


def _map_spec(meta: dict, tab: int) -> dict:
    """The tab's first effective map stage, with a root: saved, else derived."""
    maps = [e["spec"] for e in core.stage_entries(meta, tab) if e["kind"] == "map"]
    _require(maps, f"tab {tab} has no map stage — --from-map needs a region map")
    spec = maps[0]
    # saved root, else a tree derived from the grid; NEITHER is still legal —
    # the page shows ONE undivided region and regionLeaves(None) is one "Bin"
    root = core.region_root(spec) or core.tree_from_grid(spec.get("grid") or {})
    return {**spec, "root": root}


def dump(rvr: str | Path, bins: dict | None, tab: int = 0, w2: str | None = None,
         no_cuts: bool = False, allow_unset_lumi: bool = False) -> dict:
    """``bins`` is the external {name: cuts} dict, or None for --from-map."""
    meta, buf = rvr6.read(rvr)
    data, view = meta["data"], meta.get("view") or {}
    columns = rvr6.columns(meta, buf)
    samples = [s["name"] for s in data["samples"]]
    rows = data["rows"]
    ds = np.frombuffer(buf, np.dtype("<i4"), rows, data["sample_at"])

    core.require_lumi(meta, allow_unset_lumi)

    tab_entry = core.tab_entry(meta, tab)
    cuts = {} if no_cuts else (rvr6.effective(tab_entry, view, "cuts") or {})
    shown = core.shown_of(tab_entry, view, samples)
    keep = core.sel_mask(columns, data["cols"], cuts, shown, ds, samples)

    if bins is None:
        _require(w2 is None,
                 "--w2 with --from-map is not supported: the viewer's region counting "
                 "has no w² column — use bins.json for that cross-check")
        num = core.region_count(meta, columns, ds, keep, _map_spec(meta, tab))
        return core.region_flat(num)

    srcs = core.sources(view, samples)
    src_rows = [np.isin(ds, core.src_members(view, samples, s)) for s in srcs]
    wl = core.row_weights(meta, columns, ds)
    if w2 is not None:
        _require(w2 in columns, f"no column {w2!r} — have: {sorted(columns)}")
        w2l = columns[w2] * core.lumi_table(meta)[ds] ** 2
    else:
        w2l = wl * wl
    # external bin names are the cross-check contract: joined by NAME, passed
    # through VERBATIM — regionFlat's renaming/suffixing is --from-map only
    out = {}
    for name in sorted(bins):
        m = keep & bin_mask(columns, bins[name])
        out[name] = {}
        for k, sm in enumerate(src_rows):
            sel = m & sm
            s2 = core._seq_sum(w2l[sel])
            if w2 is not None:
                # a negative or NaN summed w² can only come from a bad --w2
                # column (ww² is non-negative and cannot cancel); refuse
                # rather than understate the error
                _require(s2 >= 0, f"column {w2!r} sums to an invalid w² "
                                  f"({s2}) in bin {name!r} for {srcs[k]!r}")
            # _or0 matches regionFlat: an overflow-cancelled Σw (inf - inf =
            # NaN from validator-finite weights × lumi) flattens to 0 on BOTH
            # paths, and never leaks NaN into the JSON
            out[name][srcs[k]] = {"nominal": [
                float(int(sel.sum())), core._or0(core._seq_sum(wl[sel])),
                math.sqrt(s2)]}
    return out


def add_arguments(ap: argparse.ArgumentParser) -> None:
    ap.add_argument("rvr")
    ap.add_argument("bins", nargs="?", default=None,
                    help="json: {bin name: [[column, op, value], ...]}")
    ap.add_argument("--from-map", action="store_true",
                    help="bins from the tab's region map (saved root, else its grid); "
                         "bin names are the viewer's regionBinName output")
    ap.add_argument("--tab", type=int, default=0)
    ap.add_argument("--w2", default=None,
                    help="per-event weight^2 column for the error term (bins.json path only)")
    ap.add_argument("--no-cuts", action="store_true",
                    help="do not apply the tab's effective cuts (visibility still applies)")
    ap.add_argument("--tab-cuts", action="store_true",
                    help="deprecated no-op: the tab's cuts now apply by default")
    ap.add_argument("--allow-unset-lumi", action="store_true")
    ap.add_argument("-o", "--out", default=None, help="write here instead of stdout")


def run(a: argparse.Namespace) -> int:
    _require((a.bins is None) != (not a.from_map),
             "give exactly one bins source: a bins.json path or --from-map")
    if a.tab_cuts:
        print("--tab-cuts is deprecated: the tab's cuts now apply by default "
              "(use --no-cuts to disable)", file=sys.stderr)
    _require(not (a.from_map and a.w2),
             "--w2 with --from-map is not supported: the viewer's region counting "
             "has no w² column — use bins.json for that cross-check")
    bins = None if a.from_map else json.loads(Path(a.bins).read_text())
    out = dump(a.rvr, bins, tab=a.tab, w2=a.w2,
               no_cuts=a.no_cuts, allow_unset_lumi=a.allow_unset_lumi)
    text = json.dumps(out, indent=4, sort_keys=True) + "\n"
    if a.out:
        Path(a.out).write_text(text)
    else:
        sys.stdout.write(text)
    if a.out:
        meta, _ = rvr6.read(a.rvr)
        Path(a.out).with_suffix(".cuts.txt").write_text(
            "\n".join(core.cuts_lines(meta, a.tab, a.no_cuts)) + "\n")
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
