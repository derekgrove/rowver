"""Read, validate, pack, and write ``.rvr`` 9 cartridges.

The container is ``gzip(u32 little-endian header length | UTF-8 JSON header | column buffer)``.
The header is ``rowver``, the ``data`` and ``made`` drawers, and consumer keys;
``FORMAT.md`` is the whole standard.
"""
from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import math
import os
import re
import sys
import tempfile
import warnings
import zlib
from pathlib import Path
from typing import Any, Mapping, NamedTuple

import awkward as ak
import numpy as np

ROWVER = 9
VERSION = "9.0.0"
MAX_HEADER_BYTES = 8 * 1024 * 1024
MAX_DATA_BYTES = 1024 * 1024 * 1024
MAX_VALS = 25
PCTS = (1, 5, 10, 20, 30, 40, 50, 60, 70, 80, 90, 95, 99)
WIDTH = {"f8": 8, "f4": 4, "i8": 8, "i4": 4, "i2": 2, "i1": 1,
         "u8": 8, "u4": 4, "u2": 2, "u1": 1, "b1": 1}
EXACT = 2 ** 53
DRAWERS = ("rowver", "data", "made")
RETIRED = ("about", "view", "tabs", "opens_on")


class FormatError(ValueError):
    """A cartridge is malformed or contradicts its own column buffer."""


class Jagged(NamedTuple):
    """One list per event as two flat arrays: how many items each event has, and every item in event order."""
    counts: np.ndarray
    content: np.ndarray


def rows_in(value: Jagged | np.ndarray) -> int:
    """Events in a column value."""
    return len(value.counts) if isinstance(value, Jagged) else len(value)


def _fail(message: str) -> None:
    raise FormatError(message)


def _option(value: Any, allowed: Any) -> bool:
    """Membership that treats an unhashable or non-string value as simply not a member."""
    return isinstance(value, str) and value in allowed


