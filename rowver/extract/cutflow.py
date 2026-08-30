"""Dump a tab's cutflow ladder from a .rvr — the sankey dialog's own numbers.

    rowver extract cutflow file.rvr [--tab 0] [--allow-unset-lumi] [-o DIR]

The ladder is the tab's EFFECTIVE cuts (locks honored) in activeCuts order:
a loaded cartridge has no session cutorder, so the order is column order.
Each event walks down until a cut rejects it, counting itself at every step
it reached — select.ts cutflow(), one pass. There is no --no-cuts: a cutflow
IS its cuts. Visibility always applies; a hidden sample is never counted.

NaN AT A LADDER STEP FAILS (the step test is lo <= x <= hi), while the
selection mask lets NaN PASS a range cut — so on a NaN-bearing cut column
the last row is smaller than the mask's kept total. That is the page's own
split, ported and fixture-pinned, not fixed here.

Sources are the viewer's gOf: a group name, or an ungrouped sample standing
as itself; the `by` matrix is per-source weighted yield per step. A weighted
sample resolving to no luminosity anywhere is REFUSED unless
--allow-unset-lumi is passed (the x1-in-a-x400-study trap).

Outputs (SCHEMAS.md kind `cutflow` v1, identity `t<tab>_cutflow`) go to
./extracts/<cart-stem>/ or -o DIR: `.data.npz` (steps, raw, w, by, sources),
`.meta.json`, `.cuts.txt` (ladder polarity). The table is also printed.
"""
import argparse
import json
import sys
from pathlib import Path

import numpy as np

from . import ExtractError, core
from .. import rvr6

VERSION = 1


def dump(rvr: str | Path, tab: int = 0, allow_unset_lumi: bool = False) -> dict:
    meta, buf = rvr6.read(rvr)
    data, view = meta["data"], meta.get("view") or {}
    columns = rvr6.columns(meta, buf)
    samples = [s["name"] for s in data["samples"]]
    ds = np.frombuffer(buf, np.dtype("<i4"), data["rows"], data["sample_at"])

    core.require_lumi(meta, allow_unset_lumi)
    tab_entry = core.tab_entry(meta, tab)
    cuts = rvr6.effective(tab_entry, view, "cuts") or {}
    shown = core.shown_of(tab_entry, view, samples)
    ww = core.row_weights(meta, columns, ds)
    flow = core.cutflow(columns, data["cols"], cuts, shown, ds, samples, ww, view)
    flow["active"] = core.active_cuts(data["cols"], cuts, None)
    flow["weighted"] = core.is_weighted(meta)
    flow["lumi"] = {s: float(l) for s, l in
                    zip(samples, core.lumi_table(meta))}
    flow["cuts"] = {name: core.json_cut_spec(
                        {c["name"]: c for c in data["cols"]}[name], cuts[name])
                    for name in flow["active"]}
    flow["look"] = {s: core.source_look(view, samples, s) for s in flow["sources"]}
    return flow


def add_arguments(ap: argparse.ArgumentParser) -> None:
    ap.add_argument("rvr")
    ap.add_argument("--tab", type=int, default=0)
    ap.add_argument("--allow-unset-lumi", action="store_true")
    ap.add_argument("-o", "--out", default=None,
                    help="output directory (default ./extracts/<cart-stem>/)")


def run(a: argparse.Namespace) -> int:
    flow = dump(a.rvr, tab=a.tab, allow_unset_lumi=a.allow_unset_lumi)
    out = Path(a.out) if a.out else Path("extracts") / Path(a.rvr).stem
    out.mkdir(parents=True, exist_ok=True)
    name = f"t{a.tab}_cutflow"

    np.savez(out / f"{name}.data.npz",
             steps=core.unicode_array(flow["steps"]),
             raw=flow["raw"].astype("<f8"), w=flow["w"].astype("<f8"),
             by=flow["by"].astype("<f8"),
             sources=core.unicode_array(flow["sources"]))
    (out / f"{name}.meta.json").write_text(json.dumps(
        {"kind": "cutflow", "version": VERSION, "tab": a.tab,
         "cart": Path(a.rvr).name, "weighted": flow["weighted"],
         "steps": flow["steps"], "active": flow["active"],
         "cuts": flow["cuts"], "sources": flow["sources"],
         "look": flow["look"], "lumi": flow["lumi"]},
        indent=4, sort_keys=True) + "\n")
    meta, _ = rvr6.read(a.rvr)
    (out / f"{name}.cuts.txt").write_text(
        "\n".join(core.cuts_lines(meta, a.tab, ladder=True)) + "\n")

    width = max(len(s) for s in flow["steps"])
    for step, raw, w in zip(flow["steps"], flow["raw"], flow["w"]):
        line = f"{step:<{width}}  {int(raw):>12,}"
        if flow["weighted"]:
            line += f"  {w:>16.6g}"
        print(line)
    print(f"wrote {out / name}.{{data.npz,meta.json,cuts.txt}}", file=sys.stderr)
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
