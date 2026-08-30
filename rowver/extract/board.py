"""Dump the tab's yields board from a .rvr — one source per square, every
square read against ONE summed background shelf.

    rowver extract board file.rvr [--tab 0] [--no-cuts]
                                  [--allow-unset-lumi] [-o DIR]

The board is a stage singleton (at most one per tab), identity
`t<tab>_s<stage>_board` (SCHEMAS.md kind `board` v1). ONE pass, the
viewer's own (core/board.ts boardNumbers): every kept row is added to
every named source it belongs to, so a square and the shelf are
consistent by construction. The shelf sums FIRST — it is every square's
B — and pooling in bkgs order double-counts sources that share a
dataset, which is reported as `overlap` pairs, never silently.

The error mode is the board's own (`spec.err`, default poisson_neff);
`err_down`/`err_up` are ABSENT when it is `off`. Metrics (S/B,
S/√(B+ΔB²), ZBi) are written one array per metric SHOWN — an absent or
empty `met` means ALL THREE (board.ts:168), not none. The ZBi
systematic is the view's `syst` with the format's own fallback: a
garbled value means 5%, never a silent 0% — the value actually used is
stamped into meta.json only. Empty squares are `""` sources with NaN
numbers, distinct from a zero-yield source.
"""
import argparse
import json
import sys
from pathlib import Path

import numpy as np

from . import ExtractError, core
from .. import rvr6

VERSION = 1

METS = ("sob", "sig", "zbi")


def mets_of(spec: dict) -> list[str]:
    """Which figures of merit the board shows — board.ts metsOf(). ABSENT
    or empty means all three: a board written before the picker existed
    showed all three and must keep doing so."""
    want = spec.get("met")
    if not isinstance(want, list) or not want:
        return list(METS)
    return [k for k in want if k in METS]


