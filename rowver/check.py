"""Try a recipe before pressing: `rowver check <study> [--sample NAME | --all] [--parallel [N]] [--json]`.

Loads cart.py as a press would and runs it on the first 1,000 events of one sample's first file:
the first sample, the one named, or with --all every sample (one file open each, so a study of
many remote samples takes a while). For each it reports the branches the recipe read, the events
read and kept, and each column's kind, or why the recipe or the file failed.
"""
import multiprocessing
import sys
import time
import traceback
from concurrent.futures import ProcessPoolExecutor, as_completed
from pathlib import Path

from . import terminal
from .probe import first, kind, row
from .run import PROBE, RecipeError, describe, load, stop, trial, tree_of, twice, worker_setup


def column(name, array, kept):
    """One column's kind, type and first values; a column with the wrong number of values is unsupported."""
    found, text = kind(array)
    entry = {"name": name, "kind": found, "type": text, "first": first(array) if found != "unsupported" else []}
    count = len(array) if hasattr(array, "__len__") else None
    if found != "unsupported" and count != kept:
        entry.update(kind="unsupported", note=f"{count:,} values for {kept:,} kept events")
    return entry


def one(cart, sample, index):
    """The check of one sample as one JSON-ready line: checked, or an error of kind input or recipe."""
    started = time.monotonic()
    line = {"index": index, "sample": sample}
    files = list(cart.fileset[sample]["files"].items())
    if not files:
        return {"event": "error", "kind": "input", **line, "message": "This sample has no files.", "traceback": ""}
    path, spec = files[0]
    line.update(path=path, tree=spec if isinstance(spec, str) else spec["tree"])
    stop = PROBE if isinstance(spec, str) else min(PROBE, spec["stop"])
    metadata = {**cart.fileset[sample]["metadata"], "file": path}
    tree, reason = twice(lambda: tree_of(path, spec))
    if tree is None:
        return {"event": "error", "kind": "input", **line, "message": reason, "traceback": ""}
    try:
        touched, read, kept, columns = trial(cart, tree, metadata, stop)
    except RecipeError as error:
        return {"event": "error", "kind": "recipe", **line, "message": str(error),
                "traceback": "".join(traceback.format_exception(error))}
    except Exception as error:
        return {"event": "error", "kind": "input", **line, "message": describe(error), "traceback": ""}
    return {"event": "checked", **line, "entries": int(tree.num_entries), "read": int(read), "kept": int(kept),
            "touched": touched, "columns": [column(name, array, kept) for name, array in columns.items()],
            "seconds": round(time.monotonic() - started, 3)}


def one_in_worker(study, sample, index):
    """one() inside a worker process, which loads the recipe for itself."""
    try:
        cart = load(study)
    except RecipeError as error:
        return {"event": "error", "kind": "recipe", "index": index, "sample": sample, "message": str(error),
                "traceback": "".join(traceback.format_exception(error))}
    return one(cart, sample, index)


def show(line):
    """One sample's check for people."""
    where = f"{line['sample']}: {Path(line['path']).name}" if "path" in line else line["sample"]
    print(where)
    if line["event"] == "error":
        what = "The recipe failed" if line["kind"] == "recipe" else "The file could not be read"
        terminal.write(f"  {what}: {line['message']}")
        print()
        return
    print(f"  Read {line['read']:,} events, kept {line['kept']:,} ({line['seconds']:.1f} s)")
    terminal.write(f"  Branches read: {', '.join(line['touched']) or 'none'}")
    if line["columns"]:
        name_width = max(len(entry["name"]) for entry in line["columns"])
        type_width = max(len(entry["type"]) for entry in line["columns"])
        for entry in line["columns"]:
            print(row(f"  {entry['name']:{name_width}}  {entry['kind']:11}  {entry['type']:{type_width}}  ",
                      entry.get("note") or ", ".join(map(str, entry["first"]))))
    print()


def main(study, sample=None, every=False, workers=1, json=False):
    """Check the chosen samples; exit 0 when every one was checked and 1 otherwise."""
    study = Path(study)
    saved = sys.stdout
    if json:
        sys.stdout = sys.stderr  # a recipe's print() must not land among the JSON lines
    say = terminal.emit if json else show
    try:
        try:
            cart = load(study)
        except RecipeError as error:
            trace = "".join(traceback.format_exception(error))
            if json:
                terminal.error("recipe", str(error), trace)
            else:
                print(trace, file=sys.stderr, end="")
            return 1
        names = list(cart.fileset)
        if sample is not None and sample not in names:
            message = f"The recipe has no sample named {sample}. Its samples: {', '.join(names) or 'none'}."
            if json:
                terminal.error("input", message, "")
            else:
                print(message, file=sys.stderr)
            return 1
        chosen = names if every else [sample] if sample is not None else names[:1]
        if not chosen:
            if json:
                terminal.error("input", "The recipe has no samples.", "")
            else:
                print("The recipe has no samples.", file=sys.stderr)
            return 1
        if json:
            terminal.emit({"event": "start", "study": study.name, "samples": chosen, "workers": workers})
        failures = 0
        if workers > 1 and len(chosen) > 1:
            pool = ProcessPoolExecutor(min(workers, len(chosen)), mp_context=multiprocessing.get_context("spawn"),
                                       initializer=worker_setup, initargs=(json,))
            try:
                futures = [pool.submit(one_in_worker, str(study), name, names.index(name)) for name in chosen]
                for future in as_completed(futures):
                    line = future.result()
                    failures += line["event"] == "error"
                    say(line)
            except BaseException:
                stop(pool)  # workers ignore SIGINT, so drop the queued samples and end them here
                raise
            pool.shutdown()
        else:
            for name in chosen:
                line = one(cart, name, names.index(name))
                failures += line["event"] == "error"
                say(line)
        return 1 if failures else 0
    except KeyboardInterrupt:
        if json:
            terminal.error("cancelled", "The check was cancelled.", "")
        else:
            print("\nThe check was cancelled.", file=sys.stderr)
        return 1
    finally:
        sys.stdout = saved
