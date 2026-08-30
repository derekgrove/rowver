"""The effective-state resolver: what the viewer page actually resolves.

Every extractor is a thin consumer of this module. It ports, faithfully:

- stage iteration: block traversal, ``tabs[].plot`` defaults merged into every
  leaf, ``view.stage`` shadowing/locks, stable stage paths
  (rowver_web ``rvr6.ts`` plotOfStage:913 / stageIntoState:932,
  ``state.ts`` newPlot:19)
- cut resolution: ``activeCuts`` ordering — ``cutorder`` first, then column
  order — and the visibility/``shown`` mask (``select.ts`` :39, :52)
- the region walk: ``treeFromGrid`` + the region tree, NOT a Cartesian
  product (``region.ts`` :304, :83, :132)
- the math: binning (``bin.ts``), error modes and FOMs (``stats.ts``,
  arithmetic verbatim), lumi/group resolution (``rvr6.lumi_of``)

NaN facts pinned by the shared fixture, all the page's own behavior: a NaN
value PASSES a range cut in the selection mask (``x < lo || x > hi`` rejects;
NaN compares false) and FAILS a chip cut. In LINEAR binning a NaN row counts
as kept but fills no bin and neither tail; in LOG binning ``x > 0`` is false
for NaN, so it rides with x <= 0 — crammed into the low bin when overflow is
on, dropped when it is off — and never counts as a tail.

Tolerance for cross-implementation parity: raw counts exact, weighted sums
1e-8 rtol. Signed reductions accumulate left-to-right in row order like the
page (``_seq_sum``) so cancellation cannot flip a sign, but bit parity is
still never claimed.
"""
from __future__ import annotations

import math
import re
from copy import deepcopy
from decimal import ROUND_HALF_UP, Decimal
from typing import Any

import numpy as np

from . import ExtractError
from .. import rvr6

MAX_CELLS = 6

# ---------------------------------------------------------------- stats (stats.ts) ----

_LANCZOS = (676.5203681218851, -1259.1392167224028, 771.32342877765313,
            -176.61502916214059, 12.507343278686905, -0.13857109526572012,
            9.9843695780195716e-6, 1.5056327351493116e-7)


def log_gamma(x: float) -> float:
    if x < 0.5:
        return math.log(math.pi / math.sin(math.pi * x)) - log_gamma(1 - x)
    x -= 1
    a = 0.99999999999980993
    t = x + 7.5
    for i in range(8):
        a += _LANCZOS[i] / (x + i + 1)
    return 0.5 * math.log(2 * math.pi) + (x + 0.5) * math.log(t) - t + math.log(a)


def betacf(a: float, b: float, x: float) -> float:
    tiny = 1e-30
    qab, qap, qam = a + b, a + 1, a - 1
    c, d = 1.0, 1 - qab * x / qap
    if abs(d) < tiny:
        d = tiny
    d = 1 / d
    h = d
    for m in range(1, 501):
        m2 = 2 * m
        aa = m * (b - m) * x / ((qam + m2) * (a + m2))
        d = 1 + aa * d
        if abs(d) < tiny:
            d = tiny
        c = 1 + aa / c
        if abs(c) < tiny:
            c = tiny
        d = 1 / d
        h *= d * c
        aa = -(a + m) * (qab + m) * x / ((a + m2) * (qap + m2))
        d = 1 + aa * d
        if abs(d) < tiny:
            d = tiny
        c = 1 + aa / c
        if abs(c) < tiny:
            c = tiny
        d = 1 / d
        del_ = d * c
        h *= del_
        if abs(del_ - 1) < 3e-16:
            break
    return h


def ibeta(a: float, b: float, x: float) -> float:
    if x <= 0:
        return 0.0
    if x >= 1:
        return 1.0
    lb = (log_gamma(a + b) - log_gamma(a) - log_gamma(b)
          + a * math.log(x) + b * math.log(1 - x))
    if x < (a + 1) / (a + b + 2):
        return math.exp(lb) * betacf(a, b, x) / a
    return 1 - math.exp(lb) * betacf(b, a, 1 - x) / b


_NI_A = (-3.969683028665376e+01, 2.209460984245205e+02, -2.759285104469687e+02,
         1.383577518672690e+02, -3.066479806614716e+01, 2.506628277459239e+00)
_NI_B = (-5.447609879822406e+01, 1.615858368580409e+02, -1.556989798598866e+02,
         6.680131188771972e+01, -1.328068155288572e+01)
_NI_C = (-7.784894002430293e-03, -3.223964580411365e-01, -2.400758277161838e+00,
         -2.549732539343734e+00, 4.374664141464968e+00, 2.938163982698783e+00)
_NI_D = (7.784695709041462e-03, 3.224671290700398e-01, 2.445134137142996e+00,
         3.754408661907416e+00)


def norm_inv(p: float) -> float:
    """Acklam's inverse normal CDF, arithmetic identical to the page's."""
    a, b, c, d = _NI_A, _NI_B, _NI_C, _NI_D
    if p <= 0 or p >= 1:
        return math.nan
    if p < 0.02425:
        q = math.sqrt(-2 * math.log(p))
        return ((((((c[0] * q + c[1]) * q + c[2]) * q + c[3]) * q + c[4]) * q + c[5])
                / ((((d[0] * q + d[1]) * q + d[2]) * q + d[3]) * q + 1))
    if p <= 0.97575:
        q = p - 0.5
        r = q * q
        return (((((a[0] * r + a[1]) * r + a[2]) * r + a[3]) * r + a[4]) * r + a[5]) * q \
            / (((((b[0] * r + b[1]) * r + b[2]) * r + b[3]) * r + b[4]) * r + 1)
    q = math.sqrt(-2 * math.log(1 - p))
    return -((((((c[0] * q + c[1]) * q + c[2]) * q + c[3]) * q + c[4]) * q + c[5])
             / ((((d[0] * q + d[1]) * q + d[2]) * q + d[3]) * q + 1))


def gamma_p(a: float, x: float) -> float:
    if x <= 0 or a <= 0:
        return 0.0
    if x < a + 1:
        ap, s = a, 1 / a
        del_ = s
        for _ in range(500):
            ap += 1
            del_ *= x / ap
            s += del_
            if abs(del_) < abs(s) * 1e-14:
                break
        return s * math.exp(-x + a * math.log(x) - log_gamma(a))
    b, c, d = x + 1 - a, 1e300, 1 / (x + 1 - a)
    h = d
    for i in range(1, 501):
        an = -i * (i - a)
        b += 2
        d = an * d + b
        if abs(d) < 1e-300:
            d = 1e-300
        c = b + an / c
        if abs(c) < 1e-300:
            c = 1e-300
        d = 1 / d
        del_ = d * c
        h *= del_
        if abs(del_ - 1) < 1e-14:
            break
    return 1 - math.exp(-x + a * math.log(x) - log_gamma(a)) * h


