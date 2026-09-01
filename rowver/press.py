"""The PRESS: the library that turns a study's cart.py recipe into a .rvr.

Study recipes are and remain ``studies/<name>/cart.py`` — this module was
itself called cart.py once, which confused the press with the recipes it
presses, hence the rename (owner decision, 2026-08-28).

run_rowver.py exports one ``CART`` (.rvr file) from a study.

a cart.py is intended to be a basic recipe to build your .rvr file, its a:
1) fileset (remote or local paths to .root files)
2)

The fileset follows Coffea's familiar shape:

    {"sample": {"files": {"file.root": "Events"},
                "metadata": {"weights": "genWeight"}}}

Every sample must explicitly declare ``metadata['weights']``.
Use a branch name or callable for event weights, or ``None`` for an unweighted
sample. Not declaring a weights expression is refused, because forgotten weights
and intentionally unweighted simulation are not the same thing.
"""
from __future__ import annotations

import glob
import hashlib
import re
import importlib.util
import inspect
import json
import multiprocessing
import os
import signal
import threading
from datetime import datetime, timezone
from multiprocessing.connection import wait
from pathlib import Path
from typing import Any, Callable, Mapping
from urllib.parse import urlsplit, urlunsplit

import awkward as ak
import numpy as np
import uproot

from . import rvr6

STUDIES = Path("studies")  # resolved against the caller's cwd, not the package
RESERVED_META = frozenset({"sample", "file", "tree"})
WEIGHT_MISSPELLINGS = frozenset({
    "weight", "Weights", "WEIGHTS", "weigths", "WEIGHTS_BRANCH", "HAS_WEIGHTS",
})


class CartError(ValueError):
    """A study is ambiguous, unsafe to build, or produces invalid columns."""


def study_names(root: Path = STUDIES) -> list[str]:
    """Return study directories that contain a cart.py."""
    return sorted(path.parent.name for path in root.glob("*/cart.py"))


def _remote(path: str) -> bool:
    return "://" in path


def _open_tree(path: str, tree: str):
    """Open one selected tree with the press's source-specific policy."""
    options = ({"handler": uproot.source.xrootd.XRootDSource}
               if path.startswith("root://") else {})
    return uproot.open({path: tree}, **options)


def _redacted_source(path: str) -> str:
    """Keep a useful URL path while dropping userinfo, query, and fragment secrets."""
    if not _remote(path):
        return path
    parsed = urlsplit(path)
    safe_netloc = parsed.netloc.rsplit("@", 1)[-1]
    return urlunsplit((parsed.scheme, safe_netloc, parsed.path, "", ""))


def _redacted_error(exc: BaseException, path: str) -> str:
    """Remove URL credentials if a backend echoes them in its error."""
    text = str(exc)
    if not _remote(path):
        return text
    parsed = urlsplit(path)
    redacted = _redacted_source(path)
    without_query = urlunsplit((parsed.scheme, parsed.netloc, parsed.path, "", ""))
    for echoed in (path, path.split("#", 1)[0], without_query):
        text = text.replace(echoed, redacted)
    safe_netloc = parsed.netloc.rsplit("@", 1)[-1]
    if parsed.netloc:
        text = text.replace(parsed.netloc, safe_netloc)
    for parameter in parsed.query.split("&"):
        if parameter:
            key = parameter.split("=", 1)[0]
            text = text.replace(parameter, f"{key}=<redacted>")
    if parsed.fragment:
        text = text.replace(f"#{parsed.fragment}", "#<redacted>")
    return text


def _source_failure(sample: str, path: str, exc: Exception) -> CartError:
    return CartError(
        f"cannot open sample {sample!r} file {_redacted_source(path)!r}: "
        f"{_redacted_error(exc, path)}"
    )


def _study_hash(study: Path) -> str:
    """Fingerprint executable Python owned by one Study directory."""
    digest = hashlib.sha256()
    for source in sorted(study.rglob("*.py")):
        try:
            contents = source.read_bytes()
        except OSError as exc:
            raise CartError(f"cannot read Study recipe {source}: {exc}") from exc
        digest.update(str(source.relative_to(study)).encode())
        digest.update(b"\0")
        digest.update(contents)
        digest.update(b"\0")
    return digest.hexdigest()


