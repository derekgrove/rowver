"""Throwaway sample/source process prototypes for the parallel-press study."""
from __future__ import annotations

import argparse
import hashlib
import json
import multiprocessing
import os
import random
import resource
import shutil
import sys
import tempfile
import time
from concurrent.futures import ProcessPoolExecutor, as_completed
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import numpy as np

from rowver import rvr6
from rowver.press import Cart, CartError, _open_sample_tree


@dataclass(frozen=True)
class Task:
    sample_ordinal: int
    source_ordinal: int | None
    sample: str
    sources: tuple[tuple[str, str], ...]
    delay: float = 0.0


_START_BARRIER: Any = None


def _worker_ready(barrier: Any) -> None:
    global _START_BARRIER
    _START_BARRIER = barrier
    barrier.wait()


def _rss_bytes() -> int:
    value = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    return int(value if sys.platform == "darwin" else value * 1024)


def _study_hash(study: Path) -> str:
    return hashlib.sha256((study / "cart.py").read_bytes()).hexdigest()


def _run_task(study: str, expected_hash: str, task: Task, step_size: int | str,
              transport: str, shard_dir: str | None) -> dict[str, Any]:
    started, cpu_started = time.perf_counter(), time.process_time()
    if _study_hash(Path(study)) != expected_hash:
        raise CartError("study recipe changed after tasks were resolved")
    cart = Cart.load(study)
    samples = list(cart.fileset)
    if task.sample_ordinal >= len(samples) or samples[task.sample_ordinal] != task.sample:
        raise CartError("study sample order changed after tasks were resolved")
    entry = cart.fileset[task.sample]
    resolved = tuple(entry["files"].items())
    if task.source_ordinal is None:
        if resolved != task.sources:
            raise CartError("study source list changed after tasks were resolved")
    elif (task.source_ordinal >= len(resolved)
          or resolved[task.source_ordinal] != task.sources[0]):
        raise CartError("study source order changed after tasks were resolved")
    cart.fileset = {task.sample: {
        "files": dict(task.sources),
        "metadata": entry["metadata"],
    }}
    if task.delay:
        time.sleep(task.delay)
    rows, weights = cart.skim(step_size)
    arrays = rows[task.sample]
    weight = weights.get(task.sample)
    result: dict[str, Any] = {
        "sample_ordinal": task.sample_ordinal,
        "source_ordinal": task.source_ordinal,
        "sample": task.sample,
        "rows": arrays,
        "weight": weight,
        "worker_wall_seconds": time.perf_counter() - started,
        "worker_cpu_seconds": time.process_time() - cpu_started,
        "worker_peak_rss_bytes": _rss_bytes(),
    }
    if transport == "shard":
        assert shard_dir is not None
        ordinal = (f"{task.sample_ordinal:04d}-sample" if task.source_ordinal is None else
                   f"{task.sample_ordinal:04d}-{task.source_ordinal:06d}")
        path = Path(shard_dir) / f"{ordinal}-{os.getpid()}.npz"
        payload = dict(arrays)
        if weight is not None:
            payload["__weight__"] = weight
        shard_started = time.perf_counter()
        np.savez(path, **payload)
        result["shard_write_seconds"] = time.perf_counter() - shard_started
        result["shard_bytes"] = path.stat().st_size
        result["shard"] = str(path)
        result.pop("rows")
        result.pop("weight")
    return result


def _tasks(cart: Cart, mode: str, delays: random.Random | None) -> list[Task]:
    tasks = []
    for sample_ordinal, (sample, entry) in enumerate(cart.fileset.items()):
        sources = tuple(entry["files"].items())
        if mode == "sample":
            tasks.append(Task(sample_ordinal, None, sample, sources,
                              delays.random() * 0.05 if delays else 0.0))
        else:
            tasks.extend(Task(sample_ordinal, source_ordinal, sample, (source,),
                              delays.random() * 0.05 if delays else 0.0)
                         for source_ordinal, source in enumerate(sources))
    return tasks


