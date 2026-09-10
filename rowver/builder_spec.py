"""Validate and compile the browser builder's non-executable StudySpec v1."""
from __future__ import annotations

import json
import math
import re
from pathlib import Path, PurePosixPath
from typing import Any, Mapping

SCHEMA = 1
MAX_DEPTH = 12
MAX_ITEMS = 32
NAME = re.compile(r"^[A-Za-z][A-Za-z0-9_-]{0,63}$")
COMPARE = frozenset({"<", "<=", "==", "!=", ">=", ">"})
UNARY = frozenset({"abs", "neg", "count", "sum", "min", "max"})
BINARY = frozenset({"add", "sub", "mul", "div"})


class SpecError(ValueError):
    def __init__(self, code: str, message: str):
        super().__init__(message)
        self.code = code


def _error(code: str, message: str) -> None:
    raise SpecError(code, message)


def _object(value: Any, keys: set[str], where: str, optional: set[str] | None = None) -> dict:
    if not isinstance(value, dict):
        _error("schema.type", f"{where} must be an object")
    optional = optional or set()
    unknown = set(value) - keys - optional
    missing = keys - set(value)
    if unknown or missing:
        _error("schema.keys", f"{where} has missing {sorted(missing)} or unknown {sorted(unknown)} keys")
    return value


def _text(value: Any, where: str, *, limit: int = 256) -> str:
    if not isinstance(value, str) or not value or len(value) > limit or "\x00" in value:
        _error("schema.text", f"{where} must be non-empty text no longer than {limit} characters")
    return value


def _branch(value: Any, where: str) -> str:
    return _text(value, where, limit=512)


def validate_value(value: Any, where: str = "value", depth: int = 0) -> dict:
    if depth > MAX_DEPTH:
        _error("schema.depth", f"{where} exceeds the expression depth limit")
    if not isinstance(value, dict) or not isinstance(value.get("op"), str):
        _error("schema.value", f"{where} must be a ValueSpec object")
    op = value["op"]
    if op == "branch":
        _object(value, {"op", "name"}, where)
        _branch(value["name"], f"{where}.name")
    elif op == "scalar":
        _object(value, {"op", "value"}, where)
        number = value["value"]
        if isinstance(number, bool) or not isinstance(number, (int, float)) or not math.isfinite(number):
            _error("schema.number", f"{where}.value must be finite")
    elif op in UNARY:
        _object(value, {"op", "value"}, where)
        validate_value(value["value"], f"{where}.value", depth + 1)
    elif op in BINARY:
        _object(value, {"op", "left", "right"}, where)
        validate_value(value["left"], f"{where}.left", depth + 1)
        validate_value(value["right"], f"{where}.right", depth + 1)
    elif op == "ranked":
        _object(value, {"op", "branch", "orderBy", "rank", "direction"}, where)
        _branch(value["branch"], f"{where}.branch")
        _branch(value["orderBy"], f"{where}.orderBy")
        if isinstance(value["rank"], bool) or not isinstance(value["rank"], int) or not 0 <= value["rank"] <= 7:
            _error("schema.rank", f"{where}.rank must be an integer from 0 through 7")
        if value["direction"] not in {"asc", "desc"}:
            _error("schema.direction", f"{where}.direction must be asc or desc")
    else:
        _error("schema.op", f"{where} has unknown operation {op!r}")
    return value


def validate_predicate(value: Any, where: str = "keep", depth: int = 0) -> dict:
    if depth > MAX_DEPTH:
        _error("schema.depth", f"{where} exceeds the predicate depth limit")
    if not isinstance(value, dict) or not isinstance(value.get("op"), str):
        _error("schema.predicate", f"{where} must be a PredicateSpec object")
    op = value["op"]
    if op == "true":
        _object(value, {"op"}, where)
    elif op == "compare":
        _object(value, {"op", "left", "operator", "right"}, where)
        if value["operator"] not in COMPARE:
            _error("schema.operator", f"{where}.operator is not a supported comparison")
        validate_value(value["left"], f"{where}.left", depth + 1)
        validate_value(value["right"], f"{where}.right", depth + 1)
    elif op in {"all", "any"}:
        _object(value, {"op", "items"}, where)
        items = value["items"]
        if not isinstance(items, list) or not 1 <= len(items) <= MAX_ITEMS:
            _error("schema.items", f"{where}.items must contain 1 to {MAX_ITEMS} predicates")
        for index, item in enumerate(items):
            validate_predicate(item, f"{where}.items[{index}]", depth + 1)
    elif op == "not":
        _object(value, {"op", "item"}, where)
        validate_predicate(value["item"], f"{where}.item", depth + 1)
    else:
        _error("schema.op", f"{where} has unknown operation {op!r}")
    return value


