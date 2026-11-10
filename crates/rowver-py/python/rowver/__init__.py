"""Read .rvr files: columns as NumPy arrays, list columns as Awkward arrays, and whole files as
Arrow tables, pandas data frames or Parquet. `rvr`, the command that makes them, comes with it.

    import rowver
    f = rowver.open("study.rvr")
    f["MET"]                 # a NumPy array, one value per row
    f["Jet_pt"]              # an Awkward array, a list per row
    f.to_pandas()            # every column, with each row's sample and entry
    f.show()                 # every name f[name] reads, and its kind
"""

import difflib as _difflib
import json as _json
import operator as _operator
import os as _os
import weakref as _weakref

import numpy as _np

from ._rowver import Error
from ._rowver import File as _File
from ._rowver import drop_tool as _drop_tool
from ._rowver import set_tool as _set_tool
from ._rowver import validate as _validate

__all__ = ["Error", "File", "open", "validate", "set_tool", "drop_tool"]

# The NumPy type of each .rvr kind; a file holds its values little-endian.
_KINDS = {
    "b1": _np.dtype("?"), "i1": _np.dtype("i1"), "i2": _np.dtype("<i2"), "i4": _np.dtype("<i4"),
    "i8": _np.dtype("<i8"), "u1": _np.dtype("u1"), "u2": _np.dtype("<u2"), "u4": _np.dtype("<u4"),
    "u8": _np.dtype("<u8"), "f4": _np.dtype("<f4"), "f8": _np.dtype("<f8"),
}

# The columns every file has besides its own: each row's sample index, and entry number.
_ROW = ("sample", "entry")


def _typed(raw, kind):
    """The bytes Rust handed over, seen as values of their kind without a copy (copied only if
    they are not aligned for it, as an empty buffer is not), and kept from being changed."""
    values = raw.view(_KINDS[kind])
    if not values.flags.aligned:
        values = values.copy()
    values.flags.writeable = False
    return values


def _names(names, every):
    """Column names asked for: all of `every` when none, one name given alone, or a list."""
    if names is None:
        return list(every)
    if isinstance(names, str):
        return [names]
    if isinstance(names, (bytes, bytearray)) or not hasattr(names, "__iter__"):
        raise TypeError(f"{names!r} names no column: give a column's name, or a list of them")
    names = list(names)
    for name in names:
        if not isinstance(name, str):
            raise TypeError(f"{name!r} is not a column's name: name each column by its text")
    return names


def _listed(values, offsets):
    """One column as `f[name]` gives it: its values, or for a list column an Awkward array."""
    if offsets is None:
        return values
    ak = _needs("awkward", "a list column")
    return ak.Array(ak.contents.ListOffsetArray(ak.index.Index64(offsets), ak.contents.NumpyArray(values)))


def _records(values, offsets):
    """One column as a field of an Awkward record array: text (objects) as strings, None where there is none."""
    if offsets is None and values.dtype == object:
        import awkward as ak

        return ak.Array(values.tolist())
    return _listed(values, offsets)


def _kind(record):
    """A column's kind in words, as `rvr dump` says it: `f4`, or `jagged f4` for a list column."""
    return f"jagged {record['content']['kind']}" if record["kind"] == "jagged" else record["kind"]


def _arrow(values, offsets):
    """One column as Arrow: its values, or for a list column a list (64-bit offsets)."""
    import pyarrow as pa

    return pa.array(values) if offsets is None else pa.LargeListArray.from_arrays(offsets, values)


def _needs(module, what):
    """Import an optional package, or say how to install it."""
    try:
        return __import__(module)
    except ImportError:
        raise ImportError(f"{what} needs {module}: pip install {module}") from None


def _whole(text):
    """A whole number of JSON text: an int, or a Decimal past the digits Python turns into an int."""
    try:
        return int(text)
    except ValueError:
        import decimal

        return decimal.Decimal(text)


def _real(text):
    """A JSON number with a fraction or exponent: a float, or a Decimal where a float would be infinite, or 0 of one that is not."""
    value = float(text)
    if value in (float("inf"), float("-inf")) or (value == 0 and text.lower().split("e")[0].strip("-0.")):
        import decimal

        return decimal.Decimal(text)
    return value


def _once(pairs):
    """A JSON object's members as a dict, refused when it holds a name twice (a dict keeps one)."""
    held = dict(pairs)
    if len(held) != len(pairs):
        raise ValueError("a member is held twice")
    return held


