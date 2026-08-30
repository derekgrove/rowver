"""Dump every 1D plot on a tab from a .rvr — the numbers the page draws.

    rowver extract plot1d file.rvr [--tab 0] [--zoom p80|p90|p95|p99|full]
                                   [--no-cuts] [--allow-unset-lumi] [-o DIR]

Each 1d stage entry (blocks included) becomes one output set, identity
`t<tab>_s<stage>[_c<cell>]_1d_<var>` (SCHEMAS.md kind `plot1d` v1):
`.data.npz` (edges, centres, sources, h, h2, n, tails, err bands),
`.meta.json` (the resolved spec + the window actually framed), `.cuts.txt`.

The window is the page's: an integer column gets one bin per value; AUTO
tracks --zoom (default p90, the viewer's default) through the packed
whole-cart quantile (p80..p99, required wire content), rounded to 4
significant digits; MANUAL bounds are parsed like the page parses them and
an inverted window falls back to the column's own range. The choice and
the resolved window are stamped into meta.json.

Sources are frame order — the viewer's sources() filtered to the plot's own
srcs/mute lists and to sources with at least one shown dataset. That is the
order ROOT stacks in; plotly re-sorts by total at draw time (layout, not
content). SHAPE-normalized plots are written normalized, n stays raw, and
error bands are computed on what is drawn — all the page's behavior.
"""
import argparse
import json
import sys
from pathlib import Path

import numpy as np

from . import ExtractError, core
from .. import rvr6

VERSION = 2  # v2: colours required in meta.json (2026-08-30)


def frame(spec: dict, col: dict, values, view: dict | None, samples: list[str],
          shown: dict[str, bool], ds, keep, ww, zoom: str = "p90") -> dict:
    """One resolved 1d plot — ui/frame.ts frameFor()'s numbers, no styling."""
    # JS truthiness on BOTH keys: an empty [] binspace is truthy there (and
    # then !== 'log', so linear), while '' or absent falls to the logx flag
    binspace = spec.get("binspace")
    logbx = binspace == "log" if core._js_truthy(binspace) \
        else core._js_truthy(spec.get("logx"))
    ax = core.resolve_axis(col, zoom, spec.get("xauto"),
                           spec.get("xmin"), spec.get("xmax"),
                           spec.get("nb"), logbx)
    lo, hi, nb = ax["lo"], ax["hi"], ax["nb"]
    oflow = spec.get("oflow") is not False
    srcs = spec.get("srcs")
    only = set(srcs) if isinstance(srcs, list) else None
    mute = spec.get("mute")
    muted = {x for x in mute if isinstance(x, str)} \
        if isinstance(mute, list) else set()
    keep = np.asarray(keep, bool)
    ds = np.asarray(ds)
    bins = []
    for name in core.sources(view, samples):
        if (only is not None and name not in only) or name in muted:
            continue
        members = [d for d in core.src_members(view, samples, name)
                   if shown.get(samples[d], True)]
        if not members:
            continue  # no shown dataset: the page's `visible` flag stays off
        b = core.binned(values, ww, keep & np.isin(ds, members),
                        lo, hi, nb, ax["log"], oflow)
        b["name"] = name
        bins.append(b)
    if spec.get("norm") == "shape":
        # every source to unit area; n stays raw counts — frameFor's own scale
        for b in bins:
            tot = core._seq_sum(np.asarray(b["h"], float))
            if not tot > 0:
                continue
            b["h"] = b["h"] / tot
            b["h2"] = b["h2"] / (tot * tot)  # a variance scales by the square
    mode = spec.get("err")
    err = None
    # the backends gate on the RAW value (truthy and !== 'off') but call
    # errFor(String(err)) — so err: ["sqrtn"] IS sqrtn, and err: ["off"]
    # stringifies to 'off', whose null bands the page draws as ZEROS
    if core._js_truthy(mode) and mode != "off":
        mode_s = mode if isinstance(mode, str) else core._js_str(mode)
        # the band rides what is drawn: the (possibly shape-scaled) moments
        err = (np.zeros((len(bins), nb)), np.zeros((len(bins), nb)))
        for k, b in enumerate(bins):
            for i in range(nb):
                e, _ = core.err_for(mode_s, float(b["h"][i]),
                                    float(b["h2"][i]), float(b["n"][i]))
                err[0][k, i], err[1][k, i] = e or (0.0, 0.0)
    ax.update({
        "edges": core.edge_list(lo, hi, nb, ax["log"]),
        "centres": core.centres(lo, hi, nb, ax["log"]),
        # a log request over a lo <= 0 window silently stays linear (bin.ts
        # edges); `log` records the request, log_active what actually spaced
        "log_active": core.edges(lo, hi, nb, ax["log"])["on"],
        "oflow": oflow, "bins": bins, "err": err,
    })
    return ax