def validate_spec(value: Any, sources: set[str] | None = None) -> dict:
    spec = _object(value, {"schema", "name", "samples", "columns", "keep"}, "study",
                   optional={"labels"})
    if spec["schema"] != SCHEMA:
        _error("schema.version", f"study.schema must be {SCHEMA}")
    if not isinstance(spec["name"], str) or NAME.fullmatch(spec["name"]) is None:
        _error("schema.name", "study.name must start with a letter and contain only letters, digits, _ or -")
    samples = spec["samples"]
    if not isinstance(samples, list) or not 1 <= len(samples) <= 256:
        _error("schema.samples", "study.samples must contain 1 to 256 entries")
    sample_names: set[str] = set()
    file_count = 0
    for i, sample in enumerate(samples):
        sample = _object(sample, {"name", "files", "weights"}, f"samples[{i}]")
        name = _text(sample["name"], f"samples[{i}].name")
        if name in sample_names:
            _error("schema.duplicate", f"two samples are named {name!r}")
        sample_names.add(name)
        files = sample["files"]
        if not isinstance(files, list) or not files:
            _error("schema.files", f"samples[{i}].files must be a non-empty list")
        seen_files: set[tuple[str, str]] = set()
        for j, file in enumerate(files):
            file = _object(file, {"source", "path", "tree"}, f"samples[{i}].files[{j}]")
            source = _text(file["source"], f"samples[{i}].files[{j}].source", limit=64)
            if sources is not None and source not in sources:
                _error("path.source", f"unknown source {source!r}")
            path = _text(file["path"], f"samples[{i}].files[{j}].path", limit=2048)
            pure = PurePosixPath(path)
            if pure.is_absolute() or ".." in pure.parts or "://" in path:
                _error("path.escape", f"{path!r} is not a safe source-relative path")
            tree = _text(file["tree"], f"samples[{i}].files[{j}].tree", limit=512)
            key = (source + "/" + path, tree)
            if key in seen_files:
                _error("schema.duplicate", f"samples[{i}] repeats {path}:{tree}")
            seen_files.add(key)
            file_count += 1
        weights = sample["weights"]
        if weights is not None:
            weights = _object(weights, {"branch"}, f"samples[{i}].weights")
            _branch(weights["branch"], f"samples[{i}].weights.branch")
    columns = spec["columns"]
    if not isinstance(columns, list) or not 1 <= len(columns) <= 256:
        _error("schema.columns", "study.columns must contain 1 to 256 entries")
    column_names: set[str] = set()
    for i, column in enumerate(columns):
        column = _object(column, {"name", "source"}, f"columns[{i}]")
        name = _text(column["name"], f"columns[{i}].name")
        if name == "weight" or name in column_names:
            _error("schema.duplicate", f"duplicate or reserved column name {name!r}")
        column_names.add(name)
        validate_value(column["source"], f"columns[{i}].source")
    validate_predicate(spec["keep"])
    labels = spec.get("labels", {})
    if not isinstance(labels, dict) or any(name not in column_names for name in labels):
        _error("schema.labels", "labels must be keyed by declared column names")
    for column, mapping in labels.items():
        if not isinstance(mapping, dict):
            _error("schema.labels", f"labels[{column!r}] must be an object")
        numeric_keys: set[int] = set()
        for raw, label in mapping.items():
            try:
                number = int(raw)
            except (TypeError, ValueError) as exc:
                raise SpecError("schema.labels", f"label key {raw!r} is not an integer") from exc
            if str(number) != str(raw) or number in numeric_keys:
                _error("schema.labels", f"label key {raw!r} is not a canonical unique integer")
            numeric_keys.add(number)
            _text(label, f"labels[{column!r}][{raw!r}]")
    if file_count > 1024:
        _error("limit.files", "a study may name at most 1024 files")
    return spec