def board_numbers(meta: dict, columns: dict, ds: np.ndarray, keep: np.ndarray,
                  spec: dict) -> dict:
    """The board's numbers over the kept rows — core/board.ts boardNumbers().

    check6 has already validated the spec (shape 1..4, known sources,
    enum err/met), so the viewer's coercion clamps have nothing to do.
    """
    view = meta.get("view") or {}
    samples = [s["name"] for s in meta["data"]["samples"]]
    rows, cols = spec.get("rows", 2), spec.get("cols", 2)
    cells = list(spec.get("cells") or [])[:rows * cols]
    cells += [None] * (rows * cols - len(cells))
    bkgs = [s for s in (spec.get("bkgs") or []) if s]

    names: list[str] = []
    for s in list(cells) + bkgs:
        if s and s not in names:
            names.append(s)
    keep = np.asarray(keep, bool)
    ww = core.row_weights(meta, columns, ds)
    acc: dict[str, dict] = {}
    members: dict[str, list[int]] = {}
    for s in names:
        members[s] = core.src_members(view, samples, s)
        sel = keep & np.isin(ds, members[s])
        acc[s] = {"w": core._seq_sum(ww[sel]),
                  "w2": core._seq_sum(ww[sel] ** 2),
                  "n": int(sel.sum())}

    raw_err = spec.get("err")
    mode = core._js_str(raw_err) if core._js_truthy(raw_err) \
        else "poisson_neff"
    # the format's fallback: a garbled view.syst means 5%, never a silent
    # 0% — two tools must not disagree about one number (board.ts:126)
    v = view.get("syst")
    syst_raw = core._js_number("0.05" if v is None else core._js_str(v))
    syst = syst_raw if np.isfinite(syst_raw) and syst_raw >= 0 else 0.05
    weighted = core.is_weighted(meta)

    # the shelf first: it is every square's B, so it exists before any
    # square is read; pooled sequentially in bkgs order (the viewer's +=)
    bkg = {"w": 0.0, "w2": 0.0, "n": 0}
    for s in bkgs:
        bkg["w"] += acc[s]["w"]
        bkg["w2"] += acc[s]["w2"]
        bkg["n"] += acc[s]["n"]

    fell_back = 0

    def read(src: str | None) -> dict:
        # the viewer's zero(): an empty square is ZEROS with NaN metrics in
        # memory — the wire's NaN-for-empty encoding is run()'s job
        nonlocal fell_back
        if not src:
            return {"src": "", "w": 0.0, "w2": 0.0, "n": 0, "err": None,
                    "sob": np.nan, "sig": np.nan, "zbi": np.nan}
        a = acc[src]
        e, fb = core.err_for(mode, a["w"], a["w2"], a["n"])
        if fb:
            fell_back += 1
        return {"src": src, "w": a["w"], "w2": a["w2"], "n": a["n"],
                "err": e,
                "sob": a["w"] / bkg["w"] if bkg["w"] > 0 else np.nan,
                "sig": core.sig_fom(a["w"], bkg["w"], bkg["w2"], weighted),
                "zbi": core.zbi(a["w"], bkg["w"], bkg["w2"], syst, weighted)}

    squares = [read(s) for s in cells]
    shelf = [read(s) for s in bkgs]

    overlap: list[tuple[str, str]] = []
    for i in range(len(bkgs)):
        for j in range(i + 1, len(bkgs)):
            if set(members[bkgs[i]]) & set(members[bkgs[j]]):
                overlap.append((bkgs[i], bkgs[j]))

    return {"rows": rows, "cols": cols, "cells": cells, "squares": squares,
            "shelf": shelf, "bkgs": bkgs, "bkg": bkg,
            "mode": mode, "syst": syst, "weighted": weighted,
            "fell_back": fell_back, "overlap": overlap,
            "sources": names, "mets": mets_of(spec)}


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

    entries = [e for e in core.stage_entries(meta, tab)
               if e["kind"] == "board"]
    if not entries:
        raise ExtractError(f"tab {tab} has no board")
    e = entries[0]  # a stage singleton — check6 refuses a second

    num = board_numbers(meta, columns, ds, keep, e["spec"])
    num["name"], num["path"], num["spec"] = e["name"], e["path"], e["spec"]
    num["active"] = core.active_cuts(data["cols"], cuts, None)
    by_name = {c["name"]: c for c in data["cols"]}
    num["cuts"] = {n: core.json_cut_spec(by_name[n], cuts[n])
                   for n in num["active"]}
    num["lumi"] = {s: float(l) for s, l in
                   zip(samples, core.lumi_table(meta))}
    num["look"] = {s: core.source_look(view, samples, s)
                   for s in num["sources"]}
    return num


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

    sq = num["squares"]

    def cell(key):  # the wire's NaN-for-empty, distinct from a zero yield
        return np.array([q[key] if q["src"] else np.nan for q in sq], "<f8")

    arrays = {
        "rows": np.int8(num["rows"]), "cols": np.int8(num["cols"]),
        "cell_sources": core.unicode_array([q["src"] for q in sq]),
        "raw": cell("n"),
        "yield": cell("w"),
        "w2": cell("w2"),
        "bkgs": core.unicode_array(num["bkgs"]),
        "bkg_raw": np.array([q["n"] for q in num["shelf"]], "<f8"),
        "bkg_w": np.array([q["w"] for q in num["shelf"]], "<f8"),
        "bkg_w2": np.array([q["w2"] for q in num["shelf"]], "<f8"),
        "bkg_total_raw": np.float64(num["bkg"]["n"]),
        "bkg_total_w": np.float64(num["bkg"]["w"]),
        "bkg_total_w2": np.float64(num["bkg"]["w2"]),
        "weighted": np.bool_(num["weighted"]),
        "err_fallback_count": np.int8(num["fell_back"]),
        "overlap": np.array(num["overlap"], dtype="<U64").reshape(-1, 2),
    }
    if num["mode"] != "off":
        arrays["err_down"] = np.array(
            [q["err"][0] if q["err"] else np.nan for q in sq], "<f8")
        arrays["err_up"] = np.array(
            [q["err"][1] if q["err"] else np.nan for q in sq], "<f8")
    for met in num["mets"]:
        arrays[met] = np.array([q[met] for q in sq], "<f8")
    np.savez(out / f"{name}.data.npz", **arrays)

    (out / f"{name}.meta.json").write_text(json.dumps(
        {"kind": "board", "version": VERSION, "tab": a.tab,
         "cart": Path(a.rvr).name, "path": num["path"], "spec": num["spec"],
         "err": num["mode"], "syst": num["syst"], "mets": num["mets"],
         "weighted": num["weighted"], "sources": num["sources"],
         "look": num["look"], "active": num["active"], "cuts": num["cuts"],
         "lumi": num["lumi"]},
        indent=4, sort_keys=True) + "\n")
    meta, _ = rvr6.read(a.rvr)
    (out / f"{name}.cuts.txt").write_text(
        "\n".join(core.cuts_lines(meta, a.tab, no_cuts=a.no_cuts)) + "\n")

    for r in range(num["rows"]):
        line = []
        for c in range(num["cols"]):
            q = num["squares"][r * num["cols"] + c]
            line.append(f"{q['src'] or '·':<12}"
                        + (f" {q['w']:>10.4g}" if q["src"] else " " * 11))
        print("  ".join(line))
    print(f"shelf B = {num['bkg']['w']:g} ({', '.join(num['bkgs']) or 'empty'})")
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
