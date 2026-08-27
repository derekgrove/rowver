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

import contextlib
import shutil
import sys
import threading
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
SPIN = "|/-\\"          # one tick per redraw: motion is the liveness signal
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


def cells(text: str) -> int:
    """Display cells, not code points: east-asian wide glyphs occupy two."""
    return sum(2 if unicodedata.east_asian_width(ch) in "WF" else 1 for ch in text)


def pct_of(done: int, total: int) -> str:
    return f"{100 * done / total:.0f}%" if total else "·"


def clock(seconds: float) -> str:
    minutes, sec = divmod(max(int(seconds), 0), 60)
    hours, minutes = divmod(minutes, 60)
    return f"{hours}:{minutes:02d}:{sec:02d}" if hours else f"{minutes:02d}:{sec:02d}"


def R(*bits) -> tuple[str, str]:
    """One box row from pieces: a str, or (str, colour). Returns (plain, painted) —
    identical display cells, so the plain twin is what the box measures."""
    plain, painted = "", ""
    for bit in bits:
        text, colour = bit if isinstance(bit, tuple) else (bit, None)
        plain += text
        painted += paint(text, colour) if colour else text
    return plain, painted


def box(sections: list[list[tuple[str, str]]]) -> list[str]:
    """Draw borders AFTER the text: the frame is sized to the longest row, every
    row is padded to it, and the right border always closes."""
    width = max((cells(plain) for sec in sections for plain, _ in sec), default=0)
    rule = paint("─" * (width + 2), DIM)
    edge = paint("│", DIM)
    lines = [paint("┌", DIM) + rule + paint("┐", DIM)]
    for at, sec in enumerate(sections):
        if at:
            lines.append(paint("├", DIM) + rule + paint("┤", DIM))
        for plain, painted in sec:
            lines.append(f"{edge} {painted}{' ' * (width - cells(plain))} {edge}")
    lines.append(paint("└", DIM) + rule + paint("┘", DIM))
    return lines


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
        self.pressing = False    # skim finished; pack + gzip in progress
        self.spins = 0
        self.lock = threading.Lock()   # draw is called from chunk() and the ticker

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
    def chunk(self, sample: str | None, scanned: int, kept: int) -> None:
        if sample is None:                 # build's signal: skim done, pressing now
            self.pressing = True
            self.draw()
            return
        with self.lock:                    # the ticker must never see a half-updated row
            self.scanned[sample] += scanned
            self.kept[sample] += kept
            self.current = sample
        now = time.monotonic()
        boundary = self.scanned[sample] >= self.total.get(sample, 0)
        if TTY and (now - self.last_draw > 0.15 or boundary):
            self.draw()
        elif not TTY and boundary:
            self.draw()

    @contextlib.contextmanager
    def live(self):
        """Redraw twice a second while the build runs, so quiet stretches —
        concatenating a sample, opening the next file, the final gzip — still
        visibly tick. Daemon thread; exits with the context."""
        if not TTY:
            yield
            return
        stop = threading.Event()
        def tick() -> None:
            while not stop.wait(0.5):
                self.draw()
        thread = threading.Thread(target=tick, daemon=True)
        thread.start()
        try:
            yield
        finally:
            stop.set()
            thread.join()

    # -- rendering ---------------------------------------------------------
    def draw(self, final: bool = False) -> None:
        with self.lock:
            self._draw(final)

    def _draw(self, final: bool) -> None:
        self.last_draw = time.monotonic()
        out: list[str] = []
        scanned = sum(self.scanned.values())
        kept = sum(self.kept.values())
        total = sum(self.total.values())
        elapsed = time.monotonic() - self.t0
        rate = scanned / elapsed if elapsed > 0 else 0.0
        eta = (total - scanned) / rate if rate > 0 else 0.0
        self.spins += 1
        spin = "" if final else SPIN[self.spins % len(SPIN)]
        doing = (f"pressing the cartridge (pack + gzip)… {spin}"
                 if self.pressing and not final else None)
        # The frame must fit the physical terminal BOTH ways: a wrapped line or a frame
        # taller than the screen breaks the cursor-up redraw. Names are measured in
        # display cells (CJK glyphs take two), counts are width-capped, and an EXACT
        # vertical fit still scrolls one row off via the trailing newline — so the
        # three-line fallback triggers at equality too.
        size = shutil.get_terminal_size((120, 40))
        cols, screen_rows = (size.columns, size.lines) if TTY else (120, 10 ** 9)
        if TTY and (cols < 72 or len(self.order) + 5 >= screen_rows):
            # EVERY sample keeps a live row: pack compact cells into as many columns
            # as it takes to fit the screen. A finished cell sits full while the rest
            # move. Only a truly minuscule window falls back to the summary box.
            free = screen_rows - 6            # frame borders + footer + trailing newline
            ncols = -(-len(self.order) // max(free, 1)) if self.order else 1
            namew = min(max((len(n) for n in self.order), default=0),
                        (cols - 4) // max(ncols, 1) - 13)
            if free >= 1 and namew >= 8:
                cells = []
                for name in self.order:
                    n, t = self.scanned[name], self.total.get(name, 0)
                    cells.append(R(f"{clip(name, namew)} ",
                                   (bar(n, t, 6),
                                    OK_C if t and n >= t else self.kind[name][1]),
                                   f" {pct_of(n, t):>4}"))
                grid = []
                for i in range(0, len(cells), ncols):
                    row = cells[i:i + ncols]
                    plain = " ".join(p for p, _ in row)
                    painted = " ".join(q for _, q in row)
                    pad = " " * ((ncols - len(row)) * (namew + 13))
                    grid.append((plain + pad, painted + pad))
                foot = clip(f"{short(scanned)}/{short(total)} {pct_of(scanned, total)}"
                            f"  kept {short(kept)}  "
                            + (doing or f"eta {clock(eta)}  {spin}"),
                            (namew + 13) * ncols - 1)
                out.extend(box([grid, [R(foot)]]))
                self._flush(out, final)
                return
            width = max(8, min(cols - 1, 60))
            here = self.current or "—"
            n, t = self.scanned.get(here, 0), self.total.get(here, 0)
            rows = [R(clip(f"{short(scanned)}/{short(total)} "
                           f"{pct_of(scanned, total)}  kept {short(kept)}  "
                           f"eta {clock(eta)}", width - 4)),
                    R(clip(doing or f"now {clip(here, 24)} {short(n)}/{short(t)} {spin}",
                           width - 4))]
            out.extend(box([rows]))
            self._flush(out, final)
            return
        # exact counts take whatever the biggest number needs; wide only when the whole
        # row provably fits
        wnum = len(f"{max([*self.total.values(), *self.scanned.values(), 1]):,}")
        wide = cols >= max(100, 70 + 3 * wnum)   # closed wide row is 69 + 3*wnum cells
        samples = []
        for name in self.order:
            tag, colour = self.kind[name]
            n, k, t = self.scanned[name], self.kept[name], self.total.get(name, 0)
            gauge = (bar(n, t, 22 if wide else 12),
                     OK_C if t and n >= t else colour)
            if wide:
                eff = f"{100 * k / n:5.1f}%" if n else "     ·"
                samples.append(R(f"{clip(name, 22)} ", (tag, colour), " ", gauge,
                                 f" {n:>{wnum},}/{t:<{wnum},} kept {k:>{wnum},} {eff}"))
            else:
                eff = f"{100 * k / n:4.0f}%" if n else "   ·"
                samples.append(R(f"{clip(name, 17)} ", (tag, colour), " ", gauge,
                                 f" {short(n):>6}/{short(t):<6} kept {short(k):>6} {eff}"))
        pct = f"{100 * scanned / total:5.1f}%" if total else "     ·"
        if wide:
            footer = [R(f"scanned {scanned:>11,}/{total:,}  {pct}   "
                        f"kept {kept:>11,}   est. buffer "
                        f"{kept * self.est_row_bytes / 2**20:7.1f} MiB"),
                      R(f"{rate / 1e6:4.2f}M rows/s   elapsed {clock(elapsed)}   "
                        + (doing or f"eta {clock(eta)}  {spin}"))]
        else:
            footer = [R(f"{short(scanned)}/{short(total)} {pct}  kept {short(kept)}  "
                        f"~{kept * self.est_row_bytes / 2**20:.0f} MiB"),
                      R(f"{rate / 1e6:4.2f}M rows/s  {clock(elapsed)} gone  "
                        + (doing or f"eta {clock(eta)}  {spin}"))]
        out.extend(box([samples, footer]))
        self._flush(out, final)

    def _flush(self, out: list[str], final: bool) -> None:
        if TTY and self.lines:
            sys.stdout.write(f"{ESC}{self.lines}A")
        if TTY:
            out = [line + f"{ESC}K" for line in out]
        sys.stdout.write("\n".join(out) + "\n")
        if not TTY and not final:
            sys.stdout.write("\n")
        if TTY and self.lines > len(out):
            sys.stdout.write(f"{ESC}0J")   # a shrinking frame erases what it uncovered
        sys.stdout.flush()
        self.lines = len(out)


