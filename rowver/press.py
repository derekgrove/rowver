"""Press a study's ``cart.py`` recipe into one ``.rvr`` cartridge.

A recipe declares a Coffea-shaped fileset, a KEEP selection, the columns to
press, and ``metadata["weights"]`` for every sample: a branch name, a callable,
or ``None`` for an unweighted sample.
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
from copy import deepcopy
from datetime import datetime, timezone
from multiprocessing.connection import wait
from pathlib import Path
from typing import Any, Callable, Iterator, Mapping
from urllib.parse import urlsplit, urlunsplit

import awkward as ak
import numpy as np
import uproot

from . import rvr6

STUDIES = Path("studies")
RESERVED_META = frozenset({"sample", "file", "tree"})
WEIGHT_MISSPELLINGS = frozenset({
    "weight", "Weights", "WEIGHTS", "weigths", "WEIGHTS_BRANCH", "HAS_WEIGHTS",
})
PROBE_ENTRIES = 1024
FALLBACK_ENTRIES = 100_000


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


def _open_sample_tree(sample: str, path: str, tree: str):
    """Open a tree and translate source failures consistently for users."""
    try:
        return _open_tree(path, tree)
    except Exception as exc:
        raise _source_failure(sample, path, exc) from exc


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


def _call_shape(function: Callable[..., Any], where: str) -> int:
    """Return 1 or 2 for a hook taking ``events`` or ``events, metadata``."""
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


def _invoke(function: Callable[..., Any], events: ak.Array, meta: dict, where: str) -> Any:
    return function(events, meta) if _call_shape(function, where) == 2 else function(events)


def _byte_count(text: str) -> int:
    """'100 MB' -> 100_000_000, with uproot's step-size grammar."""
    scale = {"b": 1, "kb": 10**3, "mb": 10**6, "gb": 10**9, "tb": 10**12,
             "kib": 2**10, "mib": 2**20, "gib": 2**30, "tib": 2**40}
    match = re.fullmatch(r"\s*([0-9.]+(?:[eE][+-]?[0-9]+)?)\s*([A-Za-z]+)\s*", text)
    if not match or match.group(2).lower() not in scale:
        raise CartError(f'cannot read step size "{text}"')
    return int(float(match.group(1)) * scale[match.group(2).lower()])


def _top_level(name: str) -> str:
    """The top-level field a branch, or an RNTuple sub-field path, belongs to."""
    return name.lstrip(".").split(".", 1)[0]


def _read_chunk(tree, touched: set[str], start: int, stop: int):
    """Read one entry range as ``(events, lazy)``; ``lazy`` is None after an eager fallback."""
    try:
        lazy = tree.arrays(entry_start=start, entry_stop=stop, library="ak",
                           virtual=True, array_cache=None)
    except (TypeError, ValueError) as exc:
        if "virtual" not in str(exc):
            raise
        return tree.arrays(entry_start=start, entry_stop=stop, library="ak"), None
    layout = lazy.layout
    if not touched or not isinstance(layout, ak.contents.RecordArray) or layout.is_tuple:
        return lazy, lazy
    eager = tree.arrays(filter_name=lambda name: _top_level(name) in touched,
                        entry_start=start, entry_stop=stop, library="ak", array_cache=None)
    return _overlay(lazy, eager), lazy


def _overlay(lazy: ak.Array, eager: ak.Array) -> ak.Array:
    """Replace lazy placeholders with eager fields of the same form; the rest stay lazy."""
    events = lazy
    for name in eager.fields:
        if name in lazy.layout.fields and lazy.layout.content(name).form.is_equal_to(
                eager.layout.content(name).form):
            events = ak.with_field(events, eager[name], name)
    return events


def _newly_touched(lazy, known: set[str]) -> list[str]:
    """Branches whose lazy placeholders a hook materialised during this chunk."""
    layout = lazy.layout
    if not isinstance(layout, ak.contents.RecordArray) or layout.is_tuple:
        return []
    return [name for name in layout.fields
            if name not in known and layout.content(name).is_any_materialized]


def _entries_per_chunk(tree, touched: set[str], budget: int, total: int) -> int:
    """Entries whose touched branches fill ``budget`` bytes, sized from the tree header."""
    if total <= 0:
        return 1
    touched_bytes = 0
    for name in touched:
        try:
            touched_bytes += int(tree[name].uncompressed_bytes)
        except Exception:
            continue
    per_entry = touched_bytes / total
    if per_entry <= 0:
        try:
            per_entry = int(tree.file.source.num_bytes) / total
        except Exception:
            per_entry = 0.0
    if per_entry <= 0:
        return min(FALLBACK_ENTRIES, total)
    return max(min(int(budget / per_entry), total), 1)