def gamma_p_inv(a: float, pr: float) -> float:
    if a <= 0:
        return 0.0
    lo, hi = 0.0, a + 10 * math.sqrt(a) + 20
    guard = 0
    while gamma_p(a, hi) < pr and guard < 60:
        hi *= 2
        guard += 1
    for _ in range(120):
        m = (lo + hi) / 2
        if gamma_p(a, m) < pr:
            lo = m
        else:
            hi = m
    return (lo + hi) / 2


EALPHA = (1 - 0.6826894921370859) / 2

ERR_MODES = ("off", "sumw2", "sqrtn", "poisson_neff", "poisson_raw")


def garwood(n: float) -> tuple[float, float]:
    """Garwood's exact Poisson 68.27% interval, as distances from n."""
    return (n - (gamma_p_inv(n, EALPHA) if n > 0 else 0),
            gamma_p_inv(n + 1, 1 - EALPHA) - n)


def err_for(mode: str, sw: float, sw2: float, n: float) \
        -> tuple[tuple[float, float] | None, bool]:
    """[down, up] as positive distances, and whether a Poisson form fell back."""
    sym = math.sqrt(max(sw2, 0))
    if mode == "off":
        return None, False
    if mode == "sqrtn":
        e = abs(sw / n if n > 0 else 0) * math.sqrt(n)
        return (e, e), False
    if mode == "poisson_neff":
        if not sw > 0 or not sw2 > 0:
            return (sym, sym), True
        neff_ = sw * sw / sw2
        sc = sw / neff_
        g = garwood(neff_)
        return (g[0] * sc, g[1] * sc), False
    if mode == "poisson_raw":
        if not n > 0 or not sw > 0:
            return (sym, sym), True
        wbar = abs(sw / n)
        g = garwood(n)
        return (g[0] * wbar, g[1] * wbar), False
    return (sym, sym), False


def sig_fom(s: float, b: float, b2: float, has_weights: bool) -> float:
    den = b + (b2 if has_weights else 0)
    return s / math.sqrt(den) if den > 0 else math.nan


def zbi(s: float, b: float, b2: float, syst: float, has_weights: bool) -> float:
    # JS `syst ?` treats NaN as falsy; Python `if syst` would not
    syst = syst if syst == syst else 0
    sb2 = (b2 if has_weights else 0) + (syst * b * syst * b if syst else 0)
    if not (s > 0 and b > 0 and sb2 > 0):
        return math.nan
    tau = b / sb2
    p = ibeta(s + b, 1 + tau * b, 1 / (1 + tau))
    if math.isnan(p):
        return math.nan
    if p <= 0:
        return math.inf
    return -norm_inv(p)


def sob(s: float, b: float) -> float:
    """S/B, NaN when there is no B — region.ts regionRead's `sb`."""
    return s / b if b > 0 else math.nan


def neff(sw: float, sw2: float) -> float:
    if not sw > 0 or not sw2 > 0:
        return math.nan
    return sw * sw / sw2


def _seq_sum(a: np.ndarray) -> float:
    """Left-to-right float64 sum — the page's own accumulation order. np.sum's
    pairwise reduction can differ by more than any rtol under negative-weight
    cancellation; cumsum is sequential, so its last element is the page's.
    `+ 0.0` pins the sign of zero: the page's accumulator starts at +0 and
    IEEE addition from +0 never yields -0, but cumsum starts at the first
    ELEMENT, so a lone -0.0 weight would leak through."""
    return float(a.cumsum()[-1] + 0.0) if a.size else 0.0


# ---------------------------------------------------------------- binning (bin.ts) ----

def edges(lo: float, hi: float, nb: int, logb: bool = False) -> dict:
    """Evenly spaced in x, or in log10(x) when asked AND the low edge is positive."""
    on = bool(logb) and lo > 0 and hi > lo
    a = math.log10(lo) if on else lo
    b = math.log10(hi) if on else hi
    span = b - a
    if span != span:  # JS `(b - a) || 1`: NaN is falsy there
        span = 0.0
    return {"on": on, "a": a, "b": b, "s": nb / (span or 1)}


def _pow10(m: float) -> float:
    """10**m the way Math.pow does it: overflow is Infinity, never an error."""
    try:
        return 10.0 ** m
    except OverflowError:
        return math.inf


def centres(lo: float, hi: float, nb: int, logb: bool = False) -> list[float]:
    e = edges(lo, hi, nb, logb)
    out = []
    for i in range(nb):
        m = e["a"] + (e["b"] - e["a"]) * (i + 0.5) / nb
        out.append(_pow10(m) if e["on"] else m)
    return out


def edge_list(lo: float, hi: float, nb: int, logb: bool = False) -> list[float]:
    e = edges(lo, hi, nb, logb)
    out = []
    for i in range(nb + 1):
        # nb=0 is validator-legal; JS 0/0 is NaN and edgeList returns [NaN]
        m = e["a"] + (e["b"] - e["a"]) * (i / nb if nb else math.nan)
        out.append(_pow10(m) if e["on"] else m)
    return out


def _bin_index(x: np.ndarray, e: dict) -> np.ndarray:
    """Float bin index per row; NaN where the page would index h[NaN] (a no-op)."""
    with np.errstate(invalid="ignore", divide="ignore"):
        if e["on"]:
            # NaN x: `x > 0` is false in both implementations -> index -1, like x <= 0
            idx = np.where(x > 0,
                           np.floor((np.log10(np.where(x > 0, x, 1)) - e["a"]) * e["s"]),
                           -1.0)
        else:
            idx = np.floor((x - e["a"]) * e["s"])
    return idx


def binned(values, ww, keep, lo: float, hi: float, nb: int,
           logb: bool = False, oflow: bool = True) -> dict:
    """Sum(w) and Sum(w²) per bin, tails crammed or dropped — bin.ts binned().

    ``ww`` is the per-row effective weight (w × lumi, or ones when unweighted).
    """
    v = np.asarray(values, float)
    w = np.asarray(ww, float)
    k = np.asarray(keep, bool)
    x, w = v[k], w[k]
    e = edges(lo, hi, nb, logb)
    with np.errstate(invalid="ignore"):
        over_m, under_m = x > hi, x < lo
    idx = _bin_index(x, e)
    with np.errstate(invalid="ignore"):
        low, high = idx < 0, idx >= nb
    nan = np.isnan(idx)
    sel = np.ones(x.size, bool) if oflow else ~(low | high)
    clamped = np.where(low, 0.0, np.where(high, nb - 1.0, idx))
    # nb=0 passes both validators: the page's h[-1] write is a silent no-op
    # on a zero-length typed array, so nothing fills but kept/tails still count
    fill = sel & ~nan if nb > 0 else np.zeros(x.size, bool)
    bi = clamped[fill].astype(np.int64)
    wb = w[fill]
    return {
        "h": np.bincount(bi, weights=wb, minlength=nb),
        "h2": np.bincount(bi, weights=wb * wb, minlength=nb),
        "n": np.bincount(bi, minlength=nb).astype(float),
        "over": int(over_m.sum()), "under": int(under_m.sum()),
        "overW": _seq_sum(w[over_m]), "underW": _seq_sum(w[under_m]),
        "kept": int(sel.sum()),
    }


