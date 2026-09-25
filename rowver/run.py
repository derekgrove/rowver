"""Turn a study's cart.py into one .rvr file beside it.

    rowver run <study directory> [--out PATH] [--parallel [N]] [--progress json]
"""
import hashlib
import importlib.util
import inspect
import multiprocessing
import os
import signal
import sys
import threading
import time
import traceback
from concurrent.futures import ProcessPoolExecutor, as_completed
from datetime import datetime, timezone
from pathlib import Path

import uproot

from . import rvr, terminal
from .terminal import JsonProgress, Progress

PROBE = 1000
STEP = 100_000
GRACE = 3.0  # seconds the main thread has to take a signal before the watchdog ends the press

# what the watchdog needs if the main thread cannot take a signal: the running pool, and whether it did
RUNNING = {"pool": None, "cancelled": False}


class RecipeError(Exception):
    """cart.py could not be loaded, or a hook or column failed on real events."""


class PressError(Exception):
    """The press stopped for a reason that is not a bug: kind is recipe, input or write."""

    def __init__(self, kind, message):
        super().__init__(message)
        self.kind = kind


class Cancelled(BaseException):
    """SIGTERM or SIGINT reached the presser."""


def describe(error):
    """One line naming an exception and its message."""
    return f"{type(error).__name__}: {error}"


def guarded(what, function, *args):
    """Run recipe code; its failure becomes a RecipeError that says which part failed."""
    try:
        return function(*args)
    except RecipeError:
        raise
    except Exception as error:
        raise RecipeError(f"{what}: {describe(error)}") from error


class Probe:
    """Stands in for the event array on a probe run and records the fields the recipe reads."""

    def __init__(self, events, touched):
        self.events, self.touched = events, touched

    def __getattr__(self, name):
        self.touched.add(name)
        return getattr(self.events, name)

    def __getitem__(self, where):
        if isinstance(where, str):
            self.touched.add(where)
            return self.events[where]
        return Probe(self.events[where], self.touched)

    def __setitem__(self, where, value):
        self.events[where] = value

    def __len__(self):
        return len(self.events)