def _raise_parallel_error(kind: str, text: str) -> None:
    if kind == "interrupt":
        raise KeyboardInterrupt(text)
    if kind == "exit":
        raise SystemExit(text)
    raise CartError(text)


def _parallel_worker(study: str, expected_hash: str, step_size: int | str,
                     tasks, messages, parent_watch) -> None:
    """Reload one Study and return source results over an ordinary process pipe."""
    signal.signal(signal.SIGINT, signal.SIG_IGN)

    def stop_with_parent() -> None:
        try:
            parent_watch.recv_bytes()
        except (EOFError, OSError):
            os._exit(1)

    threading.Thread(target=stop_with_parent, daemon=True).start()
    current_path: str | None = None
    try:
        if _study_hash(Path(study)) != expected_hash:
            raise CartError("study recipe changed before parallel workers started")
        cart = Cart.load(study)
        full_fileset = cart.fileset
        messages.send(("ready",))
        while True:
            task = tasks.get()
            if task is None:
                break
            sample_at, source_at, sample, path, tree = task
            current_path = path
            if _study_hash(Path(study)) != expected_hash:
                raise CartError("study recipe changed after parallel tasks were resolved")
            samples = list(full_fileset)
            sources = list(full_fileset.get(sample, {}).get("files", {}).items())
            if (sample_at >= len(samples) or samples[sample_at] != sample
                    or source_at >= len(sources) or sources[source_at] != (path, tree)):
                raise CartError("study fileset changed after parallel tasks were resolved")

            scanned = 0

            class Progress:
                def source(self, name: str, source: str, tree_name: str) -> None:
                    messages.send(("source", name, source, tree_name))

                def __call__(self, name: str | None, raw: int, kept: int) -> None:
                    nonlocal scanned
                    scanned += raw
                    messages.send(("chunk", name, raw, kept))

                def _size(self, name: str, kept: int, wide: dict[str, bool]) -> None:
                    messages.send(("size", name, kept, wide))

            entry = full_fileset[sample]
            cart.fileset = {sample: {
                "files": {path: tree},
                "metadata": entry["metadata"],
            }}
            try:
                rows, weights = cart.skim(step_size, Progress())
            finally:
                cart.fileset = full_fileset
            messages.send(("result", sample_at, source_at, sample, scanned,
                           rows[sample], weights.get(sample)))
            current_path = None
        messages.send(("done",))
    except BaseException as exc:
        text = _redacted_error(exc, current_path) if current_path else str(exc)
        if not isinstance(exc, CartError):
            text = f"{type(exc).__name__}: {text}"
        kind = ("interrupt" if isinstance(exc, KeyboardInterrupt) else
                "exit" if isinstance(exc, SystemExit) else "failure")
        try:
            messages.send(("error", kind, text))
        except Exception:
            pass
    finally:
        messages.close()


def _open_sample_tree(sample: str, path: str, tree: str):
    """Open a tree and translate source failures consistently for users."""
    try:
        return _open_tree(path, tree)
    except Exception as exc:
        raise _source_failure(sample, path, exc) from exc


def _call_shape(function: Callable[..., Any], where: str) -> int:
    try:
        signature = inspect.signature(function)
    except (TypeError, ValueError) as exc:
        raise CartError(f"{where} must have an inspectable one- or two-argument signature") from exc
    positional = [p for p in signature.parameters.values()
                  if p.kind in (p.POSITIONAL_ONLY, p.POSITIONAL_OR_KEYWORD)]
    required = [p for p in positional if p.default is p.empty]
    variadic = any(p.kind is p.VAR_POSITIONAL for p in signature.parameters.values())
    if variadic or not 1 <= len(positional) <= 2 or len(required) > 2:
        raise CartError(f"{where} must accept events, or events and metadata")
    return len(positional)


REMOTE = ("root://", "http://", "https://")