def binned2d(vx, vy, ww, keep, lox: float, hix: float, nbx: int,
             loy: float, hiy: float, nby: int,
             lbx: bool = False, lby: bool = False, oflow: bool = True,
             ds=None, ok_ds=None) -> dict:
    """One flat (nby*nbx) grid, row-major with y outer — bin.ts binned2D().

    ``ok_ds`` is the viewer's per-DATASET filter (srcs/mute plots): rows whose
    dataset is off are skipped before anything counts, exactly like the page.
    """
    k = np.asarray(keep, bool)
    if ok_ds is not None:
        k = k & np.asarray(ok_ds, bool)[np.asarray(ds)]
    x = np.asarray(vx, float)[k]
    y = np.asarray(vy, float)[k]
    w = np.asarray(ww, float)[k]
    ex, ey = edges(lox, hix, nbx, lbx), edges(loy, hiy, nby, lby)
    bx, by = _bin_index(x, ex), _bin_index(y, ey)
    with np.errstate(invalid="ignore"):
        out_m = (bx < 0) | (bx >= nbx) | (by < 0) | (by >= nby)
    nan = np.isnan(bx) | np.isnan(by)
    sel = np.ones(x.size, bool) if oflow else ~out_m
    bx = np.clip(np.where(np.isnan(bx), 0, bx), 0, max(nbx - 1, 0))
    by = np.clip(np.where(np.isnan(by), 0, by), 0, max(nby - 1, 0))
    fill = sel & ~nan if nbx > 0 and nby > 0 else np.zeros(x.size, bool)
    flat = (by[fill] * nbx + bx[fill]).astype(np.int64)
    return {"h": np.bincount(flat, weights=w[fill], minlength=nbx * nby),
            "kept": int(sel.sum())}


# ------------------------------------------------------- cuts & visibility (select.ts) ----

def bounds(col: dict) -> list[float]:
    return col.get("nice") or col["lim"]


def narrowed(col: dict, spec: Any) -> bool:
    """Has this cut been moved off its default — select.ts narrowed()."""
    if spec is None:
        return False
    if isinstance(spec, dict):
        chosen = spec.get("in")
        return chosen is not None and len(chosen) != len(col.get("vals") or [])
    b = bounds(col)
    lo, hi = spec
    return (lo is not None and lo > b[0]) or (hi is not None and hi < b[1])


def cut_range(col: dict, spec: Any) -> tuple[float, float]:
    """Inclusive range with an end-stop handle meaning OPEN — select.ts openRange()."""
    b = bounds(col)
    lo = spec[0] if spec and spec[0] is not None else b[0]
    hi = spec[1] if spec and spec[1] is not None else b[1]
    return (lo if lo > b[0] else -math.inf, hi if hi < b[1] else math.inf)


def active_cuts(cols: list[dict], cuts: dict, cutorder: list[str] | None) -> list[str]:
    """Column NAMES actually narrowing, cutorder first then column order."""
    order = cutorder or []

    def at(name: str) -> int:
        try:
            return order.index(name)
        except ValueError:
            return 2 ** 53

    idx = [i for i, c in enumerate(cols)
           if c["name"] in cuts and narrowed(c, cuts[c["name"]])]
    idx.sort(key=lambda i: (at(cols[i]["name"]), i))
    return [cols[i]["name"] for i in idx]


def shown_of(tab: dict | None, view: dict | None, samples: list[str]) -> dict[str, bool]:
    """Visibility from the shared view when locked, else the tab — rvr6.ts shownOf()."""
    lst = rvr6.effective(tab, view, "shown")
    on = set(lst) if lst is not None else None
    return {s: (s in on if on is not None else True) for s in samples}


def sel_mask(columns: dict, cols: list[dict], cuts: dict,
             shown: dict[str, bool], ds: np.ndarray, samples: list[str]) -> np.ndarray:
    """One boolean per event — select.ts mask(). NaN PASSES a range cut here."""
    keep = np.asarray([shown.get(s, True) for s in samples], bool)[ds]
    by_name = {c["name"]: c for c in cols}
    for name, spec in (cuts or {}).items():
        col = by_name.get(name)
        if col is None or not narrowed(col, spec):
            continue
        v = columns[name]
        if isinstance(spec, dict):
            keep = keep & np.isin(v, spec["in"])
        else:
            lo, hi = cut_range(col, spec)
            with np.errstate(invalid="ignore"):
                keep = keep & ~((v < lo) | (v > hi))
    return keep


# ------------------------------------------------------- lumi, weights, groups ----

def lumi_table(meta: dict) -> np.ndarray:
    """Per-sample luminosity, outermost tier first; an UNWEIGHTED sample is 1."""
    view = meta.get("view") or {}
    return np.array([rvr6.lumi_of(s["name"], view) if s["weighted"] else 1.0
                     for s in meta["data"]["samples"]])


def is_weighted(meta: dict) -> bool:
    """Whether counting scales by w × lumi — ctx.ts weighted() at load time.

    The page's weight TOGGLE (st.wt) is session state: docOfV6 never reads a
    tab's `wt` into state, so a loaded page is always weighted when the file
    has weights. A wire `wt` key is IGNORED on load but round-trips untouched
    through the tab's raw keys — ignored, not scrubbed.
    """
    return meta["data"].get("weight_col") is not None


def row_weights(meta: dict, columns: dict, ds: np.ndarray) -> np.ndarray:
    """w × lumi per row when weighted, else ones — what every counting loop uses."""
    if not is_weighted(meta):
        return np.ones(ds.size)
    return columns[meta["data"]["weight_col"]] * lumi_table(meta)[ds]


def grouping(view: dict | None) -> tuple[dict, dict, list[str]]:
    g = (view or {}).get("groups") or {}
    return g.get("of") or {}, g.get("sub") or {}, list(g.get("order") or [])


def group_names(view: dict | None, samples: list[str]) -> list[str]:
    of, _, order = grouping(view)
    seen = list(order)
    for s in samples:
        g = of.get(s)
        if g and g not in seen:
            seen.append(g)
    return seen


def sources(view: dict | None, samples: list[str]) -> list[str]:
    """Group names then ungrouped samples, dataset order — colour.ts sources()."""
    of, _, _ = grouping(view)
    return group_names(view, samples) + [s for s in samples if not of.get(s)]


# the viewer's offered palette (ui/frame.ts PALETTE20) — one list for chips,
# wires, and automatic series colours, so a record can never disagree
PALETTE20 = (
    "#4E79A7", "#F28E2B", "#59A14F", "#E15759", "#B07AA1",
    "#EDC948", "#76B7B2", "#FF9DA7", "#9C755F", "#BAB0AC",
    "#A0CBE8", "#FFBE7D", "#8CD17D", "#FF9D9A", "#D4A6C8",
    "#F1CE63", "#86BCB6", "#FABFD2", "#D7B5A6", "#79706E",
)


