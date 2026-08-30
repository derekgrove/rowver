"""Render extracted plot records to PNG — a faithful plot of the SAME
numbers, never a pixel-clone claim (design decision, extract plan).

    rowver render <dir | name.meta.json> [-o DIR] [--pdf] [--dpi N]

Reads the three-file record (`.meta.json` + `.data.npz`) that
`rowver extract plot1d` wrote — the cartridge is not needed — and draws it
with the viewer's semantics so a side-by-side eyeball is easy:

- bars sources OVERLAY as outline step histograms by default; they stack
  (filled, cumulative, one total band) only when the spec's ``stack`` is
  truthy — the backends' own gate. Stacking is FRAME order, the ROOT
  backend's (the default engine); plotly's largest-at-bottom re-sort is
  draw-time layout.
- overlaid bars carry their own recorded error bars (bars always draw
  errors); a points source (``look.draw == 'points'``) is markers, never
  stacked, and ``errstyle: 'none'`` suppresses its bars — both viewer rules.
- the stacked band is err_for(mode, Σh, Σh2, Σn) with the totals
  accumulated source-by-source in draw order, the backends' own loop.
- colours come from the record's ``colours`` (stamped at extract time,
  where the viewer's global assignment is reproducible); a record without
  them falls back to a local palette walk over the drawn sources only.
- the y frame ports autoYRange (stacked positive/negative envelope, error
  reach, log floor from drawn positives, the two stacking orders); MANUAL
  y bounds (``yauto`` exactly false) apply only when BOTH parse finite and
  ordered — anything else keeps the full auto range — and on a log y axis
  the low bound clamps to 1e-12, plotly's rule.
- log-spaced bins draw with their true widths from ``edges``; ``logx``
  engages only when the window's low edge is positive (the record already
  resolved the silent-linear fallback as ``window.log_active``).

``--style`` picks an mplhep sheet: ``root`` (default — mplhep's ROOT
look-alike, closest to the site's default JSROOT engine, with no PyROOT
dependency), ``cms``/``atlas``/``lhcb``/``alice``, or ``plain`` (bare
matplotlib). A genuine PyROOT engine is deliberately NOT here: ROOT
cannot ride a pip extra, and the mplhep ROOT sheet covers the look.

Pull panels are DEFERRED to a later render pass, with boards and 2D.
Needs matplotlib + mplhep (``pip install rowver[render]``; ``plain``
works with matplotlib alone).
"""
import argparse
import json
import sys
import zipfile
from pathlib import Path

import numpy as np

from . import ExtractError, core

VERSION = 1  # renders plot1d v1 records

STYLES = ("root", "cms", "atlas", "lhcb", "alice", "plain")


def _style_context(style: str):
    """The mplhep sheet for --style, as a matplotlib style context."""
    import contextlib
    import matplotlib.pyplot as plt
    if style == "plain":
        return contextlib.nullcontext()
    try:
        import mplhep
    except ImportError as exc:
        raise ExtractError(
            f"--style {style} needs mplhep — pip install rowver[render] "
            f"(or use --style plain)") from exc
    sheet = {"root": mplhep.style.ROOT, "cms": mplhep.style.CMS,
             "atlas": mplhep.style.ATLAS, "lhcb": mplhep.style.LHCb2,
             "alice": mplhep.style.ALICE}[style]
    return plt.style.context(sheet)


def _fallback_colours(sources: list[str], look: dict) -> dict[str, str]:
    """Palette walk over the DRAWN sources only — for a record with no
    stamped ``colours``. Cannot reproduce the viewer's global slots."""
    taken = {str((look.get(s) or {}).get("colour", "")).lower()
             for s in sources} - {""}
    out: dict[str, str] = {}
    for s in sources:
        own = (look.get(s) or {}).get("colour")
        if own:
            out[s] = own
        else:
            c = next((p for p in core.PALETTE20 if p.lower() not in taken),
                     core.PALETTE20[len(out) % len(core.PALETTE20)])
            out[s] = c
            taken.add(c.lower())
    return out


def _band(mode, h_rows, h2_rows, n_rows) -> np.ndarray:
    """(nb, 2) [down, up] for the stacked total: totals accumulated
    source-by-source in draw order (the backends' loop), then err_for."""
    tot = np.zeros(h_rows.shape[1] if h_rows.size else 0)
    tot2, totn = np.zeros_like(tot), np.zeros_like(tot)
    for k in range(h_rows.shape[0]):
        tot = tot + h_rows[k]
        tot2 = tot2 + h2_rows[k]
        totn = totn + n_rows[k]
    mode_s = mode if isinstance(mode, str) else core._js_str(mode)
    return np.array([core.err_for(mode_s, float(tot[i]), float(tot2[i]),
                                  float(totn[i]))[0] or (0.0, 0.0)
                     for i in range(tot.size)]), tot