def _nat(value: Any, where: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        _fail(f"{where} must be a non-negative integer")
    return value


def _text(value: Any, where: str) -> str:
    if not isinstance(value, str) or not value.strip():
        _fail(f"{where} must be non-blank text")
    return value


def _utf8(value: str, where: str) -> bytes:
    """The UTF-8 bytes of a string; a lone surrogate has none."""
    try:
        return value.encode("utf-8")
    except UnicodeEncodeError:
        _fail(f"{where} contains an unpaired surrogate")


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


def _length(kind: str, rows: int) -> int:
    """Bytes a column of ``kind`` occupies for ``rows`` events."""
    return (rows + 7) // 8 if kind == "b1" else WIDTH[kind] * rows


def _span(at: Any, kind: str, count: int, size: int, where: str) -> tuple[int, int]:
    """Bounds-check an aligned span of ``count`` values of ``kind``; return its start and byte length."""
    at = _nat(at, f"{where} offset")
    length = _length(kind, count)
    if at % WIDTH[kind] or at + length > size:
        _fail(f"{where} is unaligned or outside the buffer")
    return at, length


def _spans(col: dict, rows: int) -> list[tuple[int, int]]:
    """The ``(at, length)`` byte spans of a validated column: one, or offsets and content for a jagged one."""
    if col["kind"] != "jagged":
        return [(col["at"], _length(col["kind"], rows))]
    content = col["content"]
    return [(col["at"], _length("i8", rows + 1)),
            (content["at"], _length(content["kind"], content["count"]))]


def cart_digest(source: str) -> str:
    """SHA-256 of the UTF-8 encoding of a recipe's text."""
    return hashlib.sha256(_utf8(source, "made.cart.source")).hexdigest()


def made_digest(made: Mapping[str, Any]) -> str:
    """SHA-256 of the canonical JSON of ``made`` with its ``sha256`` key absent."""
    body = {key: value for key, value in made.items() if key != "sha256"}
    try:
        text = json.dumps(body, sort_keys=True, separators=(",", ":"),
                          ensure_ascii=False, allow_nan=False)
    except (TypeError, ValueError, RecursionError) as exc:
        raise FormatError(f"made is not canonical JSON: {exc}") from exc
    return hashlib.sha256(_utf8(text, "made")).hexdigest()


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


def _made(made: dict, samples: list[dict], digests: bool) -> None:
    """Validate the recipe snapshot: every key present, the sample map matching data, and when asked both digests."""
    for key in ("study_name", "by", "built", "sha256", "samples", "keep", "prepare", "cart"):
        if key not in made:
            _fail(f"made.{key} is required")
    _text(made["study_name"], "made.study_name")
    for key in ("by", "built", "sha256"):
        if not isinstance(made[key], str):
            _fail(f"made.{key} must be text")
    for key in ("title", "note"):
        if key in made and not isinstance(made[key], str):
            _fail(f"made.{key} must be text")
    _made_samples(made["samples"], samples)
    _made_source(made["keep"], "made.keep", nullable=False)
    _made_source(made["prepare"], "made.prepare")
    cart = made["cart"]
    if cart is not None:
        if not isinstance(cart, dict) or any(not isinstance(cart.get(key), str)
                                             for key in ("sha256", "source")):
            _fail("made.cart must be null or hold sha256 and source text")
        if digests and cart["sha256"] != cart_digest(cart["source"]):
            _fail("made.cart.sha256 does not match made.cart.source")
    if digests and made["sha256"] != made_digest(made):
        _fail("made.sha256 does not match the snapshot")


def _made_samples(records: Any, samples: list[dict]) -> None:
    if not isinstance(records, dict) or set(records) != {sample["name"] for sample in samples}:
        _fail("made.samples must be keyed by exactly the data.samples names")
    for name, record in records.items():
        where = f'made sample "{name}"'
        if not isinstance(record, dict) or any(key not in record
                                               for key in ("files", "weights", "metadata")):
            _fail(f"{where} needs files, weights, and metadata")
        if not isinstance(record["files"], dict):
            _fail(f"{where} files must be keyed by file UUID")
        for uuid, file in record["files"].items():
            if not isinstance(uuid, str):
                _fail(f"{where} file UUIDs must be text")
            _made_file(file, where)
        _made_source(record["weights"], f"{where} weights")
        if not isinstance(record["metadata"], dict):
            _fail(f"{where} metadata must be an object")


def _made_file(file: Any, where: str) -> None:
    """A source file is its bare name, its tree, and its entry count."""
    if not isinstance(file, dict):
        _fail(f"{where} has an invalid file record")
    name = _text(file.get("file"), f"{where} file name")
    if "/" in name or "\\" in name:
        _fail(f"{where} file name {name!r} carries a directory")
    _text(file.get("tree"), f"{where} file tree")
    _nat(file.get("entries"), f"{where} file entries")


def _wire_clean(value: Any) -> str | None:
    """Why a header would not survive every reader intact, or None when it would."""
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
                stack.append(key)
                stack.append(inner)
        elif isinstance(item, (list, tuple)):
            if id(item) in seen:
                continue
            seen.add(id(item))
            stack.extend(item)
        elif isinstance(item, str):
            try:
                item.isascii() or _utf8(item, "header")
            except FormatError:
                return "header strings must not contain unpaired surrogates"
        elif isinstance(item, bool):
            continue
        elif isinstance(item, int) and abs(item) > EXACT:
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
    if abs(out) > EXACT:
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
    problem = _wire_clean(meta)
    if problem:
        _fail(problem)
    return meta, raw[4 + size:]


def _bit_values(buf: bytes | bytearray | memoryview, at: int, rows: int) -> list[int]:
    """Validate tail bits and find distinct values without unpacking event arrays."""
    if not rows:
        return []
    packed = np.frombuffer(buf, dtype=np.uint8, count=_length("b1", rows), offset=at)
    tail = rows % 8
    if tail and int(packed[-1]) >> tail:
        _fail("boolean column has nonzero unused tail bits")
    any_true = bool(np.any(packed))
    all_true = bool(np.all(packed[:-1] == 255)) and int(packed[-1]) == (
        (1 << tail) - 1 if tail else 255)
    return ([0] if not all_true else []) + ([1] if any_true else [])


def _samples(samples: list, rows: int) -> tuple[list[int], list[bool]]:
    """Validate data.samples; return their row counts and weighted flags."""
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
    return counts, weighted


def _weight_col(data: dict, weighted: list[bool]) -> str | None:
    weight_col = data.get("weight_col")
    if weight_col is not None and (not isinstance(weight_col, str) or not weight_col.strip()):
        _fail("data.weight_col must be a non-empty string or null")
    if any(weighted) and weight_col is None:
        _fail("weighted samples require a weights column")
    if not any(weighted) and weight_col is not None:
        _fail("a data-only file must not declare a weights column")
    return weight_col


def _col_source(source: Any, where: str) -> None:
    """Shape-check a column's provenance; nothing in it is executed or cross-checked."""
    if not isinstance(source, dict):
        _fail(f"{where} source must be an object")
    if source.get("class") == "copy":
        if any(not isinstance(source.get(key), str) or not source[key]
               for key in ("branch", "dtype")):
            _fail(f"{where} copy source needs branch and dtype text")
        return
    if source.get("class") != "derived":
        _fail(f"{where} source class must be copy or derived")
    inputs = source.get("from")
    if (not isinstance(inputs, list)
            or any(not isinstance(x, str) or not x for x in inputs)
            or len(set(inputs)) != len(inputs)
            or not _option(source.get("reproducible"), ("cartridge", "source"))
            or not isinstance(source.get("python"), str)):
        _fail(f"{where} derived source needs from, reproducible, and python")


def _jagged(col: dict, name: str, rows: int, size: int) -> list[tuple[int, int, str]]:
    """Validate a jagged column's offsets and content spans; return them as byte ranges."""
    content = col.get("content")
    if not isinstance(content, dict) or not _option(content.get("kind"), WIDTH):
        _fail(f'column "{name}" content needs a kind')
    count = _nat(content.get("count"), f'column "{name}" content count')
    at, length = _span(col.get("at"), "i8", rows + 1, size, f'column "{name}" offsets')
    content_at, content_length = _span(content.get("at"), content["kind"], count, size,
                                       f'column "{name}" content')
    return [(at, at + length, f'column "{name}" offsets'),
            (content_at, content_at + content_length, f'column "{name}" content')]


def _statistics(col: dict, name: str, kind: str, rows: int) -> None:
    """Validate lim, nice, the percentile ladder, vals, and names; an empty column carries the stub."""
    lim = _pair(col.get("lim"), f'column "{name}" lim')
    nice = _pair(col.get("nice"), f'column "{name}" nice')
    pcts = [_finite(col.get(f"p{q}"), f'column "{name}" p{q}') for q in PCTS]
    if nice[0] > lim[0] or nice[1] < lim[1]:
        _fail(f'column "{name}" nice range does not enclose lim')
    chain = [lim[0], *pcts, lim[1]]
    if any(a > b for a, b in zip(chain, chain[1:])):
        _fail(f'column "{name}" percentiles are outside lim or reversed')
    vals = col.get("vals")
    if not rows and (lim != (0, 0) or nice != (0, 1) or any(pcts)
                     or (kind[0] != "f" and vals != [])):
        _fail(f'column "{name}" is empty and must carry the empty-column stub')
    if vals is not None:
        if kind[0] == "f" or not isinstance(vals, list):
            _fail(f'column "{name}" has invalid categorical values')
        if (any(isinstance(x, bool) or not isinstance(x, int) for x in vals)
                or len(set(vals)) != len(vals) or vals != sorted(vals)):
            _fail(f'column "{name}" categorical values must be unique ascending integers')
    if "names" in col:
        names = col["names"]
        if vals is None or not isinstance(names, dict) or set(names) != {str(x) for x in vals} \
                or any(not isinstance(label, str) for label in names.values()):
            _fail(f'column "{name}" names must label every vals member exactly once')


def _column(col: Any, i: int, rows: int, size: int, names: set[str]
            ) -> tuple[str, list[tuple[int, int, str]]]:
    """Validate one column descriptor; return its name and the byte ranges it occupies."""
    if not isinstance(col, dict):
        _fail(f"data.cols[{i}] must be an object")
    name, kind = col.get("name"), col.get("kind")
    if not isinstance(name, str) or not name.strip() or name in names:
        _fail(f"column {i} has an empty or duplicate name")
    if not isinstance(col.get("sha256"), str):
        _fail(f'column "{name}" needs a sha256')
    if "source" in col:
        _col_source(col["source"], f'column "{name}"')
    if kind == "jagged":
        return name, _jagged(col, name, rows, size)
    if not _option(kind, WIDTH):
        _fail(f'column "{name}" has unknown kind {kind!r}')
    at, length = _span(col.get("at"), kind, rows, size, f'column "{name}"')
    _statistics(col, name, kind, rows)
    return name, [(at, at + length, f'column "{name}"')]


def _columns(cols: list, rows: int, size: int, weight_col: str | None
             ) -> list[tuple[int, int, str]]:
    """Validate data.cols; return every byte range they occupy."""
    names: set[str] = set()
    ranges: list[tuple[int, int, str]] = []
    declared_bytes = _length("i4", rows) + (_length("f8", rows) if weight_col else 0)
    for i, col in enumerate(cols):
        name, spans = _column(col, i, rows, size, names)
        names.add(name)
        ranges.extend(spans)
        declared_bytes += sum(end - at for at, end, _ in spans)
    if weight_col in names:
        _fail(f'{weight_col!r} is both the weights column and a data column')
    if declared_bytes != size:
        _fail(f"column declarations need {declared_bytes} bytes; data declares {size}")
    return ranges


def _offsets(data: dict, rows: int, size: int, weight_col: str | None,
             ranges: list[tuple[int, int, str]]) -> None:
    """Place the weights and sample index; refuse any overlap between buffer ranges."""
    if weight_col is not None:
        weight_at = _nat(data.get("weight_at"), "data.weight_at")
        if weight_at % WIDTH["f8"] or weight_at + _length("f8", rows) > size:
            _fail("weights are unaligned or outside the buffer")
        ranges.append((weight_at, weight_at + _length("f8", rows), "weights"))
    elif "weight_at" in data:
        _fail("an unweighted file must not declare data.weight_at")
    sample_at = data["sample_at"]
    if sample_at % WIDTH["i4"] or sample_at + _length("i4", rows) > size:
        _fail("sample index is unaligned or outside the buffer")
    ranges.append((sample_at, sample_at + _length("i4", rows), "sample index"))
    ranges.sort()
    for previous, current in zip(ranges, ranges[1:]):
        if current[0] < previous[1]:
            _fail(f"{current[2]} overlaps {previous[2]}")


def _payload(buf: bytes | bytearray | memoryview, data: dict, rows: int,
             sample_rows: list[int], weight_col: str | None, digests: bool) -> None:
    """Check the buffer against its descriptors: sample counts, finite weights, lim, vals, digests."""
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
    if digests:
        _hashes(slab, data, rows)


def _hashes(slab: memoryview, data: dict, rows: int) -> None:
    """Recompute the buffer and per-column SHA-256 digests the header declares."""
    if data["sha256"] != hashlib.sha256(slab).hexdigest():
        _fail("data.sha256 does not match the buffer")
    for col in data["cols"]:
        digest = hashlib.sha256()
        for at, length in _spans(col, rows):
            digest.update(slab[at:at + length])
        if col["sha256"] != digest.hexdigest():
            _fail(f'column "{col["name"]}" sha256 does not match its payload')


def _jagged_payload(slab: memoryview, col: dict, rows: int) -> None:
    """Offsets must start at 0, never decrease, and end at the content count; content must be finite."""
    offsets = np.frombuffer(slab, dtype="<i8", count=rows + 1, offset=col["at"])
    content = col["content"]
    if offsets[0] != 0 or offsets[-1] != content["count"] or np.any(offsets[1:] < offsets[:-1]):
        _fail(f'column "{col["name"]}" offsets do not describe its content')
    if content["kind"] == "b1":
        _bit_values(slab, content["at"], content["count"])
    elif content["kind"][0] == "f":
        values = np.frombuffer(slab, dtype="<" + content["kind"], count=content["count"],
                               offset=content["at"])
        if not np.isfinite(values).all():
            _fail(f'column "{col["name"]}" contains NaN or infinity')


def _column_payload(slab: memoryview, col: dict, rows: int) -> None:
    if col["kind"] == "jagged":
        _jagged_payload(slab, col, rows)
        return
    if col["kind"] == "b1":
        actual_vals = _bit_values(slab, col["at"], rows)
        actual = [actual_vals[0], actual_vals[-1]] if rows else [0, 0]
        if col["lim"] != actual:
            _fail(f'column "{col["name"]}" lim does not match its payload')
        if col.get("vals") is not None and col["vals"] != actual_vals:
            _fail(f'column "{col["name"]}" vals do not match its payload')
        return
    values = np.frombuffer(slab, dtype="<" + col["kind"], count=rows, offset=col["at"])
    if col["kind"][0] == "f" and not np.isfinite(values).all():
        _fail(f'column "{col["name"]}" contains NaN or infinity')
    if rows:
        actual = [float(values.min()), float(values.max())]
        if [float(x) for x in col["lim"]] != actual:
            _fail(f'column "{col["name"]}" lim does not match its payload')
    if col.get("vals") is not None:
        actual_vals = [int(x) for x in np.unique(values)]
        if col["vals"] != actual_vals:
            _fail(f'column "{col["name"]}" vals do not match its payload')


def check(meta: dict, buf: bytes | bytearray | memoryview) -> None:
    """Validate a header against its buffer; boolean payloads stay packed."""
    _check(meta, buf, digests=True)


def _check(meta: dict, buf: bytes | bytearray | memoryview, *, digests: bool) -> None:
    """The validator; ``digests`` recomputes every SHA-256, which pack skips for what it just built."""
    if not isinstance(meta, dict) or type(meta.get("rowver")) is not int or meta["rowver"] != ROWVER:
        _fail(f"RowVer reads .rvr {ROWVER} only")
    retired = [key for key in RETIRED if key in meta]
    if retired:
        _fail(f"header carries retired keys {retired}")
    for key, value in meta.items():
        if key not in DRAWERS and not isinstance(value, dict):
            _fail(f"consumer key {key!r} must hold an object")
    for drawer in DRAWERS[1:]:
        if not isinstance(meta.get(drawer), dict):
            _fail(f"header has no valid {drawer!r} drawer")
    data = meta["data"]
    rows = _nat(data.get("rows"), "data.rows")
    size = _nat(data.get("bytes"), "data.bytes")
    _nat(data.get("sample_at"), "data.sample_at")
    if not isinstance(data.get("sha256"), str):
        _fail("data.sha256 must be text")
    if size > MAX_DATA_BYTES:
        _fail(f"column data exceeds the {MAX_DATA_BYTES}-byte limit")
    if len(buf) != size:
        _fail(f"buffer is {len(buf)} bytes; data declares {size}")
    samples, cols = data.get("samples"), data.get("cols")
    if not isinstance(samples, list) or not samples:
        _fail("data.samples must be a non-empty list")
    if not isinstance(cols, list) or not cols:
        _fail("data.cols must be a non-empty list")
    sample_rows, weighted = _samples(samples, rows)
    weight_col = _weight_col(data, weighted)
    ranges = _columns(cols, rows, size, weight_col)
    _offsets(data, rows, size, weight_col, ranges)
    _payload(buf, data, rows, sample_rows, weight_col, digests)
    _made(meta["made"], samples, digests)


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


def _flat(value: Any, where: str) -> np.ndarray:
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


def _array(value: Any, where: str) -> np.ndarray | Jagged:
    """A flat numeric array, or a Jagged whose counts are non-negative and add up to its content."""
    if not isinstance(value, Jagged):
        return _flat(value, where)
    counts, content = _flat(value.counts, f"{where} counts"), _flat(value.content, where)
    if counts.dtype.kind not in "iu" or np.any(counts < 0) or counts.sum(dtype=object) != len(content):
        _fail(f"{where} counts must be non-negative integers that add up to its content")
    return Jagged(counts.astype(np.int64, copy=False), content)


def _kind(arrays: list[np.ndarray], source: Mapping[str, Any] | None) -> str:
    """The stored kind: a copy keeps the dtype it was read as, a derived column widens to f8, i4, or b1."""
    populated = [a for a in arrays if a.size] or arrays
    dtypes = [a.dtype for a in populated]
    copy = source is not None and source.get("class") == "copy"
    if copy:
        try:
            dtypes.append(np.dtype(source["dtype"]))
        except TypeError as exc:
            raise FormatError(f"copy dtype {source['dtype']!r} is not a NumPy dtype") from exc
    dtype = np.result_type(*dtypes)
    native = f"{dtype.kind}{dtype.itemsize}"
    if dtype.kind == "b":
        return "b1"
    if copy and dtype.kind == "f" and any(d.kind in "iu" and d.itemsize == 8 for d in dtypes):
        _fail("a copy read as 64-bit integers beside floats or the other signedness has no exact common dtype")
    if copy and native in WIDTH:
        return native
    if all(a.dtype.kind in "biu" for a in populated):
        i32 = np.iinfo(np.int32)
        if all(not a.size or i32.min <= a.min() and a.max() <= i32.max for a in populated):
            return "i4"
        if any(a.size and (a.min() < -EXACT or a.max() > EXACT) for a in populated):
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


def _wire_number(value: float) -> int | float:
    """An integer where a JavaScript reader keeps it exact, otherwise a float."""
    return int(value) if value == int(value) and abs(value) <= EXACT else _finite(value, "nice")


def _stats(values: np.ndarray, integer: bool) -> dict:
    """lim, nice, percentiles, and for an integer column with at most MAX_VALS distinct values, vals."""
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
    floats = values.astype(np.float64, copy=False)
    out = {"lim": [lo, hi], "nice": [_wire_number(nice_lo), _wire_number(nice_hi)],
           **{f"p{q}": float(np.percentile(floats, q)) for q in PCTS}}
    if integer and len(np.unique(values[:4096])) <= MAX_VALS:
        unique = np.unique(values)
        if len(unique) <= MAX_VALS and -EXACT <= unique[0] and unique[-1] <= EXACT:
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
        lengths = {rows_in(value) for value in arrays[sample].values()}
        if len(lengths) != 1:
            _fail(f'sample "{sample}" columns have different lengths')
        row_counts[sample] = lengths.pop()
    for name in column_names:
        if len({isinstance(arrays[sample][name], Jagged) for sample in sample_names}) != 1:
            _fail(f'column "{name}" is one value per event in some samples and one list per event in others')
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
                 weighted: Mapping[str, bool], weight_arrays: Mapping[str, np.ndarray],
                 sources: Mapping[str, Mapping[str, Any]]) -> tuple[dict, bytes]:
    """Lay out weights, columns of four bytes or more widest first, the sample index, narrower columns, then packed bits."""
    rows = sum(row_counts.values())
    pieces: list[bytes] = []
    data: dict[str, Any] = {
        "rows": rows,
        "bytes": 0,
        "sha256": "",
        "weight_col": "weight" if weight_arrays else None,
        "sample_at": 0,
        "samples": [{"name": name, "rows": row_counts[name],
                     "weighted": weighted[name]} for name in sample_names],
        "cols": [],
    }

    def put(raw: bytes) -> int:
        """Append one span to the buffer; return where it starts."""
        at, data["bytes"] = data["bytes"], data["bytes"] + len(raw)
        pieces.append(raw)
        return at

    if weight_arrays:
        combined = np.concatenate([weight_arrays[name] for name in sample_names])
        data["weight_at"] = put(combined.astype("<f8", copy=False).tobytes())
    spans: list[tuple[str, bytes, dict]] = []
    for name in column_names:
        values = [arrays[sample][name] for sample in sample_names]
        entry: dict[str, Any] = {"name": name}
        if isinstance(values[0], Jagged):
            kind = _kind([value.content for value in values], sources.get(name))
            offsets = np.zeros(rows + 1, dtype="<i8")
            np.cumsum(np.concatenate([value.counts for value in values]), out=offsets[1:])
            _, raw = _encoded(kind, [value.content for value in values])
            entry.update(kind="jagged", at=0,
                         content={"kind": kind, "at": 0, "count": int(offsets[-1])})
            spans += [("i8", offsets.tobytes(), entry), (kind, raw, entry["content"])]
            raw, stats = offsets.tobytes() + raw, {}
        else:
            kind = _kind(values, sources.get(name))
            encoded, raw = _encoded(kind, values)
            entry.update(kind=kind, at=0)
            spans.append((kind, raw, entry))
            stats = _stats(encoded, kind[0] != "f")
            if kind[0] != "f" and "vals" not in stats:
                warnings.warn(f'column "{name}" has more than {MAX_VALS} distinct values, '
                              "or values beyond 2**53, and carries no vals", stacklevel=3)
        if name in sources:
            entry["source"] = dict(sources[name])
        entry["sha256"] = hashlib.sha256(raw).hexdigest()
        entry.update(stats)
        data["cols"].append(entry)
    spans.sort(key=lambda span: (span[0] == "b1", -WIDTH[span[0]]))
    wide = sum(WIDTH[kind] >= 4 for kind, _, _ in spans)
    for kind, raw, owner in spans[:wide]:
        owner["at"] = put(raw)
    index = np.concatenate([np.full(row_counts[name], i, dtype="<i4")
                            for i, name in enumerate(sample_names)])
    data["sample_at"] = put(index.tobytes())
    for kind, raw, owner in spans[wide:]:
        owner["at"] = put(raw)
    buf = b"".join(pieces)
    data["sha256"] = hashlib.sha256(buf).hexdigest()
    return data, buf