def auto_colours(view: dict | None, samples: list[str]) -> dict[str, str]:
    """ONE colour assignment for everything drawn — ui/frame.ts autoColours().
    Explicit look colours claim their slot from EVERY namespace (sample, sub,
    group) before assignment; each GLOBAL source, in sources() order, takes
    its own look colour or the first palette colour nobody holds. Computed at
    extract time over the full context, because a record that kept only its
    drawn sources could not reproduce the global slot arithmetic."""
    look = (view or {}).get("look") or {}
    taken = {str(lk["colour"]).lower()
             for space in ("sample", "sub", "group")
             for lk in (look.get(space) or {}).values()
             if isinstance(lk, dict) and lk.get("colour")}
    out: dict[str, str] = {}
    for name in sources(view, samples):
        own = (source_look(view, samples, name) or {}).get("colour")
        if own:
            out[name] = own
            continue
        c = next((p for p in PALETTE20 if p.lower() not in taken),
                 PALETTE20[len(out) % len(PALETTE20)])
        out[name] = c
        taken.add(c.lower())
    return out


def source_look(view: dict | None, samples: list[str], name: str) -> dict | None:
    """Resolved style for one source — frame.ts sourceLook(): the same
    group -> subgroup -> sample namespace order as source membership."""
    look = (view or {}).get("look") or {}
    _, sub, _ = grouping(view)
    if name in group_names(view, samples):
        space = look.get("group") or {}
    elif name in sub.values():
        space = look.get("sub") or {}
    else:
        space = look.get("sample") or {}
    return space.get(name)


def src_members(view: dict | None, samples: list[str], name: str) -> list[int]:
    """Datasets behind a group, subgroup, or sample name — colour.ts srcMembers()."""
    of, sub, _ = grouping(view)
    members = [i for i, s in enumerate(samples)
               if of.get(s) == name or (not of.get(s) and s == name)]
    if members:
        return members
    members = [i for i, s in enumerate(samples) if sub.get(s) == name]
    if members:
        return members
    return [samples.index(name)] if name in samples else []


# ------------------------------------------------------- stage iteration (rvr6.ts) ----

# newPlot()'s literal, minus the meta-dependent varx/vary and the never-saved keys.
PLOT_DEFAULTS = {
    "mode": "1d", "nb": 40, "decouple": False, "compare": False,
    "logx": False, "logy": False, "logz": False,
    "pull": False, "pmin": "", "pmax": "", "oflow": True, "d3style": "bars",
    "norm": "yield", "layer": "all", "layer2": "all",
    "view": "one", "cmpmode": "ratio",
    "xtitle": "", "ytitle": "", "titles": {}, "err": "off",
    "xauto": True, "xmin": "", "xmax": "", "yauto": True, "ymin": "", "ymax": "",
    "zauto": True, "zmin": "", "zmax": "",
    "cmt": "", "anno": True, "drawcol": "#3D9BE9", "draw": {},
    "merge": None, "lines": None,
}


def plot_of_stage(item: dict, col_names: list[str],
                  defaults: dict | None) -> dict | None:
    """One stage entry -> one resolved plot spec — rvr6.ts plotOfStage()."""
    if item.get("kind") not in ("1d", "2d", "3d"):
        return None
    rest = {k: deepcopy(v) for k, v in item.items() if k not in ("kind", "cells")}
    # a non-dict `plot` passes both validators untouched; the page object-spreads
    # it into index-keyed noise keys ("0": "x"). DELIBERATE divergence: that
    # noise is not reproduced — treated as no defaults rather than crash
    if not isinstance(defaults, dict):
        defaults = None
    spec = {
        **deepcopy(PLOT_DEFAULTS),
        "varx": col_names[0] if col_names else "",
        "vary": col_names[1] if len(col_names) > 1 else (col_names[0] if col_names else ""),
        **deepcopy(defaults or {}),
        **rest,
        "mode": item["kind"],
    }
    # never-saved keys: newPlot deletes these even when the wire carries them;
    # blockRaw is save bookkeeping outside the normalized spec, so a wire key
    # squatting on it is dropped rather than exposed as resolved content
    for k in ("id", "opts", "cmton", "lbx", "lby", "blockRaw"):
        spec.pop(k, None)
    if "srcs" in spec:
        # defensive — check6 refuses duplicate/unknown srcs, on leaves AND in
        # tab.plot defaults; mirrors plotOfStage's own belt-and-braces filter,
        # PRESENCE-keyed like its `!== undefined` (a null srcs is deleted too)
        srcs = spec["srcs"]
        if not isinstance(srcs, list) or any(not isinstance(x, str) for x in srcs):
            spec.pop("srcs", None)
        else:
            spec["srcs"] = list(dict.fromkeys(srcs))
    return spec


def _sanitize(text: str) -> str:
    return re.sub(r"_+", "_", re.sub(r"[^A-Za-z0-9_]+", "_", text)).strip("_")


def stage_entries(meta: dict, tab: int = 0) -> list[dict]:
    """Every leaf of a tab's effective stage: kind, stable path, name, resolved spec."""
    tabs = meta.get("tabs") or []
    tab_entry = tabs[tab] if 0 <= tab < len(tabs) else {}
    view = meta.get("view") or {}
    stage = rvr6.effective(tab_entry, view, "stage") or []
    col_names = [c["name"] for c in meta["data"]["cols"]]
    defaults = tab_entry.get("plot")

    entries: list[dict] = []

    def add(kind: str, path: str, spec: dict) -> None:
        entries.append({"kind": kind, "path": path, "spec": spec})

    def leaf(item: dict, path: str, sheet: int | None, quad: int | None,
             layout: list | None) -> None:
        if item.get("kind") in ("board", "map"):
            spec = deepcopy(item)
            # placement is resolved content, exactly as the page parks it; the
            # save bookkeeping (`at`, `blockRaw`) is not — the path holds
            # position, and a WIRE key squatting on a bookkeeping name is
            # dropped rather than exposed as if the page had resolved it
            spec.pop("at", None)
            spec.pop("blockRaw", None)
            if sheet is not None:
                spec["sheet"], spec["quad"] = sheet, quad
                spec["layout"] = list(layout) if isinstance(layout, list) else None
            add(item["kind"], path, spec)
            return
        p = plot_of_stage(item, col_names, defaults)
        if p:
            # stageIntoState stamps these onto every resolved plot; blockRaw
            # (the whole block, kept for the save round-trip) is excluded
            p["sheet"], p["quad"] = sheet, quad
            if sheet is not None:
                p["layout"] = list(layout) if isinstance(layout, list) else None
            add(p["mode"], path, p)

    blocks = 0
    for i, item in enumerate(stage):
        if item.get("kind") == "block":
            sheet_id = blocks
            blocks += 1
            layout = item.get("layout")
            for q, cell in enumerate((item.get("cells") or [])[:MAX_CELLS]):
                if cell:
                    leaf(cell, f"t{tab}.s{i}.c{q}", sheet_id, q, layout)
        else:
            leaf(item, f"t{tab}.s{i}", None, None, None)

    for e in entries:
        # unique by construction: the name embeds the entry's own stage path
        base = e["path"].replace(".", "_") + "_" + e["kind"]
        if e["kind"] in ("1d", "2d", "3d"):
            base += "_" + _sanitize(str(e["spec"].get("varx", "")))
            if e["kind"] != "1d":
                base += "_" + _sanitize(str(e["spec"].get("vary", "")))
        e["name"] = base
    return entries