def _y_range(hs, h2s, ns, errs, errstyles, is_pts, stacked, logy,
             mode) -> tuple:
    """ui/frame.ts autoYRange(), over the record's rows. Overlay and points
    reach uses the RECORDED bands ``errs`` (the same numbers drawn); only
    the stacked total recomputes via ``mode`` (None = error model off)."""
    top, bottom, floor = 0.0, 0.0, np.inf

    def reach(y, dn, up):
        nonlocal top, bottom, floor
        if y + up > top:
            top = y + up
        if y - dn < bottom:
            bottom = y - dn
        if 0 < y < floor:
            floor = y
        if 0 < y - dn < floor:
            floor = y - dn

    def ext(k, i):
        if errs is None or (is_pts[k] and errstyles[k] == "none"):
            return (0.0, 0.0)
        return (float(errs[0][k][i]), float(errs[1][k][i]))

    bars = [k for k, p in enumerate(is_pts) if not p]
    nb = len(hs[bars[0]]) if bars else 0
    if stacked and bars:
        for i in range(nb):
            # explicit left-to-right accumulation, the viewer's own `+=` —
            # python's sum() compensates and numpy may reduce pairwise,
            # either of which can move a cancelling total
            pos = neg = w2 = n = 0.0
            for k in bars:
                y = float(hs[k][i])
                if y > 0:
                    pos += y
                else:
                    neg += y
                w2 += float(h2s[k][i])
                n += float(ns[k][i])
            reach(pos, 0.0, 0.0)
            if neg < 0:
                reach(neg, 0.0, 0.0)
            if mode is not None:
                e, _ = core.err_for(mode, pos + neg, w2, n)
                if e:
                    reach(pos + neg, e[0], e[1])
        if logy:
            # exactly two stacking orders exist in the product: frame order
            # (ROOT) and largest-total-first (plotly) — walk both floors
            by_total = sorted(
                bars, key=lambda k: -core._seq_sum(np.asarray(hs[k], float)))
            for order in (bars, by_total):
                for i in range(nb):
                    run = 0.0
                    for k in order:
                        run += float(hs[k][i])
                        if 0 < run < floor:
                            floor = run
    else:
        for k in bars:
            for i in range(nb):
                reach(float(hs[k][i]), *ext(k, i))
    for k, p in enumerate(is_pts):
        if p:
            for i in range(len(hs[k])):
                reach(float(hs[k][i]), *ext(k, i))
    if not logy:
        lo = bottom * 1.15 if bottom < 0 else 0.0
        hi = (top or 1.0) * 1.15
        if not hi > lo:
            hi = lo + 1
        return lo, hi
    if not np.isfinite(floor) or floor <= 0:
        floor = 0.5
    floor *= 0.5
    hi = (top or 1.0) * 3
    if not hi > floor:
        hi = floor * 10
    return floor, hi


