"""Build the four frozen v4 goldens — run ONCE at 0.13.0, committed, then never re-run.

These replace golden_0.9.11 / 0.10.0 / 0.11.0, which are v3 and can no longer be opened:
0.13.0 declares its weights in META and drops the v2/v3 readers, so a file with weights at
an undeclared byte 0 is refused rather than guessed at. The three SCENARIOS they guarded
are carried over unchanged, and a fourth is added for the capability that caused the
version bump.

    _tabs   two tabs opening on the SECOND, made_by, groups + a loose sample, an FOM pair,
            continuous and chip cuts, shown, and per-plot cmt / draw / merge / lines /
            sheet+quad / layers / err / shape-norm. Every optional part of the contract.
    _table  a board with NO `met` key — pins that its absence still means all three FOMs.
    _met    `met = ["zbi","sob"]`, deliberately OUT of canonical order, so a reader that
            tidies a saved board goes red instead of silently rearranging it.
    _data   NO WEIGHTS. The case v4 exists for: weights false, no column at offset 0, the
            weighted/raw toggle and the luminosity box gone, S/sqrt(B) instead of
            S/sqrt(B+sigma^2), and the two error modes that actually differ.

THIS SCRIPT NO LONGER RUNS, ON PURPOSE. It is the record of how the four goldens were
made, not a thing to re-run: a golden is frozen bytes with a sha256 pin in smoke.py, so
rebuilding one would break the very guard it exists to be. It also reads a `demo_higgs`
study that this project no longer carries. Read it to see what the goldens contain; to
add a NEW capability, cut a NEW golden beside these rather than regenerating them.

The first three are CMS Open Data (an H->ZZ->4l study, CC0) strided down to
~23k events; the fourth is the demo dimuon study (DoubleMuParked 2012, CC0) strided to a
similar size. No locks: a shipped lock would trip the battery's "locks must ship off"
check. mtime=0 keeps the bytes deterministic, so re-running reproduces identical files
unless something real changed — the sha pins in smoke.py are the enforcement.
"""
import gzip
import hashlib
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import load
import page
import study

HERE = Path(__file__).parent
STRIDE = 16

SIG = "MC · H(125)→ZZ→4l"
ZZ4MU, ZZ4E, ZZ2E2MU = "MC · ZZ→4mu", "MC · ZZ→4e", "MC · ZZ→2e2mu"
GROUPS = {"signal": [SIG], "ZZ": [ZZ4MU, ZZ4E]}      # ZZ2E2MU stays loose on purpose
TIGHT = {"MZ1": [76, 106], "MZ2": [20, 120], "M4L": [70, 180], "channel": [0, 2]}
BOARD = {"rows": 2, "cols": 2,
         "cells": ["signal", "ZZ", ZZ2E2MU, None],
         "bkgs": ["ZZ", ZZ2E2MU],
         "plots": [{"varx": "M4L", "nb": 30, "logy": True, "logz": True}],
         "channel": "metrics", "err": "poisson_neff"}


def strided(name: str) -> dict:
    """Every 16th event of each sample — shapes survive, size does not."""
    st = study.load(name)
    full = load.load(name, st["dir"])
    return ({ds: {c: v[::STRIDE] for c, v in cols.items()} for ds, cols in full.items()},
            getattr(st["config"], "LABELS", None),
            study.weights(st["config"], st["dir"] / "config.py"))


def wide_state(meta):
    """Tab 1: loose cuts, five plots exercising draw, merge, lines and a sheet pair."""
    return page.state(
        meta, cuts={"MZ1": [40, 120], "MZ2": [12, 120], "PT_lep_lead": [5, 200]},
        groups=GROUPS, fom=("signal", "ZZ"), shown=["*"], lumi=1,
        cms="off", rlabel="CMS open data",
        plots=[
            {"varx": "M4L", "nb": 55, "xauto": False, "xmin": 70, "xmax": 180,
             "logy": True,
             "cmt": "golden file, frozen at RowVer 0.13.0 — if a page cannot open this, "
                    "the .rvr format broke its promise",
             "draw": {"1d|M4L": {
                 "shapes": [{"type": "line", "x0": 125.0, "x1": 125.0,
                             "y0": 0.0, "y1": 0.9, "xref": "x", "yref": "paper",
                             "line": {"color": "#E8384F", "width": 2}}],
                 "notes": [{"text": "H(125)", "x": 125.0, "xref": "x", "y": 0.92,
                            "yref": "paper", "showarrow": True, "ax": 46, "ay": -8,
                            "arrowhead": 3, "arrowwidth": 1.5, "arrowcolor": "#E8384F",
                            "font": {"color": "#E8384F", "size": 13}}]}}},
            {"mode": "2d", "varx": "MZ1", "vary": "MZ2", "nb": 40,
             "layer": "signal", "layer2": "ZZ", "view": "two", "logz": True},
            {"varx": "cosThetaStar", "nb": 20, "norm": "shape",
             "merge": {"ZZ": "all ZZ", ZZ2E2MU: "all ZZ"}},
            {"varx": "PT_lep_lead", "nb": 40, "sheet": 1, "quad": 0, "lines": ["signal"]},
            {"varx": "AbsEta_lep_lead", "nb": 30, "sheet": 1, "quad": 1},
        ])