def resolve_tab(meta: dict, tab: int = 0) -> dict:
    """The whole effective state of one tab, as the page would resolve it."""
    tabs = meta.get("tabs") or []
    tab_entry = tabs[tab] if 0 <= tab < len(tabs) else {}
    view = meta.get("view") or {}
    samples = [s["name"] for s in meta["data"]["samples"]]
    cuts = rvr6.effective(tab_entry, view, "cuts") or {}
    return {
        "cuts": cuts,
        # cutorder is page SESSION state — docOfV6 never reads it off a tab, so
        # a loaded ladder is column order; a wire cutorder key is ignored (it
        # still round-trips untouched through the tab's raw keys)
        "active": active_cuts(meta["data"]["cols"], cuts, None),
        "shown": shown_of(tab_entry, view, samples),
        "entries": stage_entries(meta, tab),
        "weighted": is_weighted(meta),
        "lumi": lumi_table(meta),
    }


# ------------------------------------------------------- the region walk (region.ts) ----

def _js_num(x: Any) -> str:
    """A number the way a JS template string spells it: 200.0 -> "200",
    1e-6 -> "0.000001", 1e-7 -> "1e-7", 1.5e21 -> "1.5e+21". Beyond 2^53 JS
    prints the SHORTEST round-tripping decimal of the double the wire's JSON
    parse produced, not the exact integer — ints route through float there.
    Only ever fed validator-FINITE values: NaN/Inf would spell nan/inf here
    but NaN/Infinity in JS, and check6 refuses a non-finite threshold."""
    if isinstance(x, int) and not isinstance(x, bool):
        return str(x) if abs(x) < 2 ** 53 else _js_num(float(x))
    if not isinstance(x, float):
        return str(x)
    if x.is_integer() and abs(x) < 2 ** 53:
        return str(int(x))
    r = repr(x)  # shortest round-trip digits, same as JS
    if r.endswith(".0"):
        r = r[:-2]  # integer-valued beyond 2^53, still decimal notation
    if "e" not in r:
        return r
    mantissa, exponent = r.split("e")
    e = int(exponent)
    if -7 < e < 21:
        # JS prints decimal notation for |x| in [1e-6, 1e21)
        return format(Decimal(r), "f")
    return f"{mantissa}e{'+' if e > 0 else '-'}{abs(e)}"


def cut_label(cut: dict, yes: bool) -> str:
    if "in" in cut:
        return (f"{cut['col']} = " + "/".join(_js_num(v) for v in cut["in"])) if yes \
            else f"{cut['col']} other"
    return f"{cut['col']} < {_js_num(cut['lt'])}" if yes \
        else f"{cut['col']} ≥ {_js_num(cut['lt'])}"


def region_bin_name(path: list[str]) -> str:
    """Operators transliterated, never stripped — region.ts regionBinName()."""
    raw = "_".join(["Bin"] + list(path))
    raw = raw.replace("<", "_lt_").replace("≥", "_ge_").replace("≤", "_le_")
    raw = re.sub(r"_+", "_", re.sub(r"[^A-Za-z0-9_]+", "_", raw))
    return re.sub(r"_$", "", raw)


def tree_from_grid(grid: dict) -> dict | None:
    """A declared grid -> the binary partition it means — region.ts treeFromGrid().

    Non-exhaustive leaves each open TAIL unsplit; exhaustive splits every band.
    """
    vars_ = [v for v in (grid.get("vars") or []) if v and v.get("col") and v.get("bands")]

    def build(d: int) -> dict | None:
        if d >= len(vars_):
            return None
        v = vars_[d]

        def chain(k: int) -> dict | None:
            if k >= len(v["bands"]):
                return None if grid.get("exhaustive") is False else build(d + 1)
            b = v["bands"][k]
            cut = ({"col": v["col"], "in": list(b)} if isinstance(b, list)
                   else {"col": v["col"], "lt": b})
            return {"cut": cut, "yes": build(d + 1), "no": chain(k + 1)}

        return chain(0)

    return build(0)


def region_root(spec: dict) -> dict | None:
    """What the map COUNTS: its saved `root`, and nothing else.

    The page's regionCount reads only spec.root — a grid with no root is a
    recipe not yet applied, and the map shows ONE undivided region. An
    explicit bins request (e.g. `--from-map`) may derive a tree with
    `tree_from_grid`; page parity must not.
    """
    return spec.get("root") or None


def region_leaves(root: dict | None) -> list[dict]:
    """Every leaf of the partition, tree order, with the cuts that define it."""
    out: list[dict] = []

    def walk(node: dict | None, path: list[str],
             win: dict[str, tuple], vals: list[dict]) -> None:
        if not node or not node.get("cut"):
            out.append({
                "path": list(path), "name": region_bin_name(path),
                "windows": [{"col": c, "lo": r[0], "hi": r[1]} for c, r in win.items()],
                "vals": [dict(v) for v in vals],
            })
            return
        c = node["cut"]
        if "in" in c:
            walk(node.get("yes"), path + [cut_label(c, True)], win,
                 vals + [{"col": c["col"], "in": list(c["in"])}])
            walk(node.get("no"), path + [cut_label(c, False)], win, vals)
            return
        held = win.get(c["col"], (None, None))
        # INTERSECT with the held window, never overwrite
        y = dict(win)
        y[c["col"]] = (held[0], c["lt"] if held[1] is None else min(held[1], c["lt"]))
        n = dict(win)
        n[c["col"]] = (c["lt"] if held[0] is None else max(held[0], c["lt"]), held[1])
        walk(node.get("yes"), path + [cut_label(c, True)], y, vals)
        walk(node.get("no"), path + [cut_label(c, False)], n, vals)

    walk(root, [], {}, [])
    return out


def leaf_masks(root: dict | None, columns: dict, keep: np.ndarray) -> list[np.ndarray]:
    """One boolean array per leaf, tree order. NaN fails every predicate and lands
    on the `no` side — that is what makes the two sides exhaustive."""
    out: list[np.ndarray] = []

    def walk(node: dict | None, m: np.ndarray) -> None:
        if not node or not node.get("cut"):
            out.append(m)
            return
        c = node["cut"]
        v = columns.get(c["col"])
        if v is None:
            t = np.zeros(m.size, bool)
        elif "in" in c:
            t = np.isin(v, c["in"])
        else:
            with np.errstate(invalid="ignore"):
                t = v < c["lt"]
        walk(node.get("yes"), m & t)
        walk(node.get("no"), m & ~t)

    walk(root, np.asarray(keep, bool))
    return out


