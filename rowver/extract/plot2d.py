"""Dump every 2D (or 3D) plot on a tab from a .rvr — the grid the page draws.

    rowver extract plot2d file.rvr [--tab 0] [--zoom p80|p90|p95|p99|full]
                                   [--no-cuts] [--allow-unset-lumi] [-o DIR]
    rowver extract plot3d ...      (same record; the viewer's 3D mode renders
                                    the SAME two-variable grid as a surface)

Each 2d stage entry (blocks included) becomes one output set, identity
`t<tab>_s<stage>[_c<cell>]_2d_<varx>_<vary>` (SCHEMAS.md kind `plot2d` v1).
The parity object is the viewer's aggregate: `h` (nby, nbx) — row-major,
y outer, exactly binned2D's flat grid reshaped — plus scalar `kept`.
Per-source moment grids ride as labeled extras (`x_h_<source>`,
`x_h2_<source>`, `x_n_<source>`), an extension, not a parity claim.

Both axes resolve exactly as frameFor frames them: an integer column gets
one bin per value; AUTO tracks --zoom through the packed whole-cart
quantile; MANUAL bounds parse like the page and an inverted window falls
back to the column's own range. The y bin count is the page's
`nby ?? nb`; y log spacing couples to `binspacey`/`logy` the way x couples
to `binspace`/`logx`. A plot with its own srcs/mute lists narrows the grid
through the page's per-DATASET filter (frameFor's okDs); without them the
grid is of the whole selection.
"""
import argparse
import json
import sys
from pathlib import Path

import numpy as np

from . import ExtractError, core
from .. import rvr6

VERSION = 1


def frame(spec: dict, colx: dict, coly: dict, vx, vy, view: dict | None,
          samples: list[str], shown: dict[str, bool], ds, keep, ww,
          zoom: str = "p90") -> dict:
    """One resolved 2d/3d plot — ui/frame.ts frameFor()'s grid, no styling."""
    binspace = spec.get("binspace")
    logbx = binspace == "log" if core._js_truthy(binspace) \
        else core._js_truthy(spec.get("logx"))
    binspacey = spec.get("binspacey")
    logby = binspacey == "log" if core._js_truthy(binspacey) \
        else core._js_truthy(spec.get("logy"))
    ax = core.resolve_axis(colx, zoom, spec.get("xauto"),
                           spec.get("xmin"), spec.get("xmax"),
                           spec.get("nb"), logbx)
    # the page's `spec.nby ?? spec.nb`: null and absent both fall to nb
    nby_raw = spec.get("nby")
    ay = core.resolve_axis(coly, zoom, spec.get("yauto"),
                           spec.get("ymin"), spec.get("ymax"),
                           spec.get("nb") if nby_raw is None else nby_raw,
                           logby)
    oflow = spec.get("oflow") is not False
    keep = np.asarray(keep, bool)
    ds = np.asarray(ds)
    drawn, narrows = core.drawn_sources(view, samples, shown, spec)

    # okDs exists ONLY when the plot narrows (srcs or mute) — frameFor builds
    # it from sourceAt, so an EMPTY srcs list means an empty grid, and a
    # hidden member dataset is off in it (keep already excludes those rows)
    ok = None
    if narrows:
        ok = np.zeros(len(samples), bool)
        for _, members in drawn:
            ok[members] = True

    b2 = core.binned2d(vx, vy, ww, keep,
                       ax["lo"], ax["hi"], ax["nb"],
                       ay["lo"], ay["hi"], ay["nb"],
                       ax["log"], ay["log"], oflow, ds=ds, ok_ds=ok)

    # per-source extras: the same fills split by source membership — an
    # extension over binned2D, which returns no per-source 2D
    extras = []
    for name, members in drawn:
        g = core.binned2d(vx, vy, ww, keep & np.isin(ds, members),
                          ax["lo"], ax["hi"], ax["nb"],
                          ay["lo"], ay["hi"], ay["nb"],
                          ax["log"], ay["log"], oflow)
        extras.append({"name": name, "h": g["h"], "h2": g["h2"], "n": g["n"]})

    for a in (ax, ay):
        a["edges"] = core.edge_list(a["lo"], a["hi"], a["nb"], a["log"])
        a["centres"] = core.centres(a["lo"], a["hi"], a["nb"], a["log"])
        # a log request over a lo <= 0 window silently stays linear (bin.ts
        # edges); `log` records the request, log_active what actually spaced
        a["log_active"] = core.edges(a["lo"], a["hi"], a["nb"], a["log"])["on"]
    return {"x": ax, "y": ay, "oflow": oflow,
            "h": b2["h"], "kept": b2["kept"], "extras": extras}