def _draw(meta_path: Path, dpi: int, size: tuple[float, float],
          style: str = "root"):
    """Load and validate one record, return the drawn (fig, name) —
    render() saves it; the tests assert on it."""
    try:
        import matplotlib
        matplotlib.use("Agg")
        import matplotlib.pyplot as plt
    except ImportError as exc:
        raise ExtractError(
            "rendering needs matplotlib — pip install rowver[render]") from exc

    meta_path = Path(meta_path)
    try:
        meta = json.loads(meta_path.read_text())
    except (json.JSONDecodeError, UnicodeDecodeError) as exc:
        raise ExtractError(f"{meta_path.name}: not a record — {exc}") from exc
    if meta.get("kind") != "plot1d":
        raise ExtractError(f"{meta_path.name}: kind {meta.get('kind')!r} — "
                           f"only plot1d renders for now")
    if meta.get("version") != VERSION:
        raise ExtractError(f"{meta_path.name}: plot1d v{meta.get('version')} "
                           f"— this renderer draws v{VERSION}")
    if not isinstance(meta, dict) or not isinstance(meta.get("spec"), dict) \
            or not isinstance(meta.get("window"), dict):
        raise ExtractError(f"{meta_path.name}: malformed record metadata")
    name = meta_path.name.removesuffix(".meta.json")
    try:
        with np.load(meta_path.with_name(f"{name}.data.npz")) as z:
            edges = z["edges"]
            centres = z["centres"]
            sources = [str(s) for s in z["sources"]]
            h, h2, n = z["h"], z["h2"], z["n"]
            if ("err_down" in z) != ("err_up" in z):
                raise ExtractError(
                    f"{name}: one error band without its partner")
            err = (z["err_down"], z["err_up"]) if "err_down" in z else None
    except (OSError, KeyError, ValueError, zipfile.BadZipFile) as exc:
        raise ExtractError(f"{name}: unreadable npz — {exc}") from exc
    K, nb = len(sources), len(centres)
    if not (h.shape == h2.shape == n.shape == (K, nb)
            and edges.shape == (nb + 1,)
            and (err is None or err[0].shape == err[1].shape == (K, nb))):
        raise ExtractError(f"{name}: npz shapes disagree with the schema")
    try:  # the json carries the same edges/centres/sources — a record whose
        # mirrors contradict each other is refused, not silently resolved
        coherent = (list(meta.get("sources") or []) == sources
                    and np.array_equal(
                        np.asarray(meta.get("edges"), dtype=float), edges)
                    and np.array_equal(
                        np.asarray(meta.get("centres"), dtype=float), centres)
                    and meta["window"].get("nb") == nb)
    except (TypeError, ValueError):
        coherent = False
    if not coherent:
        raise ExtractError(f"{name}: npz and meta.json mirrors disagree")

    spec, look = meta["spec"], meta.get("look") or {}
    colours = meta.get("colours") or _fallback_colours(sources, look)
    widths = np.diff(edges)
    is_pts = [(look.get(s) or {}).get("draw") == "points" for s in sources]
    errstyles = [(look.get(s) or {}).get("errstyle") for s in sources]
    stacked = core._js_truthy(spec.get("stack"))
    raw_mode = spec.get("err")
    mode_on = core._js_truthy(raw_mode) and raw_mode != "off"
    mode_s = (raw_mode if isinstance(raw_mode, str)
              else core._js_str(raw_mode)) if mode_on else None
    if mode_on and err is None:
        raise ExtractError(f"{name}: spec has an error mode but the npz "
                           f"carries no bands")
    if not mode_on and err is not None:
        raise ExtractError(f"{name}: the npz carries error bands but the "
                           f"spec's error mode is off")
    bars = [k for k, p in enumerate(is_pts) if not p]

    with _style_context(style):
        fig, ax = plt.subplots(figsize=size, dpi=dpi)
        if stacked and bars:
            bottom = np.zeros(nb)
            for k in bars:
                s = sources[k]
                ax.bar(edges[:-1], h[k], width=widths, bottom=bottom,
                       align="edge", color=colours[s], edgecolor="white",
                       linewidth=0.3, label=s)
                bottom = bottom + h[k]
            if mode_on:
                band, tot = _band(raw_mode, h[bars], h2[bars], n[bars])
                ax.bar(edges[:-1], band[:, 0] + band[:, 1], width=widths,
                       bottom=tot - band[:, 0], align="edge", fill=False,
                       hatch="/////", edgecolor="0.35", linewidth=0.0,
                       label=f"stat ({raw_mode})")
        else:
            for k in bars:
                s = sources[k]
                ax.stairs(h[k], edges, color=colours[s], lw=1.6, label=s)
                if err is not None:  # the RECORDED bars, verbatim — bars
                    # always draw errors; errstyle is points-only
                    ax.errorbar(centres, h[k], yerr=(err[0][k], err[1][k]),
                                fmt="none", ecolor=colours[s], lw=1,
                                capsize=0)

        for k, s in enumerate(sources):
            if not is_pts[k]:
                continue
            show_err = err is not None and errstyles[k] != "none"
            yerr = (err[0][k], err[1][k]) if show_err else None
            ax.errorbar(centres, h[k], yerr=yerr, fmt="o", ms=4,
                        color=colours[s], label=s, capsize=0, lw=1.2)

        w = meta["window"]
        logy = core._js_truthy(spec.get("logy"))
        if core._js_truthy(spec.get("logx")) and w["lo"] > 0:
            ax.set_xscale("log")
        if logy:
            ax.set_yscale("log")
        lo_y, hi_y = _y_range(h, h2, n, err, errstyles, is_pts, stacked,
                              logy, mode_s)
        if spec.get("yauto") is False:
            # manual applies only when BOTH bounds parse finite and ordered
            # (the backends' rule); anything else keeps the full auto range
            my_lo = core.parse_float(spec.get("ymin"))
            my_hi = core.parse_float(spec.get("ymax"))
            if np.isfinite(my_lo) and np.isfinite(my_hi) and my_hi > my_lo:
                if logy:  # plotly clamps the log low bound, never rejects it
                    my_lo = max(my_lo, 1e-12)
                lo_y, hi_y = my_lo, my_hi
        ax.set_ylim(lo_y, hi_y)
        ax.set_xlim(edges[0], edges[-1])
        ax.set_xlabel(spec.get("xtitle") or spec.get("varx"))
        ax.set_ylabel(spec.get("ytitle") or
                      ("a.u." if spec.get("norm") == "shape"
                       else "Σw / bin" if meta.get("weighted")
                       else "events / bin"))
        # furniture keeps modest FIXED sizes in every style: the mplhep
        # sheets are tuned for square figures and their default typography
        # collides at wide aspect ratios; the sheet still owns ticks, axes,
        # fonts and line weights — the "look"
        ax.set_title(f"{meta.get('cart', '')} — {name}", loc="left",
                     fontsize=11)
        handles, _ = ax.get_legend_handles_labels()
        ax.legend(frameon=False, fontsize=9,
                  ncol=2 if len(handles) > 6 else 1)
        ax.annotate(f"window {w['from']} (zoom {w['zoom']})"
                    + (", log bins" if w.get("log_active") else "")
                    + (", stacked" if stacked else ""),
                    xy=(0.99, 1.02), xycoords="axes fraction",
                    ha="right", fontsize=8, color="0.4")
    return fig, name


