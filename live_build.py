#!/usr/bin/env python
"""live_build.py — build a cartridge with a live terminal dashboard.  DEV PREVIEW.

Same build as ``run_rowver.py``; the only difference is what the terminal shows.
The study's KEEP hook is wrapped so every chunk the skim reads is observed —
``cart.py`` and ``run_rowver.py`` are untouched, so this file can live on a dev
branch without affecting the production build.

    python live_build.py jetmet_2018 --out /tmp/test.rvr

In a real terminal the dashboard redraws in place; when piped, it prints a
snapshot frame at each sample boundary instead. Known dev-preview limit: a
PREPARE hook that changes row counts skews the scanned totals (the wrapper
counts post-PREPARE rows) — the adoption path, a real skim callback, fixes
that along with the provenance note below.
"""
from __future__ import annotations

import argparse
import shutil
import sys
import time
from pathlib import Path

import awkward as ak
import uproot

from cart import Cart, CartError, _invoke
from rvr6 import FormatError

# ---------------------------------------------------------------- terminal ----
TTY = sys.stdout.isatty()
ESC = "\x1b["


def paint(text: str, code: str) -> str:
    return f"{ESC}{code}m{text}{ESC}0m" if TTY else text


DIM, BOLD = "2", "1"
DATA_C, MC_C, SIG_C, OK_C = "36", "33", "35", "32"   # cyan, yellow, magenta, green


def bar(done: int, total: int, width: int) -> str:
    fill = width if not total else round(width * min(done, total) / total)
    return "█" * fill + "░" * (width - fill)


def short(n: int) -> str:
    if n >= 1_000_000: return f"{n / 1e6:.2f}M"
    if n >= 1_000: return f"{n / 1e3:.0f}k"
    return str(n)


def pct_of(done: int, total: int) -> str:
    return f"{100 * done / total:.0f}%" if total else "·"


def clock(seconds: float) -> str:
    seconds = max(int(seconds), 0)
    return f"{seconds // 60:02d}:{seconds % 60:02d}"