def _chunks(tree, touched: set[str], step_size: int | str) -> Iterator[tuple[ak.Array, int]]:
    """Yield ``(events, entries)`` per chunk; a fresh touched set starts with a small probe."""
    total = int(tree.num_entries)
    budget = None if isinstance(step_size, int) else _byte_count(step_size)
    if budget is None:
        per_chunk: int | None = max(step_size, 1)
    else:
        per_chunk = _entries_per_chunk(tree, touched, budget, total) if touched else None
    first = per_chunk if touched else min(PROBE_ENTRIES, per_chunk or PROBE_ENTRIES)
    start, stop = 0, min(first, total)
    while start < total:
        events, lazy = _read_chunk(tree, touched, start, stop)
        yield events, stop - start
        newly = _newly_touched(lazy, touched) if lazy is not None else []
        touched.update(newly)
        if budget is not None and (newly or per_chunk is None):
            per_chunk = _entries_per_chunk(tree, touched, budget, total)
        start, stop = stop, min(stop + per_chunk, total)


def _bits(column: np.ndarray) -> int:
    """Bits per row this chunk needs in the cartridge; never above what pack uses."""
    if column.dtype.kind == "b":
        return 1
    i32 = np.iinfo(np.int32)
    if column.dtype.kind in "iu" and column.min() >= i32.min and column.max() <= i32.max:
        return 32
    return 64


def _joined(arrays: list[np.ndarray]) -> np.ndarray:
    """Concatenate chunks, letting only populated ones decide the dtype."""
    populated = [a for a in arrays if a.size]
    return np.concatenate(populated or arrays) if arrays else np.empty(0, dtype=np.float64)


def _notify(progress: Any, event: str, *args: Any) -> None:
    """Call an optional lifecycle method of a progress callback."""
    method = getattr(progress, event, None)
    if method is not None:
        method(*args)


