"""Width-aware terminal output shared by sequential and parallel runs."""
import shutil
import sys
import textwrap
import time
from pathlib import Path


def width():
    """Leave one spare column so live output never reaches the wrap boundary."""
    return max(1, shutil.get_terminal_size().columns - 1)


def clip(text, columns):
    """Shorten a filename in the middle to fit its column."""
    if len(text) <= columns:
        return text
    left = (columns - 1) // 2
    right = columns - left - 1
    return text[:left] + "…" + (text[-right:] if right else "")


def write(text):
    """Wrap a permanent message to the current terminal width."""
    print(textwrap.fill(text, width=width(), break_on_hyphens=False), flush=True)


class Progress:
    """Print completed files and an optional single-line live status."""

    def __init__(self, total):
        """Start a run's file counter and average throughput clock."""
        self.total, self.done, self.scanned = total, 0, 0
        self.start, self.layout, self.live = time.monotonic(), None, False

    def clear(self):
        """Erase the transient status before printing permanent output."""
        if self.live:
            print("\r\x1b[2K", end="", flush=True)
            self.live = False

    def status(self, path, done=None, total=0, kept=0):
        """Update one line on terminals; keep redirected logs free of redraws."""
        if not sys.stdout.isatty():
            return
        self.clear()
        counter = f"{self.done + 1}/{self.total}"
        detail = "opening" if done is None else f"{done / total if total else 1:.0%} | {done:,} read | {kept:,} kept"
        available = width() - len(counter) - len(detail) - 4
        name = f"  {clip(Path(path).name, available)}" if available >= 12 else ""
        print(clip(f"{counter}{name}  {detail}", width()), end="", flush=True)
        self.live = True

    def finish(self, path, entries, kept):
        """Print a table row, or a wrapped file record when columns will not fit."""
        self.clear()
        self.done += 1
        self.scanned += entries
        rate = round(self.scanned / max(time.monotonic() - self.start, 0.001))
        count_width = max(4, len(str(self.total)) * 2 + 1)
        values = [f"{entries:,}", f"{kept:,}", f"{rate:,}"]
        sizes = [max(minimum, len(value)) for minimum, value in zip((13, 11, 12), values)]
        name_width = width() - count_width - sum(sizes) - 8
        layout = (name_width, *sizes)
        counter = f"{self.done}/{self.total}"
        name = Path(path).name
        if name_width >= 20:
            if layout != self.layout:
                labels = "  ".join(f"{label:>{size}}" for label, size in zip(("Entries", "Kept", "Avg ev/s"), sizes))
                print(f"{'Done':>{count_width}}  {'File':{name_width}}  {labels}")
            numbers = "  ".join(f"{value:>{size}}" for value, size in zip(values, sizes))
            print(f"{counter:>{count_width}}  {clip(name, name_width):{name_width}}  {numbers}", flush=True)
        else:
            write(f"[{counter}] {name}")
            write(f"  Entries: {entries:,} | Kept: {kept:,} | Avg: {rate:,} ev/s")
            print()
        self.layout = layout

    def samples(self, samples):
        """Print the same sample totals after either execution mode."""
        print("\nSamples")
        for sample, entry in samples.items():
            rows = len(next(iter(entry["columns"].values())))
            write(f"  {sample}: {rows:,} rows")
