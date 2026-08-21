"""Skim ROOT trees into the pikl of columns that load.py reads.

The Cart says which files to read, which events to keep, and what to store for each one.
Every hook it holds — keep, prepare, and any column that is a function — may take a
second argument, that sample's metadata, with the file being read folded into it. See
cart.py for the whole contract.
"""
import gzip
import pickle
import sys
import time
from datetime import datetime, timezone
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

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


def reject_masked(column: str, values: np.ndarray) -> np.ndarray:
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
    assert not miss, (f"column {column}: {miss} of {len(values)} events have no value. "
                      f"Might need to add a condition to KEEP that guarantees events pass this column.")
    return values.data # This converts a MaskedArray to normal ndarray, saving size when pikl'd


def call_hook(fn, chunk: ak.Array, meta: dict):
    """Call fn(chunk) or fn(chunk, meta), whichever it was written to take."""
    return fn(chunk, meta) if fn.__code__.co_argcount >= 2 else fn(chunk)


_VIRTUAL: bool | None = None


def supports_virtual(ttree) -> bool:
    """Checks whether the user's uproot can hand back lazy arrays"""
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
    """Branch names most like the one asked for"""
    import difflib
    hit = difflib.get_close_matches(want, list(have), n=n, cutoff=0.4)
    return hit or sorted(have)[:n]


def value_of(spec, chunk: ak.Array, meta: dict, where: str):
    """One column's values: a string names a branch to copy, anything else is called."""
    if not isinstance(spec, str):
        try:
            return call_hook(spec, chunk, meta)
        except Exception as e:
            # awkward raises 20 frames deep with no idea which column or sample it was in
            raise AssertionError(
                f"{where} on sample {meta['sample']!r} raised {type(e).__name__}: {e}\n"
                f"  (a hook taking two arguments is handed the metadata as the second)") from e
    # awkward raises with the whole array in the message; say which branch and what the
    # file does have, because a cart written for other files lands here first
    if spec not in chunk.fields:
        # AssertionError, not KeyError, whose repr escapes the newlines out of what is
        # three lines of the most useful text in the tool
        raise AssertionError(
            f"{where} reads branch {spec!r}, which {meta.get('file') or 'this file'} does "
            f"not have.\n  It has {len(chunk.fields)} branches; the closest names are "
            f"{_closest(spec, chunk.fields)}.\n  Edit your study's cart.py.")
    return chunk[spec]


def evaluate_columns(cart, chunk: ak.Array, meta: dict) -> dict[str, np.ndarray]:
    """Evaluate every column of the cart on one chunk, weights included."""
    out = {}
    for name, spec in cart.columns.items():
        values = value_of(spec, chunk, meta, f"columns[{name!r}]")
        out[name] = one_per_event(name, reject_masked(name, ak.to_numpy(values)))
    if cart.weights_column:
        out[cart.weights_column] = weights_for(cart, chunk, meta)

    lengths = {len(v) for v in out.values()}
    assert len(lengths) == 1, f"columns disagree on length: {sorted(lengths)}"
    return out


def one_per_event(column: str, values: np.ndarray) -> np.ndarray:
    """Refuse a column holding more than one number per event."""
    assert values.ndim == 1, (
        f"column {column} has shape {values.shape}: {values.shape[1:]} numbers per event, "
        f"not one. Reduce it — ak.max(..., axis=1), [:, 0] — before storing it.")
    return values


def weights_for(cart, chunk: ak.Array, meta: dict) -> np.ndarray:
    """This sample's weights: its branch, its function, or ones where it declares none."""
    spec = cart.weights_spec(meta["sample"])
    if spec is None:
        return np.ones(len(chunk), np.float64)
    values = value_of(spec, chunk, meta, f"{meta['sample']}: metadata['weights']")
    return reject_masked(cart.weights_column, ak.to_numpy(values)).astype(np.float64)


