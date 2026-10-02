"""Turn a study's cart.py into one .rvr file beside it.

    rowver run <study directory> [--out PATH] [--parallel [N]] [--progress json] [--onto FILE [--plan]]
"""
import hashlib
import importlib.util
import inspect
import multiprocessing
import os
import signal
import struct
import sys
import threading
import time
import traceback
from concurrent.futures import ProcessPoolExecutor, as_completed
from datetime import datetime, timezone
from pathlib import Path

import numpy as np
import uproot

from . import extend, rvr, terminal
from .terminal import JsonProgress, Progress

PROBE = 1000
ENTRY = "__rowver_entry__"  # each event's entry number, carried through prepare and keep
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


def modules(study):
    """The recipe's own modules, taken right after it loads: every imported .py file inside the
    study folder but cart.py and anything installed there (under a site-packages, or a Python
    prefix inside the folder), as {path relative to the folder: its text as Python decodes it},
    in path order."""
    home = Path(study).resolve()
    prefixes = [p for p in {Path(sys.prefix).resolve(), Path(sys.base_prefix).resolve()} if p.is_relative_to(home)]
    found = {}
    for module in list(sys.modules.values()):
        file = getattr(module, "__file__", None)
        path = Path(file).resolve() if file else None
        if (path and path.suffix == ".py" and path.is_relative_to(home) and path != home / "cart.py"
                and "site-packages" not in path.parts and not any(path.is_relative_to(p) for p in prefixes)):
            found[path.relative_to(home).as_posix()] = importlib.util.decode_source(path.read_bytes())
    return dict(sorted(found.items()))


def call(hook, events, metadata):
    """Run a hook that takes events, or events and metadata."""
    return hook(events, metadata) if len(inspect.signature(hook).parameters) == 2 else hook(events)


def select(cart, events, metadata, first=0, wrap=lambda events: events):
    """Run prepare and keep on one chunk whose first entry is `first`; return the survivors and each
    survivor's entry number. The number rides along as a field, so whatever prepare and keep do
    (select, reorder), every row keeps the entry it came from."""
    events[ENTRY] = np.arange(first, first + len(events), dtype=np.int64)
    if cart.prepare is not None:
        events = wrap(guarded("prepare", call, cart.prepare, events, metadata))
    kept = guarded("keep", lambda: events[call(cart.keep, events, metadata)])
    try:
        return kept, np.asarray(kept[ENTRY], dtype=np.int64)
    except Exception as error:
        raise RecipeError("prepare or keep returned events without the fields they were given "
                          f"({describe(error)}); add fields to the events, or select from them") from error


def value(spec, events, metadata):
    """One column of one chunk: a branch or field by name, or a hook's result."""
    return events[spec] if isinstance(spec, str) else call(spec, events, metadata)


def evaluate(cart, events, metadata):
    """The columns of one chunk of selected events."""
    return {name: guarded(f"column {name}", value, spec, events, metadata) for name, spec in cart.columns.items()}


def trial(cart, tree, metadata, stop=PROBE):
    """Run the recipe on the first events of a tree: the branches it read, the events read and kept,
    and the columns it made from them."""
    touched = set()
    wrap = lambda events: events if isinstance(events, Probe) else Probe(events, touched)
    events = wrap(tree.arrays(entry_stop=stop))
    kept, _ = select(cart, events, metadata, wrap=wrap)
    columns = evaluate(cart, kept, metadata)
    return sorted(touched & set(tree.keys())), len(events), len(kept), columns


def touched(cart, tree, metadata):
    """The branches the recipe reads, found by running it on the first events of a tree."""
    return trial(cart, tree, metadata)[0]


def tree_of(path, spec):
    """Open one input and return its tree."""
    return uproot.open(path)[spec if isinstance(spec, str) else spec["tree"]]


def read(cart, metadata, path, spec, show=lambda done, total, kept: None):
    """Skim one file, or its first `stop` entries: columns, file record, the entry number of each kept
    row, entries scanned, rows kept."""
    tree_name, stop = (spec, None) if isinstance(spec, str) else (spec["tree"], spec["stop"])
    tree = tree_of(path, spec)
    record = {"tree": tree_name, "uuid": str(tree.file.uuid), "entries": int(tree.num_entries)}
    if stop is not None:
        record["stop"] = stop
    total = tree.num_entries if stop is None else min(stop, tree.num_entries)
    columns, entries, done, kept = {name: [] for name in cart.columns}, [], 0, 0
    for chunk in tree.iterate(touched(cart, tree, metadata), step_size=STEP, entry_stop=stop):
        events, numbers = select(cart, chunk, metadata, done)
        entries.append(numbers)
        for name, array in evaluate(cart, events, metadata).items():
            columns[name].append(array)
        done, kept = done + len(chunk), kept + len(events)
        show(done, total, kept)
    entries = np.concatenate(entries) if entries else np.empty(0, np.int64)
    return columns, {path: record}, entries, done, kept


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
    """Read one file with one retry, reporting it done or failed: (columns, files, entry numbers,
    entries scanned, rows kept) or None."""
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
    progress.finish(path, *result[3:])
    return result[:5], None


