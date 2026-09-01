"""Temporary instrumentation for the parallel-press performance study.

Run from the repository root:

    python -m benchmarks.parallel_press STUDY --output result.rvr --json result.json

This module intentionally instruments the existing sequential path without adding
benchmark hooks to the product API.  Delete it when the study is complete.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import resource
import sys
import time
from collections import defaultdict
from contextlib import ExitStack
from pathlib import Path
from typing import Any, Callable
from unittest.mock import patch

import numpy as np

from rowver import rvr6
from rowver.press import Cart, _open_sample_tree, _redacted_source
import rowver.press as press


def _source_id(path: str) -> str:
    safe = _redacted_source(path)
    name = Path(safe.split("://", 1)[-1]).name or "source"
    digest = hashlib.sha256(safe.encode()).hexdigest()[:12]
    return f"{name}#{digest}"


def _peak_rss_bytes() -> int:
    value = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    return int(value if sys.platform == "darwin" else value * 1024)


class _Timer:
    def __init__(self) -> None:
        self.seconds: dict[str, float] = defaultdict(float)
        self.calls: dict[str, int] = defaultdict(int)

    def measure(self, phase: str, function: Callable[..., Any], *args: Any,
                **kwargs: Any) -> Any:
        started = time.perf_counter()
        try:
            return function(*args, **kwargs)
        finally:
            self.seconds[phase] += time.perf_counter() - started
            self.calls[phase] += 1


class _NumpyProbe:
    def __init__(self, timer: _Timer):
        self._timer = timer

    def __getattr__(self, name: str) -> Any:
        return getattr(np, name)

    def concatenate(self, *args: Any, **kwargs: Any) -> Any:
        return self._timer.measure("source_concatenation", np.concatenate,
                                   *args, **kwargs)


class _GzipProbe:
    def __init__(self, timer: _Timer, module: Any):
        self._timer, self._module = timer, module

    def __getattr__(self, name: str) -> Any:
        return getattr(self._module, name)

    def compress(self, *args: Any, **kwargs: Any) -> Any:
        return self._timer.measure("gzip", self._module.compress, *args, **kwargs)


class _OsProbe:
    def __init__(self, timer: _Timer, module: Any):
        self._timer, self._module = timer, module

    def __getattr__(self, name: str) -> Any:
        return getattr(self._module, name)

    def fsync(self, *args: Any, **kwargs: Any) -> Any:
        return self._timer.measure("fsync", self._module.fsync, *args, **kwargs)

    def replace(self, *args: Any, **kwargs: Any) -> Any:
        return self._timer.measure("atomic_replace", self._module.replace, *args, **kwargs)


class _TreeProbe:
    def __init__(self, tree: Any, source: dict[str, Any], timer: _Timer):
        self._tree, self._source, self._timer = tree, source, timer

    def __getattr__(self, name: str) -> Any:
        return getattr(self._tree, name)

    def arrays(self, *args: Any, **kwargs: Any) -> Any:
        started = time.perf_counter()
        try:
            return self._tree.arrays(*args, **kwargs)
        finally:
            elapsed = time.perf_counter() - started
            # With virtual Awkward arrays, physical reads may occur later inside
            # PREPARE/KEEP/column evaluation.  Name this narrow timer honestly and
            # use source_active plus the source byte counter for the inclusive cost.
            self._source["array_request_seconds"] += elapsed
            self._timer.seconds["array_request"] += elapsed
            self._timer.calls["array_request"] += 1


class _SourceProbe:
    def __init__(self, source: Any, record: dict[str, Any], timer: _Timer):
        self._source, self._record, self._timer = source, record, timer

    def __enter__(self) -> _TreeProbe:
        tree = self._timer.measure("source_enter", self._source.__enter__)
        self._tree = tree
        self._active_started = time.perf_counter()
        return _TreeProbe(tree, self._record, self._timer)

    def __exit__(self, *args: Any) -> Any:
        active = time.perf_counter() - self._active_started
        byte_source = getattr(getattr(self._tree, "file", None), "source", None)
        self._record["active_seconds"] = active
        self._record["bytes_read"] = int(getattr(byte_source, "num_requested_bytes", 0))
        self._timer.seconds["source_active"] += active
        self._timer.calls["source_active"] += 1
        return self._timer.measure("source_close", self._source.__exit__, *args)


class _Progress:
    def __init__(self, sources: list[dict[str, Any]]):
        self.sources = sources
        self.current: dict[str, Any] | None = None
        self.scanned = self.kept = 0

    def source(self, sample: str, path: str, tree: str) -> None:
        self.current = {
            "sample": sample,
            "source": _source_id(path),
            "tree": tree,
            "open_seconds": 0.0,
            "active_seconds": 0.0,
            "array_request_seconds": 0.0,
            "bytes_read": 0,
            "rows_scanned": 0,
            "rows_kept": 0,
        }
        self.sources.append(self.current)

    def __call__(self, sample: str | None, scanned: int, kept: int) -> None:
        if sample is None:
            return
        self.scanned += scanned
        self.kept += kept
        assert self.current is not None
        self.current["rows_scanned"] += scanned
        self.current["rows_kept"] += kept


def measure(study: str | Path, output: str | Path,
            *, step_size: int | str = "100 MB") -> dict[str, Any]:
    """Measure one real sequential build and return a JSON-compatible record."""
    timer = _Timer()
    wall_started, cpu_started = time.perf_counter(), time.process_time()
    error: dict[str, str] | None = None
    sources: list[dict[str, Any]] = []
    progress = _Progress(sources)
    result: dict[str, Any] | None = None

    try:
        cart = timer.measure("study_load_and_validation", Cart.load, study)
        original_open = _open_sample_tree
        original_invoke = press._invoke
        original_evaluate = Cart._evaluate
        original_check = rvr6.check
        original_pack = rvr6.pack
        original_write = rvr6.write

        def open_source(sample: str, path: str, tree: str) -> _SourceProbe:
            started = time.perf_counter()
            try:
                source = original_open(sample, path, tree)
            finally:
                elapsed = time.perf_counter() - started
                timer.seconds["source_open"] += elapsed
                timer.calls["source_open"] += 1
                assert progress.current is not None
                progress.current["open_seconds"] += elapsed
            return _SourceProbe(source, progress.current, timer)

        def invoke(function: Callable[..., Any], events: Any, meta: dict,
                   where: str) -> Any:
            phase = where.lower() if where in {"PREPARE", "KEEP"} else None
            return (timer.measure(phase, original_invoke, function, events, meta, where)
                    if phase else original_invoke(function, events, meta, where))

        def evaluate(spec: Any, events: Any, meta: dict, where: str) -> Any:
            phase = "weights" if where.endswith(" weights") else "columns"
            return timer.measure(phase, original_evaluate, spec, events, meta, where)

        def check(*args: Any, **kwargs: Any) -> Any:
            return timer.measure("validation", original_check, *args, **kwargs)

        def pack(*args: Any, **kwargs: Any) -> Any:
            return timer.measure("pack", original_pack, *args, **kwargs)

        def write(*args: Any, **kwargs: Any) -> Any:
            return timer.measure("write", original_write, *args, **kwargs)

        with ExitStack() as stack:
            stack.enter_context(patch.object(press, "_open_sample_tree", open_source))
            stack.enter_context(patch.object(press, "_invoke", invoke))
            stack.enter_context(patch.object(Cart, "_evaluate", staticmethod(evaluate)))
            stack.enter_context(patch.object(press, "np", _NumpyProbe(timer)))
            stack.enter_context(patch.object(rvr6, "check", check))
            stack.enter_context(patch.object(rvr6, "pack", pack))
            stack.enter_context(patch.object(rvr6, "write", write))
            stack.enter_context(patch.object(rvr6, "gzip", _GzipProbe(timer, rvr6.gzip)))
            stack.enter_context(patch.object(rvr6, "os", _OsProbe(timer, rvr6.os)))
            result = cart.build(output, step_size=step_size, progress=progress)
    except Exception as exc:
        error = {"type": type(exc).__name__, "message": str(exc)}

    wall = time.perf_counter() - wall_started
    cpu = time.process_time() - cpu_started
    timer.seconds.setdefault("ordered_merge", 0.0)
    timer.calls.setdefault("ordered_merge", 0)
    timer.seconds.setdefault("result_transfer", 0.0)
    timer.calls.setdefault("result_transfer", 0)
    record = {
        "schema": 1,
        "mode": "sequential",
        "workers": 1,
        "study": str(Path(study)),
        "step_size": step_size,
        "machine": {
            "platform": platform.platform(),
            "architecture": platform.machine(),
            "logical_cpus": os.cpu_count(),
            "python": platform.python_version(),
        },
        "totals": {
            "wall_seconds": wall,
            "process_cpu_seconds": cpu,
            "process_cpu_utilization": cpu / wall if wall else 0.0,
            "peak_rss_bytes": _peak_rss_bytes(),
            "bytes_read": sum(source["bytes_read"] for source in sources),
            "rows_scanned": progress.scanned,
            "rows_kept": progress.kept,
            "failures": int(error is not None),
            "retries": None,
        },
        "phases": {
            phase: {"seconds": seconds, "calls": timer.calls[phase]}
            for phase, seconds in sorted(timer.seconds.items())
        },
        "sources": sources,
        "result": result,
        "error": error,
    }
    return record


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("study", help="Study name or directory")
    parser.add_argument("--output", type=Path, required=True, help="Measured .rvr output")
    parser.add_argument("--json", type=Path, required=True, help="Machine-readable result")
    parser.add_argument("--step-size", default="100 MB")
    args = parser.parse_args(argv)
    step_size: int | str = (int(args.step_size) if args.step_size.isdigit()
                            else args.step_size)
    record = measure(args.study, args.output, step_size=step_size)
    args.json.parent.mkdir(parents=True, exist_ok=True)
    args.json.write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {args.json} — {record['totals']['wall_seconds']:.3f} s")
    if record["error"]:
        print(f"error: {record['error']['type']}: {record['error']['message']}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