def zoom_state(meta):
    """Tab 2, the boot tab: tight Z windows, the 4e channel chipped off, err bars on."""
    return page.state(
        meta, cuts=TIGHT, groups=GROUPS, fom=("signal", "ZZ"), shown=["*"], lumi=1,
        expcuts=True,
        plots=[
            {"varx": "M4L", "nb": 40, "xauto": False, "xmin": 70, "xmax": 180, "err": True,
             "cmt": "the boot tab — the file says open here, not on tab 1"},
            {"mode": "2d", "varx": "MZ1", "vary": "MZ2", "nb": 30,
             "layer": "signal", "layer2": "ZZ", "view": "three", "cmpmode": "ratio"},
        ])


def board_state(meta, met=None):
    """A 2x2 board on the metrics channel over a two-sample shelf."""
    table = dict(BOARD) if met is None else dict(BOARD, met=met)
    return page.state(
        meta, cuts=TIGHT, groups=GROUPS, fom=("signal", "ZZ"), shown=["*"], lumi=1,
        table=table,
        plots=[{"varx": "M4L", "nb": 40, "xauto": False, "xmin": 70, "xmax": 180,
                "err": "sumw2"}])


def data_states(meta):
    """The weightless pair: the spectrum, then the Z with Poisson errors and a 2D map."""
    spectrum = page.state(
        meta, cuts={"q_pair": [-1]}, lumi=1, cms="off",
        plots=[{"varx": "M_mumu", "nb": 120, "logx": True, "logy": True, "lbx": True,
                "span": "full",
                "cmt": "golden file, frozen at RowVer 0.13.0 — CMS open DATA, no weights"}])
    zed = page.state(
        meta, cuts={"q_pair": [-1], "M_mumu": [60, 120]}, lumi=1, cms="off",
        plots=[{"varx": "M_mumu", "nb": 60, "xauto": False, "xmin": 60, "xmax": 120,
                "err": "poisson_raw", "span": "full"},
               {"mode": "2d", "varx": "M_mumu", "vary": "AbsEta_mu_lead", "nb": 30,
                "span": "full"}])
    return spectrum, zed


def write(out: Path, columns, labels, wcol, tabs, tab):
    """Deterministic bytes: mtime=0, so the sha pin means what it says."""
    meta, buf = page.build(columns, labels=labels, weights=wcol,
                           tabs=tabs, extra={"tab": tab})
    meta["tabs"], meta["tab"], meta["title"] = tabs, tab, out.name
    header = json.dumps(meta).encode()
    out.write_bytes(gzip.compress(
        len(header).to_bytes(4, "little") + header + buf, 6, mtime=0))
    sha = hashlib.sha256(out.read_bytes()).hexdigest()
    print(f"{out.name:28s} {out.stat().st_size/1e6:5.2f} MB  n={meta['n']:>7,}  "
          f"samples={len(meta['datasets'])}  tabs={len(meta['tabs'])} tab={meta['tab']}  "
          f"weights={meta['weights']!r}")
    print(f"{'':28s} sha256: {sha}")
    return meta


def main() -> None:
    cols_h, labels_h, w_h = strided("demo_higgs")
    meta0, _ = page.pack(cols_h, labels_h, w_h)
    wide, zoom = wide_state(meta0), zoom_state(meta0)

    write(HERE / "golden_0.13.0_tabs.rvr", cols_h, labels_h, w_h,
          [{"name": "wide", "state": wide}, {"name": "zoom", "state": zoom}], 1)
    write(HERE / "golden_0.13.0_table.rvr", cols_h, labels_h, w_h,
          [{"name": "plots", "state": zoom}, {"name": "board", "state": board_state(meta0)}], 0)
    write(HERE / "golden_0.13.0_met.rvr", cols_h, labels_h, w_h,
          [{"name": "plots", "state": zoom},
           {"name": "board", "state": board_state(meta0, ["zbi", "sob"])}], 0)

    cols_d, labels_d, w_d = strided("dimuon_demo")
    assert w_d is None, "the data golden must be the weightless one"
    metad, _ = page.pack(cols_d, labels_d, w_d)
    spectrum, zed = data_states(metad)
    write(HERE / "golden_0.13.0_data.rvr", cols_d, labels_d, w_d,
          [{"name": "spectrum", "state": spectrum}, {"name": "the Z", "state": zed}], 0)


if __name__ == "__main__":
    main()
