"""Write an .rvr file that holds only columns derived from another .rvr file's columns.

    from rowver.derive import derive
    header = derive("parent.rvr", "derived.rvr", source, {"mT2": mt2}, by="my program 1.0")

The derived file has the parent's rows, samples, files, sample and entry arrays and its `made`,
none of the parent's columns, and a `derived` section that names the parent by its checksums,
the parent columns that were read, and the text that computed the new ones. See FORMAT.md.
"""
import hashlib
import json
import os
import platform
from collections.abc import Mapping
from datetime import datetime, timezone
from pathlib import Path

import awkward as ak
import numpy as np

from . import extend, rvr

SUFFIX = ".deriving"


class Refused(ValueError):
    """The derived file cannot be written as asked; nothing was written."""


class Exists(Refused, FileExistsError):
    """The target is already there and was not to be replaced."""


class Columns(Mapping):
    """The parent's own columns by name, each decompressed and checked against its sha256 when
    first asked for, and its sample columns, each with its sample's value on every row; `read`
    holds the checksum of every column asked for."""

    def __init__(self, header, section):
        self.data, self.section, self.read, self.held, self.sample = header["data"], section, {}, {}, None
        self.records = {c["name"]: c for c in self.data["cols"]}
        self.shared = {c["name"]: c for c in self.data["sample_cols"] if c["name"] not in self.records}

    def array(self, record, kind, count, label):
        """One of the parent's arrays, refused unless its bytes have the sha256 its header says."""
        raw = rvr.decode(self.section, record, rvr.dtype(kind).itemsize)
        if hashlib.sha256(raw).hexdigest() != record["sha256"]:
            raise Refused(f"The parent's {label} does not match its sha256; validate the parent.")
        return np.frombuffer(raw, rvr.dtype(kind), count)

    def index(self):
        """Each row's sample index, read from the parent once."""
        if self.sample is None:
            self.sample = self.array(self.data["sample"], "i4", self.data["rows"], "sample")
        return self.sample

    def spread(self, name):
        """One sample column on every row, refused unless its values have the sha256 its header says."""
        record = self.shared[name]
        if rvr.sample_checksum(record["kind"], record["values"]) != record["sha256"]:
            raise Refused(f"The parent's sample column {name} does not match its sha256; validate the parent.")
        return rvr.broadcast(record, self.index())

    def __getitem__(self, name):
        """One column, loaded and recorded as read the first time it is asked for."""
        if name in self.shared:
            if name not in self.held:
                self.held[name] = self.spread(name)
            self.read[name] = self.shared[name]["sha256"]
            return self.held[name]
        if name not in self.records:
            raise KeyError(f"{name} is not a column the parent itself holds")
        if name not in self.held:
            record, rows = self.records[name], self.data["rows"]
            if record["kind"] != "jagged":
                self.held[name] = self.array(record, record["kind"], rows, name)
            else:
                offsets, content = self.array(record["offsets"], "i8", rows + 1, f"{name} offsets"), record["content"]
                self.held[name] = ak.unflatten(self.array(content, content["kind"], content["count"],
                                                          f"{name} content"), np.diff(offsets))
            self.read[name] = rvr.column_checksum(record)
        return self.held[name]

    def __contains__(self, name):
        """Whether the parent holds the column or sample column; asking does not read it."""
        return name in self.records or name in self.shared

    def __iter__(self):
        """The parent's column names, in its order, then its sample columns'."""
        return iter([*self.records, *self.shared])

    def __len__(self):
        """How many columns and sample columns the parent holds."""
        return len(self.records) + len(self.shared)


def checked(name, result, rows):
    """What a function returned as a column the file can hold: one flat value per row as NumPy,
    or one list of flat values per row as Awkward; refused otherwise."""
    if isinstance(result, ak.Array):
        form = ak.type(result).content
        jagged = (isinstance(form, ak.types.ListType) and isinstance(form.content, ak.types.NumpyType)
                  and form.parameters.get("__array__") is None)  # text is a list of bytes, and not a column
        if not jagged and not isinstance(form, ak.types.NumpyType):
            raise Refused(f"The column {name} is {ak.type(result)}: neither one number per row nor one list "
                          "of numbers per row.")
        array, numbers = (result, np.dtype(form.content.primitive)) if jagged else (ak.to_numpy(result), None)
    else:
        array, jagged, numbers = np.asarray(result), False, None
    numbers = array.dtype if numbers is None else numbers
    if not jagged and array.ndim != 1:
        raise Refused(f"The column {name} has {array.ndim} dimensions; a column has one value, or one list, per row.")
    if numbers.kind not in "fiub" or rvr.kind(numbers) not in rvr.KINDS:
        raise Refused(f"The column {name} is of kind {numbers}, which an .rvr file does not hold.")
    if len(array) != rows:
        raise Refused(f"The column {name} has {len(array):,} entries; the parent has {rows:,} rows.")
    if not jagged and numbers.kind == "f" and not np.isfinite(array).all():
        raise Refused(f"The column {name} holds a value that is not finite.")
    return array


