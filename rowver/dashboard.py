"""Live one-pass progress for ``rowver run``."""
from __future__ import annotations

import contextlib
import shutil
import sys
import threading
import time
import unicodedata
from pathlib import PurePosixPath
from urllib.parse import urlsplit

from .press import Cart, _remote

ESC = "\x1b["
DIM = "2"
SPIN = "|/-\\"


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


def clock(seconds: float) -> str:
    minutes, sec = divmod(max(int(seconds), 0), 60)
    hours, minutes = divmod(minutes, 60)
    return f"{hours}:{minutes:02d}:{sec:02d}" if hours else f"{minutes:02d}:{sec:02d}"


class Dashboard:
    """Own one output stream and the state of the skimming and pressing phases."""

    def __init__(self, cart: Cart, output=None) -> None:
        self.output = output if output is not None else sys.stdout
        self.tty = self.output.isatty()
        self.scanned = {name: 0 for name in cart.fileset}
        self.kept = {name: 0 for name in cart.fileset}
        self.phase = "starting"
        self.current = ""
        self.current_file = ""
        self.t0 = time.monotonic()
        self.lines = 0
        self.last_draw = 0.0
        self.spins = 0
        self.reported: set[str] = set()
        self.lock = threading.Lock()

    def _paint(self, text: str, code: str) -> str:
        return f"{ESC}{code}m{text}{ESC}0m" if self.tty else text

    def _box(self, rows: list[str]) -> list[str]:
        width = max((cells(row) for row in rows), default=0)
        rule = self._paint("─" * (width + 2), DIM)
        edge = self._paint("│", DIM)
        return ([self._paint("┌", DIM) + rule + self._paint("┐", DIM)]
                + [f"{edge} {row}{' ' * (width - cells(row))} {edge}" for row in rows]
                + [self._paint("└", DIM) + rule + self._paint("┘", DIM)])

    def _spin(self, final: bool) -> str:
        self.spins += 1
        return "" if final else SPIN[self.spins % len(SPIN)]

    def source(self, sample: str, path: str, tree: str) -> None:
        """Observe a source immediately before the real skim opens it."""
        with self.lock:
            self.current = sample
            self.current_file = path
            self.phase = "skimming"
        if self.tty:
            self.draw()
        else:
            self._write_lines([f"skimming {_clean(sample)}: {source_name(path)}"])

    def sample_done(self, sample: str) -> None:
        """Report a finished sample once; no total is known in advance."""
        with self.lock:
            if sample in self.reported:
                return
            self.reported.add(sample)
            line = self._skim_line(sample)
        if self.tty:
            self.draw()
        else:
            self._write_lines([line])

    def chunk(self, sample: str | None, scanned: int, kept: int) -> None:
        """Count one processed chunk; ``sample=None`` marks the start of pressing."""
        with self.lock:
            if sample is None:
                self.phase = "pressing"
            else:
                self.scanned[sample] += scanned
                self.kept[sample] += kept
                self.current = sample
        if sample is None:
            if self.tty:
                self.draw()
            else:
                self._write_lines(["pressing cartridge…"])
        elif self.tty and time.monotonic() - self.last_draw > 0.15:
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
        if not self.tty:
            return
        with self.lock:
            self._draw(final)

    def _draw(self, final: bool) -> None:
        self.last_draw = time.monotonic()
        scanned, kept = sum(self.scanned.values()), sum(self.kept.values())
        elapsed = clock(time.monotonic() - self.t0)
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
        rows = [clip(heading, width).rstrip(),
                clip(detail, width).rstrip(),
                clip(f"{scanned:,} scanned · {kept:,} kept · elapsed {elapsed}", width).rstrip()]
        self._flush(self._box(rows))

    def _write_lines(self, lines: list[str]) -> None:
        self.output.write("\n".join(lines) + "\n")
        self.output.flush()

    def _flush(self, lines: list[str]) -> None:
        if self.lines:
            self.output.write(f"{ESC}{self.lines}A")
        lines = [line + f"{ESC}K" for line in lines]
        self._write_lines(lines)
        if self.lines > len(lines):
            self.output.write(f"{ESC}0J")
            self.output.flush()
        self.lines = len(lines)