def _byte_count(text: str) -> int:
    """'100 MB' -> 100_000_000. Matches the grammar uproot step-size strings use,
    scientific notation included."""
    scale = {"b": 1, "kb": 10**3, "mb": 10**6, "gb": 10**9, "tb": 10**12,
             "kib": 2**10, "mib": 2**20, "gib": 2**30, "tib": 2**40}
    match = re.fullmatch(r"\s*([0-9.]+(?:[eE][+-]?[0-9]+)?)\s*([A-Za-z]+)\s*", text)
    if not match or match.group(2).lower() not in scale:
        raise CartError(f'cannot read step size "{text}"')
    return int(float(match.group(1)) * scale[match.group(2).lower()])


def _invoke(function: Callable[..., Any], events: ak.Array, meta: dict, where: str) -> Any:
    return function(events, meta) if _call_shape(function, where) == 2 else function(events)


def _source(value: Any) -> dict | None:
    """Describe a branch or callable as data; never turn prose back into executable code."""
    if value is None:
        return None
    if isinstance(value, str):
        return {"kind": "branch", "value": value}
    try:
        source = inspect.getsource(value).strip()
    except (OSError, TypeError) as exc:
        raise CartError(f"callable {value!r} has no recoverable Python source") from exc
    return {"kind": "python", "name": getattr(value, "__name__", type(value).__name__),
            "doc": inspect.getdoc(value) or "", "source": source}


def _jsonable(value: Any, where: str) -> Any:
    try:
        json.dumps(value, allow_nan=False)
    except (TypeError, ValueError) as exc:
        raise CartError(f"{where} must be finite JSON data: {exc}") from exc
    return value


def _one_dimensional(value: Any, where: str, *, boolean: bool = False) -> np.ndarray:
    """Convert an Awkward result without accepting missing or jagged values."""
    try:
        out = ak.to_numpy(value, allow_missing=True)
    except (TypeError, ValueError) as exc:
        raise CartError(f"{where} must contain exactly one regular value per event") from exc
    if np.ma.isMaskedArray(out) and np.ma.getmaskarray(out).any():
        raise CartError(f"{where} contains missing values")
    out = np.asarray(out)
    if out.ndim != 1:
        raise CartError(f"{where} must contain exactly one value per event, not shape {out.shape}")
    if boolean:
        if out.dtype.kind != "b":
            raise CartError(f"{where} must be boolean")
        return out
    if out.dtype.kind not in "biuf":
        raise CartError(f"{where} must be numeric")
    if out.dtype.kind == "f" and not np.isfinite(out).all():
        raise CartError(f"{where} contains NaN or infinity")
    return out


