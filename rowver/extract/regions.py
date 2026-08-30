"""Dump the tab's region map from a .rvr — the leaf partition, tree order.

    rowver extract regions file.rvr [--tab 0] [--no-cuts]
                                    [--allow-unset-lumi] [-o DIR]

The map is a stage singleton (at most one per tab), identity
`t<tab>_s<stage>_map` (SCHEMAS.md kind `regions` v1). What is counted is
the map's saved `root` — a grid with no root is a recipe not yet applied
and the map shows ONE undivided region (the page's own regionCount rule;
`--from-map` on `extract flattened` is the explicit-request face that
derives a tree from the grid). Bin names are the viewer's regionBinName,
operators transliterated, duplicates suffixed like regionFlat.

Asimov significance (q0 = 2*sum[(s+b)ln(1+s/b) - s], Z = sqrt(q0), no
systematic) is computed exactly when the page would show it: a truthy
`sig` AND a non-empty `bkg` list; otherwise the stamped placeholder is
the page's own (z NaN, all counts 0). A contributing bin whose pooled
raw background is under 5 counts as `thin`; an s > 0, b <= 0 bin is
DROPPED AND COUNTED, never silently skipped. `neff` is the effective
entries per cell (w²/Σw², NaN when either moment is not positive). The
view's ZBi systematic (format fallback 5%) is stamped into meta.json for
the table's metric column.
"""
import argparse
import json
import sys
from pathlib import Path

import numpy as np

from . import ExtractError, core
from .. import rvr6

VERSION = 1


def dump(rvr: str | Path, tab: int = 0, no_cuts: bool = False,
         allow_unset_lumi: bool = False) -> dict:
    meta, buf = rvr6.read(rvr)
    data, view = meta["data"], meta.get("view") or {}
    columns = rvr6.columns(meta, buf)
    samples = [s["name"] for s in data["samples"]]
    ds = np.frombuffer(buf, np.dtype("<i4"), data["rows"], data["sample_at"])

    core.require_lumi(meta, allow_unset_lumi)
    tab_entry = core.tab_entry(meta, tab)
    cuts = {} if no_cuts else rvr6.effective(tab_entry, view, "cuts") or {}
    shown = core.shown_of(tab_entry, view, samples)
    keep = core.sel_mask(columns, data["cols"], cuts, shown, ds, samples)

    entries = [e for e in core.stage_entries(meta, tab) if e["kind"] == "map"]
    if not entries:
        raise ExtractError(f"tab {tab} has no region map")
    e = entries[0]  # a stage singleton — check6 refuses a second
    spec = e["spec"]

    num = core.region_count(meta, columns, ds, keep, spec)
    flat = core.region_flat(num)

    # the page's own gate: significance is ready only with a truthy sig
    # AND at least one background; the placeholder below is the page's too
    sig_name = spec.get("sig")
    backgrounds = [b for b in (spec.get("bkg") or []) if isinstance(b, str)]
    if core._js_truthy(sig_name) and backgrounds:
        asimov = core.region_sig(flat, core._js_str(sig_name), backgrounds)
    else:
        asimov = {"z": float("nan"), "bins": 0, "dropped": 0, "thin": 0}

    v = view.get("syst")
    syst_raw = core._js_number("0.05" if v is None else core._js_str(v))
    syst = syst_raw if np.isfinite(syst_raw) and syst_raw >= 0 else 0.05

    with np.errstate(invalid="ignore", divide="ignore"):
        neff = np.where((num["w"] > 0) & (num["w2"] > 0),
                        num["w"] * num["w"] / num["w2"], np.nan)

    return {"name": e["name"], "path": e["path"], "spec": spec,
            "bins": list(flat), "srcs": num["srcs"], "n": num["n"],
            "w": num["w"], "w2": num["w2"], "total": num["total"],
            "neff": neff, "asimov": asimov, "sig": sig_name,
            "bkg": backgrounds, "syst": syst,
            "weighted": num["weighted"], "leaves": num["leaves"],
            "active": core.active_cuts(data["cols"], cuts, None),
            "cuts": {n: core.json_cut_spec(
                         {c["name"]: c for c in data["cols"]}[n], cuts[n])
                     for n in core.active_cuts(data["cols"], cuts, None)},
            "lumi": {s: float(l) for s, l in
                     zip(samples, core.lumi_table(meta))},
            "look": {s: core.source_look(view, samples, s)
                     for s in num["srcs"]}}


def add_arguments(ap: argparse.ArgumentParser) -> None:
    ap.add_argument("rvr")
    ap.add_argument("--tab", type=int, default=0)
    ap.add_argument("--no-cuts", action="store_true",
                    help="do not apply the tab's cuts (visibility still applies)")
    ap.add_argument("--allow-unset-lumi", action="store_true")
    ap.add_argument("-o", "--out", default=None,
                    help="output directory (default ./extracts/<cart-stem>/)")


def run(a: argparse.Namespace) -> int:
    num = dump(a.rvr, tab=a.tab, no_cuts=a.no_cuts,
               allow_unset_lumi=a.allow_unset_lumi)
    out = Path(a.out) if a.out else Path("extracts") / Path(a.rvr).stem
    out.mkdir(parents=True, exist_ok=True)
    name = num["name"]

    np.savez(out / f"{name}.data.npz",
             bins=core.unicode_array(num["bins"]),
             sources=core.unicode_array(num["srcs"]),
             n=num["n"].astype("<f8"), w=num["w"].astype("<f8"),
             w2=num["w2"].astype("<f8"),
             total=np.float64(num["total"]),
             neff=num["neff"].astype("<f8"),
             asimov_z=np.float64(num["asimov"]["z"]),
             asimov_dropped=np.int8(num["asimov"]["dropped"]),
             asimov_thin=np.int8(num["asimov"]["thin"]))
    (out / f"{name}.meta.json").write_text(json.dumps(
        {"kind": "regions", "version": VERSION, "tab": a.tab,
         "cart": Path(a.rvr).name, "path": num["path"], "spec": num["spec"],
         "bins": num["bins"], "sources": num["srcs"],
         "total": core.json_num(num["total"]),
         "asimov": {k: core.json_num(v) if k == "z" else v
                    for k, v in num["asimov"].items()},
         "sig": num["sig"], "bkg": num["bkg"], "syst": num["syst"],
         "weighted": num["weighted"], "look": num["look"],
         "active": num["active"], "cuts": num["cuts"], "lumi": num["lumi"]},
        indent=4, sort_keys=True) + "\n")
    meta, _ = rvr6.read(a.rvr)
    (out / f"{name}.cuts.txt").write_text(
        "\n".join(core.cuts_lines(meta, a.tab, no_cuts=a.no_cuts)) + "\n")

    width = max((len(b) for b in num["bins"]), default=3)
    for j, b in enumerate(num["bins"]):
        tot = core._seq_sum(np.asarray(num["w"][j], float))
        print(f"{b:<{width}}  Σw {tot:>12.6g}  raw "
              f"{int(np.sum(num['n'][j])):>10,}")
    a_ = num["asimov"]
    print(f"asimov Z = {a_['z']:g} ({a_['bins']} bins, {a_['dropped']} "
          f"dropped, {a_['thin']} thin)")
    print(f"wrote {out / name}.{{data.npz,meta.json,cuts.txt}}",
          file=sys.stderr)
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
