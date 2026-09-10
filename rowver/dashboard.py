"""Live progress for ``rowver run``: one-pass by default, indexed with exact totals in robust mode."""
from __future__ import annotations

import contextlib
import shutil
import sys
import threading
import time
import unicodedata
from pathlib import PurePosixPath
from urllib.parse import urlsplit

from .press import Cart, CartError, _open_sample_tree, _remote, _source_failure

ESC = "\x1b["
DIM = "2"
SPIN = "|/-\\"
UNWEIGHTED_C, WEIGHTED_C, OK_C = "36", "33", "32"


def _clean(text: str) -> str:
    """Replace control characters so user text cannot repaint the terminal."""
    return "".join("·" if unicodedata.category(ch) in ("Cc", "Cf") or ch == "\x7f"
                   else ch for ch in str(text))


def cells(text: str) -> int:
    """Terminal display cells, counting wide East Asian characters twice."""
    return sum(2 if unicodedata.east_asian_width(ch) in "WF" else 1 for ch in text)


def _take_cells(text: str, width: int, *, tail: bool = False) -> str:
    chars = reversed(text) if tail else iter(text)
    selected: list[str] = []
    used = 0
    for ch in chars:
        size = 2 if unicodedata.east_asian_width(ch) in "WF" else 1
        if used + size > width:
            break
        selected.append(ch)
        used += size
    if tail:
        selected.reverse()
    return "".join(selected)


def clip(text: str, width: int) -> str:
    """Sanitize, clip, and pad text to exactly ``width`` display cells."""
    out = _take_cells(_clean(text), max(width, 0))
    return out + " " * max(width - cells(out), 0)


def source_name(path: str, width: int = 28) -> str:
    """A safe, middle-elided basename bounded by display cells."""
    parsed = urlsplit(path) if _remote(path) else None
    raw = PurePosixPath(parsed.path if parsed else path).name
    if not raw:
        raw = parsed.hostname or "remote source" if parsed else path
    name = _clean(raw)
    if cells(name) <= width:
        return name
    remaining = max(width - 1, 0)
    left = remaining // 2
    right = remaining - left
    return _take_cells(name, left) + "…" + _take_cells(name, right, tail=True)


def bar(done: int, total: int, width: int) -> str:
    fill = width if not total else round(width * min(done, total) / total)
    return "█" * fill + "░" * (width - fill)


def short(n: int) -> str:
    """At most six characters at any magnitude."""
    for div, unit in ((1e12, "T"), (1e9, "G"), (1e6, "M"), (1e3, "k")):
        if n >= div:
            value = n / div
            return f"{value:.2f}{unit}" if value < 100 else f"{value:.0f}{unit}"
    return str(n)


def pct_of(done: int, total: int) -> str:
    return f"{100 * done / total:.0f}%" if total else "·"


def clock(seconds: float) -> str:
    minutes, sec = divmod(max(int(seconds), 0), 60)
    hours, minutes = divmod(minutes, 60)
    return f"{hours}:{minutes:02d}:{sec:02d}" if hours else f"{minutes:02d}:{sec:02d}"


