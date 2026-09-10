"""Press a study's ``cart.py`` recipe into one ``.rvr`` cartridge.

A recipe declares a Coffea-shaped fileset, a KEEP selection, the columns to
press, and ``metadata["weights"]`` for every sample: a branch name, a callable,
or ``None`` for an unweighted sample. Hooks read the events they are given and
return new values; writing into a NumPy view of an Awkward buffer is outside
Awkward's contract and corrupts what every later hook and the cartridge see.
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
from typing import Any, Callable, Iterator, Mapping, NamedTuple
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


class Trace(NamedTuple):
    """What a skim observed: each column's inputs, dtypes, and copy status, who reshaped the record, and each source's identity."""
    inputs: dict[str, set[str]]
    derived: set[str]
    writers: set[tuple[str, ...]]
    dtypes: dict[str, set[str]]
    files: dict[tuple[str, str], dict]


class _Selection(NamedTuple):
    """One chunk after PREPARE and KEEP: the survivors, what the hooks read, and how they reshaped the record."""
    events: ak.Array
    reads: set[str]
    rebuilt: bool
    writers: set[tuple[str, ...]]


class _Skimmed(NamedTuple):
    """One source's results as a parallel worker returns them."""
    sample: str
    scanned: int
    rows: dict[str, np.ndarray]
    weight: np.ndarray | None
    trace: Trace


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


def _field_names(where: Any) -> Iterator[str]:
    """The field names an item selection names, at any depth of a tuple or list."""
    if isinstance(where, str):
        yield where
    elif isinstance(where, (list, tuple)):
        for item in where:
            yield from _field_names(item)


_READS: set[str] = set()
_WRITES: set[str] = set()


def _quiet(method: Callable[..., Any]) -> Callable[..., Any]:
    """Wrap an array method so its own layout reads do not count as whole-record use."""
    def wrapped(self: Any, *args: Any, **kwargs: Any) -> Any:
        _Events._selecting += 1
        try:
            return method(self, *args, **kwargs)
        finally:
            _Events._selecting -= 1
    return wrapped


class _Events(ak.Array):
    """Records in ``_READS`` the fields a hook selects, or ``*`` once it reaches the whole record."""

    _selecting = 0

    @property
    def _layout(self) -> Any:
        if not _Events._selecting:
            _READS.add("*")
        return self.__dict__["_layout"]

    @_layout.setter
    def _layout(self, layout: Any) -> None:
        if not _Events._selecting:
            _WRITES.add("*")
        self.__dict__["_layout"] = layout

    fields = property(_quiet(ak.Array.fields.fget))
    __len__ = _quiet(ak.Array.__len__)
    __getattr__ = _quiet(ak.Array.__getattr__)

    @_quiet
    def __getitem__(self, where: Any) -> Any:
        parts = where if isinstance(where, tuple) else (where,)
        if any(isinstance(part, (int, np.integer)) for part in parts):
            _READS.add("*")
        _READS.update(_field_names(where))
        return super().__getitem__(where)

    @_quiet
    def __setitem__(self, where: Any, what: Any) -> None:
        _WRITES.update(_field_names(where))
        super().__setitem__(where, what)


_WATCHERS: dict[type, type] = {}


def _watcher(base: type) -> type:
    """The ``_Events`` subclass that observes arrays of class ``base``."""
    if issubclass(base, _Events):
        return base
    if base not in _WATCHERS:
        _WATCHERS[base] = _Events if base is ak.Array else type(base.__name__, (_Events, base), {})
    return _WATCHERS[base]


def _watched(events: ak.Array) -> ak.Array:
    """Return ``events`` observed by ``_Events``, keeping any record name and behavior it already has."""
    name = events.layout.purelist_parameter("__record__") or "rowver.events"
    local = events.behavior or {}
    base = {**ak.behavior, **local}.get(("*", name), ak.Array)
    behavior = {**local, ("*", name): _watcher(base)}
    return ak.with_parameter(events, "__record__", name, behavior=behavior)


def _expand(read: set[str], branches: set[str]) -> set[str]:
    """The branches a hook's selections name; ``*`` means it may have used every branch."""
    return set(branches) if "*" in read else read & branches


def _inputs(read: set[str], branches: set[str], prepared: set[str], rebuilt: bool) -> set[str]:
    """Branches a column's values came from; a rebuilt record or a made field adds PREPARE's reads."""
    own = _expand(read, branches)
    return own | _expand(prepared, branches) if rebuilt or read - branches else own