def _order_tasks(tasks: list[Task], schedule: str,
                 known_entries: dict[str, int] | None = None
                 ) -> tuple[list[Task], float, dict[str, int]]:
    if schedule == "declaration":
        return tasks, 0.0, {}
    started = time.perf_counter()
    entries = dict(known_entries or {})
    if not entries:
        for task in tasks:
            total = 0
            for path, tree in task.sources:
                with _open_sample_tree(task.sample, path, tree) as source:
                    total += int(source.num_entries)
            key = f"{task.sample_ordinal}:{task.source_ordinal}"
            entries[key] = total
    expected = {f"{task.sample_ordinal}:{task.source_ordinal}" for task in tasks}
    if set(entries) != expected or any(type(value) is not int or value < 0
                                       for value in entries.values()):
        raise CartError("entry-count hints do not match the resolved tasks")
    ordered = sorted(tasks, key=lambda task: entries[
        f"{task.sample_ordinal}:{task.source_ordinal}"], reverse=True)
    return ordered, 0.0 if known_entries else time.perf_counter() - started, entries


def _load_result(result: dict[str, Any]) -> tuple[dict[str, np.ndarray], np.ndarray | None,
                                                   float, int]:
    path = result.get("shard")
    if path is None:
        return result["rows"], result["weight"], 0.0, 0
    started = time.perf_counter()
    with np.load(path, allow_pickle=False) as saved:
        rows = {name: saved[name].copy() for name in saved.files if name != "__weight__"}
        weight = saved["__weight__"].copy() if "__weight__" in saved.files else None
    return rows, weight, time.perf_counter() - started, int(result["shard_bytes"])


def _merge(cart: Cart, mode: str, results: list[dict[str, Any]]) -> tuple[dict, dict, float,
                                                                         float, int]:
    started = time.perf_counter()
    by_sample: dict[str, list[tuple[int, dict[str, np.ndarray], np.ndarray | None]]] = {
        sample: [] for sample in cart.fileset
    }
    load_seconds = shard_bytes = 0
    for result in sorted(results, key=lambda item: (
            item["sample_ordinal"], item["source_ordinal"] if item["source_ordinal"] is not None else -1)):
        rows, weight, elapsed, size = _load_result(result)
        load_seconds += elapsed
        shard_bytes += size
        ordinal = result["source_ordinal"] if mode == "source" else 0
        by_sample[result["sample"]].append((ordinal, rows, weight))
    merged: dict[str, dict[str, np.ndarray]] = {}
    weights: dict[str, np.ndarray] = {}
    for sample, pieces in by_sample.items():
        pieces.sort(key=lambda item: item[0])
        merged[sample] = {}
        for name in cart.columns:
            arrays = [piece[1][name] for piece in pieces if piece[1][name].size]
            # Cart.skim represents an entirely empty sample as float64, but an empty
            # source must not promote integer arrays from its non-empty siblings.
            merged[sample][name] = (np.concatenate(arrays) if arrays else
                                    np.empty(0, dtype=np.float64))
        if cart.fileset[sample]["metadata"]["weights"] is not None:
            weights[sample] = np.concatenate([piece[2] for piece in pieces])
    return merged, weights, time.perf_counter() - started, load_seconds, shard_bytes