class Cart:
    """A ROOT fileset, one preselection, and the columns exposed in RowVer."""

    def __init__(self, *, fileset: Mapping[str, Mapping[str, Any]],
                 columns: Mapping[str, str | Callable[..., Any]],
                 keep: Callable[..., Any], prepare: Callable[..., Any] | None = None,
                 labels: Mapping[str, Mapping[Any, str]] | None = None,
                 title: str | None = None, note: str | None = None):
        self.fileset = {name: dict(entry) for name, entry in fileset.items()}
        self.columns = dict(columns)
        self.keep = keep
        self.prepare = prepare
        self.labels = dict(labels or {})
        self.title = title
        self.note = note
        self.name: str | None = None
        self.dir: Path | None = None
        self.source: Path | None = None
        self._recipe_hash: str | None = None
        self._validated = False

    @classmethod
    def load(cls, study: str | Path, root: Path = STUDIES) -> "Cart":
        """Load a study's ``cart.py``: STUDY is a name under ``root``, or any study directory path."""
        if not str(study):
            raise CartError("study must be a name under studies/ or a path to a study directory")
        named = (root / study / "cart.py").resolve() if Path(study).name == str(study) else None
        direct = (Path(study).expanduser() / "cart.py").resolve()
        source = named if named is not None and named.is_file() else direct
        if not source.is_file():
            expected = f"{named} or {direct}" if named is not None and named != direct else str(direct)
            raise CartError(f"no study {str(study)!r}: expected {expected}")
        recipe_hash = _study_hash(source.parent)
        spec = importlib.util.spec_from_file_location(f"rowver_study_{source.parent.name}", source)
        if spec is None or spec.loader is None:
            raise CartError(f"cannot import {source}")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        cart = getattr(module, "CART", None)
        if not isinstance(cart, cls):
            raise CartError(f"{source} must export CART = Cart(...)")
        cart.name, cart.dir, cart.source = source.parent.name, source.parent, source
        cart.fileset = cart._expand_files(source.parent)
        cart.validate()
        if _study_hash(source.parent) != recipe_hash:
            raise CartError("study recipe changed while it was being loaded")
        cart._recipe_hash = recipe_hash
        return cart

    def _expand_files(self, base: Path) -> dict[str, dict]:
        out: dict[str, dict] = {}
        for sample_name, raw in self.fileset.items():
            if not isinstance(raw, dict) or set(raw) - {"files", "metadata"}:
                raise CartError(f'sample "{sample_name}" must contain only files and metadata')
            files, metadata = raw.get("files"), raw.get("metadata")
            if not isinstance(files, dict) or not isinstance(metadata, dict):
                raise CartError(f'sample "{sample_name}" needs files and metadata objects')
            expanded: dict[str, str] = {}
            for pattern, tree in files.items():
                if not isinstance(pattern, str) or not pattern or not isinstance(tree, str) or not tree:
                    raise CartError(f'sample "{sample_name}" file paths and trees must be strings')
                if _remote(pattern):
                    matches = [pattern]
                else:
                    candidate = Path(pattern)
                    if not candidate.is_absolute():
                        candidate = base / candidate
                    matches = sorted(glob.glob(str(candidate)))
                    if not matches and not glob.has_magic(str(candidate)):
                        matches = [str(candidate)]
                for match in matches:
                    expanded[str(Path(match).resolve()) if not _remote(match) else match] = tree
            out[sample_name] = {"files": expanded, "metadata": dict(metadata)}
        return out

    def validate(self) -> None:
        """Reject ambiguous weights and bad hooks before any ROOT file is opened."""
        if not self.fileset:
            raise CartError("FILESET is empty")
        if not self.columns:
            raise CartError("COLUMNS is empty")
        if any(not isinstance(name, str) or not name.strip() for name in self.fileset):
            raise CartError("sample names must be non-empty strings")
        if len(set(self.fileset)) != len(self.fileset):
            raise CartError("sample names must be unique")
        if any(not isinstance(name, str) or not name.strip() or name == "weight"
               for name in self.columns):
            raise CartError("column names must be non-empty and may not be 'weight'")
        if not callable(self.keep):
            raise CartError("KEEP must be callable")
        _call_shape(self.keep, "KEEP")
        if self.prepare is not None:
            if not callable(self.prepare):
                raise CartError("PREPARE must be callable or None")
            _call_shape(self.prepare, "PREPARE")
        for name, value in self.columns.items():
            if not (isinstance(value, str) and value) and not callable(value):
                raise CartError(f'COLUMNS["{name}"] must be a branch name or callable')
            if callable(value):
                _call_shape(value, f'COLUMNS["{name}"]')
                _source(value)
        for sample, entry in self.fileset.items():
            if not entry["files"]:
                raise CartError(f'sample "{sample}" matched no files')
            metadata = entry["metadata"]
            collision = RESERVED_META & set(metadata)
            if collision:
                raise CartError(f'sample "{sample}" metadata uses reserved keys {sorted(collision)}')
            near = WEIGHT_MISSPELLINGS & set(metadata)
            if near:
                raise CartError(f'sample "{sample}" misspells weights as {sorted(near)}')
            if "weights" not in metadata:
                raise CartError(
                    f'sample "{sample}" must explicitly set metadata["weights"] to a branch, '
                    "a callable, or None for an unweighted sample")
            weight = metadata["weights"]
            if not (weight is None or isinstance(weight, str) and weight or callable(weight)):
                raise CartError(f'sample "{sample}" has an invalid weights declaration')
            if callable(weight):
                _call_shape(weight, f'{sample} weights')
                _source(weight)
            _jsonable({k: v for k, v in metadata.items() if k != "weights"},
                      f'sample "{sample}" metadata')
        for column, mapping in self.labels.items():
            if column not in self.columns or not isinstance(mapping, Mapping):
                raise CartError(f'LABELS["{column}"] must name a real column')
        if self.title is not None and not isinstance(self.title, str):
            raise CartError("title must be text")
        if self.note is not None and not isinstance(self.note, str):
            raise CartError("note must be text")
        self._validated = True

    def _meta(self, sample: str, path: str, tree: str) -> dict:
        return {**self.fileset[sample]["metadata"],
                "sample": sample, "file": path, "tree": tree}

    @staticmethod
    def _evaluate(spec: str | Callable[..., Any], events: ak.Array,
                  meta: dict, where: str) -> Any:
        return events[spec] if isinstance(spec, str) else _invoke(spec, events, meta, where)

    def skim(self, step_size: int | str = "100 MB",
             progress: Callable[[str | None, int, int], None] | None = None) -> tuple[dict, dict]:
        """Read files sequentially, apply hooks, and materialise validated NumPy columns.

        ``progress``, when given, is called after each fully processed chunk with
        ``(sample, raw_rows, kept_rows)`` — raw is the tree entries the chunk covered
        (pre-PREPARE, the same currency as ``num_entries``), kept is what survived KEEP.
        A callback object may additionally define ``source(sample, path, tree)`` and
        ``sample_done(sample)`` lifecycle methods; plain callables remain supported.
        ``build`` additionally calls it once with ``(None, 0, 0)`` when skimming ends
        and the pack/compress/write phase begins — the long quiet stretch a watcher
        would otherwise read as a hang. An exception from the callback aborts the
        build; nothing is written.
        """
        if not self._validated:
            self.validate()
        output: dict[str, dict[str, np.ndarray]] = {}
        output_weights: dict[str, np.ndarray] = {}
        # Refuse a doomed build EARLY: the wire format caps the column buffer, and a
        # press that will blow it should say so at the first offending chunk, not
        # after an hour of skimming (measured: 80 minutes, then the pack refused).
        # The estimate mirrors pack's own layout — 4-byte sample index per row, one
        # 8-byte weight slab when any sample is weighted, and per column 4 bytes
        # while its chunks stay integer-typed inside int32 (pack narrows those to
        # i4) or 8 once anything wider appears. A column can only WIDEN, so the
        # running total is a lower bound tightening toward pack's exact size:
        # a refusal here is certain, never premature.
        kept_rows = 0
        col_wide: dict[str, bool] = {name: False for name in self.columns}
        any_weighted = any(entry["metadata"]["weights"] is not None
                           for entry in self.fileset.values())
        i32 = np.iinfo(np.int32)
        for sample, entry in self.fileset.items():
            chunks: dict[str, list[np.ndarray]] = {name: [] for name in self.columns}
            weight_chunks: list[np.ndarray] = []
            for path, tree_name in entry["files"].items():
                metadata = self._meta(sample, path, tree_name)
                source_progress = getattr(progress, "source", None)
                if source_progress is not None:
                    source_progress(sample, path, tree_name)
                source = _open_sample_tree(sample, path, tree_name)
                with source as tree:
                    if isinstance(step_size, int):
                        entries_per_chunk = step_size
                    elif path.startswith(REMOTE):
                        # num_entries_for reads every basket header of every branch —
                        # thousands of round trips on a remote file (measured: minutes
                        # against EOS). One byte ratio gives the same order of
                        # magnitude for the cost of a single stat.
                        per_entry = (tree.file.source.num_bytes
                                     / max(int(tree.num_entries), 1))
                        entries_per_chunk = int(_byte_count(step_size)
                                                / max(per_entry, 1e-9))
                    elif hasattr(tree, "num_entries_for"):
                        entries_per_chunk = int(tree.num_entries_for(step_size))
                    else:
                        entries_per_chunk = 100_000
                    entries_per_chunk = max(entries_per_chunk, 1)
                    for start in range(0, int(tree.num_entries), entries_per_chunk):
                        stop = min(start + entries_per_chunk, int(tree.num_entries))
                        try:
                            # Uproot 5.7 virtual arrays materialise only branches a hook
                            # touches. This matters on thousand-branch NanoAOD files.
                            events = tree.arrays(entry_start=start, entry_stop=stop,
                                                 library="ak", virtual=True)
                        except TypeError:
                            # RNTuple implementations without virtual arrays still stay
                            # bounded to one chunk; correctness is unchanged.
                            events = tree.arrays(entry_start=start, entry_stop=stop, library="ak")
                        prepared = (_invoke(self.prepare, events, metadata, "PREPARE")
                                    if self.prepare is not None else events)
                        try:
                            prepared_rows = len(prepared)
                        except TypeError as exc:
                            raise CartError("PREPARE must return an event array") from exc
                        mask = _one_dimensional(
                            _invoke(self.keep, prepared, metadata, "KEEP"),
                            f'{sample} KEEP for {Path(path).name}', boolean=True)
                        if len(mask) != prepared_rows:
                            raise CartError(f'{sample} KEEP returned {len(mask)} values for '
                                            f'{prepared_rows} events')
                        selected = prepared[mask]
                        for name, spec in self.columns.items():
                            value = self._evaluate(spec, selected, metadata, f'COLUMNS["{name}"]')
                            column = _one_dimensional(value, f'{sample}.{name}')
                            if len(column) != len(selected):
                                raise CartError(f'{sample}.{name} returned {len(column)} values for '
                                                f'{len(selected)} selected events')
                            chunks[name].append(column)
                            if not col_wide[name] and (
                                    column.dtype.kind not in "biu" or
                                    (column.size and (column.min() < i32.min or
                                                      column.max() > i32.max))):
                                col_wide[name] = True
                        weight = entry["metadata"]["weights"]
                        if weight is not None:
                            values = _one_dimensional(
                                self._evaluate(weight, selected, metadata, f'{sample} weights'),
                                f'{sample}.weight').astype(np.float64, copy=False)
                            if len(values) != len(selected):
                                raise CartError(f'{sample}.weight length does not match its events')
                            if not np.isfinite(values).all():
                                raise CartError(f'{sample}.weight contains NaN or infinity')
                            weight_chunks.append(values)
                        kept_rows += len(selected)
                        row_bytes = (4 + (8 if any_weighted else 0)
                                     + sum(8 if wide else 4
                                           for wide in col_wide.values()))
                        if kept_rows * row_bytes > rvr6.MAX_DATA_BYTES:
                            raise CartError(
                                f"the kept rows no longer fit the cartridge: "
                                f"{kept_rows:,} rows x {row_bytes} bytes exceeds the "
                                f"{rvr6.MAX_DATA_BYTES}-byte column limit (reached in "
                                f'sample "{sample}") — tighten KEEP or trim the fileset')
                        size_progress = getattr(progress, "_size", None)
                        if size_progress is not None:
                            size_progress(sample, len(selected), dict(col_wide))
                        if progress is not None:
                            progress(sample, stop - start, len(selected))
            output[sample] = {
                name: np.concatenate(values) if values else np.empty(0, dtype=np.float64)
                for name, values in chunks.items()
            }
            if entry["metadata"]["weights"] is not None:
                output_weights[sample] = (np.concatenate(weight_chunks) if weight_chunks
                                          else np.empty(0, dtype=np.float64))
            sample_done = getattr(progress, "sample_done", None)
            if sample_done is not None:
                sample_done(sample)
        return output, output_weights

    def _parallel_skim(self, workers: int, step_size: int | str,
                       progress: Callable[[str | None, int, int], None] | None
                       ) -> tuple[dict, dict]:
        """Process one source per task; merge and all side effects stay here."""
        tasks = [
            (sample_at, source_at, sample, path, tree)
            for sample_at, (sample, entry) in enumerate(self.fileset.items())
            for source_at, (path, tree) in enumerate(entry["files"].items())
        ]
        workers = min(workers, len(tasks))
        context = multiprocessing.get_context("spawn")
        task_queue = context.SimpleQueue()
        processes = []
        connections = []
        lifelines = []
        expected_hash = self._recipe_hash
        if expected_hash is None:
            raise CartError("parallel builds require an unchanged loaded Study")
        succeeded = False
        try:
            for number in range(workers):
                receive, send = context.Pipe(duplex=False)
                parent_watch, lifeline = context.Pipe(duplex=False)
                process = context.Process(
                    target=_parallel_worker,
                    args=(str(self.dir), expected_hash, step_size, task_queue, send,
                          parent_watch),
                    name=f"rowver-source-{number + 1}",
                )
                try:
                    process.start()
                except BaseException:
                    receive.close()
                    send.close()
                    parent_watch.close()
                    lifeline.close()
                    raise
                processes.append(process)
                connections.append(receive)
                lifelines.append(lifeline)
                send.close()
                parent_watch.close()

            ready: set[Any] = set()
            while len(ready) < workers:
                available = wait(connections, timeout=0.1)
                for connection in available:
                    try:
                        message = connection.recv()
                    except EOFError as exc:
                        raise CartError("a parallel worker exited before it was ready") from exc
                    if message[0] == "ready":
                        ready.add(connection)
                    elif message[0] == "error":
                        _raise_parallel_error(*message[1:])
                    else:
                        raise CartError("a parallel worker sent an invalid startup message")
                for process in processes:
                    if process.exitcode not in (None, 0):
                        raise CartError(
                            f"a parallel worker exited unexpectedly (exit code {process.exitcode})"
                        )

            next_task = workers
            for task in tasks[:workers]:
                task_queue.put(task)

            results: dict[tuple[int, int], tuple[str, int, dict, np.ndarray | None]] = {}
            finished_sources = {sample: 0 for sample in self.fileset}
            active = set(connections)
            kept_rows = 0
            col_wide = {name: False for name in self.columns}
            any_weighted = any(entry["metadata"]["weights"] is not None
                               for entry in self.fileset.values())
            while active:
                available = wait(active, timeout=0.1)
                for connection in available:
                    try:
                        message = connection.recv()
                    except EOFError as exc:
                        process = processes[connections.index(connection)]
                        process.join(0.1)
                        if process.exitcode not in (None, 0):
                            raise CartError(
                                f"a parallel worker exited unexpectedly "
                                f"(exit code {process.exitcode})"
                            ) from exc
                        raise CartError("a parallel worker exited without completing") from exc
                    kind = message[0]
                    if kind == "source":
                        source_progress = getattr(progress, "source", None)
                        if source_progress is not None:
                            source_progress(*message[1:])
                    elif kind == "chunk":
                        if progress is not None:
                            progress(*message[1:])
                    elif kind == "size":
                        _, sample, count, source_wide = message
                        kept_rows += count
                        for name, wide in source_wide.items():
                            col_wide[name] = col_wide[name] or wide
                        row_bytes = (4 + (8 if any_weighted else 0)
                                     + sum(8 if wide else 4
                                           for wide in col_wide.values()))
                        if kept_rows * row_bytes > rvr6.MAX_DATA_BYTES:
                            raise CartError(
                                f"the kept rows no longer fit the cartridge: "
                                f"{kept_rows:,} rows x {row_bytes} bytes exceeds the "
                                f"{rvr6.MAX_DATA_BYTES}-byte column limit (reached in "
                                f'sample "{sample}") — tighten KEEP or trim the fileset')
                    elif kind == "result":
                        _, sample_at, source_at, sample, scanned, rows, weight = message
                        key = (sample_at, source_at)
                        if key in results:
                            raise CartError("a parallel worker returned a source twice")
                        results[key] = (sample, scanned, rows, weight)
                        if next_task < len(tasks):
                            task_queue.put(tasks[next_task])
                            next_task += 1
                        else:
                            task_queue.put(None)
                        finished_sources[sample] += 1
                        if finished_sources[sample] == len(self.fileset[sample]["files"]):
                            sample_done = getattr(progress, "sample_done", None)
                            if sample_done is not None:
                                sample_done(sample)
                    elif kind == "error":
                        _raise_parallel_error(*message[1:])
                    elif kind == "done":
                        active.remove(connection)
                    else:
                        raise CartError("a parallel worker sent an invalid result message")
                for process in processes:
                    if process.exitcode not in (None, 0):
                        raise CartError(
                            f"a parallel worker exited unexpectedly (exit code {process.exitcode})"
                        )

            if len(results) != len(tasks):
                raise CartError("parallel workers did not return every source")
            for process in processes:
                process.join()
                if process.exitcode != 0:
                    raise CartError(
                        f"a parallel worker exited unexpectedly (exit code {process.exitcode})"
                    )

            output: dict[str, dict[str, np.ndarray]] = {}
            output_weights: dict[str, np.ndarray] = {}
            for sample_at, (sample, entry) in enumerate(self.fileset.items()):
                pieces = [results[(sample_at, source_at)]
                          for source_at in range(len(entry["files"]))]
                output[sample] = {}
                for name in self.columns:
                    arrays = [rows[name] for _, scanned, rows, _ in pieces if scanned]
                    output[sample][name] = (np.concatenate(arrays) if arrays else
                                            np.empty(0, dtype=np.float64))
                if entry["metadata"]["weights"] is not None:
                    weights = [weight for _, _, _, weight in pieces]
                    if any(weight is None for weight in weights):
                        raise CartError(f'weighted sample "{sample}" lost its weights')
                    output_weights[sample] = np.concatenate(weights)
            succeeded = True
            return output, output_weights
        finally:
            if not succeeded:
                for process in processes:
                    if process.is_alive():
                        process.terminate()
                for process in processes:
                    process.join()
            for connection in connections:
                connection.close()
            for lifeline in lifelines:
                lifeline.close()
            task_queue.close()

    def _provenance(self, rows: Mapping[str, Mapping[str, np.ndarray]], built: str) -> dict:
        if self.source is None:
            config = None
        else:
            text = self.source.read_text(encoding="utf-8")
            stat = self.source.stat()
            config = {"path": str(self.source), "mtime": stat.st_mtime,
                      "sha256": hashlib.sha256(text.encode()).hexdigest(), "source": text}
        samples = []
        for name, entry in self.fileset.items():
            samples.append({
                "name": name,
                "rows": len(next(iter(rows[name].values()))),
                "files": [{"path": path, "tree": tree}
                          for path, tree in entry["files"].items()],
                "metadata": {k: v for k, v in entry["metadata"].items() if k != "weights"},
                "weights": _source(entry["metadata"]["weights"]),
            })
        return {
            "by": f"RowVer {rvr6.VERSION}", "built": built, "skimmed": built,
            "samples": samples,
            "keep": _source(self.keep),
            "prepare": _source(self.prepare),
            "columns": {name: _source(value) for name, value in self.columns.items()},
            "cart": config,
        }

    def build(self, destination: str | Path | None = None,
              *, step_size: int | str = "100 MB",
              progress: Callable[[str | None, int, int], None] | None = None,
              workers: int | None = None) -> dict:
        """Skim and atomically write ``<study>.rvr``.  This is the normal public API."""
        if self.name is None or self.dir is None:
            raise CartError("load the cart with Cart.load(<study>) before building it")
        if workers is not None and (type(workers) is not int or workers < 1):
            raise CartError("workers must be a positive integer")
        if workers is not None and workers > 1 and self.source is None:
            raise CartError("parallel builds require a Cart loaded from a study directory")
        rows, weights = (self.skim(step_size, progress)
                         if workers is None or workers == 1
                         else self._parallel_skim(workers, step_size, progress))
        if progress is not None:
            progress(None, 0, 0)          # skim done; packing and compressing now
        built = datetime.now(timezone.utc).isoformat()
        about = {"name": self.name}
        if self.title:
            about["title"] = self.title
        if self.note:
            about["note"] = self.note
        meta, buf = rvr6.pack(
            rows,
            weighted={name: self.fileset[name]["metadata"]["weights"] is not None
                      for name in self.fileset},
            weights=weights,
            labels=self.labels,
            about=about,
            made=self._provenance(rows, built),
        )
        target = Path(destination) if destination else self.dir / f"{self.name}.rvr"
        if target.suffix != ".rvr":
            target = target / f"{self.name}.rvr"
        return rvr6.write(target, meta, buf)

    def summary(self) -> str:
        """Return a terse pre-build summary."""
        name = self.name or "unbound study"
        lines = [f"{name}: {len(self.fileset)} samples, {len(self.columns)} columns"]
        for sample, entry in self.fileset.items():
            mode = "unweighted" if entry["metadata"].get("weights") is None else "weighted"
            lines.append(f"  {sample}: {len(entry['files'])} file(s), {mode}")
        lines.append("  columns pressed into cartridge:")
        lines.extend(f"    {name}" for name in self.columns)
        return "\n".join(lines)