def dump(rvr: str | Path, tab: int = 0, zoom: str = "p90",
         no_cuts: bool = False, allow_unset_lumi: bool = False,
         kind: str = "2d") -> list[dict]:
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
        if e["kind"] != kind:
            continue
        spec = e["spec"]
        colx = by_name.get(spec.get("varx"))
        coly = by_name.get(spec.get("vary"))
        if colx is None or coly is None:
            continue  # frameFor returns null for an unknown x or y column
        f = frame(spec, colx, coly, columns[colx["name"]], columns[coly["name"]],
                  view, samples, shown, ds, keep, ww, zoom)
        f["name"], f["path"], f["spec"] = e["name"], e["path"], spec
        f["sources"] = [x["name"] for x in f["extras"]]
        f["weighted"] = core.is_weighted(meta)
        f["look"] = {s: core.source_look(view, samples, s)
                     for s in f["sources"]}
        assignment = core.auto_colours(view, samples)
        f["colours"] = {s: assignment[s] for s in f["sources"]}
        f["active"] = active
        f["cuts"] = {n: core.json_cut_spec(by_name[n], cuts[n]) for n in active}
        f["lumi"] = {s: float(l)
                     for s, l in zip(samples, core.lumi_table(meta))}
        plots.append(f)
    if not plots:
        raise ExtractError(f"tab {tab} has no {kind} plots")
    return plots


def _extra_keys(sources: list[str]) -> list[str]:
    """Sanitized per-source key stems, GLOBALLY unique: a collision (with a
    natural name or an earlier generated suffix — "a_b_2" exists both ways)
    increments until the candidate is unused, stack order."""
    out: list[str] = []
    used: set[str] = set()
    for s in sources:
        base = k = core._sanitize(s)
        i = 1
        while k in used:
            i += 1
            k = f"{base}_{i}"
        used.add(k)
        out.append(k)
    return out


def add_arguments(ap: argparse.ArgumentParser) -> None:
    ap.add_argument("rvr")
    ap.add_argument("--tab", type=int, default=0)
    ap.add_argument("--zoom", choices=("p80", "p90", "p95", "p99", "full"),
                    default="p90",
                    help="AUTO-window quantile for BOTH axes (the viewer's "
                         "machine-local setting; default p90, the viewer's default)")
    ap.add_argument("--no-cuts", action="store_true",
                    help="do not apply the tab's cuts (visibility still applies)")
    ap.add_argument("--allow-unset-lumi", action="store_true")
    ap.add_argument("-o", "--out", default=None,
                    help="output directory (default ./extracts/<cart-stem>/)")