class _Capacity:
    """Refuse a press as soon as its kept rows can no longer fit the column buffer."""

    def __init__(self, cart: "Cart") -> None:
        self.rows = 0
        self.bits = {name: 1 for name in cart.columns}
        weighted = any(entry["metadata"]["weights"] is not None
                       for entry in cart.fileset.values())
        self.row_bytes = 4 + (8 if weighted else 0)

    def add(self, sample: str, kept: int, bits: Mapping[str, int]) -> None:
        self.rows += kept
        for name, width in bits.items():
            self.bits[name] = max(self.bits[name], width)
        packed = self.rows * self.row_bytes + sum(
            (self.rows * width + 7) // 8 for width in self.bits.values())
        if packed > rvr6.MAX_DATA_BYTES:
            raise CartError(
                f"the kept rows no longer fit the cartridge: "
                f"{self.rows:,} rows need {packed} bytes, exceeding the "
                f"{rvr6.MAX_DATA_BYTES}-byte column limit (reached in "
                f'sample "{sample}") — tighten KEEP or trim the fileset')


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


def _raise_parallel_error(kind: str, text: str) -> None:
    if kind == "interrupt":
        raise KeyboardInterrupt(text)
    if kind == "exit":
        raise SystemExit(text)
    raise CartError(text)


def _check_alive(processes: list) -> None:
    for process in processes:
        if process.exitcode not in (None, 0):
            raise CartError(
                f"a parallel worker exited unexpectedly (exit code {process.exitcode})"
            )


def _task_matches(fileset: Mapping[str, dict], task: tuple) -> bool:
    """True when a task still names the same source at the same position."""
    sample_at, source_at, sample, path, tree = task
    samples = list(fileset)
    sources = list(fileset.get(sample, {}).get("files", {}).items())
    return (sample_at < len(samples) and samples[sample_at] == sample
            and source_at < len(sources) and sources[source_at] == (path, tree))


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
            if not _task_matches(full_fileset, task):
                raise CartError("study fileset changed after parallel tasks were resolved")

            scanned = 0

            class Progress:
                def source(self, name: str, source: str, tree_name: str) -> None:
                    messages.send(("source", name, source, tree_name))

                def __call__(self, name: str | None, raw: int, kept: int) -> None:
                    nonlocal scanned
                    scanned += raw
                    messages.send(("chunk", name, raw, kept))

                def _size(self, name: str, kept: int, bits: dict[str, int]) -> None:
                    messages.send(("size", name, kept, bits))

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


def _wait_ready(processes: list, connections: list) -> None:
    """Block until every worker has reloaded the study, surfacing any that die first."""
    ready: set[Any] = set()
    while len(ready) < len(connections):
        for connection in wait(connections, timeout=0.1):
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
        _check_alive(processes)


class Cart:
    """A ROOT fileset, one preselection, and the columns exposed in RowVer."""

    def __init__(self, *, fileset: Mapping[str, Mapping[str, Any]],
                 columns: Mapping[str, str | Callable[..., Any]],
                 keep: Callable[..., Any], prepare: Callable[..., Any] | None = None,
                 labels: Mapping[str, Mapping[Any, str]] | None = None,
                 view: Mapping[str, Any] | None = None,
                 title: str | None = None, note: str | None = None):
        self.fileset = {name: dict(entry) for name, entry in fileset.items()}
        self.columns = dict(columns)
        self.keep = keep
        self.prepare = prepare
        self.labels = dict(labels or {})
        self.view = deepcopy(dict(view or {}))
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
        """Resolve local globs against the study directory; remote URLs pass through."""
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
        _jsonable(self.view, "view")
        self._validated = True

    def _meta(self, sample: str, path: str, tree: str) -> dict:
        return {**self.fileset[sample]["metadata"],
                "sample": sample, "file": path, "tree": tree}

    @staticmethod
    def _evaluate(spec: str | Callable[..., Any], events: ak.Array,
                  meta: dict, where: str) -> Any:
        return events[spec] if isinstance(spec, str) else _invoke(spec, events, meta, where)

    def _select(self, events: ak.Array, metadata: dict, sample: str, path: str) -> ak.Array:
        """Run PREPARE and KEEP on one chunk and return the events that survive."""
        prepared = (_invoke(self.prepare, events, metadata, "PREPARE")
                    if self.prepare is not None else events)
        try:
            count = len(prepared)
        except TypeError as exc:
            raise CartError("PREPARE must return an event array") from exc
        mask = _one_dimensional(_invoke(self.keep, prepared, metadata, "KEEP"),
                                f'{sample} KEEP for {Path(path).name}', boolean=True)
        if len(mask) != count:
            raise CartError(f'{sample} KEEP returned {len(mask)} values for {count} events')
        return prepared[mask]

    def _columns_of(self, selected: ak.Array, metadata: dict, sample: str) -> dict[str, np.ndarray]:
        """Evaluate every declared column on the selected events."""
        out: dict[str, np.ndarray] = {}
        for name, spec in self.columns.items():
            value = self._evaluate(spec, selected, metadata, f'COLUMNS["{name}"]')
            column = _one_dimensional(value, f'{sample}.{name}')
            if len(column) != len(selected):
                raise CartError(f'{sample}.{name} returned {len(column)} values for '
                                f'{len(selected)} selected events')
            out[name] = column
        return out

    def _weights_of(self, selected: ak.Array, metadata: dict, sample: str) -> np.ndarray:
        """Evaluate the sample's weights declaration on the selected events."""
        weight = self.fileset[sample]["metadata"]["weights"]
        values = _one_dimensional(
            self._evaluate(weight, selected, metadata, f'{sample} weights'),
            f'{sample}.weight').astype(np.float64, copy=False)
        if len(values) != len(selected):
            raise CartError(f'{sample}.weight length does not match its events')
        if not np.isfinite(values).all():
            raise CartError(f'{sample}.weight contains NaN or infinity')
        return values

    def skim(self, step_size: int | str = "100 MB",
             progress: Callable[[str | None, int, int], None] | None = None) -> tuple[dict, dict]:
        """Read every source in order, run the hooks, and return ``(columns, weights)`` by sample."""
        if not self._validated:
            self.validate()
        capacity = _Capacity(self)
        output: dict[str, dict[str, np.ndarray]] = {}
        output_weights: dict[str, np.ndarray] = {}
        for sample, entry in self.fileset.items():
            chunks: dict[str, list[np.ndarray]] = {name: [] for name in self.columns}
            weight_chunks: list[np.ndarray] = []
            touched: set[str] = set()
            for path, tree_name in entry["files"].items():
                metadata = self._meta(sample, path, tree_name)
                _notify(progress, "source", sample, path, tree_name)
                with _open_sample_tree(sample, path, tree_name) as tree:
                    for events, entries in _chunks(tree, touched, step_size):
                        selected = self._select(events, metadata, sample, path)
                        columns = self._columns_of(selected, metadata, sample)
                        for name, column in columns.items():
                            chunks[name].append(column)
                        if entry["metadata"]["weights"] is not None:
                            weight_chunks.append(self._weights_of(selected, metadata, sample))
                        kept = len(selected)
                        capacity.add(sample, kept, {
                            name: _bits(column) for name, column in columns.items()
                            if column.size and capacity.bits[name] < 64})
                        size = getattr(progress, "_size", None)
                        if size is not None:
                            size(sample, kept, dict(capacity.bits))
                        if progress is not None:
                            progress(sample, entries, kept)
            output[sample] = {name: _joined(values) for name, values in chunks.items()}
            if entry["metadata"]["weights"] is not None:
                output_weights[sample] = (np.concatenate(weight_chunks) if weight_chunks
                                          else np.empty(0, dtype=np.float64))
            _notify(progress, "sample_done", sample)
        return output, output_weights

    def _spawn(self, context, number: int, step_size: int | str, task_queue) -> tuple:
        """Start one worker; return its process, its message pipe, and its lifeline."""
        receive, send = context.Pipe(duplex=False)
        parent_watch, lifeline = context.Pipe(duplex=False)
        process = context.Process(
            target=_parallel_worker,
            args=(str(self.dir), self._recipe_hash, step_size, task_queue, send, parent_watch),
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
        send.close()
        parent_watch.close()
        return process, receive, lifeline

    def _collect(self, tasks: list, processes: list, connections: list, task_queue,
                 progress: Callable[[str | None, int, int], None] | None) -> dict:
        """Relay worker messages to the progress callback until every source has returned."""
        results: dict[tuple[int, int], tuple[str, int, dict, np.ndarray | None]] = {}
        finished = {sample: 0 for sample in self.fileset}
        capacity = _Capacity(self)
        next_task = len(connections)
        active = set(connections)
        while active:
            for connection in wait(active, timeout=0.1):
                try:
                    message = connection.recv()
                except EOFError as exc:
                    process = processes[connections.index(connection)]
                    process.join(0.1)
                    _check_alive([process])
                    raise CartError("a parallel worker exited without completing") from exc
                kind, *body = message
                if kind == "source":
                    _notify(progress, "source", *body)
                elif kind == "chunk":
                    if progress is not None:
                        progress(*body)
                elif kind == "size":
                    capacity.add(*body)
                elif kind == "result":
                    sample_at, source_at, sample, scanned, rows, weight = body
                    if (sample_at, source_at) in results:
                        raise CartError("a parallel worker returned a source twice")
                    results[(sample_at, source_at)] = (sample, scanned, rows, weight)
                    if next_task < len(tasks):
                        task_queue.put(tasks[next_task])
                        next_task += 1
                    else:
                        task_queue.put(None)
                    finished[sample] += 1
                    if finished[sample] == len(self.fileset[sample]["files"]):
                        _notify(progress, "sample_done", sample)
                elif kind == "error":
                    _raise_parallel_error(*body)
                elif kind == "done":
                    active.remove(connection)
                else:
                    raise CartError("a parallel worker sent an invalid result message")
            _check_alive(processes)
        if len(results) != len(tasks):
            raise CartError("parallel workers did not return every source")
        return results

    def _merge(self, results: dict) -> tuple[dict, dict]:
        """Concatenate per-source results in declaration order."""
        output: dict[str, dict[str, np.ndarray]] = {}
        output_weights: dict[str, np.ndarray] = {}
        for sample_at, (sample, entry) in enumerate(self.fileset.items()):
            pieces = [results[(sample_at, source_at)]
                      for source_at in range(len(entry["files"]))]
            output[sample] = {
                name: _joined([rows[name] for _, scanned, rows, _ in pieces if scanned])
                for name in self.columns
            }
            if entry["metadata"]["weights"] is not None:
                weights = [weight for *_, weight in pieces]
                if any(weight is None for weight in weights):
                    raise CartError(f'weighted sample "{sample}" lost its weights')
                output_weights[sample] = np.concatenate(weights)
        return output, output_weights

    def _parallel_skim(self, workers: int, step_size: int | str,
                       progress: Callable[[str | None, int, int], None] | None
                       ) -> tuple[dict, dict]:
        """Skim one source per task in worker processes; merging and every side effect stay here."""
        if self._recipe_hash is None:
            raise CartError("parallel builds require an unchanged loaded Study")
        tasks = [
            (sample_at, source_at, sample, path, tree)
            for sample_at, (sample, entry) in enumerate(self.fileset.items())
            for source_at, (path, tree) in enumerate(entry["files"].items())
        ]
        workers = min(workers, len(tasks))
        context = multiprocessing.get_context("spawn")
        task_queue = context.SimpleQueue()
        processes: list = []
        connections: list = []
        lifelines: list = []
        succeeded = False
        try:
            for number in range(workers):
                process, receive, lifeline = self._spawn(context, number, step_size, task_queue)
                processes.append(process)
                connections.append(receive)
                lifelines.append(lifeline)
            _wait_ready(processes, connections)
            for task in tasks[:workers]:
                task_queue.put(task)
            results = self._collect(tasks, processes, connections, task_queue, progress)
            for process in processes:
                process.join()
            _check_alive(processes)
            output = self._merge(results)
            succeeded = True
            return output
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
        """Skim and atomically write ``<study>.rvr``; the normal public API."""
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
            progress(None, 0, 0)
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
        meta["view"].update(deepcopy(self.view))
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