def render(meta_path: str | Path, out_dir: str | Path | None = None,
           fmt: str = "png", dpi: int = 150,
           size: tuple[float, float] = (10.0, 5.0),
           style: str = "root") -> Path:
    """One record -> one image, written beside the record unless out_dir.

    ``size`` is the figure in inches. The page fills whatever viewport it
    gets, so there is no one true aspect ratio — this is the calibration
    knob (``--size WxH``): match your screen's panel once and reuse it.
    """
    if not (np.isfinite(dpi) and dpi > 0):
        raise ExtractError(f"--dpi {dpi} — must be positive and finite")
    if not (np.isfinite(size[0]) and size[0] > 0
            and np.isfinite(size[1]) and size[1] > 0):
        raise ExtractError(f"--size {size[0]}x{size[1]} — must be positive "
                           f"and finite")
    if style not in STYLES:
        raise ExtractError(f"--style {style!r} — have: {', '.join(STYLES)}")
    meta_path = Path(meta_path)
    try:
        fig, name = _draw(meta_path, dpi, size, style)
    except ExtractError:
        raise
    except (KeyError, TypeError, AttributeError, IndexError) as exc:
        # a structurally broken record must SKIP in a directory walk, not
        # abort it with a traceback
        raise ExtractError(f"{meta_path.name}: malformed record — "
                           f"{type(exc).__name__}: {exc}") from exc
    try:
        out = Path(out_dir) if out_dir else meta_path.parent
        out.mkdir(parents=True, exist_ok=True)
        target = out / f"{name}.{fmt}"
        fig.savefig(target, bbox_inches="tight", dpi=dpi)
    finally:
        import matplotlib.pyplot as plt
        plt.close(fig)
    return target


def add_arguments(ap: argparse.ArgumentParser) -> None:
    ap.add_argument("target",
                    help="a record directory, or one <name>.meta.json")
    ap.add_argument("-o", "--out", default=None,
                    help="image directory (default: beside the record)")
    ap.add_argument("--pdf", action="store_true", help="write PDF instead")
    ap.add_argument("--dpi", type=int, default=150)
    ap.add_argument("--size", default="10x5", metavar="WxH",
                    help="figure inches (default 10x5). The page fills the "
                         "viewport, so match your screen's panel here")
    ap.add_argument("--style", choices=STYLES, default="root",
                    help="mplhep sheet (default root — the JSROOT "
                         "look-alike); plain = bare matplotlib")


def _parse_size(text: str) -> tuple[float, float]:
    try:
        w, _, h = text.lower().partition("x")
        return (float(w), float(h))
    except ValueError:
        raise ExtractError(f"--size {text!r} — want WxH inches, e.g. 10x5") \
            from None


def run(a: argparse.Namespace) -> int:
    target = Path(a.target)
    metas = sorted(target.glob("*.meta.json")) if target.is_dir() else [target]
    fmt = "pdf" if a.pdf else "png"
    size = _parse_size(a.size)
    done = 0
    for m in metas:
        try:
            print(render(m, a.out, fmt=fmt, dpi=a.dpi, size=size,
                         style=a.style))
            done += 1
        except ExtractError as err:
            if not target.is_dir():
                raise  # a named record's failure is the command's failure
            print(f"skip {m.name}: {err}", file=sys.stderr)
    if not done:
        raise ExtractError(f"nothing rendered under {target}")
    return 0


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    add_arguments(ap)
    try:
        return run(ap.parse_args(argv))
    except (ExtractError, OSError) as err:
        print(err, file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
