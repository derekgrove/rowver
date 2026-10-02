"""Progress output shared by sequential and parallel runs: a width-aware table, or JSON lines."""
import json
import os
import shutil
import sys
import textwrap
import time
from datetime import timedelta
from pathlib import Path

from . import __version__

# POSIX guarantees a pipe write of up to 512 bytes lands whole, so worker lines never interleave
PIPE_BUF = 512


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

    json = False

    def __init__(self, total):
        """Start a run's file counter and average throughput clock."""
        self.total, self.done, self.scanned = total, 0, 0
        self.start, self.layout, self.live = time.monotonic(), None, False

    def begin(self, study, samples, files, workers, out, inputs):
        """The study's name and size, before the first file."""
        write(study)
        write(f"Samples: {samples:,} | Files: {files:,} | Workers: {workers}")
        print()

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

    def finish(self, path, entries, kept, seconds=None):
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

    def failed(self, path, message):
        """A file that could not be read twice: say so and carry on."""
        self.clear()
        self.done += 1
        write(f"[{self.done}/{self.total}] {Path(path).name} could not be read, left out: {message}")
        self.layout = None

    def samples(self, samples):
        """Print the same sample totals after either execution mode."""
        print("\nSamples")
        for sample, entry in samples.items():
            rows = len(next(iter(entry["columns"].values())))
            write(f"  {sample}: {rows:,} rows")

    def pack(self, rows):
        """Nothing to show while the columns are packed."""

    def wrote(self, path, rows, columns, size, seconds, failed, left_out, added=None):
        """Where the file went, how big it is, and any file that was not read."""
        print()
        write(f"Wrote {path}")
        if added:
            write(f"Added: {', '.join(added)}")
        write(f"{rows:,} rows | {columns} columns | {size / 1e6:.1f} MB | {timedelta(seconds=round(seconds))}")
        if failed and added:
            write(f"{len(failed):,} files of the recipe are not in the cartridge and were not read.")
        elif failed:
            write(f"{len(failed):,} of {self.total:,} files could not be read and are not in the file.")
        if left_out:
            write(f"Samples left out, no file read: {', '.join(left_out)}")


def emit(event, whole=False):
    """One JSON object as one line in one write; a worker's line is kept under PIPE_BUF by shortening its message."""
    line = json.dumps(event, separators=(",", ":"))
    message = event.get("message", "")
    while whole and len(line) >= PIPE_BUF and message:
        message = message[:max(0, len(message) - (len(line) + 1 - PIPE_BUF) - 3)]
        line = json.dumps({**event, "message": message + "..."}, separators=(",", ":"))
    data = (line + "\n").encode("ascii")
    while data:
        data = data[os.write(1, data):]


class JsonProgress:
    """The same calls as Progress, written as one JSON object per line on stdout for the app.

    Workers write their own file lines, so the parent writes only start, pack, wrote and error,
    and the lines of files it read itself.
    """

    json = True

    def __init__(self, places):
        """places: each input path's place in the start line's inputs, which every file line names."""
        self.places = places

    def file(self, path, state, **fields):
        """One line about one file."""
        emit({"event": "file", "index": self.places.get(path), "path": path, "state": state, **fields}, whole=True)

    def begin(self, study, samples, files, workers, out, inputs):
        """The study, its size, the presser version, and every input in order."""
        emit({"event": "start", "study": study, "samples": samples, "files": files, "workers": workers,
              "presser": __version__, "out": str(out), "inputs": [list(pair) for pair in inputs]})

    def clear(self):
        """Nothing is drawn, so nothing to erase."""

    def status(self, path, done=None, total=0, kept=0):
        """opening, then one reading line per chunk."""
        if done is None:
            self.file(path, "opening")
        else:
            self.file(path, "reading", done=int(done), total=int(total), kept=int(kept))

    def finish(self, path, entries, kept, seconds):
        """done, with the entries scanned, the rows kept and the time the file took."""
        self.file(path, "done", entries=int(entries), kept=int(kept), seconds=round(seconds, 3))

    def failed(self, path, message):
        """failed, with the reader's own message."""
        self.file(path, "failed", message=message)

    def samples(self, samples):
        """Sample totals are in the written file; the app reads them there."""

    def pack(self, rows):
        """The files are read; the columns are being packed and written."""
        emit({"event": "pack", "rows": int(rows)})

    def wrote(self, path, rows, columns, size, seconds, failed, left_out, added=None):
        """The file is written; added names the columns added to a cartridge, when that is what was done."""
        emit({"event": "wrote", "path": str(path), "rows": int(rows), "columns": columns, "bytes": size,
              "seconds": round(seconds, 3), "failed": failed, "left_out": left_out,
              **({"added": added} if added else {})})


def error(kind, message, trace):
    """The press stopped: kind is recipe, input, write, differs, nothing or cancelled."""
    emit({"event": "error", "kind": kind, "message": message, "traceback": trace})