def _encoded(kind: str, arrays: list[np.ndarray]) -> tuple[np.ndarray, bytes]:
    """Concatenate ``arrays`` as ``kind``; return the values and their little-endian bytes, bits packed."""
    dtype = "u1" if kind == "b1" else "<" + kind
    values = np.concatenate([array.astype(dtype, copy=False) for array in arrays])
    if kind == "b1":
        return values, np.packbits(values, bitorder="little").tobytes()
    return values, values.tobytes()


def _names(labels: Mapping[str, Mapping[Any, str]], cols: list[dict]) -> None:
    """Attach value labels to every labelled column; each must name every stored value once."""
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
        by_name[column]["names"] = rendered


def pack(by_sample: Mapping[str, Mapping[str, Any]], *,
         weighted: Mapping[str, bool], weights: Mapping[str, Any] | None = None,
         labels: Mapping[str, Mapping[Any, str]] | None = None,
         made: Mapping[str, Any],
         sources: Mapping[str, Mapping[str, Any]] | None = None) -> tuple[dict, bytes]:
    """Pack columns and the recipe snapshot into a header and buffer; ``made.sha256`` is set here, last."""
    sample_names, column_names, arrays, row_counts = _pack_inputs(by_sample, weighted)
    weight_arrays = _pack_weights(sample_names, row_counts, weighted, weights)
    data, buf = _pack_buffer(sample_names, column_names, arrays, row_counts,
                             weighted, weight_arrays, sources or {})
    if labels:
        _names(labels, data["cols"])
    snapshot = {key: value for key, value in made.items() if key != "sha256"}
    snapshot["sha256"] = made_digest(snapshot)
    meta = {"rowver": ROWVER, "data": data, "made": snapshot}
    _check(meta, buf, digests=False)
    return meta, buf