def region_count(meta: dict, columns: dict, ds: np.ndarray, keep: np.ndarray,
                 spec: dict) -> dict:
    """Per-leaf per-source [n, w, w2] over the kept rows — region.ts regionCount()."""
    view = meta.get("view") or {}
    samples = [s["name"] for s in meta["data"]["samples"]]
    # ABSENT srcs means every source the page offers; an EMPTY list means none.
    # Presence-only, like the page's `spec.srcs === undefined` — a null srcs
    # is validator-illegal and crashes there too (regionCount .slice() on null)
    srcs = list(spec["srcs"]) if "srcs" in spec else sources(view, samples)
    root = region_root(spec)
    leaves = region_leaves(root)
    masks = leaf_masks(root, columns, keep)
    ww = row_weights(meta, columns, ds)
    src_rows = [np.isin(ds, src_members(view, samples, s)) for s in srcs]
    L, K = len(leaves), len(srcs)
    n = np.zeros((L, K))
    w = np.zeros((L, K))
    w2 = np.zeros((L, K))
    for j, m in enumerate(masks):
        for k, sm in enumerate(src_rows):
            sel = m & sm
            n[j, k] = int(sel.sum())
            w[j, k] = _seq_sum(ww[sel])
            w2[j, k] = _seq_sum(ww[sel] ** 2)
    any_src = np.zeros(ds.size, bool)
    for sm in src_rows:
        any_src |= sm
    total = _seq_sum(ww[np.asarray(keep, bool) & any_src])
    return {"leaves": leaves, "srcs": srcs, "n": n, "w": w, "w2": w2,
            "weighted": is_weighted(meta), "total": total}


def region_flat(num: dict) -> dict:
    """{bin: {process: {"nominal": [raw, yield, sqrt(sum w²)]}}} — region.ts regionFlat().

    Every process declared in every bin, even at zero; duplicate names suffixed.
    """
    out: dict = {}
    used: dict[str, int] = {}
    for j, leaf in enumerate(num["leaves"]):
        name = leaf["name"] or "Bin"
        seen = used.get(name)
        if seen:
            used[name] = seen + 1
            name = f"{name}_{seen + 1}"
        else:
            used[name] = 1
        out[name] = {
            s: {"nominal": [_or0(num["n"][j, k]), _or0(num["w"][j, k]),
                            math.sqrt(max(_or0(num["w2"][j, k]), 0))]}
            for k, s in enumerate(num["srcs"])
        }
    return out


def view_syst(view: dict) -> float:
    """The ZBi systematic actually used — board.ts:126-129 / RegionMap.tsx:
    Number(String(view.syst ?? '0.05')), anything garbled or negative is
    the format's 5%, never a silent 0%."""
    v = view.get("syst")
    raw = _js_number("0.05" if v is None else _js_str(v))
    return raw if math.isfinite(raw) and raw >= 0 else 0.05


def region_sig(flat: dict, sig: str, bkg: list[str]) -> dict:
    """Asimov significance, no systematic — region.ts regionSig().

    q0 = 2*sum[(s+b)ln(1+s/b) - s], Z = sqrt(q0). s = 0 contributes
    nothing and is kept; s > 0 with b <= 0 has no defined likelihood and
    is DROPPED AND COUNTED, never silently skipped. ``thin`` counts
    contributing bins whose pooled RAW background is under 5.
    """
    q0, bins, dropped, thin = 0.0, 0, 0, 0
    for row in flat.values():
        cell = row.get(sig)
        s = cell["nominal"][1] if cell else 0.0
        b = braw = 0.0
        for k in bkg:  # sequential, the viewer's own += in bkg order
            c = row.get(k)
            b += c["nominal"][1] if c else 0.0
            braw += c["nominal"][0] if c else 0.0
        if s == 0:
            bins += 1
            continue
        if not (s > 0) or not (b > 0):
            dropped += 1
            continue
        t = 2 * ((s + b) * math.log(1 + s / b) - s)
        if not math.isfinite(t):
            dropped += 1
            continue
        q0 += t
        bins += 1
        if braw < 5:
            thin += 1
    return {"z": math.sqrt(max(q0, 0.0)), "bins": bins,
            "dropped": dropped, "thin": thin}


def _or0(v: float) -> float:
    """JS `x || 0`: NaN and NEGATIVE ZERO (both falsy there) become +0 — an
    overflow-cancelled Σw must flatten identically in both implementations."""
    v = float(v)
    return 0.0 if v != v or v == 0 else v


# ------------------------------------------------------- axis windows (ui/frame.ts) ----

def _js_str(v: Any) -> str:
    """String(v) over JSON-representable values — what parseFloat receives."""
    if v is None:
        return "null"
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, (int, float)):
        return _js_num(v) if math.isfinite(v) else \
            ("NaN" if v != v else ("Infinity" if v > 0 else "-Infinity"))
    if isinstance(v, str):
        return v
    if isinstance(v, list):
        return ",".join("" if x is None else _js_str(x) for x in v)
    return "[object Object]"  # a dict; the page would parse this to NaN too


# ECMAScript WhiteSpace + LineTerminator — NOT python's whitespace: JS trims
# U+FEFF (ZWNBSP) which str.strip() keeps, and python's default strip eats
# U+0085 (NEL) which JS keeps
_JS_WS = ("\t\n\x0b\x0c\r \xa0     "
          "         "
          "  　﻿")

# re.ASCII throughout: python \d matches Unicode digits ("١٢", "１２"), the
# ECMAScript numeric grammar does not
_PARSEFLOAT = re.compile(
    r"[+-]?(?:Infinity|\d+\.?\d*(?:[eE][+-]?\d+)?|\.\d+(?:[eE][+-]?\d+)?)",
    re.ASCII)


def parse_float(v: Any) -> float:
    """parseFloat(String(v)): the longest numeric prefix, NaN when none."""
    m = _PARSEFLOAT.match(_js_str(v).lstrip(_JS_WS))
    if not m:
        return math.nan
    t = m.group()
    return math.copysign(math.inf, -1 if t[0] == "-" else 1) \
        if t.lstrip("+-") == "Infinity" else float(t)


_NUM_DEC = re.compile(r"[+-]?(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?$", re.ASCII)
_NUM_RADIX = re.compile(r"0([xX][0-9a-fA-F]+|[oO][0-7]+|[bB][01]+)$", re.ASCII)
_NUM_INF = re.compile(r"([+-]?)Infinity$", re.ASCII)


