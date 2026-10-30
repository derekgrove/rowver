"""Read .rvr files: columns as NumPy arrays, list columns as Awkward arrays, and whole files as
Arrow tables, pandas data frames or Parquet. `rvr`, the command that makes them, comes with it.

    import rowver
    f = rowver.open("study.rvr")
    f["MET"]                 # a NumPy array, one value per row
    f["Jet_pt"]              # an Awkward array, a list per row
    f.to_pandas()            # every column, with each row's sample and entry
"""

import json

import numpy as np

from ._rowver import Error
from ._rowver import File as _File
from ._rowver import validate as _validate

__all__ = ["Error", "File", "open", "validate"]

#: The NumPy type of each .rvr kind; a file holds its values little-endian.
KINDS = {
    "b1": np.dtype("?"), "i1": np.dtype("i1"), "i2": np.dtype("<i2"), "i4": np.dtype("<i4"),
    "i8": np.dtype("<i8"), "u1": np.dtype("u1"), "u2": np.dtype("<u2"), "u4": np.dtype("<u4"),
    "u8": np.dtype("<u8"), "f4": np.dtype("<f4"), "f8": np.dtype("<f8"),
}

#: The columns every file has besides its own: each row's sample index, and entry number.
ROW = ("sample", "entry")


def _typed(raw, kind):
    """The bytes Rust handed over, seen as values of their kind without a copy (copied only if
    they are not aligned for it, as an empty buffer is not), and kept from being changed."""
    values = raw.view(KINDS[kind])
    if not values.flags.aligned:
        values = values.copy()
    values.flags.writeable = False
    return values


def _names(names, every):
    """Column names asked for: all of `every` when none, one name given alone, or a list."""
    if names is None:
        return list(every)
    return [names] if isinstance(names, str) else list(names)


def _needs(module, what):
    """Import an optional package, or say how to install it."""
    try:
        return __import__(module)
    except ImportError:
        raise ImportError(f"{what} needs {module}: pip install {module}") from None


class File:
    """An open .rvr file. Its header is read and checked when it opens; each column is read when
    it is first asked for, and kept."""

    def __init__(self, path):
        self.path = str(path)
        self._file = _File(self.path)
        self.header = json.loads(self._file.header_text)
        self._held = {}

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
    def files(self):
        """Every input file read: its sample, path, and the rows it gave."""
        return self.header["data"]["files"]

    @property
    def holes(self):
        """What the press chose but could not read: {sample, file, entries, kind, why} per hole."""
        made = self.header["made"]["files"]
        return [{"sample": f["sample"], "file": f["path"], "entries": tuple(h["entries"]),
                 "kind": h["kind"], "why": h["why"]}
                for f in self.files for h in made[f["sample"]]["files"][f["path"]].get("holes", [])]

    @property
    def complete(self):
        """Whether the press read every entry it chose: the file has no holes."""
        return not self.holes

    def __len__(self):
        return self.rows

    def __contains__(self, name):
        return name in self.columns or name in self.derived or name in ROW

    def __repr__(self):
        derived = f", {len(self.derived)} derived" if self.derived else ""
        return f"<rowver.File {self.path!r}: {self.rows} rows, {len(self.columns)} columns{derived}, {len(self.samples)} samples>"

    def __getitem__(self, name):
        """A column by name: a NumPy array, or an Awkward array for a list column. `sample` is
        each row's index into `samples`, and `entry` its entry number in its file's tree."""
        values, offsets = self.read([name])[name]
        if offsets is None:
            return values
        ak = _needs("awkward", "a list column")
        content = ak.contents.ListOffsetArray(ak.index.Index64(offsets), ak.contents.NumpyArray(values))
        return ak.Array(content)

    def read(self, names=None):
        """Columns by name (every one, stored and derived, when not given), the stored ones read at once on every core: name -> (values,
        offsets), where `offsets` is None for a one-value-per-row column and otherwise says that
        row r holds values[offsets[r]:offsets[r + 1]]. The arrays are read-only."""
        names = _names(names, self.columns + self.derived)
        unknown = [n for n in names if n not in self]
        if unknown:
            raise KeyError(f"{', '.join(map(str, unknown))}: not a column of {self.path}; it has {', '.join(self.columns + self.derived)}")
        wanted = [n for n in dict.fromkeys(names) if n not in self._held and n not in ROW]
        for name, (kind, raw, offsets) in zip(wanted, self._file.columns(wanted)):
            if offsets is not None:
                offsets.flags.writeable = False
            self._held[name] = (_typed(raw, kind), offsets)
        for name in ROW:
            if name in names and name not in self._held:
                values = getattr(self._file, name)()
                values.flags.writeable = False
                self._held[name] = (values, None)
        return {n: self._held[n] for n in names}

    def forget(self, *names):
        """Let go of columns held in memory, the ones named or else every one: each is read, or
        worked out, again when next asked for."""
        for name in names or list(self._held):
            self._held.pop(name, None)

    def arrays(self, columns=None):
        """The columns (every one, stored and derived, when not given) as one Awkward record
        array, a record per row."""
        ak = _needs("awkward", "arrays()")
        names = _names(columns, self.columns + self.derived)
        self.read(names)  # at once: derived columns that share what they use work it out once
        return ak.zip({n: self[n] for n in names}, depth_limit=1)

    def _arrow(self, name, values, offsets):
        """One column as Arrow: its values, or for a list column a list (64-bit offsets)."""
        import pyarrow as pa

        return pa.array(values) if offsets is None else pa.LargeListArray.from_arrays(offsets, values)

    def to_arrow(self, columns=None):
        """The columns (all when not given) as a pyarrow Table, with `sample` (each row's sample
        name) and `entry` first; a list column is an Arrow large list."""
        pa = _needs("pyarrow", "to_arrow()")
        names = [n for n in _names(columns, self.columns + self.derived) if n not in ROW]
        read = self.read([*ROW, *names])
        table = {
            "sample": pa.DictionaryArray.from_arrays(read["sample"][0], pa.array(self.samples)),
            "entry": read["entry"][0],
        }
        table.update({n: self._arrow(n, *read[n]) for n in names})
        return pa.table(table)

    def to_pandas(self, columns=None):
        """The columns (all when not given) as a pandas DataFrame, with `sample` (a Categorical)
        and `entry` first. A flat column keeps its NumPy type; a list column needs pyarrow and is
        held as Arrow lists."""
        pd = _needs("pandas", "to_pandas()")
        names = [n for n in _names(columns, self.columns + self.derived) if n not in ROW]
        read = self.read([*ROW, *names])
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
                frame[name] = pd.arrays.ArrowExtensionArray(self._arrow(name, values, offsets))
        return pd.DataFrame(frame)

    def to_parquet(self, path, columns=None):
        """Write the columns (all when not given) to a Parquet file, as `to_arrow` gives them."""
        _needs("pyarrow", "to_parquet()")
        import pyarrow.parquet as pq

        pq.write_table(self.to_arrow(columns), str(path))


def open(path):
    """Open a .rvr file."""
    return File(path)


def validate(path):
    """Check a file completely, as `rvr validate` does: its (rows, columns), or Error saying
    what is wrong."""
    return _validate(str(path))