def only(cart, names):
    """The cart making just the columns named, or all of them when none is named."""
    if names is not None:
        cart.columns = {name: cart.columns[name] for name in names}
    return cart


def read_in_worker(study, sample, path, spec, place, json, names=None):
    """attempt() inside a worker process, which loads the cart for itself and writes its own JSON lines."""
    cart = only(load(study), names)
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
            pieces[sample].append(result[:3])
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


def skim_parallel(cart, study, jobs, workers, progress, failed, names=None):
    """Read every file across worker processes; {sample: [(columns, files), ...]} in fileset order,
    whichever file finished first, so the rows come out as a sequential press lays them."""
    pieces = {sample: [] for sample in cart.fileset}
    unreadable = probe(cart, jobs, progress)
    for place, reason in unreadable.items():
        failed.append({"sample": jobs[place][0], "path": jobs[place][1], "message": reason})
    pool = ProcessPoolExecutor(workers, mp_context=multiprocessing.get_context("spawn"),
                               initializer=worker_setup, initargs=(progress.json,))
    RUNNING["pool"] = pool
    finished = {}
    try:
        futures = {pool.submit(read_in_worker, str(study), sample, path, spec, place, progress.json, names): place
                   for place, (sample, path, spec) in enumerate(jobs) if place not in unreadable}
        for future in as_completed(futures):
            sample, path, result, reason = future.result()
            if result is None:
                failed.append({"sample": sample, "path": path, "message": reason})
                if not progress.json:
                    progress.failed(path, reason)
            else:
                finished[futures[future]] = (sample, result[:3])
                if not progress.json:
                    progress.finish(path, *result[3:])
    except BaseException:
        stop(pool)
        raise
    pool.shutdown()
    RUNNING["pool"] = None
    for place in sorted(finished):
        sample, piece = finished[place]
        pieces[sample].append(piece)
    return pieces


def gather(cart, pieces):
    """Join each sample's columns, entry numbers and file records, and list its files in row order with
    the rows each gave, leaving out a sample none of whose files was read."""
    return {sample: {"columns": {name: rvr.join([p for c, _, _ in pieces[sample] for p in c[name]])
                                 for name in cart.columns},
                     "entry": np.concatenate([e for _, _, e in pieces[sample]]),
                     "files": {path: record for _, files, _ in pieces[sample] for path, record in files.items()},
                     "inputs": [{"sample": sample, "path": path, "rows": len(e)}
                                for _, files, e in pieces[sample] for path in files]}
            for sample in cart.fileset if pieces[sample]}


def made(study, cart, source, files, imported, hooks):
    """The recipe snapshot (cart.py and the modules it imports from its folder, and the mark of each
    hook and column), build details, and files actually processed."""
    section = {"study": study}
    if cart.title:
        section["title"] = cart.title
    if cart.note:
        section["note"] = cart.note
    section["built"] = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    section["by"] = f"rowver {rvr.VERSION}"
    section["cart"] = {"sha256": hashlib.sha256(source.encode()).hexdigest(), "source": source}
    if hooks:
        section["cart"]["hooks"] = hooks
    if imported:
        section["cart"]["modules"] = {path: {"sha256": hashlib.sha256(text.encode()).hexdigest(), "source": text}
                                      for path, text in imported.items()}
    section["files"] = files
    section["sha256"] = rvr.made_checksum(section)
    return section