def _read_chunk(tree, touched: set[str], start: int, stop: int):
    """Read one entry range as ``(events, lazy)``; ``lazy`` is None after an eager fallback."""
    try:
        lazy = tree.arrays(entry_start=start, entry_stop=stop, library="ak",
                           virtual=True, array_cache=None)
    except (TypeError, ValueError) as exc:
        if "virtual" not in str(exc):
            raise
        return _watched(tree.arrays(entry_start=start, entry_stop=stop, library="ak")), None
    layout = lazy.layout
    if not touched or not isinstance(layout, ak.contents.RecordArray) or layout.is_tuple:
        return _watched(lazy), lazy
    eager = tree.arrays(filter_name=lambda name: _top_level(name) in touched,
                        entry_start=start, entry_stop=stop, library="ak", array_cache=None)
    return _watched(_overlay(lazy, eager)), lazy


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
                rows, weights, trace = cart.skim(step_size, Progress())
            finally:
                cart.fileset = full_fileset
            messages.send(("result", sample_at, source_at, sample, scanned,
                           rows[sample], weights.get(sample), trace))
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

    def _select(self, events: ak.Array, branches: set[str], metadata: dict, sample: str,
                path: str) -> _Selection:
        """Run PREPARE and KEEP on one chunk; a KEEP that writes fields counts as rebuilding the record."""
        _READS.clear()
        _WRITES.clear()
        prepared = (_invoke(self.prepare, events, metadata, "PREPARE")
                    if self.prepare is not None else events)
        reads, writers = set(_READS), set()
        rebuilt = bool("*" in _READS or _WRITES & branches or "*" in _WRITES
                       or not isinstance(prepared, _Events))
        if rebuilt or _WRITES:
            writers.add(("prepare",))
        try:
            count = len(prepared)
        except TypeError as exc:
            raise CartError("PREPARE must return an event array") from exc
        if not isinstance(prepared, _Events):
            prepared = _watched(prepared)
        _READS.clear()
        _WRITES.clear()
        mask = _one_dimensional(_invoke(self.keep, prepared, metadata, "KEEP"),
                                f'{sample} KEEP for {Path(path).name}', boolean=True)
        if _WRITES:
            reads |= _READS
            rebuilt = True
            writers.add(("keep",))
        if len(mask) != count:
            raise CartError(f'{sample} KEEP returned {len(mask)} values for {count} events')
        return _Selection(prepared[mask], reads, rebuilt, writers)

    def _columns_of(self, selected: ak.Array, metadata: dict, sample: str
                    ) -> tuple[dict[str, np.ndarray], dict[str, set[str]], set[tuple[str, ...]]]:
        """Evaluate every declared column; return the columns, what each selected, and which ones wrote."""
        out: dict[str, np.ndarray] = {}
        reads: dict[str, set[str]] = {}
        writers: set[tuple[str, ...]] = set()
        for name, spec in self.columns.items():
            _READS.clear()
            _WRITES.clear()
            value = self._evaluate(spec, selected, metadata, f'COLUMNS["{name}"]')
            column = _one_dimensional(value, f'{sample}.{name}')
            if len(column) != len(selected):
                raise CartError(f'{sample}.{name} returned {len(column)} values for '
                                f'{len(selected)} selected events')
            out[name] = column
            reads[name] = set(_READS)
            if _WRITES:
                writers.add(("column", name))
        return out, reads, writers

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
             progress: Callable[[str | None, int, int], None] | None = None
             ) -> tuple[dict, dict, Trace]:
        """Read every source in order, run the hooks, and return ``(columns, weights, trace)``."""
        if not self._validated:
            self.validate()
        capacity = _Capacity(self)
        output: dict[str, dict[str, np.ndarray]] = {}
        output_weights: dict[str, np.ndarray] = {}
        trace = Trace({}, set(), set(), {}, {})
        for sample, entry in self.fileset.items():
            chunks: dict[str, list[np.ndarray]] = {name: [] for name in self.columns}
            weight_chunks: list[np.ndarray] = []
            touched: set[str] = set()
            for path, tree_name in entry["files"].items():
                metadata = self._meta(sample, path, tree_name)
                _notify(progress, "source", sample, path, tree_name)
                with _open_sample_tree(sample, path, tree_name) as tree:
                    trace.files[path, tree_name] = {"uuid": str(tree.file.uuid),
                                                    "entries": int(tree.num_entries)}
                    for events, entries in _chunks(tree, touched, step_size):
                        branches = set(events.fields)
                        selected = self._select(events, branches, metadata, sample, path)
                        columns, reads, writers = self._columns_of(selected.events, metadata,
                                                                   sample)
                        for name, column in columns.items():
                            chunks[name].append(column)
                            trace.dtypes.setdefault(name, set()).add(str(column.dtype))
                        rebuilt = selected.rebuilt or bool(writers)
                        hook_reads = selected.reads.union(*(reads[name] for _, name in writers))
                        for name, read in reads.items():
                            trace.inputs.setdefault(name, set()).update(
                                _inputs(read, branches, hook_reads, rebuilt))
                            if rebuilt or callable(self.columns[name]) or read - branches:
                                trace.derived.add(name)
                        trace.writers.update(selected.writers, writers)
                        if entry["metadata"]["weights"] is not None:
                            weight_chunks.append(
                                self._weights_of(selected.events, metadata, sample))
                        kept = len(selected.events)
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
        return output, output_weights, trace

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
        results: dict[tuple[int, int], _Skimmed] = {}
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
                    sample_at, source_at, sample, scanned, rows, weight, trace = body
                    if (sample_at, source_at) in results:
                        raise CartError("a parallel worker returned a source twice")
                    results[(sample_at, source_at)] = _Skimmed(sample, scanned, rows, weight, trace)
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

    def _merge(self, results: dict) -> tuple[dict, dict, Trace]:
        """Concatenate per-source results in declaration order and union their traces."""
        output: dict[str, dict[str, np.ndarray]] = {}
        output_weights: dict[str, np.ndarray] = {}
        trace = Trace({}, set(), set(), {}, {})
        for sample_at, (sample, entry) in enumerate(self.fileset.items()):
            pieces = [results[(sample_at, source_at)]
                      for source_at in range(len(entry["files"]))]
            output[sample] = {
                name: _joined([piece.rows[name] for piece in pieces if piece.scanned])
                for name in self.columns
            }
            if entry["metadata"]["weights"] is not None:
                weights = [piece.weight for piece in pieces]
                if any(weight is None for weight in weights):
                    raise CartError(f'weighted sample "{sample}" lost its weights')
                output_weights[sample] = np.concatenate(weights)
            for piece in pieces:
                for name, read in piece.trace.inputs.items():
                    trace.inputs.setdefault(name, set()).update(read)
                trace.derived.update(piece.trace.derived)
                trace.writers.update(piece.trace.writers)
                for name, seen in piece.trace.dtypes.items():
                    trace.dtypes.setdefault(name, set()).update(seen)
                trace.files.update(piece.trace.files)
        return output, output_weights, trace

    def _parallel_skim(self, workers: int, step_size: int | str,
                       progress: Callable[[str | None, int, int], None] | None
                       ) -> tuple[dict, dict, Trace]:
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

    def _writer(self, writer: tuple[str, ...]) -> Callable[..., Any]:
        """The hook a writer key names: ``("prepare",)``, ``("keep",)``, or ``("column", name)``."""
        return self.columns[writer[1]] if writer[0] == "column" else getattr(self, writer[0])

    def _writer_order(self, writer: tuple[str, ...]) -> int:
        """PREPARE first, KEEP second, then columns in declaration order."""
        keys = [("prepare",), ("keep",), *(("column", name) for name in self.columns)]
        return keys.index(writer)

    def _sources(self, trace: Trace) -> dict[str, dict]:
        """Per-column provenance for every column that ran: a copy names its branch and dtype, a derived column its inputs."""
        copied = {spec for name, spec in self.columns.items()
                  if isinstance(spec, str) and name not in trace.derived}
        out: dict[str, dict] = {}
        for name, spec in self.columns.items():
            if name not in trace.inputs:
                continue
            if isinstance(spec, str) and name not in trace.derived:
                dtype = np.result_type(*sorted(trace.dtypes[name]))
                out[name] = {"class": "copy", "branch": spec, "dtype": str(dtype)}
            else:
                inputs = trace.inputs[name]
                origins = [spec] if callable(spec) else [
                    self._writer(writer) for writer in sorted(trace.writers, key=self._writer_order)]
                out[name] = {"class": "derived", "from": sorted(inputs),
                             "reproducible": "cartridge" if inputs <= copied else "source",
                             "python": "\n\n".join(_source(origin)["source"] for origin in origins)}
        return out

    def _provenance(self, rows: Mapping[str, Mapping[str, np.ndarray]], built: str,
                    files: Mapping[tuple[str, str], dict]) -> dict:
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
                "files": [{"path": path, "tree": tree, **files[path, tree]}
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
        rows, weights, trace = (self.skim(step_size, progress)
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
            made=self._provenance(rows, built, trace.files),
            sources=self._sources(trace),
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