class Dashboard:
    """Own one output stream and the state of the indexing, skimming, and pressing phases."""

    def __init__(self, cart: Cart, output=None, *, robust: bool = False) -> None:
        self.output = output if output is not None else sys.stdout
        self.tty = self.output.isatty()
        self.robust = robust
        self.est_row_bytes = 8 * len(cart.columns) + 12
        self.order = list(cart.fileset)
        self.weighted = {
            name: entry["metadata"]["weights"] is not None
            for name, entry in cart.fileset.items()
        }
        self.sources = [
            (name, path, tree)
            for name, entry in cart.fileset.items()
            for path, tree in entry["files"].items()
        ]
        self.total: dict[str, int] = {}
        self.scanned = {name: 0 for name in self.order}
        self.kept = {name: 0 for name in self.order}
        self.phase = "indexing" if robust else "starting"
        self.index_files = len(self.sources)
        self.index_file = 1 if self.sources else 0
        self.indexed_files = 0
        self.indexed_entries = 0
        self.current = self.sources[0][0] if self.sources else ""
        self.current_file = self.sources[0][1] if self.sources else ""
        self.t0 = time.monotonic()
        self.lines = 0
        self.last_draw = 0.0
        self.spins = 0
        self.reported: set[str] = set()
        self.lock = threading.Lock()

    def _paint(self, text: str, code: str) -> str:
        return f"{ESC}{code}m{text}{ESC}0m" if self.tty else text

    def _row(self, *bits) -> tuple[str, str]:
        plain = ""
        painted = ""
        for bit in bits:
            text, colour = bit if isinstance(bit, tuple) else (bit, None)
            plain += text
            painted += self._paint(text, colour) if colour else text
        return plain, painted

    def _box(self, sections: list[list[tuple[str, str]]]) -> list[str]:
        width = max((cells(plain) for section in sections for plain, _ in section),
                    default=0)
        rule = self._paint("─" * (width + 2), DIM)
        edge = self._paint("│", DIM)
        lines = [self._paint("┌", DIM) + rule + self._paint("┐", DIM)]
        for at, section in enumerate(sections):
            if at:
                lines.append(self._paint("├", DIM) + rule + self._paint("┤", DIM))
            for plain, painted in section:
                lines.append(f"{edge} {painted}{' ' * (width - cells(plain))} {edge}")
        lines.append(self._paint("└", DIM) + rule + self._paint("┘", DIM))
        return lines

    def _colour(self, sample: str, done: int, count: int) -> str:
        return (OK_C if done >= count else
                WEIGHTED_C if self.weighted[sample] else UNWEIGHTED_C)

    def _spin(self, final: bool) -> str:
        self.spins += 1
        return "" if final else SPIN[self.spins % len(SPIN)]

    def _totals(self) -> tuple[int, int, int, float, float]:
        """Scanned, kept, and total rows, seconds elapsed, and seconds remaining."""
        scanned = sum(self.scanned.values())
        total = sum(self.total.values())
        elapsed = time.monotonic() - self.t0
        rate = scanned / elapsed if elapsed > 0 else 0.0
        eta = (total - scanned) / rate if rate > 0 else 0.0
        return scanned, sum(self.kept.values()), total, elapsed, eta

    def index(self, cart: Cart) -> None:
        """Open every source once to learn exact entry totals (robust mode only)."""
        per_sample = {name: 0 for name in self.order}
        for number, (name, path, tree) in enumerate(self.sources, 1):
            with self.lock:
                self.index_file = number
                self.current = name
                self.current_file = path
            try:
                with _open_sample_tree(name, path, tree) as source:
                    entries = int(source.num_entries)
            except CartError:
                raise
            except Exception as exc:
                raise _source_failure(name, path, exc) from exc
            with self.lock:
                per_sample[name] += entries
                self.total[name] = per_sample[name]
                self.indexed_files = number
                self.indexed_entries += entries
                snapshot = self._index_line("indexed", entries)
            if self.tty:
                self.draw()
            else:
                self._write_lines([snapshot])
        for name in self.order:
            self.total.setdefault(name, 0)

    def source(self, sample: str, path: str, tree: str) -> None:
        """Observe a source immediately before the real skim opens it."""
        with self.lock:
            self.current = sample
            self.current_file = path
            if not self.robust:
                self.phase = "skimming"
        if self.robust:
            return
        if self.tty:
            self.draw()
        else:
            self._write_lines([f"skimming {_clean(sample)}: {source_name(path)}"])

    def sample_done(self, sample: str) -> None:
        """Report a finished sample in one-pass mode, where no total is known in advance."""
        if self.robust:
            return
        with self.lock:
            if sample in self.reported:
                return
            self.reported.add(sample)
            line = self._skim_line(sample)
        if self.tty:
            self.draw()
        else:
            self._write_lines([line])

    def begin_skim(self) -> None:
        with self.lock:
            self.phase = "skimming"
            self.t0 = time.monotonic()
        if self.tty:
            self.draw()

    def _settled(self, through: str | None) -> list[str]:
        """Durable lines for unreported samples up to ``through`` that reached their exact total."""
        if not self.robust or self.tty:
            return []
        names = self.order if through is None else self.order[:self.order.index(through) + 1]
        lines = []
        for name in names:
            if name not in self.reported and self.scanned[name] >= self.total.get(name, 0):
                self.reported.add(name)
                lines.append(self._skim_line(name))
        return lines

    def chunk(self, sample: str | None, scanned: int, kept: int) -> None:
        """Count one processed chunk; ``sample=None`` marks the start of pressing."""
        with self.lock:
            if sample is None:
                lines = self._settled(None)
                if not self.tty:
                    lines.append("pressing cartridge…")
                self.phase = "pressing"
            else:
                self.scanned[sample] += scanned
                self.kept[sample] += kept
                self.current = sample
                lines = self._settled(sample)
                boundary = self.robust and self.scanned[sample] >= self.total.get(sample, 0)
        if lines:
            self._write_lines(lines)
        if sample is None:
            if self.tty:
                self.draw()
            return
        if self.tty and (time.monotonic() - self.last_draw > 0.15 or boundary):
            self.draw()

    __call__ = chunk

    def _skim_line(self, sample: str) -> str:
        return (f"skimmed {_clean(sample)}: {self.scanned[sample]:,} scanned, "
                f"{self.kept[sample]:,} kept")

    @contextlib.contextmanager
    def live(self):
        """Draw immediately, then animate TTY output twice per second."""
        with self.lock:
            self.t0 = time.monotonic()
        self.draw()
        if not self.tty:
            yield
            return
        stop = threading.Event()

        def tick() -> None:
            while not stop.wait(0.5):
                try:
                    self.draw()
                except Exception:
                    stop.set()
                    return

        thread = threading.Thread(target=tick, daemon=True)
        thread.start()
        try:
            yield
        finally:
            stop.set()
            thread.join()

    def draw(self, final: bool = False) -> None:
        if final and not self.tty:
            return
        with self.lock:
            self._draw(final)

    def _draw(self, final: bool) -> None:
        self.last_draw = time.monotonic()
        if not self.robust:
            self._draw_fast(final)
        elif self.phase == "indexing":
            self._draw_indexing(final)
        else:
            self._draw_samples(final)

    def _draw_indexing(self, final: bool) -> None:
        if not self.tty:
            if self.index_files:
                self._write_lines([self._index_line("indexing")])
            return
        spin = self._spin(final)
        elapsed = time.monotonic() - self.t0
        width = max(shutil.get_terminal_size((120, 40)).columns - 5, 1)
        rows = [
            self._row(clip(
                f"Indexing files  {self.index_file}/{self.index_files}  {spin}", width
            ).rstrip()),
            self._row(clip(
                f"{clip(self.current, 28).rstrip()} · {source_name(self.current_file)}",
                width,
            ).rstrip()),
            self._row(clip(
                f"{self.indexed_files} files indexed · {short(self.indexed_entries)} "
                f"entries · elapsed {clock(elapsed)}",
                width,
            ).rstrip()),
        ]
        self._flush(self._box([rows]), final)

    def _draw_fast(self, final: bool) -> None:
        if not self.tty:
            return
        scanned, kept, _, elapsed, _ = self._totals()
        spin = self._spin(final)
        width = max(shutil.get_terminal_size((120, 40)).columns - 5, 1)
        if self.phase == "starting":
            heading = f"Starting one-pass skim… {spin}"
            detail = "Waiting for the first source"
        elif self.phase == "pressing":
            heading = ("Cartridge written" if final
                       else f"Pressing the cartridge (pack + gzip)… {spin}")
            detail = f"{scanned:,} scanned · {kept:,} kept"
        else:
            heading = f"Skimming {_clean(self.current)}  {spin}"
            detail = source_name(self.current_file)
        rows = [
            self._row(clip(heading, width).rstrip()),
            self._row(clip(detail, width).rstrip()),
            self._row(clip(
                f"{scanned:,} scanned · {kept:,} kept · elapsed {clock(elapsed)}",
                width,
            ).rstrip()),
        ]
        self._flush(self._box([rows]), final)

    def _index_line(self, verb: str, entries: int | None = None) -> str:
        prefix = f"{verb} {self.index_file}/{self.index_files} {_clean(self.current)}: "
        if entries is None:
            return prefix + source_name(self.current_file)
        return (prefix + source_name(self.current_file)
                + f" ({short(entries)} entries; {short(self.indexed_entries)} total)")

    def _draw_samples(self, final: bool) -> None:
        """Pick the layout the terminal has room for: a table, a grid, or two lines."""
        spin = self._spin(final)
        doing = (f"pressing the cartridge (pack + gzip)… {spin}"
                 if self.phase == "pressing" and not final else None)
        cols, screen_rows = shutil.get_terminal_size((120, 40))
        if cols >= 72 and len(self.order) + 5 < screen_rows:
            lines = self._table(cols, spin, doing)
        else:
            lines = self._grid(cols, screen_rows, spin, doing) or self._tiny(cols, spin, doing)
        self._flush(lines, final)

    def _grid(self, cols: int, screen_rows: int, spin: str, doing: str | None) -> list[str] | None:
        """Several samples per line; None when even that does not fit."""
        free = screen_rows - 6
        ncols = -(-len(self.order) // max(free, 1)) if self.order else 1
        namew = min(max((cells(_clean(n)) for n in self.order), default=0),
                    (cols - 4) // max(ncols, 1) - 13)
        if free < 1 or namew < 8:
            return None
        scanned, kept, total, _, eta = self._totals()
        compact = []
        for name in self.order:
            done, count = self.scanned[name], self.total.get(name, 0)
            compact.append(self._row(f"{clip(name, namew)} ",
                                     (bar(done, count, 6), self._colour(name, done, count)),
                                     f" {pct_of(done, count):>4}"))
        grid = []
        for at in range(0, len(compact), ncols):
            row = compact[at:at + ncols]
            plain = " ".join(value for value, _ in row)
            painted = " ".join(value for _, value in row)
            pad = " " * ((ncols - len(row)) * (namew + 13))
            grid.append((plain + pad, painted + pad))
        summary = (f"{short(scanned)}/{short(total)} "
                   f"{pct_of(scanned, total)} kept {short(kept)}")
        foot = clip(summary + "  " + (doing or f"eta {clock(eta)}  {spin}"),
                    min(max((namew + 13) * ncols - 1, cells(summary)), cols - 5))
        return self._box([grid, [self._row(foot)]])

    def _tiny(self, cols: int, spin: str, doing: str | None) -> list[str]:
        """Two lines: overall progress and the current sample."""
        scanned, kept, total, _, eta = self._totals()
        width = max(8, min(cols - 1, 60))
        here = self.current or "—"
        done, count = self.scanned.get(here, 0), self.total.get(here, 0)
        rows = [self._row(clip(f"{short(scanned)}/{short(total)} "
                               f"{pct_of(scanned, total)}  kept {short(kept)}  "
                               f"eta {clock(eta)}", width - 4)),
                self._row(clip(doing or
                               f"now {clip(here, 24)} {short(done)}/{short(count)} {spin}",
                               width - 4))]
        return self._box([rows])

    def _table(self, cols: int, spin: str, doing: str | None) -> list[str]:
        """One gauge per sample and a two-line footer; wide terminals get exact counts."""
        scanned, kept, total, elapsed, eta = self._totals()
        rate = scanned / elapsed if elapsed > 0 else 0.0
        width_number = len(f"{max([*self.total.values(), *self.scanned.values(), 1]):,}")
        wide = cols >= max(100, 70 + 3 * width_number)
        samples = []
        for name in self.order:
            done, retained, count = (self.scanned[name], self.kept[name],
                                     self.total.get(name, 0))
            gauge = (bar(done, count, 22 if wide else 12), self._colour(name, done, count))
            if wide:
                efficiency = f"{100 * retained / done:5.1f}%" if done else "     ·"
                samples.append(self._row(f"{clip(name, 22)} ", gauge,
                                         f" {done:>{width_number},}/{count:<{width_number},} "
                                         f"kept {retained:>{width_number},} {efficiency}"))
            else:
                efficiency = f"{100 * retained / done:4.0f}%" if done else "   ·"
                samples.append(self._row(f"{clip(name, 17)} ", gauge,
                                         f" {short(done):>6}/{short(count):<6} "
                                         f"kept {short(retained):>6} {efficiency}"))
        percent = f"{100 * scanned / total:5.1f}%" if total else "     ·"
        if wide:
            footer = [self._row(f"scanned {scanned:>11,}/{total:,}  {percent}   "
                                f"kept {kept:>11,}   est. buffer "
                                f"{kept * self.est_row_bytes / 2**20:7.1f} MiB"),
                      self._row(f"{rate / 1e6:4.2f}M rows/s   elapsed {clock(elapsed)}   "
                                + (doing or f"eta {clock(eta)}  {spin}"))]
        else:
            footer = [self._row(f"{short(scanned)}/{short(total)} {percent}  "
                                f"kept {short(kept)}  "
                                f"~{kept * self.est_row_bytes / 2**20:.0f} MiB"),
                      self._row(f"{rate / 1e6:4.2f}M rows/s  {clock(elapsed)} gone  "
                                + (doing or f"eta {clock(eta)}  {spin}"))]
        return self._box([samples, footer])

    def _write_lines(self, lines: list[str]) -> None:
        self.output.write("\n".join(lines) + "\n")
        self.output.flush()

    def _flush(self, lines: list[str], final: bool) -> None:
        if self.tty and self.lines:
            self.output.write(f"{ESC}{self.lines}A")
        if self.tty:
            lines = [line + f"{ESC}K" for line in lines]
        self._write_lines(lines)
        if self.tty and self.lines > len(lines):
            self.output.write(f"{ESC}0J")
            self.output.flush()
        self.lines = len(lines)