def publish(written, target, replace):
    """Put a whole written file in place under its name; without replace, never over another file."""
    if replace:
        return os.replace(written, target)
    try:
        os.link(written, target)  # fails, and changes nothing, when the target exists
    except FileExistsError:
        raise Exists(f"{target} exists") from None
    except OSError:  # a filesystem without hard links
        if target.exists():
            raise Exists(f"{target} exists") from None
        os.replace(written, target)


def column_mark(function, source):
    """Mark a column by its file as the presser does, or by the whole saved source and its name."""
    if extend.own(function):
        return extend.mark(function)
    name = getattr(function, "__name__", "")
    return hashlib.sha256(f"{source}\n{name}".encode()).hexdigest()


def derive(parent, target, source, columns, by, parameters=None, labels=None, runtime=None, replace=False):
    """Write target, an .rvr file holding only columns made from the file at parent; return its header."""
    parent, target = Path(parent), Path(target)
    if not columns:
        raise Refused("There is no column to derive.")
    if not (isinstance(source, str) and isinstance(by, str)):
        raise Refused("The source and the name of the program writing the file must be text.")
    if target.exists() and target.samefile(parent):
        raise Refused("The target is the parent itself.")
    if target.exists() and not replace:
        raise Exists(f"{target} exists")
    above, section = rvr.read(parent)
    data = above["data"]
    if (data.get("sha256") != rvr.data_checksum(data)
            or above["made"].get("sha256") != rvr.made_checksum(above["made"])):
        raise Refused("The parent's header does not match its own checksums. Validate it first.")
    given = rvr.expected_sample_cols(above)
    if given is not None and sorted(given, key=repr) != sorted(data.get("sample_cols", []), key=repr):
        raise Refused("The parent's sample columns are not what its metadata gives. Validate it first.")
    if not isinstance(data.get("sample_cols"), list):
        raise Refused("The parent has no sample columns: it was pressed before they existed. Press it again.")
    held = Columns(above, section)
    clash = [name for name in columns if name in held.records]
    if clash:
        raise Refused(f"The parent already holds {extend.listed(clash)}.")
    clash = [name for name in columns if name in held.shared]
    if clash:
        raise Refused(f"The parent already holds {extend.listed(clash)} as a sample column.")
    if not all(isinstance(name, str) and name and callable(function) for name, function in columns.items()):
        raise Refused("Every column needs a name and a function.")
    try:
        parameters = json.loads(json.dumps(parameters or {}, allow_nan=False))
    except (TypeError, ValueError) as error:
        raise Refused(f"The parameters are not plain JSON values: {error}") from error
    if not isinstance(parameters, dict):
        raise Refused("The parameters must be a dict.")
    made = {name: checked(name, function(held), data["rows"]) for name, function in columns.items()}
    own, arrays = rvr.build(data["samples"], data["files"], held.index(),
                            held.array(data["entry"], "i8", data["rows"], "entry"), made, labels or {})
    own["sample_cols"] = data["sample_cols"]
    derived = {
        "built": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "by": by,
        "parents": [{"data_sha256": data["sha256"], "made_sha256": above["made"]["sha256"],
                     "derived_sha256": above["derived"]["sha256"] if "derived" in above else None,
                     "rows": data["rows"], "path": str(parent), "read": dict(held.read)}],
        "source": source,
        "columns": {name: column_mark(function, source) for name, function in columns.items()},
        "parameters": parameters,
        "runtime": runtime or (f"Python {platform.python_version()} / NumPy {np.__version__} / "
                               f"Awkward {ak.__version__}"),
    }
    header = {"rowver": rvr.VERSION, "data": own, "made": above["made"],
              "derived": {"sha256": rvr.made_checksum(derived), **derived}}
    target.parent.mkdir(parents=True, exist_ok=True)
    written = target.with_name(target.name + SUFFIX)
    try:
        rvr.write(written, header, arrays)
        publish(written, target, replace)
    finally:
        if written.is_file():
            written.unlink()
    return header
