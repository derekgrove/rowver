"""Turn a study's cart.py into one .rvr file beside it.

    rowver run <study directory> [--parallel [N]]
"""
import hashlib
import importlib.util
import inspect
import time
from concurrent.futures import ProcessPoolExecutor, as_completed
from datetime import datetime, timedelta, timezone
from pathlib import Path

import uproot

from . import rvr
from .terminal import Progress, write

PROBE = 1000
STEP = 100_000


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
    """Import <study>/cart.py and return its CART."""
    spec = importlib.util.spec_from_file_location("cart", Path(study) / "cart.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.CART


def call(hook, events, metadata):
    """Run a hook that takes events, or events and metadata."""
    return hook(events, metadata) if len(inspect.signature(hook).parameters) == 2 else hook(events)


def select(cart, events, metadata, wrap=lambda events: events):
    """Run prepare and keep on one chunk; return the survivors."""
    if cart.prepare is not None:
        events = wrap(call(cart.prepare, events, metadata))
    return events[call(cart.keep, events, metadata)]


def evaluate(cart, events, metadata):
    """The columns of one chunk of selected events."""
    return {name: events[spec] if isinstance(spec, str) else call(spec, events, metadata)
            for name, spec in cart.columns.items()}


def touched(cart, tree, metadata):
    """The branches the recipe reads, found by running it on the first events of a tree."""
    touched = set()
    wrap = lambda events: events if isinstance(events, Probe) else Probe(events, touched)
    events = wrap(tree.arrays(entry_stop=PROBE))
    evaluate(cart, select(cart, events, metadata, wrap), metadata)
    return sorted(touched & set(tree.keys()))


def read(cart, metadata, path, spec, show=lambda done, total, kept: None):
    """Skim one file, or its first `stop` entries: columns, file record, entries scanned, rows kept."""
    tree_name, stop = (spec, None) if isinstance(spec, str) else (spec["tree"], spec["stop"])
    tree = uproot.open(path)[tree_name]
    record = {"tree": tree_name, "uuid": str(tree.file.uuid), "entries": int(tree.num_entries)}
    if stop is not None:
        record["stop"] = stop
    total = tree.num_entries if stop is None else min(stop, tree.num_entries)
    columns, done, kept = {name: [] for name in cart.columns}, 0, 0
    for chunk in tree.iterate(touched(cart, tree, metadata), step_size=STEP, entry_stop=stop):
        events = select(cart, chunk, metadata)
        for name, value in evaluate(cart, events, metadata).items():
            columns[name].append(value)
        done, kept = done + len(chunk), kept + len(events)
        show(done, total, kept)
    return columns, {path: record}, done, kept


def read_in_worker(study, sample, path, spec):
    """read() inside a worker process, which loads the cart for itself."""
    cart = load(study)
    return sample, path, read(cart, {**cart.fileset[sample]["metadata"], "file": path}, path, spec)


def gather(cart, pieces):
    """Join each sample's columns and file records."""
    return {sample: {"columns": {name: rvr.join([p for c, _ in pieces[sample] for p in c[name]])
                                 for name in cart.columns},
                     "files": {path: record for _, files in pieces[sample] for path, record in files.items()}}
            for sample in cart.fileset}


def skim(cart, study, progress):
    """Read every file one after another with a live line per file."""
    out = {}
    for sample, entry in cart.fileset.items():
        pieces = []
        for path, spec in entry["files"].items():
            progress.status(path)

            def show(done, total, kept):
                """Redraw this file's line."""
                progress.status(path, done, total, kept)

            pieces.append(read(cart, {**entry["metadata"], "file": path}, path, spec, show))
            progress.finish(path, pieces[-1][2], pieces[-1][3])
        out[sample] = pieces
    return gather(cart, {s: [(c, files) for c, files, *_ in p] for s, p in out.items()})


def skim_parallel(cart, study, workers, progress):
    """Read every file across worker processes, one line per file as it finishes."""
    pieces = {sample: [] for sample in cart.fileset}
    jobs = [(sample, path, spec) for sample, entry in cart.fileset.items() for path, spec in entry["files"].items()]
    with ProcessPoolExecutor(workers) as pool:
        futures = [pool.submit(read_in_worker, str(study), *job) for job in jobs]
        for future in as_completed(futures):
            sample, path, (columns, files, done, kept) = future.result()
            pieces[sample].append((columns, files))
            progress.finish(path, done, kept)
    return gather(cart, pieces)


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


def main(study, workers):
    """Skim the study and write <study>/<name>.rvr."""
    study, start = Path(study), time.time()
    cart = load(study)
    total = sum(len(e['files']) for e in cart.fileset.values())
    write(study.name)
    write(f"Samples: {len(cart.fileset):,} | Files: {total:,} | Workers: {workers}")
    print()
    progress = Progress(total)
    try:
        samples = skim_parallel(cart, study, workers, progress) if workers > 1 else skim(cart, study, progress)
    finally:
        progress.clear()
    progress.samples(samples)
    data, buffer = rvr.pack(samples, cart.labels)
    header = {"rowver": rvr.VERSION, "data": data,
              "made": made(study.name, cart, (study / "cart.py").read_text(), samples)}
    target = study / f"{study.name}.rvr"
    rvr.write(target, header, buffer)
    print()
    write(f"Wrote {target}")
    write(f"{data['rows']:,} rows | {len(data['cols'])} columns | "
          f"{target.stat().st_size / 1e6:.1f} MB | {timedelta(seconds=round(time.time() - start))}")