def run(a: argparse.Namespace, kind: str = "2d") -> int:
    plots = dump(a.rvr, tab=a.tab, zoom=a.zoom, no_cuts=a.no_cuts,
                 allow_unset_lumi=a.allow_unset_lumi, kind=kind)
    out = Path(a.out) if a.out else Path("extracts") / Path(a.rvr).stem
    out.mkdir(parents=True, exist_ok=True)
    meta, _ = rvr6.read(a.rvr)
    cuts_txt = "\n".join(core.cuts_lines(meta, a.tab, no_cuts=a.no_cuts)) + "\n"

    for f in plots:
        name = f["name"]
        nbx, nby = f["x"]["nb"], f["y"]["nb"]
        keys = _extra_keys(f["sources"])
        arrays = {
            "h": np.asarray(f["h"], "<f8").reshape(nby, nbx),
            "kept": np.array(f["kept"], "<i8"),
            "edges_x": np.asarray(f["x"]["edges"], "<f8"),
            "edges_y": np.asarray(f["y"]["edges"], "<f8"),
            "centres_x": np.asarray(f["x"]["centres"], "<f8"),
            "centres_y": np.asarray(f["y"]["centres"], "<f8"),
            "sources": core.unicode_array(f["sources"]),
        }
        for k, x in zip(keys, f["extras"]):
            arrays[f"x_h_{k}"] = np.asarray(x["h"], "<f8").reshape(nby, nbx)
            arrays[f"x_h2_{k}"] = np.asarray(x["h2"], "<f8").reshape(nby, nbx)
            arrays[f"x_n_{k}"] = np.asarray(x["n"], "<f8").reshape(nby, nbx)
        np.savez(out / f"{name}.data.npz", **arrays)

        def axis_meta(a_: dict) -> dict:
            return {"lo": core.json_num(a_["lo"]), "hi": core.json_num(a_["hi"]),
                    "nb": a_["nb"], "ints": a_["ints"], "log": a_["log"],
                    "log_active": a_["log_active"],
                    "zoom": a_["zoom"], "from": a_["window"]}

        (out / f"{name}.meta.json").write_text(json.dumps(
            {"kind": f"plot{kind}", "version": VERSION, "tab": a.tab,
             "cart": Path(a.rvr).name, "path": f["path"], "spec": f["spec"],
             "edges": {"x": [core.json_num(v) for v in f["x"]["edges"]],
                       "y": [core.json_num(v) for v in f["y"]["edges"]]},
             "centres": {"x": [core.json_num(v) for v in f["x"]["centres"]],
                         "y": [core.json_num(v) for v in f["y"]["centres"]]},
             "window": {"x": axis_meta(f["x"]), "y": axis_meta(f["y"])},
             "oflow": f["oflow"], "weighted": f["weighted"],
             "kept": f["kept"], "sources": f["sources"],
             "extras": dict(zip(keys, f["sources"])),
             "look": f["look"], "colours": f["colours"], "lumi": f["lumi"],
             "active": f["active"], "cuts": f["cuts"]},
            indent=4, sort_keys=True) + "\n")
        (out / f"{name}.cuts.txt").write_text(cuts_txt)

        def span(a_: dict) -> str:
            spacing = " log" if a_["log_active"] else \
                " log-requested-but-linear" if a_["log"] else ""
            return f"[{a_['lo']:g}, {a_['hi']:g}] nb={a_['nb']}{spacing} " \
                   f"({a_['window']})"
        print(f"{name}: {f['spec']['varx']} {span(f['x'])} × "
              f"{f['spec']['vary']} {span(f['y'])} zoom {f['x']['zoom']}, "
              f"kept {f['kept']:,}")
        for x in f["extras"]:
            line = f"  {x['name']:<20} raw {int(np.sum(x['n'])):>10,}"
            if f["weighted"]:
                line += f"  Σw {core._seq_sum(np.asarray(x['h'], float)):>14.6g}"
            print(line)
    print(f"wrote {len(plots)} plot(s) to {out}", file=sys.stderr)
    return 0


def main(argv: list[str] | None = None, kind: str = "2d") -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    add_arguments(ap)
    try:
        return run(ap.parse_args(argv), kind=kind)
    except (ExtractError, rvr6.FormatError, OSError) as err:
        print(err, file=sys.stderr)
        return 1


def main3d(argv: list[str] | None = None) -> int:
    return main(argv, kind="3d")


if __name__ == "__main__":
    raise SystemExit(main())