def run(study: str | Path, output: str | Path, *, mode: str, workers: int,
        transport: str = "pickle", start_method: str = "spawn",
        step_size: int | str = "100 MB", built: str | None = None,
        delay_seed: int | None = None, schedule: str = "declaration",
        known_entries: dict[str, int] | None = None) -> dict[str, Any]:
    """Run one prototype; only the coordinator packs and writes the cartridge."""
    started, cpu_started = time.perf_counter(), time.process_time()
    study = Path(study).resolve()
    cart = Cart.load(study)
    tasks = _tasks(cart, mode, random.Random(delay_seed) if delay_seed is not None else None)
    tasks, index_seconds, indexed_entries = _order_tasks(tasks, schedule, known_entries)
    workers = min(workers, len(tasks))
    if workers < 1:
        raise ValueError("workers must be positive")
    context = multiprocessing.get_context(start_method)
    barrier = context.Barrier(workers + 1)
    shard_dir = tempfile.mkdtemp(prefix="rowver-parallel-shards-") if transport == "shard" else None
    results = []
    try:
        pool = ProcessPoolExecutor(max_workers=workers, mp_context=context,
                                   initializer=_worker_ready, initargs=(barrier,))
        futures = []
        try:
            futures = [pool.submit(_run_task, str(study), _study_hash(study), task,
                                   step_size, transport, shard_dir) for task in tasks]
            barrier.wait()
            for future in as_completed(futures):
                results.append(future.result())
        except BaseException:
            for future in futures:
                future.cancel()
            # ponytail: private executor state is acceptable in this throwaway
            # prototype; production needs an owned process abstraction.
            for process in pool._processes.values():
                process.terminate()
            pool.shutdown(wait=True, cancel_futures=True)
            raise
        else:
            pool.shutdown()
        rows, weights, merge_seconds, shard_load_seconds, shard_bytes = _merge(
            cart, mode, results)
        built = built or datetime.now(timezone.utc).isoformat()
        about = {"name": cart.name}
        if cart.title:
            about["title"] = cart.title
        if cart.note:
            about["note"] = cart.note
        meta, buf = rvr6.pack(
            rows,
            weighted={name: cart.fileset[name]["metadata"]["weights"] is not None
                      for name in cart.fileset},
            weights=weights,
            labels=cart.labels,
            about=about,
            made=cart._provenance(rows, built),
        )
        written = rvr6.write(output, meta, buf)
    finally:
        if shard_dir is not None:
            shutil.rmtree(shard_dir, ignore_errors=True)
    wall = time.perf_counter() - started
    return {
        "schema": 1,
        "mode": mode,
        "workers": workers,
        "transport": transport,
        "start_method": start_method,
        "schedule": schedule,
        "submission": [f"{task.sample_ordinal}:{task.source_ordinal}" for task in tasks],
        "indexed_entries": indexed_entries,
        "entry_counts_reused": bool(known_entries),
        "step_size": step_size,
        "delay_seed": delay_seed,
        "totals": {
            "wall_seconds": wall,
            "index_seconds": index_seconds,
            "coordinator_cpu_seconds": time.process_time() - cpu_started,
            "aggregate_worker_cpu_seconds": sum(r["worker_cpu_seconds"] for r in results),
            "peak_coordinator_rss_bytes": _rss_bytes(),
            "peak_worker_rss_bytes": max(r["worker_peak_rss_bytes"] for r in results),
            "merge_seconds": merge_seconds,
            "shard_write_seconds": sum(r.get("shard_write_seconds", 0.0) for r in results),
            "shard_load_seconds": shard_load_seconds,
            "shard_bytes": shard_bytes,
        },
        "tasks": [{key: result[key] for key in (
            "sample_ordinal", "source_ordinal", "sample", "worker_wall_seconds",
            "worker_cpu_seconds", "worker_peak_rss_bytes")}
                  for result in results],
        "result": written,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("study")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--json", type=Path, required=True)
    parser.add_argument("--mode", choices=("sample", "source"), required=True)
    parser.add_argument("--workers", type=int, required=True)
    parser.add_argument("--transport", choices=("pickle", "shard"), default="pickle")
    parser.add_argument("--start-method", choices=("fork", "spawn"), default="spawn")
    parser.add_argument("--step-size", default="100 MB")
    parser.add_argument("--built")
    parser.add_argument("--delay-seed", type=int)
    parser.add_argument("--schedule", choices=("declaration", "entries-desc"),
                        default="declaration")
    parser.add_argument("--entries-json", type=Path,
                        help="reuse an indexed_entries mapping from an earlier result")
    args = parser.parse_args(argv)
    if args.workers < 1:
        parser.error("--workers must be positive")
    step_size: int | str = int(args.step_size) if args.step_size.isdigit() else args.step_size
    known_entries = None
    if args.entries_json is not None:
        loaded = json.loads(args.entries_json.read_text(encoding="utf-8"))
        known_entries = loaded.get("indexed_entries", loaded)
    record = run(args.study, args.output, mode=args.mode, workers=args.workers,
                 transport=args.transport, start_method=args.start_method,
                 step_size=step_size, built=args.built, delay_seed=args.delay_seed,
                 schedule=args.schedule, known_entries=known_entries)
    args.json.parent.mkdir(parents=True, exist_ok=True)
    args.json.write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {args.json} — {record['totals']['wall_seconds']:.3f} s")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