def _js_number(v: Any) -> float:
    """Number(v) over JSON-representable values (the page's nb coercion).
    The string grammar is JS's, not python's: hex/octal/binary literals
    convert, but "inf"/"infinity"/"nan"/"1_0" spellings do NOT — only the
    exact `Infinity` names a non-finite."""
    if v is None:
        return 0.0
    if isinstance(v, bool):
        return float(v)
    if isinstance(v, (int, float)):
        return float(v)
    if isinstance(v, str):
        s = v.strip(_JS_WS)
        if not s:
            return 0.0
        m = _NUM_INF.match(s)
        if m:  # ONE optional sign — "++Infinity" is NaN in JS too
            return -math.inf if m.group(1) == "-" else math.inf
        if _NUM_RADIX.match(s):
            try:
                return float(int(s, 0))
            except OverflowError:  # Number("0x" + "f"*1000) is Infinity
                return math.inf
        if _NUM_DEC.match(s):
            return float(s)
        return math.nan
    if isinstance(v, list):  # Number([]) = 0, Number([5]) = 5, via toString
        return _js_number(_js_str(v))
    return math.nan


def _js_truthy(v: Any) -> bool:
    """JS truthiness over JSON-representable values: containers are ALWAYS
    truthy (an empty [] or {} included — the python instinct is wrong there);
    falsy is exactly null, false, 0/-0, NaN, and the empty string."""
    if isinstance(v, (list, dict)):
        return True
    if isinstance(v, float) and v != v:
        return False
    return bool(v)


def to_precision(x: float, p: int) -> float:
    """Number(x.toPrecision(p)): p significant digits, ties away from zero on
    the double's EXACT decimal expansion — same rule fmt_num pins."""
    d = Decimal(x)
    if not d:
        return x
    q = d.quantize(Decimal(1).scaleb(d.adjusted() - (p - 1)),
                   rounding=ROUND_HALF_UP)
    return float(q)


def auto_window(col: dict, pref: str) -> list[float] | None:
    """The AUTO window for one axis — ui/frame.ts autoWindow(). Every
    preference reads the packer's exact whole-cart quantile: p80..p99 are
    REQUIRED wire content (owner decision 2026-08-30; the measured fallback
    for p90-less files was deleted with it, both implementations). Null
    means the full range — FULL preference, an integer column, a degenerate
    quantile, or a non-numeric one on an unvalidated col dict."""
    if pref == "full":
        return None
    vals = col.get("vals")
    if vals is not None and len(vals) <= 200:
        return None
    q = col.get({"p80": "p80", "p95": "p95", "p99": "p99"}.get(pref, "p90"))
    lo = float(bounds(col)[0])
    if not (isinstance(q, (int, float)) and not isinstance(q, bool)
            and math.isfinite(q) and q > lo):
        return None
    return [lo, to_precision(float(q), 4)]


def resolve_axis(col: dict, zoom: str, auto: Any, vmin: Any, vmax: Any,
                 nb: Any, logb: bool) -> dict:
    """One axis of frameFor: window and bin count, exactly as the page frames
    them. An integer column gets one bin per value; otherwise AUTO tracks the
    zoom preference, MANUAL (auto exactly false) parses the typed bounds, and
    an inverted window falls back to the column's own range. Returns the
    resolved lo/hi/nb plus `ints`, `log`, and `window` provenance for meta."""
    vals = col.get("vals")
    if vals is not None and not logb and len(vals) <= 200:
        if not vals:
            # the page throws here: Math.min() of no values is Infinity and
            # Float64Array(Math.round(-Infinity)) is a RangeError — refuse
            raise ExtractError(
                f'column "{col["name"]}" has an empty value list — nothing to bin')
        lo, hi = min(vals) - 0.5, max(vals) + 0.5
        return {"lo": lo, "hi": hi, "nb": math.floor(hi - lo + 0.5),
                "ints": True, "log": False, "window": "vals", "zoom": zoom}
    b = bounds(col)
    lo, hi = float(b[0]), float(b[1])
    window = "lim"
    if auto is not False:
        w = auto_window(col, zoom)
        if w:
            lo, hi = w
            window = "auto"
    else:
        # parseFloat per bound; an unparsable bound keeps the column's own
        x, y = parse_float(vmin), parse_float(vmax)
        lo = x if math.isfinite(x) else lo
        hi = y if math.isfinite(y) else hi
        window = "manual"
    if not hi > lo:
        lo, hi = float(b[0]), float(b[1])
        window = "lim"
    n = _js_number(nb)
    n = max(2.0, min(200.0, n if n == n and n else 40.0))
    if n != int(n):
        # DELIBERATE divergence: the page does not crash on a fractional nb —
        # Float64Array(2.5) truncates to length 2 while the loops run to nb,
        # so rows binned into [floor(nb), nb) are SILENTLY DROPPED and the
        # centres/edges lists outgrow the arrays. That is incoherent as a
        # record; refuse instead of reproducing it
        raise ExtractError(
            f"nb resolves to {n} — the page silently drops rows past "
            f"bin {int(n)} on a fractional bin count; refusing to record that")
    return {"lo": lo, "hi": hi, "nb": int(n), "ints": False, "log": bool(logb),
            "window": window, "zoom": zoom}


def unicode_array(strings: list[str]) -> np.ndarray:
    """Fixed-width LITTLE-ENDIAN unicode, whatever the host — SCHEMAS.md
    string-array policy, shared by every extractor's npz writer."""
    width = max((len(s) for s in strings), default=1) or 1
    return np.array(strings, dtype=f"<U{width}")


# ------------------------------------------------------- the cutflow ladder (select.ts) ----

def fmt_num(x: float) -> str:
    """format.ts num(): '–' for non-finite; toExponential(2) at |x| >= 1e5 or
    0 < |x| < 1e-3; else 6 significant digits then en-US toLocaleString —
    comma grouping, at most 3 fraction digits, trailing zeros dropped. Ties
    round away from zero on the double's EXACT decimal expansion, both JS
    rules (toPrecision "larger n", Intl halfExpand)."""
    if not math.isfinite(x):
        return "–"
    if x == 0:
        return "0"  # signed zero included: the page prints "0" for -0 too
    d = Decimal(x)
    if abs(x) >= 1e5 or abs(x) < 1e-3:
        q = d.quantize(Decimal(1).scaleb(d.adjusted() - 2), rounding=ROUND_HALF_UP)
        e = q.adjusted()
        return f"{q.scaleb(-e):.2f}e{'+' if e >= 0 else '-'}{abs(e)}"
    if d:
        d = d.quantize(Decimal(1).scaleb(d.adjusted() - 5), rounding=ROUND_HALF_UP)
    d = d.quantize(Decimal("0.001"), rounding=ROUND_HALF_UP)
    return f"{d:,.3f}".rstrip("0").rstrip(".")


def _by_sum(a: np.ndarray) -> float:
    """select.ts:148 `acc.by[g] = (acc.by[g] || 0) + ww`: a NaN RUNNING total
    (overflow cancellation) is falsy in JS and resets to 0 before the next
    add — so a later weight UN-POISONS the per-source column, while the `w`
    total's sticky `+=` stays NaN. Row order, like every reduction."""
    c = np.cumsum(a)
    if c.size == 0:
        return 0.0
    if not np.isnan(c).any():
        return float(c[-1]) + 0.0  # (acc || 0) normalizes -0 to +0 in JS
    acc = 0.0
    for x in a:
        acc = (0.0 if acc != acc else acc) + x
    return acc


