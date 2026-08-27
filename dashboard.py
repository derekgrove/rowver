#!/usr/bin/env python
"""dashboard.py — the live terminal dashboard behind ``run_rowver.py --live``.

Purely an observer: ``Cart.build(progress=board.chunk)`` reports each processed chunk
as ``(sample, raw_rows, kept_rows)``, and this module draws the press. It touches no
hook and computes nothing the build depends on, so ``--live`` cannot change the file
that is written.

In a real terminal the dashboard redraws in place; when piped, it prints a snapshot
frame at each nonempty sample boundary instead — a zero-entry sample produces no
chunk, so only the final frame shows its (full, 0/0) bar.
"""
from __future__ import annotations

import shutil
import sys
import time
import unicodedata

import uproot

from cart import Cart, CartError

# ---------------------------------------------------------------- terminal ----
TTY = sys.stdout.isatty()
ESC = "\x1b["


def paint(text: str, code: str) -> str:
    return f"{ESC}{code}m{text}{ESC}0m" if TTY else text


DIM = "2"
DATA_C, MC_C, SIG_C, OK_C = "36", "33", "35", "32"   # cyan, yellow, magenta, green


def bar(done: int, total: int, width: int) -> str:
    fill = width if not total else round(width * min(done, total) / total)
    return "█" * fill + "░" * (width - fill)


def short(n: int) -> str:
    """At most six characters at any magnitude, so no row can outgrow its budget."""
    for div, unit in ((1e12, "T"), (1e9, "G"), (1e6, "M"), (1e3, "k")):
        if n >= div:
            v = n / div
            return f"{v:.2f}{unit}" if v < 100 else f"{v:.0f}{unit}"
    return str(n)


def clip(name: str, width: int) -> str:
    """Clip and pad by DISPLAY CELLS: east-asian wide glyphs occupy two columns.

    Control characters are replaced before measuring — a newline in a valid sample
    name would add a physical line the redraw math cannot see, and an escape byte
    could repaint the terminal.
    """
    out, used = "", 0
    for ch in name:
        if unicodedata.category(ch) in ("Cc", "Cf") or ch == "\x7f":
            ch = "·"
        w = 2 if unicodedata.east_asian_width(ch) in "WF" else 1
        if used + w > width:
            break
        out, used = out + ch, used + w
    return out + " " * (width - used)


def pct_of(done: int, total: int) -> str:
    return f"{100 * done / total:.0f}%" if total else "·"


def clock(seconds: float) -> str:
    seconds = max(int(seconds), 0)
    return f"{seconds // 60:02d}:{seconds % 60:02d}"


# --------------------------------------------------------------- dashboard ----
class Dashboard:
    """Counts what the build's progress callback reports and draws the press."""

    def __init__(self, cart: Cart) -> None:
        # f8 upper bound per column plus a weight/index share — for the buffer estimate
        self.est_row_bytes = 8 * len(cart.columns) + 12
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

    # -- events from the build --------------------------------------------
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
        # taller than the screen breaks the cursor-up redraw. Names are measured in
        # display cells (CJK glyphs take two), counts are width-capped, and an EXACT
        # vertical fit still scrolls one row off via the trailing newline — so the
        # three-line fallback triggers at equality too.
        size = shutil.get_terminal_size((120, 40))
        cols, screen_rows = (size.columns, size.lines) if TTY else (120, 10 ** 9)
        if TTY and (cols < 72 or len(self.order) + 5 >= screen_rows):
            width = max(cols - 1, 20)
            here = self.current or "—"
            n, t = self.scanned.get(here, 0), self.total.get(here, 0)
            body = [f"┌{'─' * (width - 2)}┐",
                    f"│ {short(scanned)}/{short(total)} {pct_of(scanned, total)}"
                    f"  kept {short(kept)}  eta {clock(eta)}",
                    f"│ now {clip(here, 30)} {short(n)}/{short(t)}",
                    f"└{'─' * (width - 2)}┘"]
            out.extend(paint(line[:width], DIM) for line in body)
            self._flush(out, final)
            return
        # exact counts take whatever the biggest number needs; wide only when the whole
        # row provably fits (52 fixed cells + three count fields)
        wnum = len(f"{max([*self.total.values(), *self.scanned.values(), 1]):,}")
        wide = cols >= max(100, 53 + 3 * wnum)
        rule = "─" * ((50 + 3 * wnum) if wide else 66)
        out.append(paint(f"┌{rule}┐", DIM))
        for name in self.order:
            tag, colour = self.kind[name]
            n, k, t = self.scanned[name], self.kept[name], self.total.get(name, 0)
            gauge_w = 22 if wide else 12
            gauge = paint(bar(n, t, gauge_w), OK_C if t and n >= t else colour)
            if wide:
                eff = f"{100 * k / n:5.1f}%" if n else "     ·"
                out.append(f"{paint('│', DIM)} {clip(name, 22)} {paint(tag, colour)} "
                           f"{gauge} {n:>{wnum},}/{t:<{wnum},} kept {k:>{wnum},} {eff}")
            else:
                eff = f"{100 * k / n:4.0f}%" if n else "   ·"
                out.append(f"{paint('│', DIM)} {clip(name, 17)} {paint(tag, colour)} "
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


