"""Read RowVer 6/7 cartridges and pack new RowVer 7 cartridges.

The container is ``gzip(u32 little-endian JSON length | UTF-8 JSON | column buffer)``.
The header has five drawers: ``about``, ``data``, ``view``, ``tabs``, and ``made``.
``data`` and the buffer are immutable once written; ``FORMAT.md`` and ``FORMAT7.md``
define the rest.
"""
from __future__ import annotations

import argparse
import gzip
import json
import math
import os
import re
import sys
import tempfile
import zlib
from copy import deepcopy
from pathlib import Path
from typing import Any, Container, Iterator, Mapping

import numpy as np

ROWVER = 7
READS = (6, 7)
VERSION = "7.0.0"
MAX_HEADER_BYTES = 8 * 1024 * 1024
MAX_DATA_BYTES = 1024 * 1024 * 1024
MAX_CHIPS = 25
PCTS = (1, 5, 10, 20, 30, 40, 50, 60, 70, 80, 90, 95, 99)
MAX_GRID_REGIONS = 4096
MAX_REGION_NODES = 8192

LEAF_KINDS = frozenset({"1d", "2d", "3d", "board", "map"})
LAYOUTS = frozenset({
    (6,), (3, 3), (2, 2, 2), (3, 3, 6), (6, 3, 3),
    (3, 3, 3, 3), (2, 2, 2, 2, 2, 2),
})
LOCKABLE = ("cuts", "stage", "shown")
NEUTRAL = {"cuts": {}, "stage": [], "shown": None}
NAMESPACES = ("sample", "sub", "group")


class FormatError(ValueError):
    """A cartridge is malformed or contradicts its own column buffer."""


def _fail(message: str) -> None:
    raise FormatError(message)


def _option(value: Any, allowed: Any) -> bool:
    """Membership that treats an unhashable or non-string value as simply not a member."""
    return isinstance(value, str) and value in allowed


def _unique_names(value: Any, allowed: Any) -> bool:
    """True for a list of distinct strings that all name something in ``allowed``."""
    return (isinstance(value, list)
            and all(isinstance(name, str) and name in allowed for name in value)
            and len(set(value)) == len(value))