def load(study):
    """Import <study>/cart.py and return its CART, leaving no __pycache__ in the study."""
    sys.dont_write_bytecode = True
    try:
        spec = importlib.util.spec_from_file_location("cart", Path(study) / "cart.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module.CART
    except Exception as error:
        raise RecipeError(f"cart.py: {describe(error)}") from error


def call(hook, events, metadata):
    """Run a hook that takes events, or events and metadata."""
    return hook(events, metadata) if len(inspect.signature(hook).parameters) == 2 else hook(events)


def select(cart, events, metadata, wrap=lambda events: events):
    """Run prepare and keep on one chunk; return the survivors."""
    if cart.prepare is not None:
        events = wrap(guarded("prepare", call, cart.prepare, events, metadata))
    return guarded("keep", lambda: events[call(cart.keep, events, metadata)])


def value(spec, events, metadata):
    """One column of one chunk: a branch or field by name, or a hook's result."""
    return events[spec] if isinstance(spec, str) else call(spec, events, metadata)


def evaluate(cart, events, metadata):
    """The columns of one chunk of selected events."""
    return {name: guarded(f"column {name}", value, spec, events, metadata) for name, spec in cart.columns.items()}


def touched(cart, tree, metadata):
    """The branches the recipe reads, found by running it on the first events of a tree."""
    touched = set()
    wrap = lambda events: events if isinstance(events, Probe) else Probe(events, touched)
    events = wrap(tree.arrays(entry_stop=PROBE))
    evaluate(cart, select(cart, events, metadata, wrap), metadata)
    return sorted(touched & set(tree.keys()))


def tree_of(path, spec):
    """Open one input and return its tree."""
    return uproot.open(path)[spec if isinstance(spec, str) else spec["tree"]]


def read(cart, metadata, path, spec, show=lambda done, total, kept: None):
    """Skim one file, or its first `stop` entries: columns, file record, entries scanned, rows kept."""
    tree_name, stop = (spec, None) if isinstance(spec, str) else (spec["tree"], spec["stop"])
    tree = tree_of(path, spec)
    record = {"tree": tree_name, "uuid": str(tree.file.uuid), "entries": int(tree.num_entries)}
    if stop is not None:
        record["stop"] = stop
    total = tree.num_entries if stop is None else min(stop, tree.num_entries)
    columns, done, kept = {name: [] for name in cart.columns}, 0, 0
    for chunk in tree.iterate(touched(cart, tree, metadata), step_size=STEP, entry_stop=stop):
        events = select(cart, chunk, metadata)
        for name, array in evaluate(cart, events, metadata).items():
            columns[name].append(array)
        done, kept = done + len(chunk), kept + len(events)
        show(done, total, kept)
    return columns, {path: record}, done, kept


def twice(action):
    """Run an input action, and once more if it fails: its result and None, or None and why it failed.

    A recipe error is not retried: it would fail the same way again.
    """
    for _ in range(2):
        try:
            return action(), None
        except RecipeError:
            raise
        except Exception as error:
            reason = describe(error)
    return None, reason


def attempt(cart, sample, path, spec, progress):
    """Read one file with one retry, reporting it done or failed: (columns, files, entries, kept) or None."""
    metadata = {**cart.fileset[sample]["metadata"], "file": path}

    def once():
        """Open and skim the file from the start."""
        progress.status(path)
        started = time.monotonic()
        result = read(cart, metadata, path, spec, lambda done, total, kept: progress.status(path, done, total, kept))
        return *result, time.monotonic() - started

    result, reason = twice(once)
    if result is None:
        progress.failed(path, reason)
        return None, reason
    progress.finish(path, *result[2:])
    return result[:4], None


def read_in_worker(study, sample, path, spec, place, json):
    """attempt() inside a worker process, which loads the cart for itself and writes its own JSON lines."""
    cart = load(study)
    progress = JsonProgress({path: place}) if json else Quiet()
    result, reason = attempt(cart, sample, path, spec, progress)
    return sample, path, result, reason


class Quiet:
    """A worker's progress when the parent prints the table."""

    def status(self, *args):
        """Say nothing."""

    finish = failed = status


def worker_setup(json):
    """In each worker: leave cancelling to the parent, write no bytecode, and keep stray prints off the JSON."""
    signal.signal(signal.SIGINT, signal.SIG_IGN)
    signal.signal(signal.SIGTERM, signal.SIG_DFL)
    sys.dont_write_bytecode = True
    if json:
        sys.stdout = sys.stderr


def probe(cart, jobs, progress):
    """Run the recipe on the first events of the first file that opens, so a broken hook stops the
    press before the pool starts; return {place: reason} for the files that could not be opened."""
    unreadable = {}
    for place, (sample, path, spec) in enumerate(jobs):
        metadata = {**cart.fileset[sample]["metadata"], "file": path}
        _, reason = twice(lambda: touched(cart, tree_of(path, spec), metadata))
        if reason is None:
            break
        unreadable[place] = reason
        progress.failed(path, reason)
    return unreadable


def skim(cart, jobs, progress, failed):
    """Read every file one after another; {sample: [(columns, files), ...]}."""
    pieces = {sample: [] for sample in cart.fileset}
    for sample, path, spec in jobs:
        result, reason = attempt(cart, sample, path, spec, progress)
        if result is None:
            failed.append({"sample": sample, "path": path, "message": reason})
        else:
            pieces[sample].append(result[:2])
    return pieces


def stop(pool):
    """Drop the queued files and end every worker now."""
    processes = list((pool._processes or {}).values())
    pool.shutdown(wait=False, cancel_futures=True)
    RUNNING["pool"] = None
    for process in processes:
        process.terminate()
    for process in processes:
        process.join()


def skim_parallel(cart, study, jobs, workers, progress, failed):
    """Read every file across worker processes; {sample: [(columns, files), ...]}."""
    pieces = {sample: [] for sample in cart.fileset}
    unreadable = probe(cart, jobs, progress)
    for place, reason in unreadable.items():
        failed.append({"sample": jobs[place][0], "path": jobs[place][1], "message": reason})
    pool = ProcessPoolExecutor(workers, mp_context=multiprocessing.get_context("spawn"),
                               initializer=worker_setup, initargs=(progress.json,))
    RUNNING["pool"] = pool
    try:
        futures = [pool.submit(read_in_worker, str(study), sample, path, spec, place, progress.json)
                   for place, (sample, path, spec) in enumerate(jobs) if place not in unreadable]
        for future in as_completed(futures):
            sample, path, result, reason = future.result()
            if result is None:
                failed.append({"sample": sample, "path": path, "message": reason})
                if not progress.json:
                    progress.failed(path, reason)
            else:
                pieces[sample].append(result[:2])
                if not progress.json:
                    progress.finish(path, *result[2:])
    except BaseException:
        stop(pool)
        raise
    pool.shutdown()
    RUNNING["pool"] = None
    return pieces


def gather(cart, pieces):
    """Join each sample's columns and file records, leaving out a sample none of whose files was read."""
    return {sample: {"columns": {name: rvr.join([p for c, _ in pieces[sample] for p in c[name]])
                                 for name in cart.columns},
                     "files": {path: record for _, files in pieces[sample] for path, record in files.items()}}
            for sample in cart.fileset if pieces[sample]}


def made(study, cart, source, samples):
    """The recipe snapshot, build details, and files actually processed."""
    section = {"study": study}
    if cart.title:
        section["title"] = cart.title
    if cart.note:
        section["note"] = cart.note
    section["built"] = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    section["by"] = f"rowver {rvr.VERSION}"
    section["cart"] = {"sha256": hashlib.sha256(source.encode()).hexdigest(), "source": source}
    section["files"] = {name: {"files": sample["files"]} for name, sample in samples.items()}
    section["sha256"] = rvr.made_checksum(section)
    return section


def press(study, workers, target, json, stage):
    """Load, skim, pack and write; stage["kind"] names what a failure now would be."""
    start = time.time()
    cart = load(study)
    jobs = [(sample, path, spec) for sample, entry in cart.fileset.items() for path, spec in entry["files"].items()]
    places = {}
    for place, (_, path, _) in enumerate(jobs):
        places.setdefault(path, place)
    progress = JsonProgress(places) if json else Progress(len(jobs))
    progress.begin(study.name, len(cart.fileset), len(jobs), workers, target, [job[:2] for job in jobs])
    stage["kind"], failed = "input", []
    try:
        pieces = (skim_parallel(cart, study, jobs, workers, progress, failed) if workers > 1
                  else skim(cart, jobs, progress, failed))
    finally:
        progress.clear()
    samples = gather(cart, pieces)
    if not samples:
        raise PressError("input", f"None of the {len(jobs):,} input files could be read.")
    progress.samples(samples)
    stage["kind"] = "recipe"  # packing fails only on column values the recipe made
    progress.pack(sum(len(next(iter(s["columns"].values()))) for s in samples.values()))
    data, buffer = rvr.pack(samples, cart.labels)
    header = {"rowver": rvr.VERSION, "data": data,
              "made": made(study.name, cart, (study / "cart.py").read_text(encoding="utf-8"), samples)}
    stage["kind"] = "write"
    target.parent.mkdir(parents=True, exist_ok=True)
    stage["writing"] = True
    rvr.write(target, header, buffer)
    stage["writing"] = False
    progress.wrote(target, data["rows"], len(data["cols"]), target.stat().st_size, time.time() - start,
                   failed, [sample for sample in cart.fileset if sample not in samples])


def discard(target):
    """Remove a half-written target; a folder or anything else found there is left alone."""
    try:
        if target.is_file():
            target.unlink()
    except OSError:
        pass


def cancel(signum, frame):
    """Turn SIGTERM or SIGINT into an exception, so the pool and a half-written file are cleaned up."""
    RUNNING["cancelled"] = True
    raise Cancelled(signal.Signals(signum).name)


def watch(signals, target, stage, json):
    """Python takes a signal only between bytecodes, so a main thread waiting inside an XRootD open
    would not stop for minutes. Every signal also lands on this thread's pipe at once: if the main
    thread has not taken it within GRACE seconds, end the workers and the press from here."""
    while os.read(signals, 1):
        time.sleep(GRACE)
        if RUNNING["cancelled"]:
            continue
        pool = RUNNING["pool"]
        for process in list((pool._processes or {}).values()) if pool else []:
            process.terminate()
        if stage["writing"]:
            discard(target)
        if json:
            terminal.error("cancelled", "The press was cancelled.", "")
        else:
            os.write(2, b"\nThe press was cancelled.\n")
        os._exit(1)
    os.close(signals)


def main(study, workers, out=None, json=False):
    """Press the study into out, or <study>/<name>.rvr; the exit status, 0 when the file was written."""
    study = Path(study)
    target = Path(out) if out else study / f"{study.name}.rvr"
    stage = {"kind": "recipe", "writing": False}
    saved = sys.stdout
    if json:
        sys.stdout = sys.stderr  # a recipe's print() must not land among the JSON lines
    previous = {signum: signal.signal(signum, cancel) for signum in (signal.SIGTERM, signal.SIGINT)}
    signals, wake = os.pipe()
    os.set_blocking(wake, False)
    woken = signal.set_wakeup_fd(wake, warn_on_full_buffer=False)
    threading.Thread(target=watch, args=(signals, target, stage, json), daemon=True).start()
    try:
        press(study, workers, target, json, stage)
        return 0
    except (Exception, Cancelled) as error:
        if stage["writing"]:
            discard(target)
        if isinstance(error, Cancelled):
            kind, message, trace = "cancelled", "The press was cancelled.", ""
        elif isinstance(error, PressError):
            kind, message, trace = error.kind, str(error), ""
        else:
            kind = "recipe" if isinstance(error, RecipeError) else stage["kind"]
            message = str(error) if isinstance(error, RecipeError) else describe(error)
            trace = "".join(traceback.format_exception(error))
        if json:
            terminal.error(kind, message, trace)
            return 1
        if not trace:
            print(f"\n{message}", file=sys.stderr)
            return 1
        raise
    finally:
        sys.stdout = saved
        signal.set_wakeup_fd(woken)
        os.close(wake)
        for signum, handler in previous.items():
            signal.signal(signum, handler)