class File:
    """An open .rvr file. Its header is read and checked when it opens; each column, stored or
    derived, is read when it is asked for, and is the same array for as long as anything still
    holds it: the file itself keeps none in memory. It pickles as its path, and opens again."""

    def __init__(self, path):
        self.path = str(path)
        self._file = _File(self.path)
        # A tool key may hold a whole number past the digits Python turns into an int, or a number past a float.
        self.header = _json.loads(self._file.header_text, parse_int=_whole, parse_float=_real)
        # What the header says, kept so that it still reads once the file is closed.
        self._empty, self._holes, self._tools = self._file.empty, self._file.holes(), self._file.tools()
        # What never changes, worked out once: the names f[name] reads, and each sample's rows.
        self._keys = dict.fromkeys((*_ROW, *self.columns, *self.derived))
        # The sample columns f[name] gives a value per row of, and with them every name it reads.
        self._per_sample = {c["name"]: c for c in self.header["data"].get("sample_cols", []) if c["name"] not in self._keys}
        self._keys.update(dict.fromkeys(self._per_sample))
        self._slices, start = {}, 0
        for sample in self.header["data"]["samples"]:
            self._slices[sample["name"]] = slice(start, start + sample["rows"])
            start += sample["rows"]
        # Each column handed out, by name, for as long as its arrays live: weak references to them.
        self._held = {}
        # List columns of the same lists share one offsets array in the file (docs/FORMAT.md §4),
        # and here: each such array by where it lies, while a column read still holds it.
        self._shared = {c["name"]: (c["offsets"]["at"], c["offsets"]["size"])
                        for c in self.header["data"]["cols"] if c["kind"] == "jagged" and c["offsets"]["size"]}
        self._offsets = _weakref.WeakValueDictionary()

    def __reduce__(self):
        return (File, (_os.path.abspath(self.path),))

    def _ipython_key_completions_(self):
        return self.keys()

    def __enter__(self):
        return self

    def __exit__(self, *raised):
        self.close()

    def close(self):
        """Let go of the open file; arrays handed out, and the header, stay."""
        self._file, self._held = None, {}

    def _open(self):
        """The open file, or Error once it is closed."""
        file = self._file  # read once: another thread may close it meanwhile
        if file is None:
            raise Error(f"{self.path} is closed: open it again with rowver.open to read its columns")
        return file

    @property
    def rows(self):
        """How many rows (kept events) the file holds."""
        return self.header["data"]["rows"]

    @property
    def samples(self):
        """The samples' names, in the order their rows come."""
        return [s["name"] for s in self.header["data"]["samples"]]

    @property
    def columns(self):
        """The names of the row columns, in the file's order."""
        return [c["name"] for c in self.header["data"]["cols"]]

    @property
    def derived(self):
        """The names of the derived columns: each kept as the expression that computes it from the
        file's columns, worked out when first asked for, and read as any other column."""
        return [c["name"] for c in self.header.get("derived", {}).get("columns", [])]

    @property
    def sample_columns(self):
        """Each sample column (one value per sample, from its metadata): name -> values."""
        return {c["name"]: c["values"] for c in self.header["data"].get("sample_cols", [])}

    @property
    def labels(self):
        """The samples' labels, what a plot shows, in the order of `samples`."""
        return [s["label"] for s in self.header["data"]["samples"]]

    @property
    def recipe(self):
        """The text of the recipe.toml that made the file."""
        return self.header["made"]["recipe"]["text"]

    @property
    def study(self):
        """The study the file was pressed from, as the texts of its three files, by file name:
        recipe.toml, samples.toml and samples.lock."""
        made = self.header["made"]
        return {"recipe.toml": made["recipe"]["text"], "samples.toml": made["samples"]["text"],
                "samples.lock": made["lock"]["text"]}

    @property
    def signed_by(self):
        """The public key that signed the file (checked when it opened), or None."""
        return self.header.get("signature", {}).get("key")

    @property
    def holes(self):
        """What the press chose but could not read, per hole: {sample, file, entries, kind, why},
        `entries` as the file records them, (first, last) with `last` not among them."""
        return [{"sample": sample, "file": file, "entries": (first, last), "kind": kind, "why": why}
                for sample, file, first, last, kind, why in self._holes]

    @property
    def tools(self):
        """Each tool key of the file (docs/FORMAT.md §11), by name: its object."""
        return {key: self.header[key] for key in self._tools}

    @property
    def complete(self):
        """Whether the file holds every entry its press chose: it has no holes, and is not empty."""
        return not self.holes and not self.empty

    @property
    def empty(self):
        """Whether the file is empty: its header alone, its columns let go by `rvr empty`
        (`rvr refill` presses them again). Its header reads; its columns raise Error."""
        return self._empty

    def rows_of(self, sample):
        """The rows of the sample of this name, as a slice: a sample's rows come together."""
        if not isinstance(sample, str) or sample not in self._slices:
            raise KeyError(f"{sample}: not a sample of {self.path}; it has {', '.join(self.samples)}")
        return self._slices[sample]

    def keys(self):
        """The names `f[name]` reads: sample and entry, the stored columns, the derived, then the
        sample columns (one value per sample, given per row)."""
        return list(self._keys)

    @property
    def kinds(self):
        """The kind of each name `f[name]` reads, as `rvr dump` says it: `f4`, `jagged f4`, `text`."""
        kinds = {"sample": "i4", "entry": "i8"}
        kinds.update((c["name"], _kind(c)) for c in self.header["data"]["cols"])
        kinds.update((c["name"], _kind(c)) for c in self.header.get("derived", {}).get("columns", []))
        kinds.update((name, c["kind"]) for name, c in self._per_sample.items())
        return kinds

    def show(self):
        """Print every name `f[name]` reads: its kind, and what it is."""
        what = {"sample": "each row's index into samples", "entry": "each row's entry in its file's tree"}
        what.update((c["name"], f"= {c['expression']}") for c in self.header.get("derived", {}).get("columns", []))
        what.update(dict.fromkeys(self._per_sample, "one value per sample"))
        kinds = self.kinds
        wide = max(map(len, kinds))
        print(repr(self))
        for name, kind in kinds.items():
            print(f"{name:{wide}}  {kind:10}  {what.get(name, '')}".rstrip())

    def _unknown(self, names):
        """What a name that is no column raises: the nearest names, or all of them."""
        near = [m for n in names for m in _difflib.get_close_matches(str(n), list(self._keys), 3)]
        has = f"did you mean {' or '.join(dict.fromkeys(near))}? f.keys() lists every one" if near else f"it has {', '.join(self._keys)}"
        return KeyError(f"{', '.join(map(str, names))}: not a column of {self.path}; {has}")

    def _per_row(self, name, codes):
        """A sample column's value for each row of these sample indices: text as objects, and
        whole numbers as int64 and truth values as bool, or, as any other number, as float64 when
        a sample has none, NaN for that sample."""
        column = self._per_sample[name]
        values = column["values"]
        if column["kind"] == "text":
            table = _np.array(values, dtype=object)
        elif column["kind"] in ("i8", "b1") and None not in values:
            table = _np.array(values, dtype=_np.int64 if column["kind"] == "i8" else _np.bool_)
        else:
            table = _np.array([_np.nan if v is None else float(v) for v in values], dtype=_np.float64)
        rows = table[codes]
        rows.flags.writeable = False
        return rows

    def _part(self, name, start, stop):
        """Rows `start` to `stop` of one column, read from the chunks that hold them: (values, offsets)."""
        if name in self._per_sample:
            return self._per_row(name, self._part("sample", start, stop)[0]), None
        kind, raw, offsets = self._open().slice(name, start, stop)
        if offsets is not None:
            offsets.flags.writeable = False
        return _typed(raw, kind), offsets

    # Not iterable: len(f) is its rows, which list(f) would take for the number of names to make
    # room for. f.keys() names the columns.
    __iter__ = __reversed__ = None

    def __len__(self):
        return self.rows

    def __contains__(self, name):
        return isinstance(name, str) and name in self._keys

    def __repr__(self):
        derived = f", {len(self.derived)} derived" if self.derived else ""
        empty = ", empty" if self.empty else ""
        closed = ", closed" if self._file is None else ""
        return f"<rowver.File {self.path!r}: {self.rows} rows, {len(self.columns)} columns{derived}, {len(self.samples)} samples{empty}{closed}>"

    def __getitem__(self, name):
        """A column by name: a NumPy array, or an Awkward array for a list column. `sample` is
        each row's index into `samples`, and `entry` its entry number in its file's tree."""
        if not isinstance(name, str):
            raise TypeError(f"f[...] takes one column's name, not {name!r}: f.read, f.arrays and f.to_pandas take a list of names, and f[name][a:b] or f.slice(name, a, b) gives rows a to b")
        return _listed(*self.read([name])[name])

    def _kept(self, name):
        """The column of this name handed out before, while its arrays live: (values, offsets)."""
        values, offsets = self._held.get(name, (None, None))
        values, listed = values and values(), offsets is not None
        offsets = offsets() if listed else None
        return None if values is None or (listed and offsets is None) else (values, offsets)

    def read(self, names=None, sample=None):
        """Columns by name (every one, stored and derived, when not given), the stored ones read at once on every core: name -> (values,
        offsets), where `offsets` is None for a one-value-per-row column and otherwise says that
        row r holds values[offsets[r]:offsets[r + 1]]. With `sample`, the rows of the sample of
        that name alone, only its chunks read, a column after another. The arrays are read-only:
        copy one to change it."""
        names = _names(names, self.columns + self.derived)
        unknown = [n for n in names if n not in self._keys]
        if unknown:
            raise self._unknown(unknown)
        if sample is not None:
            rows = self.rows_of(sample)
            return {n: self._part(n, rows.start, rows.stop) for n in names}
        # Another thread's close() or forget() meanwhile changes nothing here: the open file is
        # taken once, and what is handed out is noted in the dict there when this read began.
        held = self._held
        got = {n: self._kept(n) for n in dict.fromkeys(names)}
        missing = [n for n, kept in got.items() if kept is None]
        if missing:
            file = self._open()
            wanted = [n for n in missing if n not in _ROW and n not in self._per_sample]
            for name, (kind, raw, offsets) in zip(wanted, file.columns(wanted) if wanted else []):
                if offsets is not None:
                    offsets.flags.writeable = False
                    if name in self._shared:
                        offsets = self._offsets.setdefault(self._shared[name], offsets)
                got[name] = (_typed(raw, kind), offsets)
            for name in _ROW:
                if name in missing:
                    values = getattr(file, name)()
                    values.flags.writeable = False
                    got[name] = (values, None)
            for name in missing:
                if name in self._per_sample:
                    codes = got["sample"][0] if got.get("sample") else self.read(["sample"])["sample"][0]
                    got[name] = (self._per_row(name, codes), None)
                values, offsets = got[name]
                held[name] = (_weakref.ref(values), None if offsets is None else _weakref.ref(offsets))
        return {n: got[n] for n in names}

    def slice(self, name, start=None, stop=None):
        """Rows `start` to `stop` (not included) of a column, as `f[name]` gives it: only the chunks
        of the file that hold them read, each frame checked by its own checksum (a chunk's SHA-256
        is left to `rowver.validate`); a derived column is checked against its record first, once
        for the open file, over every row. A row left out is the file's first or last, a negative
        one counts from its end, and a slice (`f.slice(name, slice(a, b, step))`) reads as one of
        a list does."""
        if not isinstance(name, str):
            raise TypeError(f"{name!r} is not a column's name: name each column by its text")
        if name not in self:
            raise self._unknown([name])
        if isinstance(start, slice):
            if stop is not None:
                raise TypeError("f.slice takes a slice, or a first and a last row, not both")
            rows = range(*start.indices(self.rows))
            if len(rows) == 0:
                return _listed(*self._part(name, 0, 0))
            first = min(rows[0], rows[-1])
            part = _listed(*self._part(name, first, max(rows[0], rows[-1]) + 1))
            return part if rows.step == 1 else part[rows[0] - first::rows.step]
        start, stop = (0 if start is None else start), (self.rows if stop is None else stop)
        try:
            asked = _operator.index(start), _operator.index(stop)
        except TypeError:
            raise TypeError(f"rows {start!r} to {stop!r} of {name} are asked for: give whole numbers, or leave one out") from None
        if min(asked) < -self.rows:
            raise Error(f"rows {asked[0]} to {asked[1]} of {name} are asked for, and the file has {self.rows}: a negative row counts from its end, back to -{self.rows}")
        return _listed(*self._part(name, *(at + self.rows if at < 0 else at for at in asked)))

    def forget(self, *names):
        """Let go of the columns handed out, the ones named or else every one: each is read, or
        worked out, again when next asked for, a new array."""
        for name in names or list(self._held):
            self._held.pop(name, None)

    def arrays(self, columns=None, sample=None):
        """The columns (every one, stored and derived, when not given) as one Awkward record
        array, a record per row, with `sample` (its name, a categorical) and `entry` first; with
        `sample`, that sample's rows alone."""
        ak = _needs("awkward", "arrays()")
        names = [n for n in _names(columns, self.columns + self.derived) if n not in _ROW]
        read = self.read([*_ROW, *names], sample)
        codes = ak.index.Index64(read["sample"][0].astype(_np.int64))
        named = ak.contents.IndexedArray(codes, ak.to_layout(self.samples), parameters={"__array__": "categorical"})
        return ak.zip({"sample": ak.Array(named), "entry": read["entry"][0], **{n: _records(*read[n]) for n in names}}, depth_limit=1)

    def to_arrow(self, columns=None, sample=None):
        """The columns (all when not given) as a pyarrow Table, with `sample` (each row's sample
        name) and `entry` first; a list column is an Arrow large list. With `sample`, that
        sample's rows alone."""
        pa = _needs("pyarrow", "to_arrow()")
        names = [n for n in _names(columns, self.columns + self.derived) if n not in _ROW]
        read = self.read([*_ROW, *names], sample)
        table = {
            "sample": pa.DictionaryArray.from_arrays(read["sample"][0], pa.array(self.samples)),
            "entry": read["entry"][0],
        }
        table.update({n: _arrow(*read[n]) for n in names})
        return pa.table(table)

    def to_pandas(self, columns=None, sample=None):
        """The columns (all when not given) as a pandas DataFrame, with `sample` (a Categorical)
        and `entry` first. A flat column keeps its NumPy type; a list column needs pyarrow and is
        held as Arrow lists. With `sample`, that sample's rows alone."""
        pd = _needs("pandas", "to_pandas()")
        names = [n for n in _names(columns, self.columns + self.derived) if n not in _ROW]
        read = self.read([*_ROW, *names], sample)
        frame = {
            "sample": pd.Categorical.from_codes(read["sample"][0], self.samples),
            "entry": read["entry"][0],
        }
        for name in names:
            values, offsets = read[name]
            if offsets is None:
                frame[name] = values
            else:
                _needs("pyarrow", "to_pandas() of a list column")
                frame[name] = pd.arrays.ArrowExtensionArray(_arrow(values, offsets))
        return pd.DataFrame(frame)

    def to_parquet(self, path, columns=None, sample=None):
        """Write the columns (all when not given) to a Parquet file, as `to_arrow` gives them."""
        _needs("pyarrow", "to_parquet()")
        import pyarrow.parquet as pq

        pq.write_table(self.to_arrow(columns, sample), str(path))