def dump(rvr: str | Path, tab: int = 0, zoom: str = "p90",
         no_cuts: bool = False, allow_unset_lumi: bool = False) -> list[dict]:
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
    ww = core.row_weights(meta, columns, ds)
    by_name = {c["name"]: c for c in data["cols"]}
    active = core.active_cuts(data["cols"], cuts, None)

    plots = []
    for e in core.stage_entries(meta, tab):
        if e["kind"] != "1d":
            continue
        spec = e["spec"]
        col = by_name.get(spec.get("varx"))
        if col is None:
            continue  # frameFor returns null for an unknown x column
        f = frame(spec, col, columns[col["name"]], view, samples, shown,
                  ds, keep, ww, zoom)
        f["name"], f["path"], f["spec"] = e["name"], e["path"], spec
        f["sources"] = [b["name"] for b in f["bins"]]
        f["weighted"] = core.is_weighted(meta)
        f["look"] = {s: core.source_look(view, samples, s)
                     for s in f["sources"]}
        # resolved colours are stamped HERE, where the full context lives:
        # the viewer's assignment walks every global source and every look
        # namespace, which the record alone cannot reconstruct
        assignment = core.auto_colours(view, samples)
        f["colours"] = {s: assignment[s] for s in f["sources"]}
        f["active"] = active
        f["cuts"] = {n: core.json_cut_spec(by_name[n], cuts[n]) for n in active}
        f["lumi"] = {s: float(l)
                     for s, l in zip(samples, core.lumi_table(meta))}
        plots.append(f)
    if not plots:
        raise ExtractError(f"tab {tab} has no 1d plots")
    return plots


def add_arguments(ap: argparse.ArgumentParser) -> None:
    ap.add_argument("rvr")
    ap.add_argument("--tab", type=int, default=0)
    ap.add_argument("--zoom", choices=("p80", "p90", "p95", "p99", "full"),
                    default="p90",
                    help="AUTO-window quantile (the viewer's machine-local "
                         "setting; default p90, the viewer's default)")
    ap.add_argument("--no-cuts", action="store_true",
                    help="do not apply the tab's cuts (visibility still applies)")
    ap.add_argument("--allow-unset-lumi", action="store_true")
    ap.add_argument("-o", "--out", default=None,
                    help="output directory (default ./extracts/<cart-stem>/)")


def run(a: argparse.Namespace) -> int:
    plots = dump(a.rvr, tab=a.tab, zoom=a.zoom, no_cuts=a.no_cuts,
                 allow_unset_lumi=a.allow_unset_lumi)
    out = Path(a.out) if a.out else Path("extracts") / Path(a.rvr).stem
    out.mkdir(parents=True, exist_ok=True)
    meta, _ = rvr6.read(a.rvr)
    cuts_txt = "\n".join(core.cuts_lines(meta, a.tab, no_cuts=a.no_cuts)) + "\n"

    for f in plots:
        name = f["name"]
        arrays = {
            "edges": np.asarray(f["edges"], "<f8"),
            "centres": np.asarray(f["centres"], "<f8"),
            "sources": core.unicode_array(f["sources"]),
            "h": np.stack([b["h"] for b in f["bins"]]).astype("<f8")
                if f["bins"] else np.zeros((0, f["nb"]), "<f8"),
            "h2": np.stack([b["h2"] for b in f["bins"]]).astype("<f8")
                if f["bins"] else np.zeros((0, f["nb"]), "<f8"),
            "n": np.stack([b["n"] for b in f["bins"]]).astype("<f8")
                if f["bins"] else np.zeros((0, f["nb"]), "<f8"),
            "under": np.array([b["under"] for b in f["bins"]], "<f8"),
            "over": np.array([b["over"] for b in f["bins"]], "<f8"),
            "underW": np.array([b["underW"] for b in f["bins"]], "<f8"),
            "overW": np.array([b["overW"] for b in f["bins"]], "<f8"),
        }
        if f["err"] is not None:
            arrays["err_down"] = f["err"][0].astype("<f8")
            arrays["err_up"] = f["err"][1].astype("<f8")
        np.savez(out / f"{name}.data.npz", **arrays)
        (out / f"{name}.meta.json").write_text(json.dumps(
            {"kind": "plot1d", "version": VERSION, "tab": a.tab,
             "cart": Path(a.rvr).name, "path": f["path"], "spec": f["spec"],
             "edges": [core.json_num(v) for v in f["edges"]],
             "centres": [core.json_num(v) for v in f["centres"]],
             "window": {"lo": core.json_num(f["lo"]),
                        "hi": core.json_num(f["hi"]), "nb": f["nb"],
                        "ints": f["ints"], "log": f["log"],
                        "log_active": f["log_active"],
                        "zoom": f["zoom"], "from": f["window"]},
             "oflow": f["oflow"], "weighted": f["weighted"],
             "sources": f["sources"], "look": f["look"],
             "colours": f["colours"], "lumi": f["lumi"],
             "active": f["active"], "cuts": f["cuts"]},
            indent=4, sort_keys=True) + "\n")
        (out / f"{name}.cuts.txt").write_text(cuts_txt)

        spacing = " log" if f["log_active"] else \
            " log-requested-but-linear" if f["log"] else ""
        print(f"{name}: {f['spec']['varx']} [{f['lo']:g}, {f['hi']:g}] "
              f"nb={f['nb']}{spacing} ({f['window']}, zoom {f['zoom']})")
        for b in f["bins"]:
            line = f"  {b['name']:<20} raw {int(np.sum(b['n'])):>10,}"
            if f["weighted"]:
                line += f"  Σw {core._seq_sum(np.asarray(b['h'], float)):>14.6g}"
            print(line)
    print(f"wrote {len(plots)} plot(s) to {out}", file=sys.stderr)
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