def resolve_path(roots: Mapping[str, Path], source: str, relative: str,
                 *, must_exist: bool = True) -> Path:
    if source not in roots:
        _error("path.source", f"unknown source {source!r}")
    pure = PurePosixPath(relative)
    if pure.is_absolute() or ".." in pure.parts or "://" in relative:
        _error("path.escape", f"{relative!r} is not a safe source-relative path")
    root = roots[source].resolve()
    candidate = (root / Path(*pure.parts)).resolve()
    if not candidate.is_relative_to(root):
        _error("path.escape", f"{relative!r} leaves source {source!r}")
    if must_exist and not candidate.is_file():
        _error("path.missing", f"{source}:{relative} is not a file")
    return candidate


def _value(value: dict) -> str:
    op = value["op"]
    if op == "branch":
        return f"events[{value['name']!r}]"
    if op == "scalar":
        return repr(value["value"])
    if op == "abs":
        return f"abs({_value(value['value'])})"
    if op == "neg":
        return f"(-{_value(value['value'])})"
    if op in {"count", "sum", "min", "max"}:
        function = "num" if op == "count" else op
        extra = ", mask_identity=False" if op in {"min", "max"} else ""
        return f"ak.{function}({_value(value['value'])}, axis=1{extra})"
    if op in BINARY:
        operator = {"add": "+", "sub": "-", "mul": "*", "div": "/"}[op]
        return f"({_value(value['left'])} {operator} {_value(value['right'])})"
    if op == "ranked":
        return (f"_ranked(events, {value['branch']!r}, {value['orderBy']!r}, "
                f"{value['rank']}, {value['direction'] == 'asc'!r})")
    raise AssertionError(op)


def _predicate(value: dict) -> str:
    op = value["op"]
    if op == "true":
        return "np.ones(len(events), dtype=np.bool_)"
    if op == "compare":
        return f"({_value(value['left'])} {value['operator']} {_value(value['right'])})"
    if op in {"all", "any"}:
        operator = " & " if op == "all" else " | "
        return "(" + operator.join(_predicate(item) for item in value["items"]) + ")"
    if op == "not":
        return f"(~{_predicate(value['item'])})"
    raise AssertionError(op)


def compile_spec(value: Any, roots: Mapping[str, Path]) -> str:
    """Compile a validated spec into readable, injection-safe cart.py source."""
    spec = validate_spec(value, set(roots))
    filesets = []
    for sample in spec["samples"]:
        files = {}
        for file in sample["files"]:
            path = resolve_path(roots, file["source"], file["path"])
            if str(path) in files and files[str(path)] != file["tree"]:
                _error("schema.files", f"one sample cannot read two trees from {path}")
            files[str(path)] = file["tree"]
        weight = None if sample["weights"] is None else sample["weights"]["branch"]
        filesets.append((sample["name"], files, weight))
    labels = {column: {int(raw): label for raw, label in mapping.items()}
              for column, mapping in spec.get("labels", {}).items()}
    lines = [
        '"""Generated by RowVer Builder from structured StudySpec v1."""',
        "import awkward as ak",
        "import numpy as np",
        "from rowver import Cart",
        "",
        "def _ranked(events, branch, order_by, rank, ascending):",
        "    order = ak.argsort(events[order_by], axis=1, ascending=ascending)",
        "    arranged = events[branch][order]",
        "    return ak.pad_none(arranged, rank + 1, axis=1)[:, rank]",
        "",
        "def keep(events):",
        f"    return {_predicate(spec['keep'])}",
        "",
        "CART = Cart(",
        "    fileset={",
    ]
    for name, files, weight in filesets:
        lines.extend([
            f"        {name!r}: {{",
            f"            'files': {files!r},",
            f"            'metadata': {{'weights': {weight!r}}},",
            "        },",
        ])
    lines.extend(["    },", "    keep=keep,", "    columns={"])
    for column in spec["columns"]:
        lines.append(f"        {column['name']!r}: lambda events: {_value(column['source'])},")
    lines.append("    },")
    if labels:
        lines.append(f"    labels={labels!r},")
    lines.extend([")", ""])
    source = "\n".join(lines)
    compile(source, "cart.py", "exec")
    return source


def canonical_json(spec: Any, sources: set[str] | None = None) -> str:
    """Return one stable encoding after strict validation."""
    validate_spec(spec, sources)
    return json.dumps(spec, allow_nan=False, sort_keys=True, separators=(",", ":"))