def _nat(value: Any, where: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        _fail(f"{where} must be a non-negative integer")
    return value


def _finite(value: Any, where: str) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        _fail(f"{where} must be a finite number")
    try:
        out = float(value)
    except OverflowError:
        _fail(f"{where} must be a finite number")
    if not math.isfinite(out):
        _fail(f"{where} must be a finite number")
    return out


def _pair(value: Any, where: str) -> tuple[float, float]:
    if not isinstance(value, list) or len(value) != 2:
        _fail(f"{where} must contain two numbers")
    lo, hi = _finite(value[0], f"{where}[0]"), _finite(value[1], f"{where}[1]")
    if lo > hi:
        _fail(f"{where} is reversed")
    return lo, hi


def _made_source(value: Any, where: str, *, nullable: bool = True) -> None:
    if value is None and nullable:
        return
    if not isinstance(value, dict):
        _fail(f"{where} must be a structured source object")
    if value.get("kind") == "branch":
        if not isinstance(value.get("value"), str) or not value["value"]:
            _fail(f"{where} branch source needs a value")
    elif value.get("kind") == "python":
        if any(not isinstance(value.get(key), str) for key in ("name", "doc", "source")):
            _fail(f"{where} Python source needs name, doc, and source strings")
    else:
        _fail(f"{where} has an unknown source kind")


def _made(made: dict, samples: list[dict], columns: list[dict]) -> None:
    """Validate provenance when present; an empty or redacted drawer remains valid."""
    if not made:
        return
    if "built" in made and not isinstance(made["built"], str):
        _fail("made.built must be text")
    for key in ("keep", "prepare"):
        if key in made:
            _made_source(made[key], f"made.{key}", nullable=key == "prepare")
    if "columns" in made:
        declared = made["columns"]
        expected = {column["name"] for column in columns}
        if not isinstance(declared, dict) or set(declared) != expected:
            _fail("made.columns must describe every data column exactly once")
        for name, source in declared.items():
            _made_source(source, f'made.columns["{name}"]', nullable=False)
    if "samples" in made:
        _made_samples(made["samples"], samples)
    if "cart" in made and made["cart"] is not None:
        cart = made["cart"]
        if not isinstance(cart, dict):
            _fail("made.cart must be an object or null")
        for key in ("path", "sha256", "source"):
            if key in cart and not isinstance(cart[key], str):
                _fail(f"made.cart.{key} must be text")


def _made_samples(records: Any, samples: list[dict]) -> None:
    if not isinstance(records, list) or len(records) != len(samples):
        _fail("made.samples must describe every data sample exactly once")
    expected = {sample["name"]: sample["rows"] for sample in samples}
    seen: set[str] = set()
    for i, record in enumerate(records):
        if not isinstance(record, dict):
            _fail(f"made.samples[{i}] must be an object")
        name = record.get("name")
        if (not isinstance(name, str) or name not in expected or name in seen
                or isinstance(record.get("rows"), bool)
                or record.get("rows") != expected.get(name)):
            _fail("made.samples names and row counts must match data.samples")
        seen.add(name)
        files = record.get("files")
        if not isinstance(files, list) or any(
                not isinstance(file, dict) or not isinstance(file.get("path"), str)
                or not file["path"] or not isinstance(file.get("tree"), str)
                or not file["tree"] for file in files):
            _fail(f'made sample "{name}" has invalid files')
        if not isinstance(record.get("metadata"), dict):
            _fail(f'made sample "{name}" metadata must be an object')
        if "weights" not in record:
            _fail(f'made sample "{name}" needs a weights declaration (null = unweighted)')
        _made_source(record["weights"], f'made sample "{name}" weights')


def _wire_clean(value: Any) -> str | None:
    """Why a header would not survive a JavaScript reader intact, or None when it would."""
    seen: set[int] = set()
    stack = [value]
    while stack:
        item = stack.pop()
        if isinstance(item, dict):
            if id(item) in seen:
                continue
            seen.add(id(item))
            for key, inner in item.items():
                if not isinstance(key, str):
                    return "header mapping keys must be strings"
                stack.append(inner)
        elif isinstance(item, (list, tuple)):
            if id(item) in seen:
                continue
            seen.add(id(item))
            stack.extend(item)
        elif isinstance(item, bool):
            continue
        elif isinstance(item, int) and abs(item) > 2 ** 53:
            return "header integers must fit exactly in float64 (absolute value <= 2**53)"
    return None


def frame(meta: dict, buf: bytes) -> bytes:
    """Return the uncompressed frame, refusing headers a JavaScript reader would alter."""
    problem = _wire_clean(meta)
    if problem:
        _fail(problem)
    try:
        header = json.dumps(meta, allow_nan=False, separators=(",", ":")).encode("utf-8")
    except (TypeError, ValueError, RecursionError, UnicodeEncodeError) as exc:
        raise FormatError(f"header is not finite JSON: {exc}") from exc
    if not header or len(header) > MAX_HEADER_BYTES:
        _fail(f"header exceeds the {MAX_HEADER_BYTES}-byte limit")
    return len(header).to_bytes(4, "little") + header + bytes(buf)


def _wire_float(text: str) -> float:
    """Parse a JSON float, refusing the overflow that Python reads as inf and a save as null."""
    out = float(text)
    if not math.isfinite(out):
        raise ValueError("number overflows float64")
    return out


def _wire_int(text: str) -> int:
    """Parse a JSON integer, refusing values a JavaScript reader would round."""
    out = int(text)
    if abs(out) > 2 ** 53:
        raise ValueError("integer exceeds the float64-exact range")
    return out


def unframe(raw: bytes) -> tuple[dict, bytes]:
    """Split an uncompressed frame into metadata and its untouched column buffer."""
    if len(raw) < 4:
        _fail("file is too short to hold a header")
    size = int.from_bytes(raw[:4], "little")
    if size <= 0 or size > MAX_HEADER_BYTES:
        _fail(f"invalid header length {size}")
    if 4 + size > len(raw):
        _fail(f"header says {size} bytes, which do not fit in the file")
    try:
        meta = json.loads(
            raw[4:4 + size].decode("utf-8"),
            parse_constant=lambda value: _fail(f"header contains {value}"),
            parse_float=_wire_float, parse_int=_wire_int,
        )
    except (UnicodeDecodeError, ValueError, RecursionError) as exc:
        raise FormatError("header is not valid UTF-8 JSON") from exc
    if not isinstance(meta, dict):
        _fail("header must be a JSON object")
    return meta, raw[4 + size:]


def _cut(cut: Any, where: str) -> None:
    if isinstance(cut, list):
        if len(cut) != 2:
            _fail(f"{where} range must have two ends")
        ends = []
        for i, value in enumerate(cut):
            ends.append(None if value is None else _finite(value, f"{where}[{i}]"))
        if ends[0] is not None and ends[1] is not None and ends[0] > ends[1]:
            _fail(f"{where} range is reversed")
        return
    if not isinstance(cut, dict) or set(cut) != {"in"} or not isinstance(cut["in"], list):
        _fail(f"{where} must be a range or an {{'in': [...]}} selection")
    for i, value in enumerate(cut["in"]):
        _finite(value, f"{where}.in[{i}]")


def _cuts(cuts: Any, columns: Mapping[str, dict], where: str) -> None:
    if cuts is None:
        return
    if not isinstance(cuts, dict):
        _fail(f"{where} cuts must be an object")
    for name, cut in cuts.items():
        if name not in columns:
            _fail(f'{where} cuts on "{name}", which is not a column')
        categorical = columns[name].get("vals") is not None
        if categorical and not isinstance(cut, dict):
            _fail(f'{where} must use {{"in": [...]}} for categorical column "{name}"')
        if not categorical and not isinstance(cut, list):
            _fail(f'{where} must use a range for continuous column "{name}"')
        _cut(cut, f'{where} cut "{name}"')
        if categorical:
            values = cut["in"]
            allowed = set(columns[name]["vals"])
            if (len(set(values)) != len(values)
                    or any(isinstance(value, bool) or not isinstance(value, int)
                           or value not in allowed for value in values)):
                _fail(f'{where} cut "{name}" contains duplicate or unknown values')


def _shown(shown: Any, samples: set[str], where: str) -> None:
    if shown is None:
        return
    if not isinstance(shown, list) or any(not isinstance(x, str) for x in shown):
        _fail(f"{where} shown list must contain sample names")
    if len(set(shown)) != len(shown) or any(x not in samples for x in shown):
        _fail(f"{where} shown list contains duplicates or unknown samples")


def _stage_kind(value: Any, where: str) -> str:
    if not isinstance(value, dict) or not isinstance(value.get("kind"), str):
        _fail(f"{where} has an invalid stage item")
    return value["kind"]


def _stage(stage: Any, where: str, columns: Mapping[str, dict], sources: set[str]) -> None:
    """Validate a stage: leaves and one-level blocks of leaves."""
    if stage is None:
        return
    if not isinstance(stage, list):
        _fail(f"{where} stage must be a list")
    singletons = {"board": 0, "map": 0}
    for entry in stage:
        if _stage_kind(entry, where) != "block":
            _leaf(entry, where, columns, sources, singletons)
            continue
        cells, layout = entry.get("cells"), entry.get("layout")
        if not isinstance(cells, list) or not 1 <= len(cells) <= 6 or not any(c is not None for c in cells):
            _fail(f"{where} block must have one to six cells and at least one leaf")
        if (not isinstance(layout, list) or len(layout) != len(cells)
                or any(isinstance(x, bool) or not isinstance(x, int) for x in layout)
                or tuple(layout) not in LAYOUTS):
            _fail(f"{where} block has an invalid layout")
        for cell in cells:
            if cell is not None:
                _leaf(cell, where, columns, sources, singletons)


def _leaf(value: Any, where: str, columns: Mapping[str, dict], sources: set[str],
          singletons: dict[str, int]) -> None:
    """Validate one plot, board, or map; a block here is a nested block."""
    kind = _stage_kind(value, where)
    if kind not in LEAF_KINDS:
        _fail(f"{where} has an unknown or nested stage kind {kind!r}")
    if kind in singletons:
        singletons[kind] += 1
        if singletons[kind] > 1:
            _fail(f"{where} contains more than one {kind}")
    if kind == "board":
        _board(value, where, sources)
    elif kind == "map":
        _map(value, where, columns, sources)
    else:
        _plot(value, kind, where, columns, sources)


def _plot(value: dict, kind: str, where: str, columns: Mapping[str, dict], sources: set[str]) -> None:
    if not _option(value.get("varx"), columns):
        _fail(f"{where} {kind} plot names an unknown x column")
    if kind in {"2d", "3d"} and not _option(value.get("vary"), columns):
        _fail(f"{where} {kind} plot names an unknown y column")
    for slot in ("srcs", "mute"):
        if slot in value and not _unique_names(value[slot], sources):
            _fail(f"{where} {kind} plot has an invalid {slot} list")


def _board(value: dict, where: str, sources: set[str]) -> None:
    rows = value.get("rows", 2)
    cols = value.get("cols", 2)
    if (isinstance(rows, bool) or not isinstance(rows, int) or not 1 <= rows <= 4
            or isinstance(cols, bool) or not isinstance(cols, int) or not 1 <= cols <= 4):
        _fail(f"{where} board has an invalid shape")
    if "cells" in value:
        cells = value["cells"]
        if (not isinstance(cells, list) or len(cells) > rows * cols
                or any(cell is not None and (not isinstance(cell, str) or cell not in sources)
                       for cell in cells)):
            _fail(f"{where} board has invalid cells")
    if "bkgs" in value and not _unique_names(value["bkgs"], sources):
        _fail(f"{where} board has invalid backgrounds")
    if not _option(value.get("channel", "yield"), {"yield", "metrics"}):
        _fail(f"{where} board has an invalid channel")
    if not _option(value.get("err", "poisson_neff"),
                   {"off", "sumw2", "sqrtn", "poisson_neff", "poisson_raw"}):
        _fail(f"{where} board has an invalid uncertainty model")
    if "met" in value and not _unique_names(value["met"], {"sob", "sig", "zbi"}):
        _fail(f"{where} board has invalid metrics")


def _map(value: dict, where: str, columns: Mapping[str, dict], sources: set[str]) -> None:
    for key in ("srcs", "bkg"):
        if key in value and not _unique_names(value[key], sources):
            _fail(f"{where} map has invalid {key}")
    if "sig" in value and (not isinstance(value["sig"], str) or value["sig"] not in sources):
        _fail(f"{where} map has an unknown signal source")
    if "metric" in value and not _option(value["metric"], {"yields", "sb", "zbi", "neff"}):
        _fail(f"{where} map has an invalid metric")
    if "flip" in value and not isinstance(value["flip"], bool):
        _fail(f"{where} map flip must be boolean")
    if "floor" in value and _finite(value["floor"], f"{where} map floor") < 0:
        _fail(f"{where} map floor must be non-negative")
    if "grid" in value:
        _grid(value["grid"], where, columns)
    _region_tree(value.get("root"), where, columns)


def _grid(grid: Any, where: str, columns: Mapping[str, dict]) -> None:
    if not isinstance(grid, dict) or not isinstance(grid.get("vars", []), list):
        _fail(f"{where} map has an invalid grid")
    if "exhaustive" in grid and type(grid["exhaustive"]) is not bool:
        _fail(f"{where} map grid exhaustive must be boolean")
    variables = grid.get("vars", [])
    if len(variables) > 32:
        _fail(f"{where} map grid has too many variables")
    regions = 1
    seen: set[str] = set()
    for variable in variables:
        if (not isinstance(variable, dict)
                or not _option(variable.get("col"), columns)
                or not isinstance(variable.get("bands"), list)
                or len(variable["bands"]) > 64):
            _fail(f"{where} map grid has an invalid variable")
        if variable["col"] in seen:
            _fail(f"{where} map grid repeats a variable")
        seen.add(variable["col"])
        regions *= len(variable["bands"]) + 1
        if regions > MAX_GRID_REGIONS:
            _fail(f"{where} map grid would create too many regions")
        _bands(variable["bands"], columns[variable["col"]], where)


def _bands(bands: list, column: dict, where: str) -> None:
    """Categorical bands are disjoint value lists; numeric bands are increasing thresholds."""
    allowed = set(column.get("vals") or [])
    taken: set[int] = set()
    thresholds: set[float] = set()
    for band in bands:
        if isinstance(band, list):
            if (not allowed or not band or len(band) > MAX_CHIPS
                    or any(isinstance(member, bool) or not isinstance(member, int)
                           or member not in allowed or member in taken for member in band)
                    or len(set(band)) != len(band)):
                _fail(f"{where} map grid has an invalid categorical band")
            taken.update(band)
        else:
            threshold = _finite(band, f"{where} map grid threshold")
            if thresholds and threshold <= max(thresholds):
                _fail(f"{where} map grid thresholds must increase")
            thresholds.add(threshold)


def _region_cut(cut: Any, where: str, columns: Mapping[str, dict]) -> None:
    if not isinstance(cut, dict) or not _option(cut.get("col"), columns):
        _fail(f"{where} map cuts on an unknown column")
    if set(cut) == {"col", "lt"}:
        _finite(cut["lt"], f"{where} map threshold")
    elif set(cut) == {"col", "in"} and isinstance(cut["in"], list):
        allowed = set(columns[cut["col"]].get("vals") or [])
        selected = cut["in"]
        if (not allowed or not selected or len(selected) > MAX_CHIPS
                or any(isinstance(member, bool) or not isinstance(member, int)
                       or member not in allowed for member in selected)
                or len(set(selected)) != len(selected)):
            _fail(f"{where} map has an invalid categorical cut")
    else:
        _fail(f"{where} map has an invalid region cut")


def _region_tree(root: Any, where: str, columns: Mapping[str, dict]) -> None:
    """Validate a binary cut tree of at most 64 levels and MAX_REGION_NODES nodes."""
    count = 0

    def node(root: Any, depth: int) -> None:
        nonlocal count
        if root is None:
            return
        if depth > 64:
            _fail(f"{where} map region tree is too deep")
        count += 1
        if count > MAX_REGION_NODES:
            _fail(f"{where} map region tree has too many nodes")
        if not isinstance(root, dict) or set(root) - {"cut", "yes", "no"}:
            _fail(f"{where} map has an invalid region node")
        _region_cut(root.get("cut"), where, columns)
        node(root.get("yes"), depth + 1)
        node(root.get("no"), depth + 1)

    node(root, 0)


def _bit_values(buf: bytes | bytearray | memoryview, at: int, rows: int) -> list[int]:
    """Validate tail bits and find distinct values without unpacking event arrays."""
    if not rows:
        return []
    packed = np.frombuffer(buf, dtype=np.uint8, count=(rows + 7) // 8, offset=at)
    tail = rows % 8
    if tail and int(packed[-1]) >> tail:
        _fail("boolean column has nonzero unused tail bits")
    any_true = bool(np.any(packed))
    all_true = bool(np.all(packed[:-1] == 255)) and int(packed[-1]) == (
        (1 << tail) - 1 if tail else 255)
    return ([0] if not all_true else []) + ([1] if any_true else [])


def _samples(samples: list, rows: int) -> tuple[set[str], list[int], list[bool]]:
    """Validate data.samples; return their names, row counts, and weighted flags."""
    names: set[str] = set()
    counts: list[int] = []
    weighted: list[bool] = []
    for i, sample in enumerate(samples):
        if not isinstance(sample, dict):
            _fail(f"data.samples[{i}] must be an object")
        name = sample.get("name")
        if not isinstance(name, str) or not name.strip() or name in names:
            _fail(f"sample {i} has an empty or duplicate name")
        names.add(name)
        counts.append(_nat(sample.get("rows"), f'sample "{name}" rows'))
        if type(sample.get("weighted")) is not bool:
            _fail(f'sample "{name}" does not say whether it is weighted')
        weighted.append(sample["weighted"])
    if sum(counts) != rows:
        _fail(f"samples sum to {sum(counts)} rows; data declares {rows}")
    return names, counts, weighted


def _weight_col(data: dict, weighted: list[bool]) -> str | None:
    weight_col = data.get("weight_col")
    if weight_col is not None and (not isinstance(weight_col, str) or not weight_col.strip()):
        _fail("data.weight_col must be a non-empty string or null")
    if any(weighted) and weight_col is None:
        _fail("weighted samples require a weights column")
    if not any(weighted) and weight_col is not None:
        _fail("a data-only file must not declare a weights column")
    return weight_col


def _column(col: Any, i: int, rows: int, size: int, version: int,
            names: Container[str]) -> tuple[str, int, int]:
    """Validate one column descriptor; return its name, offset, and byte length."""
    if not isinstance(col, dict):
        _fail(f"data.cols[{i}] must be an object")
    name, kind = col.get("name"), col.get("kind")
    if not isinstance(name, str) or not name.strip() or name in names:
        _fail(f"column {i} has an empty or duplicate name")
    if kind not in (("f8", "i4", "b1") if version == 7 else ("f8", "i4")):
        _fail(f'column "{name}" has unknown kind {kind!r}')
    width = 1 if kind == "b1" else 8 if kind == "f8" else 4
    length = (rows + 7) // 8 if kind == "b1" else width * rows
    at = _nat(col.get("at"), f'column "{name}" offset')
    if at % width or at + length > size:
        _fail(f'column "{name}" is unaligned or outside the buffer')
    lim = _pair(col.get("lim"), f'column "{name}" lim')
    nice = _pair(col.get("nice"), f'column "{name}" nice')
    pcts = [_finite(col.get(f"p{q}"), f'column "{name}" p{q}') for q in PCTS]
    if nice[0] > lim[0] or nice[1] < lim[1]:
        _fail(f'column "{name}" nice range does not enclose lim')
    chain = [lim[0], *pcts, lim[1]]
    if any(a > b for a, b in zip(chain, chain[1:])):
        _fail(f'column "{name}" percentiles are outside lim or reversed')
    vals = col.get("vals")
    if vals is not None:
        if kind not in ("i4", "b1") or not isinstance(vals, list) or len(vals) > MAX_CHIPS:
            _fail(f'column "{name}" has invalid categorical values')
        if (any(isinstance(x, bool) or not isinstance(x, int) for x in vals)
                or len(set(vals)) != len(vals)):
            _fail(f'column "{name}" categorical values must be unique integers')
    return name, at, length


def _columns(cols: list, rows: int, size: int, weight_col: str | None,
             version: int) -> tuple[dict[str, dict], list[tuple[int, int, str]]]:
    """Validate data.cols; return descriptors by name and their byte ranges."""
    by_column: dict[str, dict] = {}
    ranges: list[tuple[int, int, str]] = []
    declared_bytes = rows * (4 + (8 if weight_col else 0))
    for i, col in enumerate(cols):
        name, at, length = _column(col, i, rows, size, version, by_column)
        by_column[name] = col
        ranges.append((at, at + length, f'column "{name}"'))
        declared_bytes += length
    if weight_col in by_column:
        _fail(f'{weight_col!r} is both the weights column and a data column')
    if declared_bytes != size:
        _fail(f"column declarations need {declared_bytes} bytes; data declares {size}")
    return by_column, ranges


def _offsets(data: dict, rows: int, size: int, weight_col: str | None,
             ranges: list[tuple[int, int, str]]) -> None:
    """Place the weights and sample index; refuse any overlap between buffer ranges."""
    if weight_col is not None:
        weight_at = _nat(data.get("weight_at"), "data.weight_at")
        if weight_at % 8 or weight_at + 8 * rows > size:
            _fail("weights are unaligned or outside the buffer")
        ranges.append((weight_at, weight_at + 8 * rows, "weights"))
    elif "weight_at" in data:
        _fail("an unweighted file must not declare data.weight_at")
    sample_at = data["sample_at"]
    if sample_at % 4 or sample_at + 4 * rows > size:
        _fail("sample index is unaligned or outside the buffer")
    ranges.append((sample_at, sample_at + 4 * rows, "sample index"))
    ranges.sort()
    for previous, current in zip(ranges, ranges[1:]):
        if current[0] < previous[1]:
            _fail(f"{current[2]} overlaps {previous[2]}")


def _payload(buf: bytes | bytearray | memoryview, data: dict, rows: int,
             sample_rows: list[int], weight_col: str | None) -> None:
    """Check the buffer against its descriptors: sample counts, finite weights, lim and vals."""
    slab = memoryview(buf)
    count = len(sample_rows)
    index = np.frombuffer(slab, dtype="<i4", count=rows, offset=data["sample_at"])
    if rows and (np.any(index < 0) or np.any(index >= count)):
        bad = int(index[(index < 0) | (index >= count)][0])
        _fail(f"sample index contains invalid value {bad}")
    counts = np.bincount(index, minlength=count) if rows else np.zeros(count, dtype=np.int64)
    if counts.tolist() != sample_rows:
        _fail(f"sample index counts {counts.tolist()} do not match {sample_rows}")
    if weight_col is not None:
        weights = np.frombuffer(slab, dtype="<f8", count=rows, offset=data["weight_at"])
        if not np.isfinite(weights).all():
            _fail("weights contain NaN or infinity")
    for col in data["cols"]:
        _column_payload(slab, col, rows)


def _column_payload(slab: memoryview, col: dict, rows: int) -> None:
    if col["kind"] == "b1":
        actual_vals = _bit_values(slab, col["at"], rows)
        actual = [actual_vals[0], actual_vals[-1]] if rows else [0, 0]
        if col["lim"] != actual:
            _fail(f'column "{col["name"]}" lim does not match its payload')
        if col.get("vals") is not None and col["vals"] != actual_vals:
            _fail(f'column "{col["name"]}" vals do not match its payload')
        return
    values = np.frombuffer(slab, dtype="<" + col["kind"], count=rows, offset=col["at"])
    if col["kind"] == "f8" and not np.isfinite(values).all():
        _fail(f'column "{col["name"]}" contains NaN or infinity')
    if rows:
        actual = [float(values.min()), float(values.max())]
        if [float(x) for x in col["lim"]] != actual:
            _fail(f'column "{col["name"]}" lim does not match its payload')
    if col.get("vals") is not None:
        actual_vals = [int(x) for x in np.unique(values)]
        if col["vals"] != actual_vals:
            _fail(f'column "{col["name"]}" vals do not match its payload')


def _groups(groups: Any, sample_names: set[str]) -> tuple[set[str], set[str]]:
    """Validate view.groups; return the group and subgroup names it introduces."""
    if groups is None:
        return set(), set()
    if not isinstance(groups, dict):
        _fail("view.groups must be an object")
    for key in ("of", "sub"):
        mapping = groups.get(key, {})
        if not isinstance(mapping, dict) or any(k not in sample_names or not isinstance(v, str)
                                                or not v.strip()
                                                for k, v in mapping.items()):
            _fail(f"view.groups.{key} must map known samples to names")
    group_names = set(groups.get("of", {}).values())
    sub_names = set(groups.get("sub", {}).values())
    if "order" in groups:
        order = groups["order"]
        if (not isinstance(order, list)
                or any(not isinstance(x, str) or not x.strip() for x in order)
                or len(set(order)) != len(order)):
            _fail("view.groups.order must contain unique non-empty group names")
        group_names.update(order)
    if sample_names & group_names or sample_names & sub_names or group_names & sub_names:
        _fail("sample, group, and subgroup names must be unique across namespaces")
    return group_names, sub_names


def _namespaced(value: Any, where: str) -> Iterator[tuple[str, str, Any]]:
    """Yield ``(namespace, name, item)`` from a view mapping keyed by sample, sub, and group."""
    if not isinstance(value, dict) or set(value) - set(NAMESPACES):
        _fail(f"{where} must have sample, sub, and group namespaces")
    for namespace, mapping in value.items():
        if not isinstance(mapping, dict):
            _fail(f"{where}.{namespace} must be an object")
        for name, item in mapping.items():
            yield namespace, name, item


def _lumis(lumis: Any, allowed: Mapping[str, set[str]]) -> None:
    if lumis is None:
        return
    for namespace, name, value in _namespaced(lumis, "view.lumis"):
        if name not in allowed[namespace] or value is not None and _finite(
                value, f"view.lumis.{namespace}.{name}") <= 0:
            _fail(f"view.lumis.{namespace}.{name} must name a known source and be positive or null")


def _look(look: Any, allowed: Mapping[str, set[str]]) -> None:
    if look is None:
        return
    for namespace, name, style in _namespaced(look, "view.look"):
        where = f"view.look.{namespace}.{name}"
        if name not in allowed[namespace] or not isinstance(style, dict):
            _fail(f"{where} must style a known source")
        _style(style, where)


def _style(style: dict, where: str) -> None:
    if "dash" in style and style["dash"] not in ("solid", "dash", "dot"):
        _fail(f"{where} dash must be solid, dash, or dot")
    if "lw" in style:
        lw = style["lw"]
        if (isinstance(lw, bool) or not isinstance(lw, (int, float))
                or not math.isfinite(lw) or lw <= 0 or lw > 32):
            _fail(f"{where} lw must be a positive number up to 32")
    if "role" in style and style["role"] not in ("signal", "background", "data"):
        _fail(f"{where} role must be signal, background, or data")
    if "errstyle" in style and style["errstyle"] not in ("ticks", "none"):
        _fail(f"{where} errstyle must be ticks or none")
    if "binwidth_indicator" in style and not isinstance(style["binwidth_indicator"], bool):
        _fail(f"{where} binwidth_indicator must be boolean")
    if "colour" in style:
        colour = style["colour"]
        if not isinstance(colour, str) or re.fullmatch(r"#[0-9a-fA-F]{6}", colour) is None:
            _fail(f"{where}.colour must be #RRGGBB")
    if "draw" in style and not _option(style["draw"], {"bars", "points"}):
        _fail(f"{where}.draw must be bars or points")


def _view(view: dict, columns: Mapping[str, dict], sample_names: set[str]) -> set[str]:
    """Validate the locked view; return every source name a stage may cite."""
    if "cuts" in view and view["cuts"] is None:
        _fail("locked view cuts may not be null")
    if "stage" in view and view["stage"] is None:
        _fail("locked view stage may not be null")
    _cuts(view.get("cuts"), columns, "locked view")
    _shown(view.get("shown"), sample_names, "locked view")
    group_names, sub_names = _groups(view.get("groups"), sample_names)
    sources = sample_names | group_names | sub_names
    _stage(view.get("stage"), "locked view", columns, sources)
    allowed = {"sample": sample_names, "sub": sub_names, "group": group_names}
    _lumis(view.get("lumis"), allowed)
    if "lumi" in view:
        _fail("view.lumi is not part of .rvr 6; use namespaced view.lumis")
    _look(view.get("look"), allowed)
    return sources


def _tabs(tabs: list, columns: Mapping[str, dict], sample_names: set[str],
          sources: set[str]) -> None:
    names: set[str] = set()
    for i, tab in enumerate(tabs):
        if not isinstance(tab, dict):
            _fail(f"tab {i + 1} must be an object")
        name = tab.get("name")
        if not isinstance(name, str) or not name.strip() or name in names:
            _fail(f"tab {i + 1} has an empty or duplicate name")
        names.add(name)
        if "cuts" in tab and tab["cuts"] is None:
            _fail(f'tab "{name}" cuts may not be null')
        if "stage" in tab and tab["stage"] is None:
            _fail(f'tab "{name}" stage may not be null')
        _cuts(tab.get("cuts"), columns, f'tab "{name}"')
        _shown(tab.get("shown"), sample_names, f'tab "{name}"')
        _stage(tab.get("stage"), f'tab "{name}"', columns, sources)
        defaults = tab.get("plot")
        if isinstance(defaults, dict):
            for slot in ("srcs", "mute"):
                if slot in defaults and not _unique_names(defaults[slot], sources):
                    _fail(f'tab "{name}" plot defaults carry an invalid {slot} list')


def check(meta: dict, buf: bytes | bytearray | memoryview) -> None:
    """Validate a v6 or v7 header against its buffer; boolean payloads stay packed."""
    if not isinstance(meta, dict) or meta.get("rowver") not in READS:
        _fail("RowVer supports .rvr 6 and 7")
    for drawer, kind in (("about", dict), ("data", dict), ("view", dict),
                         ("tabs", list), ("made", dict)):
        if not isinstance(meta.get(drawer), kind):
            _fail(f"header has no valid {drawer!r} drawer")
    about, data, view, tabs = meta["about"], meta["data"], meta["view"], meta["tabs"]
    if not isinstance(about.get("name"), str) or not about["name"].strip():
        _fail("about.name must be a non-empty string")

    rows = _nat(data.get("rows"), "data.rows")
    size = _nat(data.get("bytes"), "data.bytes")
    _nat(data.get("sample_at"), "data.sample_at")
    if size > MAX_DATA_BYTES:
        _fail(f"column data exceeds the {MAX_DATA_BYTES}-byte limit")
    if len(buf) != size:
        _fail(f"buffer is {len(buf)} bytes; data declares {size}")
    samples, cols = data.get("samples"), data.get("cols")
    if not isinstance(samples, list) or not samples:
        _fail("data.samples must be a non-empty list")
    if not isinstance(cols, list) or not cols:
        _fail("data.cols must be a non-empty list")

    sample_names, sample_rows, weighted = _samples(samples, rows)
    weight_col = _weight_col(data, weighted)
    by_column, ranges = _columns(cols, rows, size, weight_col, meta["rowver"])
    _offsets(data, rows, size, weight_col, ranges)
    _payload(buf, data, rows, sample_rows, weight_col)

    sources = _view(view, by_column, sample_names)
    _tabs(tabs, by_column, sample_names, sources)
    if "opens_on" in meta:
        opens = _nat(meta["opens_on"], "opens_on")
        if (tabs and opens >= len(tabs)) or (not tabs and opens != 0):
            _fail("opens_on does not name an existing tab")
    _made(meta["made"], samples, cols)


def read(path: str | Path) -> tuple[dict, bytes]:
    """Read, bound, and validate a gzip-compressed cartridge."""
    try:
        with gzip.open(path, "rb") as stream:
            raw = stream.read(MAX_HEADER_BYTES + MAX_DATA_BYTES + 5)
            if stream.read(1):
                _fail("cartridge exceeds the size limit")
    except (OSError, EOFError, zlib.error) as exc:
        raise FormatError(f"cannot decompress cartridge: {exc}") from exc
    meta, buf = unframe(raw)
    check(meta, buf)
    return meta, buf


def write(path: str | Path, meta: dict, buf: bytes) -> dict:
    """Atomically write a validated cartridge at gzip level 6, preserving ``buf`` byte-for-byte."""
    check(meta, buf)
    target = Path(path)
    target.parent.mkdir(parents=True, exist_ok=True)
    payload = gzip.compress(frame(meta, buf), compresslevel=6, mtime=0)
    handle, temporary = tempfile.mkstemp(prefix=f".{target.name}.", dir=target.parent)
    try:
        with os.fdopen(handle, "wb") as stream:
            stream.write(payload)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, target)
    except BaseException:
        try:
            os.unlink(temporary)
        except FileNotFoundError:
            pass
        raise
    return {"file": str(target), "rows": meta["data"]["rows"],
            "bytes": len(buf), "compressed": len(payload)}


def _array(value: Any, where: str) -> np.ndarray:
    if np.ma.isMaskedArray(value):
        _fail(f"{where} is masked; fill or reject missing values explicitly")
    try:
        out = np.asarray(value)
    except (TypeError, ValueError) as exc:
        raise FormatError(f"{where} is not a regular numeric array") from exc
    if out.ndim != 1:
        _fail(f"{where} must contain exactly one number per event")
    if out.dtype.kind not in "biuf":
        _fail(f"{where} is not numeric")
    if out.dtype.kind == "f" and not np.isfinite(out).all():
        _fail(f"{where} contains NaN or infinity")
    return out


def _kind(arrays: list[np.ndarray]) -> str:
    """The stored kind for one column; an empty float array (a sample with no entries) never widens flags."""
    populated = [a for a in arrays if a.size]
    if all(a.dtype.kind == "b" for a in (populated or arrays)):
        return "b1"
    integral = all(a.dtype.kind in "biu" for a in arrays)
    if integral and all(not a.size or (a.min() >= np.iinfo(np.int32).min
                                       and a.max() <= np.iinfo(np.int32).max) for a in arrays):
        return "i4"
    if integral and any(a.size and (a.min() < -(2 ** 53) or a.max() > 2 ** 53)
                        for a in arrays):
        _fail("integer columns outside int32 must fit exactly in float64 (absolute value <= 2**53)")
    return "f8"


def _bump(value: float, up: bool) -> float:
    """Round outward to two significant digits; a subnormal is its own bound."""
    if value == 0:
        return 0.0
    scale = 10 ** (math.floor(math.log10(abs(value))) - 1)
    if scale == 0:
        return value
    return (math.ceil(value / scale) if up else math.floor(value / scale)) * scale


def _stats(values: np.ndarray, integer: bool) -> dict:
    if not values.size:
        return {"lim": [0, 0], "nice": [0, 1],
                **{f"p{q}": 0 for q in PCTS},
                **({"vals": []} if integer else {})}
    lo, hi = float(values.min()), float(values.max())
    nice_lo, nice_hi = _bump(lo, False), _bump(hi, True)
    if integer:
        nice_lo, nice_hi = math.floor(nice_lo), math.ceil(nice_hi)
    if nice_lo == nice_hi:
        nice_hi = nice_lo + 1
    out = {"lim": [lo, hi], "nice": [nice_lo, nice_hi],
           **{f"p{q}": float(np.percentile(values, q)) for q in PCTS}}
    if integer:
        unique = np.unique(values)
        if len(unique) <= MAX_CHIPS:
            out["vals"] = [int(x) for x in unique]
    return out


def _pack_inputs(by_sample: Mapping[str, Mapping[str, Any]], weighted: Mapping[str, bool]
                 ) -> tuple[list[str], list[str], dict[str, dict[str, np.ndarray]], dict[str, int]]:
    """Validate the samples and columns; return names, arrays by sample, and row counts."""
    if not isinstance(by_sample, Mapping) or not by_sample:
        _fail("a cartridge needs at least one sample")
    sample_names = list(by_sample)
    if any(not isinstance(name, str) or not name.strip() for name in sample_names):
        _fail("sample names must be non-empty strings")
    if set(weighted) != set(sample_names):
        _fail("weighted declarations must name every sample exactly once")
    if any(type(weighted[name]) is not bool for name in sample_names):
        _fail("every weighted declaration must be boolean")
    first = by_sample[sample_names[0]]
    if not isinstance(first, Mapping) or not first:
        _fail("a cartridge needs at least one column")
    column_names = list(first)
    if len(set(column_names)) != len(column_names) or any(
            not isinstance(name, str) or not name.strip() or name == "weight"
            for name in column_names):
        _fail("column names must be unique, non-empty, and may not be 'weight'")
    arrays: dict[str, dict[str, np.ndarray]] = {}
    row_counts: dict[str, int] = {}
    for sample in sample_names:
        values = by_sample[sample]
        if not isinstance(values, Mapping) or list(values) != column_names:
            _fail(f'sample "{sample}" does not have the same ordered columns')
        arrays[sample] = {name: _array(values[name], f'{sample}.{name}') for name in column_names}
        lengths = {len(value) for value in arrays[sample].values()}
        if len(lengths) != 1:
            _fail(f'sample "{sample}" columns have different lengths')
        row_counts[sample] = lengths.pop()
    return sample_names, column_names, arrays, row_counts


def _pack_weights(sample_names: list[str], row_counts: Mapping[str, int],
                  weighted: Mapping[str, bool], weights: Mapping[str, Any] | None
                  ) -> dict[str, np.ndarray]:
    """Weight arrays for every sample (ones where unweighted), or empty for a data-only file."""
    if not any(weighted.values()):
        if weights and any(name in weights for name in sample_names):
            _fail("a data-only cartridge may not carry weight arrays")
        return {}
    if weights is None:
        _fail("weighted samples require weight arrays")
    out: dict[str, np.ndarray] = {}
    for sample in sample_names:
        if not weighted[sample]:
            out[sample] = np.ones(row_counts[sample], dtype=np.float64)
            continue
        if sample not in weights:
            _fail(f'weighted sample "{sample}" has no weight array')
        value = _array(weights[sample], f'{sample}.weight').astype(np.float64, copy=False)
        if len(value) != row_counts[sample]:
            _fail(f'{sample}.weight length does not match its columns')
        if not np.isfinite(value).all():
            _fail(f'{sample}.weight contains NaN or infinity')
        out[sample] = value
    return out


def _pack_buffer(sample_names: list[str], column_names: list[str],
                 arrays: Mapping[str, Mapping[str, np.ndarray]], row_counts: Mapping[str, int],
                 weighted: Mapping[str, bool], weight_arrays: Mapping[str, np.ndarray]
                 ) -> tuple[dict, bytes]:
    """Lay out weights, numeric columns widest first, the sample index, then packed bits."""
    rows = sum(row_counts.values())
    pieces: list[bytes] = []
    offset = 0
    data: dict[str, Any] = {
        "rows": rows,
        "bytes": 0,
        "weight_col": "weight" if weight_arrays else None,
        "sample_at": 0,
        "samples": [{"name": name, "rows": row_counts[name],
                     "weighted": weighted[name]} for name in sample_names],
        "cols": [],
    }
    if weight_arrays:
        data["weight_at"] = offset
        combined = np.concatenate([weight_arrays[name] for name in sample_names])
        raw = combined.astype("<f8", copy=False).tobytes()
        pieces.append(raw)
        offset += len(raw)

    kinds = {name: _kind([arrays[sample][name] for sample in sample_names])
             for name in column_names}
    entries: dict[str, dict] = {}
    bool_pieces: list[tuple[str, bytes]] = []
    for name in sorted(column_names, key=lambda name: {"f8": 0, "i4": 1, "b1": 2}[kinds[name]]):
        kind = kinds[name]
        combined = (np.concatenate([arrays[sample][name] for sample in sample_names])
                    if rows else np.empty(0))
        dtype = "u1" if kind == "b1" else "<i4" if kind == "i4" else "<f8"
        encoded = combined.astype(dtype, copy=False)
        entries[name] = {"name": name, "kind": kind, "at": offset,
                         **_stats(encoded, kind in ("i4", "b1"))}
        if kind == "b1":
            bool_pieces.append((name, np.packbits(encoded, bitorder="little").tobytes()))
            continue
        raw = encoded.tobytes()
        pieces.append(raw)
        offset += len(raw)

    data["sample_at"] = offset
    index = np.concatenate([
        np.full(row_counts[name], i, dtype="<i4") for i, name in enumerate(sample_names)
    ]) if rows else np.empty(0, dtype="<i4")
    pieces.append(index.tobytes())
    offset += index.nbytes
    for name, raw in bool_pieces:
        entries[name]["at"] = offset
        pieces.append(raw)
        offset += len(raw)
    data["cols"] = [entries[name] for name in column_names]
    data["bytes"] = offset
    return data, b"".join(pieces)


def _cats(labels: Mapping[str, Mapping[Any, str]], cols: list[dict]) -> dict[str, Any]:
    """Category names for every labelled column; each must name every stored value once."""
    cats: dict[str, Any] = {}
    by_name = {c["name"]: c for c in cols}
    for column, names in labels.items():
        if column not in by_name:
            _fail(f'labels name unknown column "{column}"')
        vals = by_name[column].get("vals")
        if vals is None:
            _fail(f'labels require categorical integer column "{column}"')
        rendered = {str(value): str(label) for value, label in names.items()}
        if set(rendered) != {str(value) for value in vals}:
            _fail(f'labels for "{column}" must name every value exactly once')
        cats[column] = {"names": rendered, "bands": []}
    return cats


def pack(by_sample: Mapping[str, Mapping[str, Any]], *,
         weighted: Mapping[str, bool], weights: Mapping[str, Any] | None = None,
         labels: Mapping[str, Mapping[Any, str]] | None = None,
         about: Mapping[str, Any] | None = None,
         made: Mapping[str, Any] | None = None) -> tuple[dict, bytes]:
    """Pack into v7; populated boolean arrays become bits, never numeric 0/1."""
    sample_names, column_names, arrays, row_counts = _pack_inputs(by_sample, weighted)
    weight_arrays = _pack_weights(sample_names, row_counts, weighted, weights)
    data, buf = _pack_buffer(sample_names, column_names, arrays, row_counts,
                             weighted, weight_arrays)
    view = {"cats": _cats(labels, data["cols"])} if labels else {}
    meta = {"rowver": ROWVER,
            "about": {"name": "study", **dict(about or {})},
            "data": data, "view": view, "tabs": [], "opens_on": 0,
            "made": dict(made or {})}
    check(meta, buf)
    return meta, buf


def columns(meta: dict, buf: bytes, *, include_weights: bool = True) -> dict[str, np.ndarray]:
    """Numeric views onto the buffer; boolean columns unpack to one-byte NumPy bool arrays."""
    check(meta, buf)
    data, out = meta["data"], {}
    for col in data["cols"]:
        if col["kind"] == "b1":
            packed = np.frombuffer(buf, dtype=np.uint8, count=(data["rows"] + 7) // 8,
                                   offset=col["at"])
            out[col["name"]] = np.unpackbits(packed, count=data["rows"],
                                             bitorder="little").view(np.bool_)
            continue
        out[col["name"]] = np.frombuffer(buf, dtype="<" + col["kind"],
                                         count=data["rows"], offset=col["at"])
    if include_weights and data.get("weight_col"):
        out[data["weight_col"]] = np.frombuffer(
            buf, dtype="<f8", count=data["rows"], offset=data["weight_at"])
    return out


def read_by_sample(path: str | Path) -> tuple[dict, dict[str, dict[str, np.ndarray]]]:
    """Read a cartridge and split its columns by declared sample index."""
    meta, buf = read(path)
    data, all_columns = meta["data"], columns(meta, buf)
    index = np.frombuffer(buf, dtype="<i4", count=data["rows"], offset=data["sample_at"])
    return meta, {
        sample["name"]: {name: values[index == i] for name, values in all_columns.items()}
        for i, sample in enumerate(data["samples"])
    }


def effective(tab: dict | None, view: dict | None, key: str) -> Any:
    """Resolve one promote-and-shadow concern."""
    if key not in NEUTRAL:
        raise KeyError(key)
    if view is not None and key in view:
        return view[key]
    return (tab or {}).get(key, deepcopy(NEUTRAL[key]))


def lumi_of(sample: str, view: dict) -> float:
    """Resolve luminosity outermost-first: group, subgroup, then sample."""
    groups, lumis = view.get("groups", {}), view.get("lumis", {})
    for namespace, name in (("group", groups.get("of", {}).get(sample)),
                            ("sub", groups.get("sub", {}).get(sample)),
                            ("sample", sample)):
        value = lumis.get(namespace, {}).get(name) if name is not None else None
        if value is not None:
            return float(value)
    return 1.0


def lock(meta: dict, key: str, from_tab: int = 0) -> dict:
    """Promote one tab value without deleting any dormant tab copy."""
    if key not in LOCKABLE:
        raise KeyError(key)
    out = deepcopy(meta)
    tabs = out.get("tabs", [])
    if not 0 <= from_tab < len(tabs):
        raise IndexError(from_tab)
    out.setdefault("view", {})[key] = deepcopy(effective(tabs[from_tab], out["view"], key))
    return out


def unlock(meta: dict, key: str) -> dict:
    """Drop the promoted value; dormant tab copies wake unchanged."""
    if key not in LOCKABLE:
        raise KeyError(key)
    out = deepcopy(meta)
    out.setdefault("view", {}).pop(key, None)
    return out


def show(path: str | Path) -> str:
    """Return the exact header as readable JSON."""
    meta, _ = read(path)
    return json.dumps(meta, indent=2, ensure_ascii=False)


def _main(argv: list[str] | None = None) -> int:
    """``check`` or ``show`` one cartridge; hostile names never crash or repaint the terminal."""
    parser = argparse.ArgumentParser(description="inspect a RowVer 6 cartridge")
    parser.add_argument("command", choices=("check", "show"))
    parser.add_argument("file", type=Path)
    args = parser.parse_args(argv)
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(errors="replace")
    try:
        meta, _ = read(args.file)
        if args.command == "show":
            print(json.dumps(meta, indent=2, ensure_ascii=False))
        else:
            name = re.sub(r"[\x00-\x1f\x7f]", "�", meta["about"]["name"])
            print(f"ok: {name} — {meta['data']['rows']:,} rows")
    except (FormatError, OSError, RecursionError) as err:
        parser.exit(1, f"{args.file}: {err or 'nesting exceeds the display depth'}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(_main())