def cut_text(col: dict, spec: Any) -> str:
    """One ladder step's label — SankeyDialog.tsx cutTextFor(). Chip values
    join the way a JS template spells them; range bounds through num()."""
    if isinstance(spec, dict):
        return f"{col['name']} = " + "/".join(_js_num(v) for v in spec.get("in") or [])
    lo, hi = cut_range(col, spec)
    if lo == -math.inf:
        return f"{col['name']} ≤ {fmt_num(hi)}"
    if hi == math.inf:
        return f"{col['name']} ≥ {fmt_num(lo)}"
    return f"{fmt_num(lo)} ≤ {col['name']} ≤ {fmt_num(hi)}"


def cutflow(columns: dict, cols: list[dict], cuts: dict, shown: dict[str, bool],
            ds: np.ndarray, samples: list[str], ww: np.ndarray,
            view: dict | None) -> dict:
    """One pass down the ladder — select.ts cutflow(). Each event advances
    until a cut rejects it, counting itself at every step it reached, so
    reordering changes the LADDER, never the final row.

    NaN FAILS a ladder step: the step test is `lo <= x <= hi` (select.ts:136),
    the OPPOSITE polarity of sel_mask's `x < lo || x > hi` — so on a
    NaN-bearing cut column the last row is SMALLER than the mask's total.
    The page has the same split; ported and fixture-pinned, not fixed.

    A source is a group name, or an ungrouped dataset's own name (gOf);
    `sources` is their first-appearance order over shown datasets.
    """
    active = active_cuts(cols, cuts, None)
    by_name = {c["name"]: c for c in cols}
    of, _, _ = grouping(view)
    gof = [of.get(s) or s for s in samples]
    shown_arr = np.asarray([shown.get(s, True) for s in samples], bool)
    srcs = list(dict.fromkeys(g for g, on in zip(gof, shown_arr) if on))
    src_idx = np.asarray([srcs.index(g) if g in srcs else -1 for g in gof])[ds]
    reach = shown_arr[ds]
    n_steps, n_srcs = len(active) + 1, len(srcs)
    raw = np.zeros(n_steps)
    w = np.zeros(n_steps)
    by = np.zeros((n_steps, n_srcs))
    for s in range(n_steps):
        if s:
            name = active[s - 1]
            spec, v = cuts[name], columns[name]
            if isinstance(spec, dict):
                ok = np.isin(v, spec["in"])
            else:
                lo, hi = cut_range(by_name[name], spec)
                with np.errstate(invalid="ignore"):
                    ok = (v >= lo) & (v <= hi)
            reach = reach & ok
        raw[s] = int(reach.sum())
        w[s] = _seq_sum(ww[reach])
        for k in range(n_srcs):
            by[s, k] = _by_sum(ww[reach & (src_idx == k)])
    steps = ["no cuts"] + [cut_text(by_name[n], cuts[n]) for n in active]
    return {"steps": steps, "raw": raw, "w": w, "by": by, "sources": srcs}


# ------------------------------------------------------- shared extractor plumbing ----

def tab_entry(meta: dict, tab: int) -> dict:
    tabs = meta.get("tabs") or []
    if not tabs:
        if tab != 0:
            raise ExtractError(f"this file has no tabs — --tab {tab} names nothing")
        return {}
    if not 0 <= tab < len(tabs):
        raise ExtractError(
            f"--tab {tab} is out of range — this file has {len(tabs)} tab(s)")
    return tabs[tab]


def require_lumi(meta: dict, allow: bool) -> None:
    """Refuse a weighted sample silently falling back to lumi 1 — the
    x1-tab-in-a-x400-study trap. `rvr6.lumi_of` is the one ladder."""
    if allow or not is_weighted(meta):
        return
    view = meta.get("view") or {}
    unset = [s["name"] for s in meta["data"]["samples"]
             if s["weighted"] and rvr6.lumi_of(s["name"], view) == 1.0
             and not _lumi_named(s["name"], view)]
    if unset:
        raise ExtractError(
            f"{len(unset)} weighted sample(s) resolve to no luminosity anywhere and fall "
            f"back to 1, e.g. {unset[:3]} — a silent default is the x1-in-a-x400-study "
            f"trap. Give their group an entry in view.lumis, or pass --allow-unset-lumi "
            f"to accept it.")


def _lumi_named(sample: str, view: dict) -> bool:
    """Whether any tier actually names a luminosity for this sample, rather than defaulting."""
    lumis, groups = view.get("lumis") or {}, view.get("groups") or {}
    return any((lumis.get(tier) or {}).get(key) is not None
               for tier, key in (("group", (groups.get("of") or {}).get(sample)),
                                 ("sub", (groups.get("sub") or {}).get(sample)),
                                 ("sample", sample)))


def cuts_lines(meta: dict, tab: int, no_cuts: bool = False,
               ladder: bool = False) -> list[str]:
    """The applied cuts as python, one line each — SCHEMAS.md .cuts.txt.

    ``ladder=True`` spells the cutflow's step test (NaN FAILS a range step);
    the default spells the selection mask (NaN passes) — the page's own split.
    """
    view = meta.get("view") or {}
    cols = meta["data"]["cols"]
    cuts = rvr6.effective(tab_entry(meta, tab), view, "cuts") or {}
    active = [] if no_cuts else active_cuts(cols, cuts, None)
    if not active:
        return ["# no cuts"]
    by_name = {c["name"]: c for c in cols}
    # valid python: columns addressed as col[<name>], non-finite bounds spelled
    # float('inf') — the file must exec against {np, col, keep}
    lines = ["# ladder semantics: NaN fails a range step, fails a chip cut" if ladder
             else "# viewer semantics: NaN passes a range cut, fails a chip cut"]
    for name in active:
        spec = cuts[name]
        if isinstance(spec, dict):
            lines.append(f"keep &= np.isin(col[{name!r}], {spec['in']!r})")
        elif ladder:
            lo, hi = cut_range(by_name[name], spec)
            lines.append(f"keep &= (col[{name!r}] >= {_py(lo)}) & "
                         f"(col[{name!r}] <= {_py(hi)})")
        else:
            lo, hi = cut_range(by_name[name], spec)
            lines.append(f"keep &= ~((col[{name!r}] < {_py(lo)}) | "
                         f"(col[{name!r}] > {_py(hi)}))")
    return lines


def _py(v: float) -> str:
    return repr(v) if math.isfinite(v) else f"float('{v!r}')"


def json_num(v: float):
    """Strict-JSON number: non-finite as the strings SCHEMAS.md names."""
    if v != v:
        return "nan"
    if math.isinf(v):
        return "inf" if v > 0 else "-inf"
    return v


def json_cut_spec(col: dict, spec: Any) -> dict:
    """The resolved cut for meta.json — chip list, or the open-aware range."""
    if isinstance(spec, dict):
        return {"in": spec.get("in") or []}
    lo, hi = cut_range(col, spec)
    return {"range": [json_num(lo), json_num(hi)]}