def open(path):
    """Open a .rvr file."""
    return File(path)


def _dumps(value):
    """`value` as JSON text, a Decimal written as its digits, as `open` and `drop_tool` give one."""
    import decimal
    import uuid

    held, mark = [], f"rowver-decimal-{uuid.uuid4()}-"

    def number(o):
        if isinstance(o, decimal.Decimal) and not o.is_finite():
            raise ValueError(f"Out of range Decimal values are not JSON compliant: {o}")
        if isinstance(o, decimal.Decimal):
            held.append(str(o))
            return f"{mark}{len(held) - 1}"
        raise TypeError(f"Object of type {type(o).__name__} is not JSON serializable")

    text = _json.dumps(value, allow_nan=False, default=number)
    for at, digits in enumerate(held):
        text = text.replace(f'"{mark}{at}"', digits, 1)
    return text


def set_tool(path, key, value):
    """Make the dict `value` the tool key `key` of the file at `path` (NaN and infinity raise ValueError)."""
    _set_tool(str(path), key, _dumps(value).encode())


def drop_tool(path, key, *keys):
    """Drop the tool keys named from the file at `path` in one change: each object (or text), alone or as a tuple."""
    held = _drop_tool(str(path), [key, *keys])

    # The file is changed: each copy, its only one, is given back whatever json.loads makes of it,
    # its text where it reads no object, its bytes where they are not UTF-8.
    def loaded(data):
        try:
            text = data.decode()
        except UnicodeDecodeError:
            return data
        try:
            return _json.loads(text, parse_int=_whole, parse_float=_real, object_pairs_hook=_once)
        except Exception:
            return text

    # A key held more than once gives a tuple of its copies, which json.loads never makes.
    objects = tuple(loaded(c[0]) if len(c) == 1 else tuple(loaded(each) for each in c) for c in held)
    return objects if keys else objects[0]


def validate(path):
    """Check a file completely, as `rvr validate` does: its (rows, columns), or Error saying
    what is wrong. An empty file checks out by its header alone; `File.empty` says it is."""
    return _validate(str(path))