def columns(meta: dict, buf: bytes, *, include_weights: bool = True
            ) -> dict[str, np.ndarray | ak.Array]:
    """Views onto the buffer: NumPy arrays, bools unpacked to one byte each, and Awkward arrays for jagged columns."""
    check(meta, buf)
    return _views(meta, buf, include_weights)


def _values(buf: bytes, span: dict, count: int) -> np.ndarray:
    """``count`` values of ``span["kind"]`` at ``span["at"]``; bits unpack to NumPy bools."""
    if span["kind"] == "b1":
        packed = np.frombuffer(buf, dtype=np.uint8, count=(count + 7) // 8, offset=span["at"])
        return np.unpackbits(packed, count=count, bitorder="little").view(np.bool_)
    return np.frombuffer(buf, dtype="<" + span["kind"], count=count, offset=span["at"])


def _views(meta: dict, buf: bytes, include_weights: bool) -> dict[str, np.ndarray | ak.Array]:
    data, out = meta["data"], {}
    for col in data["cols"]:
        if col["kind"] == "jagged":
            offsets = np.frombuffer(buf, dtype="<i8", count=data["rows"] + 1, offset=col["at"])
            content = _values(buf, col["content"], col["content"]["count"])
            out[col["name"]] = ak.unflatten(content, np.diff(offsets))
        else:
            out[col["name"]] = _values(buf, col, data["rows"])
    if include_weights and data.get("weight_col"):
        out[data["weight_col"]] = np.frombuffer(
            buf, dtype="<f8", count=data["rows"], offset=data["weight_at"])
    return out


def read_by_sample(path: str | Path) -> tuple[dict, dict[str, dict[str, np.ndarray | ak.Array]]]:
    """Read a cartridge and split its columns by declared sample index."""
    meta, buf = read(path)
    data, all_columns = meta["data"], _views(meta, buf, True)
    index = np.frombuffer(buf, dtype="<i4", count=data["rows"], offset=data["sample_at"])
    return meta, {
        sample["name"]: {name: values[index == i] for name, values in all_columns.items()}
        for i, sample in enumerate(data["samples"])
    }


def show(path: str | Path) -> str:
    """Return the exact header as readable JSON."""
    meta, _ = read(path)
    return json.dumps(meta, indent=2, ensure_ascii=False)


def _main(argv: list[str] | None = None) -> int:
    """``check`` or ``show`` one cartridge; hostile names never crash or repaint the terminal."""
    parser = argparse.ArgumentParser(description="inspect a RowVer cartridge")
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
            name = re.sub(r"[\x00-\x1f\x7f]", "�", meta["made"]["study_name"])
            print(f"ok: {name} — {meta['data']['rows']:,} rows")
    except (FormatError, OSError, RecursionError) as err:
        parser.exit(1, f"{args.file}: {err or 'nesting exceeds the display depth'}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(_main())
