"""Skim ROOT trees into the pikl of columns that load.py reads.

The `skim_config` argument these functions take is a module with two things in it:
KEEP, a function saying which events to keep, and COLUMNS, a dict saying what to
store for each one. configs/ holds them; skim_config.py loads them.

A config may also define PREPARE(chunk), which runs once per chunk before KEEP and
returns a chunk with whatever it needs attached — derived objects both KEEP and COLUMNS
want, computed once instead of per column.

KEEP and PREPARE may take a second argument, the sample's metadata from the fileset.
Take it if you need it (is_2018, isRun3); leave it off and the one-argument form still
works.
"""
import gzip
import importlib.util
import pickle
import sys
import time
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path
from types import ModuleType

import awkward as ak
import numpy as np
import uproot

PIKLS: Path = Path(__file__).with_name("pikls")
STEP: int = 250_000
BAR: int = 22          # characters of progress bar


def progress(label: str, done: int, total: int, since: float, end: bool = False) -> None:
    """One line saying a skim is alive, rewritten in place — a chunk can take minutes.

    On a terminal it redraws; piped to a file or a CI log it prints once per sample, at
    the end, because a log full of carriage returns helps nobody.
    """
    tty = sys.stderr.isatty()
    if not tty and not end:
        return
    frac = done / total if total else 1.0
    rate = done / max(time.time() - since, 1e-9)
    bar = "#" * int(BAR * frac) + "." * (BAR - int(BAR * frac))
    # READ, not kept: KEEP has not run on most of these yet, and the two differ
    line = (f"  {label[:30]:<30} [{bar}] {frac:5.1%}  {done:>11,} read  "
            f"{rate:>8,.0f}/s")
    sys.stdout.flush()   # stdout block-buffers into a pipe; without this the bar jumps ahead
    print(line if not tty else f"\r{line}", end="\n" if end or not tty else "",
          file=sys.stderr, flush=True)

def reject_masked(dict_key: str, values: np.ndarray) -> np.ndarray:
    """
    Take in an array (column) and convert to an ordinary numpy array, refuse if any value is missing.

    An expression you write in COLUMNS can produce no value for some events — asking for the
    max tau pT in an event with no taus, say. Sending that None or nan downstream could mess
    things up, so an array with anything missing is rejected rather than patched up.

        ak.max(PT_tau, axis=1)  ->  [19.5, 163.1, None]   no taus in the third event
        after ak.to_numpy       ->  [19.5, 163.1, nan]    awkward's None becomes numpy's nan
        this function           ->  refuses, naming the column and how many events
    """
    if not isinstance(values, np.ma.MaskedArray):
        return values
    miss = int(np.ma.getmaskarray(values).sum())
    assert not miss, (f"column {dict_key}: {miss} of {len(values)} events have no value. "
                      f"Might need to add a condition to KEEP that guarantees events pass this column.")
    return values.data # This converts a MaskedArray to normal ndarray, saving size when pikl'd


def with_meta(fn, chunk: ak.Array, meta: dict):
    """Call fn(chunk) or fn(chunk, meta), whichever it was written to take."""
    return fn(chunk, meta) if fn.__code__.co_argcount >= 2 else fn(chunk)


_VIRTUAL: bool | None = None


def supports_virtual(ttree) -> bool:
    """Whether this uproot can hand back lazy arrays. Older ones cannot, and say so once."""
    global _VIRTUAL
    if _VIRTUAL is None:
        import inspect
        try:
            _VIRTUAL = "virtual" in inspect.signature(ttree.arrays).parameters
        except (TypeError, ValueError):
            _VIRTUAL = False
        if not _VIRTUAL:
            print(f"\n  NOTE: uproot {uproot.__version__} has no arrays(virtual=...), so every "
                  f"branch is read,\n        not only the ones COLUMNS touches. Same numbers "
                  f"either way, but slower and\n        heavier — about 8x on a NanoAOD file. "
                  f"Lazy reads arrived in uproot 5.6.7:\n\n"
                  f"            pip install -U 'uproot>=5.6.7'\n", flush=True)
    return _VIRTUAL


def _closest(want: str, have, n: int = 6) -> list[str]:
    """Branch names most like the one asked for, so a typo or a rename is obvious."""
    import difflib
    hit = difflib.get_close_matches(want, list(have), n=n, cutoff=0.4)
    return hit or sorted(have)[:n]


def columns(skim_config: ModuleType, chunk: ak.Array) -> dict[str, np.ndarray]:
    """
    Evaluate COLUMNS on one chunk:
    a str is a predefined branch name,
    a lambda is computed and the corresponding column is filled with its output.
    """
    out = {}
    for column_name, branch_or_lambda in skim_config.COLUMNS.items():
        if isinstance(branch_or_lambda, str):
            # awkward raises with the whole array in the message; say which branch and what
            # the file does have, because a config written for other files lands here first
            if branch_or_lambda not in chunk.fields:
                raise KeyError(
                    f"COLUMNS[{column_name!r}] reads branch {branch_or_lambda!r}, which this "
                    f"file does not have.\n  It has {len(chunk.fields)} branches; the closest "
                    f"names are {_closest(branch_or_lambda, chunk.fields)}.\n"
                    f"  Edit COLUMNS in your study's config.py.")
            values = chunk[branch_or_lambda] # just fill our column with the branch that is already in the skim
        else:
            values = branch_or_lambda(chunk) # compute the lambda on the events of the skim, fill column with result
        out[column_name] = reject_masked(column_name, ak.to_numpy(values)) # actually filling the column here (writing to dict)

    lengths = {len(v) for v in out.values()}
    assert len(lengths) == 1, f"columns disagree on length: {sorted(lengths)}"
    return out