# --------------------------------------------------------------- dashboard ----
class Dashboard:
    """Counts what the KEEP wrapper reports and draws the press."""

    def __init__(self, cart: Cart, est_row_bytes: int) -> None:
        self.order = list(cart.fileset)
        self.kind = {}
        for name, entry in cart.fileset.items():
            # weights=None is the format's own data marker. SIG is strictly opt-in via
            # metadata kind="signal": the template's kind distinguishes data from
            # simulation, so every other weighted sample — declared or not — is MC.
            # Guessing from names mislabels real carts and defines no contract.
            declared = str(entry["metadata"].get("kind", "")).lower()
            if entry["metadata"]["weights"] is None:
                self.kind[name] = ("DATA", DATA_C)
            elif declared in {"signal", "sig"}:
                self.kind[name] = ("SIG ", SIG_C)
            else:
                self.kind[name] = ("MC  ", MC_C)
        self.total = {}          # sample -> entries in its files
        self.scanned = {name: 0 for name in self.order}
        self.kept = {name: 0 for name in self.order}
        self.est_row_bytes = est_row_bytes
        self.t0 = time.monotonic()
        self.lines = 0
        self.last_draw = 0.0
        self.current = ""

    def index(self, cart: Cart) -> None:
        for name, entry in cart.fileset.items():
            n = 0
            for path, tree in entry["files"].items():
                try:
                    with uproot.open({path: tree}) as t:
                        n += int(t.num_entries)
                except Exception as exc:
                    raise CartError(f'cannot open {name} file "{path}": {exc}') from exc
            self.total[name] = n

    # -- events from the wrapper ------------------------------------------
    def chunk(self, sample: str, scanned: int, kept: int) -> None:
        self.scanned[sample] += scanned
        self.kept[sample] += kept
        self.current = sample
        now = time.monotonic()
        boundary = self.scanned[sample] >= self.total.get(sample, 0)
        if TTY and (now - self.last_draw > 0.15 or boundary):
            self.draw()
        elif not TTY and boundary:
            self.draw()

    # -- rendering ---------------------------------------------------------
    def draw(self, final: bool = False) -> None:
        self.last_draw = time.monotonic()
        out: list[str] = []
        scanned = sum(self.scanned.values())
        kept = sum(self.kept.values())
        total = sum(self.total.values())
        elapsed = time.monotonic() - self.t0
        rate = scanned / elapsed if elapsed > 0 else 0.0
        eta = (total - scanned) / rate if rate > 0 else 0.0
        # The frame must fit the physical terminal BOTH ways: a wrapped line or a frame
        # taller than the screen breaks the cursor-up redraw. When the full board cannot
        # fit, fall back to a three-line aggregate. (Known preview limit: widths are
        # counted in characters, so double-cell CJK names can still wrap a row.)
        size = shutil.get_terminal_size((120, 40))
        cols, screen_rows = (size.columns, size.lines) if TTY else (120, 10 ** 9)
        if TTY and (cols < 72 or len(self.order) + 5 > screen_rows):
            width = max(cols - 1, 20)
            here = self.current or "—"
            n, t = self.scanned.get(here, 0), self.total.get(here, 0)
            body = [f"┌{'─' * (width - 2)}┐",
                    f"│ {short(scanned)}/{short(total)} {pct_of(scanned, total)}"
                    f"  kept {short(kept)}  eta {clock(eta)}",
                    f"│ now {here:.30s} {short(n)}/{short(t)}",
                    f"└{'─' * (width - 2)}┘"]
            out.extend(paint(line[:width], DIM) for line in body)
            self._flush(out, final)
            return
        wide = cols >= 100
        rule = "─" * (78 if wide else 66)
        out.append(paint(f"┌{rule}┐", DIM))
        for name in self.order:
            tag, colour = self.kind[name]
            n, k, t = self.scanned[name], self.kept[name], self.total.get(name, 0)
            gauge_w = 22 if wide else 12
            gauge = paint(bar(n, t, gauge_w), OK_C if t and n >= t else colour)
            if wide:
                eff = f"{100 * k / n:5.1f}%" if n else "     ·"
                out.append(f"{paint('│', DIM)} {name:<22.22s} {paint(tag, colour)} "
                           f"{gauge} {n:>10,}/{t:<10,} kept {k:>9,} {eff}")
            else:
                eff = f"{100 * k / n:4.0f}%" if n else "   ·"
                out.append(f"{paint('│', DIM)} {name:<17.17s} {paint(tag, colour)} "
                           f"{gauge} {short(n):>6}/{short(t):<6} kept {short(k):>6} {eff}")
        out.append(paint(f"├{rule}┤", DIM))
        pct = f"{100 * scanned / total:5.1f}%" if total else "     ·"
        if wide:
            out.append(f"{paint('│', DIM)} scanned {scanned:>11,}/{total:,}  {pct}   "
                       f"kept {kept:>11,}   est. buffer "
                       f"{kept * self.est_row_bytes / 2**20:7.1f} MiB")
            out.append(f"{paint('│', DIM)} {rate / 1e6:4.2f}M rows/s   "
                       f"elapsed {clock(elapsed)}   eta {clock(eta)}")
        else:
            out.append(f"{paint('│', DIM)} {short(scanned)}/{short(total)} {pct}  "
                       f"kept {short(kept)}  ~{kept * self.est_row_bytes / 2**20:.0f} MiB")
            out.append(f"{paint('│', DIM)} {rate / 1e6:4.2f}M rows/s  "
                       f"{clock(elapsed)} gone  eta {clock(eta)}")
        out.append(paint(f"└{rule}┘", DIM))
        self._flush(out, final)

    def _flush(self, out: list[str], final: bool) -> None:
        if TTY and self.lines:
            sys.stdout.write(f"{ESC}{self.lines}A")
        sys.stdout.write("\n".join(out) + "\n")
        if not TTY and not final:
            sys.stdout.write("\n")
        sys.stdout.flush()
        self.lines = len(out)


def wrap_keep(cart: Cart, board: Dashboard) -> None:
    """Observe every chunk through the KEEP hook; the mask passes through untouched."""
    original = cart.keep

    # ponytail: the wrapper's source leaks into made.keep provenance; give cart.skim a
    # real progress callback before adopting this beyond the dev branch.
    def keep(events, metadata):
        mask = _invoke(original, events, metadata, "KEEP")
        try:
            kept = int(ak.sum(mask))
        except Exception:
            kept = 0
        board.chunk(metadata["sample"], len(events), kept)
        return mask

    cart.keep = keep


# --------------------------------------------------------------------- run ----
def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("study")
    parser.add_argument("--out", type=Path)
    parser.add_argument("--step-size", default="100 MB")
    args = parser.parse_args(argv)
    step: int | str = int(args.step_size) if str(args.step_size).isdigit() else args.step_size

    try:
        cart = Cart.load(args.study)
        cart.validate()
        columns = len(cart.columns)
        est_row_bytes = 8 * columns + 12          # f8 upper bound + weight/index share
        board = Dashboard(cart, est_row_bytes)
        title = f" ROWVER CART PRESS · {cart.name} · {len(cart.fileset)} samples · " \
                f"{columns} columns "
        print(paint(title, BOLD))
        print(paint("indexing files…", DIM))
        board.index(cart)
        wrap_keep(cart, board)
        board.t0 = time.monotonic()
        result = cart.build(args.out, step_size=step)
    except (CartError, FormatError) as exc:
        raise SystemExit(f"error: {exc}") from exc

    board.draw(final=True)
    size = result["compressed"] / 2**20
    kept = sum(board.kept.values())
    took = time.monotonic() - board.t0
    print(paint(f" PRESSED  {result['file']}", OK_C))
    print(paint(f"          {result['rows']:,} rows · {size:.1f} MiB gzip · "
                f"{clock(took)} total · "
                f"{kept / took / 1e6:.2f}M rows/s kept-side", OK_C))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