def skim(cart, sample: str, step: int = STEP, threads: int = 0,
         label: str = "") -> dict[str, np.ndarray]:
    """
    Read one sample, keeping only the events that pass the cart's keep.

    A sample split over 32 files is one call. Read in chunks of `step` events so memory
    stays flat however big the file is, and `virtual=True` means only the branches the
    columns actually touch come off disk (lazy, no wasted loading).

    `threads` above 0 hands uproot a thread pool for basket decompression, which it
    otherwise does on the calling thread — its default is a TrivialExecutor. Decompression
    is C and drops the GIL, so this is real parallelism inside one sample.

    `label` non-empty reports progress against this sample's own total. The trees are
    opened up front to know that total, which is the same one open per file the loop
    below would do anyway — it does not cost a remote read.
    """
    files = cart.files()[sample]
    assert files, f"{sample}: no files given"

    pool = uproot.ThreadPoolExecutor(threads) if threads else None
    trees = {path: uproot.open(path)[tree] for path, tree in files.items()}
    total = sum(t.num_entries for t in trees.values())
    began, seen = time.time(), 0

    per_chunk = []
    for path, ttree in trees.items():
        meta = cart.meta(sample, path, files[path])
        assert ttree.num_entries, f"{files[path]} in {path} is empty"
        for start in range(0, ttree.num_entries, step):
            lazy = {"virtual": True} if supports_virtual(ttree) else {}
            chunk = ttree.arrays(entry_start=start,
                                 entry_stop=min(start + step, ttree.num_entries),
                                 decompression_executor=pool, **lazy)
            if cart.prepare:
                chunk = call_hook(cart.prepare, chunk, meta)
            kept = chunk[call_hook(cart.keep, chunk, meta)]
            per_chunk.append(evaluate_columns(cart, kept, meta))
            seen = min(seen + step, total)
            if label:
                progress(label, seen, total, began)
    if label:
        progress(label, total, total, began, end=True)

    # stitch the chunks back together, one column at a time
    return {column_name: np.concatenate([chunk[column_name] for chunk in per_chunk])
            for column_name in per_chunk[0]}


def _skim_one_sample(job: tuple) -> tuple:
    """Skim one sample in a worker process, re-importing the cart there."""
    import cart as cart_module
    source, sample, step, threads = job
    cart = cart_module.import_file(Path(source), "rowver_worker_cart").CART
    return sample, skim(cart, sample, step, threads)


def skim_all(cart, name: str, step: int = STEP, dest: str | Path | None = None,
         jobs: int = 1, threads: int = 0) -> dict[str, dict[str, np.ndarray]]:
    """
    Skim every sample of the cart and write them all into <dest>/<name>.pkl.

    `jobs` above 1 skims samples in parallel processes and `threads` above 0 decompresses
    baskets in parallel within each one; they compose. The result is ordered as the cart's
    fileset is whichever you use, so the pikl does not depend on how it was built.
    """
    if jobs > 1 and cart.source:
        work = [(cart.source, sample, step, threads) for sample in cart.fileset]
        with ProcessPoolExecutor(max_workers=jobs) as pool:
            done = dict(pool.map(_skim_one_sample, work))
        columns_by_sample = {s: done[s] for s in cart.fileset}   # fileset order, not finish order
    else:
        # only the serial path reports progress: parallel workers cannot share one line,
        # and interleaved bars from N processes are worse than silence
        columns_by_sample = {}
        for i, sample in enumerate(cart.fileset, 1):
            columns_by_sample[sample] = skim(
                cart, sample, step, threads, label=f"[{i}/{len(cart.fileset)}] {sample}")

    # the "results" key is the shape load.py expects; keep the two in step. `config` and
    # `skimmed` ride with it so a later --no-skim build stamps the cart that actually
    # produced these columns, not whatever cart.py happens to say today
    where = Path(dest) if dest else PIKLS
    where.mkdir(parents=True, exist_ok=True)
    with gzip.open(where / f"{name}.pkl", "wb", compresslevel=1) as pickle_file:
        pickle.dump({"results": columns_by_sample,
                     "recipe": cart.recipe(),
                     "skimmed": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")},
                    pickle_file)
    return columns_by_sample


def verify_chunking(cart, sample: str) -> dict[str, np.ndarray]:
    """
    Skim one sample twice, in chunks and in a single pass, and require identical results.

    This is what catches a stitching bug in skim(). Getting the concatenation wrong
    still gives you the right NUMBER of events, so only comparing element by element,
    against a run whose chunk boundaries fall elsewhere, would notice.
    """
    chunked = skim(cart, sample)
    unchunked = skim(cart, sample, step=10 ** 9)      # bigger than any tree

    assert chunked.keys() == unchunked.keys(), "chunked and unchunked disagree on columns"
    for column_name in chunked:
        assert np.array_equal(chunked[column_name], unchunked[column_name]), \
            f"{column_name} differs between chunked and unchunked"
    return chunked