def skim(skim_config: ModuleType, files: dict[str, str],
         step: int = STEP, metadata: dict | None = None,
         threads: int = 0, label: str = "") -> dict[str, np.ndarray]:
    """
    Read one sample, keeping only the events that pass skim_config.KEEP.

    `files` is {path: tree name} — coffea's shape, and what samples.files() gives per
    sample. A NanoAOD sample split over 32 files is one call. Read in chunks of `step`
    events so memory stays flat however big the file is, and `virtual=True` means only
    the branches COLUMNS actually touches come off disk (lazy, no wasted loading).

    `threads` above 0 hands uproot a thread pool for basket decompression, which it
    otherwise does on the calling thread — its default is a TrivialExecutor. Decompression
    is C and drops the GIL, so this is real parallelism inside one sample.

    `label` non-empty reports progress against this sample's own total. The trees are
    opened up front to know that total, which is the same one open per file the loop
    below would do anyway — it does not cost a remote read.
    """
    assert files, "no files given"

    pool = uproot.ThreadPoolExecutor(threads) if threads else None
    trees = {path: uproot.open(path)[tree] for path, tree in files.items()}
    total = sum(t.num_entries for t in trees.values())
    began, seen = time.time(), 0

    per_chunk = []
    for path, ttree in trees.items():
        tree = files[path]
        assert ttree.num_entries, f"{tree} in {path} is empty"
        for start in range(0, ttree.num_entries, step):
            lazy = {"virtual": True} if supports_virtual(ttree) else {}
            chunk = ttree.arrays(entry_start=start,
                                 entry_stop=min(start + step, ttree.num_entries),
                                 decompression_executor=pool, **lazy)
            if hasattr(skim_config, "PREPARE"):
                chunk = with_meta(skim_config.PREPARE, chunk, metadata or {})
            kept = chunk[with_meta(skim_config.KEEP, chunk, metadata or {})]
            per_chunk.append(columns(skim_config, kept))
            seen = min(seen + step, total)
            if label:
                progress(label, seen, total, began)
    if label:
        progress(label, total, total, began, end=True)

    # stitch the chunks back together, one column at a time
    return {column_name: np.concatenate([chunk[column_name] for chunk in per_chunk])
            for column_name in per_chunk[0]}


def _one(job: tuple) -> tuple:
    """Skim one sample in a worker process, re-importing the config there."""
    config_path, sample, files, step, meta, threads = job
    spec = importlib.util.spec_from_file_location("rowver_worker_config", config_path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return sample, skim(mod, files, step, meta, threads)


def pikl(name: str, fileset: dict[str, dict[str, str]], skim_config: ModuleType,
         step: int = STEP, metadata: dict[str, dict] | None = None,
         dest: str | Path | None = None,
         jobs: int = 1, threads: int = 0) -> dict[str, dict[str, np.ndarray]]:
    """
    Skim every sample and write them all into <dest>/<name>.pkl.

    `fileset` is what samples.files() returns: {sample name: {path: tree name}}.
    `metadata` is samples.meta(fs) — each sample's metadata, handed to KEEP and PREPARE.
    `jobs` above 1 skims samples in parallel processes and `threads` above 0 decompresses
    baskets in parallel within each one; they compose. The result is ordered as the fileset
    is whichever you use, so the pikl does not depend on how it was built.
    """
    meta_of = lambda s: (metadata or {}).get(s)
    if jobs > 1 and getattr(skim_config, "__file__", None):
        work = [(skim_config.__file__, s, f, step, meta_of(s), threads)
                for s, f in fileset.items()]
        with ProcessPoolExecutor(max_workers=jobs) as pool:
            done = dict(pool.map(_one, work))
        columns_by_sample = {s: done[s] for s in fileset}      # fileset order, not finish order
    else:
        # only the serial path reports progress: parallel workers cannot share one line,
        # and interleaved bars from N processes are worse than silence
        columns_by_sample = {}
        for i, (sample, files) in enumerate(fileset.items(), 1):
            columns_by_sample[sample] = skim(
                skim_config, files, step, meta_of(sample), threads,
                label=f"[{i}/{len(fileset)}] {sample}")

    # the "results" key is the shape load.py expects; keep the two in step
    where = Path(dest) if dest else PIKLS
    where.mkdir(parents=True, exist_ok=True)
    with gzip.open(where / f"{name}.pkl", "wb", compresslevel=1) as pickle_file:
        pickle.dump({"results": columns_by_sample}, pickle_file)
    return columns_by_sample


def check(skim_config: ModuleType, files: dict[str, str],
          metadata: dict | None = None) -> dict[str, np.ndarray]:
    """
    Skim one tree twice, in chunks and in a single pass, and require identical results.

    This is what catches a stitching bug in skim(). Getting the concatenation wrong
    still gives you the right NUMBER of events, so only comparing element by element,
    against a run whose chunk boundaries fall elsewhere, would notice.
    """
    chunked = skim(skim_config, files, metadata=metadata)
    unchunked = skim(skim_config, files, step=10 ** 9, metadata=metadata)  # bigger than any tree

    assert chunked.keys() == unchunked.keys(), "chunked and unchunked disagree on columns"
    for column_name in chunked:
        assert np.array_equal(chunked[column_name], unchunked[column_name]), \
            f"{column_name} differs between chunked and unchunked"
    return chunked