def press(study, workers, target, json, stage, onto=None, plan=False):
    """Load, skim, pack and write; stage["kind"] names what a failure now would be. With onto, a
    cartridge already pressed, only the columns it lacks are made, and added to it."""
    start = time.time()
    cart = load(study)
    # the recipe as it loaded, before any events are read: edits during a long press are not recorded
    source, imported, hooks = (study / "cart.py").read_text(encoding="utf-8"), modules(study), extend.marks(cart)
    jobs = [(sample, path, spec) for sample, entry in cart.fileset.items() for path, spec in entry["files"].items()]
    order, labels, names, skipped = list(cart.columns), cart.labels, None, []
    if onto:
        try:
            held, section = rvr.read(onto)
        except (OSError, ValueError, KeyError, struct.error) as error:
            raise PressError("input", f"The cartridge to add to could not be read: {describe(error)}") from error
        names, jobs, skipped = differs(extend.plan, cart, held)
        if plan:
            said = {"event": "plan", "columns": names, "files": len(jobs), "skipped": [list(s) for s in skipped]}
            terminal.emit(said) if json else print(f"Would add {', '.join(names)}, reading {len(jobs):,} files.",
                                                   file=sys.__stdout__)
            return
        only(cart, names)
        if "hooks" not in held["made"]["cart"]:
            hooks = None  # its columns were never marked, and are not vouched for now
    places = {}
    for place, (_, path, _) in enumerate(jobs):
        places.setdefault(path, place)
    progress = JsonProgress(places) if json else Progress(len(jobs))
    progress.begin(study.name, len({job[0] for job in jobs}) if onto else len(cart.fileset), len(jobs), workers,
                   target, [job[:2] for job in jobs])
    stage["kind"], failed = "input", []
    try:
        pieces = (skim_parallel(cart, study, jobs, workers, progress, failed, names) if workers > 1
                  else skim(cart, jobs, progress, failed))
    finally:
        progress.clear()
    if onto and failed:
        raise PressError("input", f"{failed[0]['path']} could not be read ({failed[0]['message']}), so nothing "
                                  "was added." if len(failed) == 1 else
                                  f"{len(failed):,} files could not be read, so nothing was added.")
    if onto:
        differs(extend.proven, held, section, pieces)
    samples = gather(cart, pieces)
    if not samples:
        raise PressError("input", f"None of the {len(jobs):,} input files could be read.")
    progress.samples(samples)
    stage["kind"] = "recipe"  # packing fails only on column values the recipe made
    progress.pack(sum(len(s["entry"]) for s in samples.values()))
    data, arrays = rvr.pack(samples, labels, [i for s in samples.values() for i in s["inputs"]])
    files = {name: {"files": sample["files"]} for name, sample in samples.items()}
    header = {"rowver": rvr.VERSION}
    if onto:
        data, arrays = extend.merge(held, section, order, labels, data, arrays)
        header, files = dict(held), held["made"]["files"]
        failed = [{"sample": sample, "path": path,
                   "message": "Not in the cartridge, so not read. Press again to read it."}
                  for sample, path in skipped]
    cart.columns = dict.fromkeys(order)  # made names no column, only the title and the note
    header.update(data=data, made=made(study.name, cart, source, files, imported, hooks))
    stage["kind"] = "write"
    target.parent.mkdir(parents=True, exist_ok=True)
    # an add may be written over the cartridge it adds to: the whole file first, beside it, then in its place
    stage["path"] = target.with_name(target.name + ".adding") if onto else target
    stage["writing"] = True
    rvr.write(stage["path"], header, arrays)
    os.replace(stage["path"], target)
    stage["writing"] = False
    progress.wrote(target, data["rows"], len(data["cols"]), target.stat().st_size, time.time() - start,
                   failed, [] if onto else [sample for sample in cart.fileset if sample not in samples],
                   names)


def differs(function, *args):
    """Run a step of adding to a cartridge; its refusal is a press that stops, of the kind differs, or
    nothing when the recipe has no new column."""
    try:
        return function(*args)
    except extend.Differs as error:
        raise PressError("differs", str(error)) from error
    except extend.Nothing as error:
        raise PressError("nothing", str(error)) from error


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
            discard(stage["path"])
        if json:
            terminal.error("cancelled", "The press was cancelled.", "")
        else:
            os.write(2, b"\nThe press was cancelled.\n")
        os._exit(1)
    os.close(signals)


def main(study, workers, out=None, json=False, onto=None, plan=False):
    """Press the study into out, or <study>/<name>.rvr; the exit status, 0 when the file was written.
    With onto, add the columns that cartridge lacks instead of making every column; with plan too,
    only say which columns those are, reading no file and writing nothing."""
    study = Path(study)
    target = Path(out) if out else study / f"{study.name}.rvr"
    stage = {"kind": "recipe", "writing": False, "path": target}
    saved = sys.stdout
    if json:
        sys.stdout = sys.stderr  # a recipe's print() must not land among the JSON lines
    previous = {signum: signal.signal(signum, cancel) for signum in (signal.SIGTERM, signal.SIGINT)}
    signals, wake = os.pipe()
    os.set_blocking(wake, False)
    woken = signal.set_wakeup_fd(wake, warn_on_full_buffer=False)
    threading.Thread(target=watch, args=(signals, target, stage, json), daemon=True).start()
    try:
        press(study, workers, target, json, stage, Path(onto) if onto else None, plan)
        return 0
    except (Exception, Cancelled) as error:
        if stage["writing"]:
            discard(stage["path"])
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
