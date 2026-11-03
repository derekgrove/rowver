# rowver (Python)

Read `.rvr` files in Python, through the same Rust reader as the `rvr` command, which this
package also installs: `pip install rowver` (Python 3.11 or later).

```python
import rowver

f = rowver.open("higgs_tautau.rvr")
f                         # <rowver.File 'higgs_tautau.rvr': 2248 rows, 11 columns, 1 derived, 2 samples>
f.samples, f.columns      # names, in the file's order
f["MET_pt"]               # NumPy array, one value per row (no copy)
f["Muon_pt"]              # Awkward array, a list per row
f["sample"], f["entry"]   # each row's sample index, and its entry in its file's tree
len(f), "MET_pt" in f     # rows; whether it has a column of that name (sample and entry too)
f.path, f.header          # where it is; its JSON header, as a dict
f.derived                 # the derived columns: kept as expressions, worked out when first asked for
f["mean_jet_pt"]          # a derived column, read as any other, checked against what the file records
f.forget("mean_jet_pt")   # let it go from memory (f.forget(), every one); f holds each until then
f.labels                  # each sample's label, what a plot shows
f.sample_columns          # per-sample values: events_read, events_total, metadata
f.recipe                  # the recipe.toml that made it
f.study                   # {"recipe.toml": …, "samples.toml": …, "samples.lock": …}
f.signed_by               # the public key that signed it, or None
f.complete                # whether it holds every entry its press chose (no holes, not empty)
f.empty                   # whether rvr empty let its columns go (rvr refill presses them again)
f.holes                   # what it could not read: [{"sample", "file", "entries", "kind", "why"}, …],
                          # entries (first, last) as the file records them, last not among them

f.arrays()                # every column as one Awkward record array, sample and entry first
f.arrays(["MET_pt"])      # sample, entry, then the columns named
f.to_arrow()              # a pyarrow Table (list columns as Arrow lists), sample and entry first
f.to_pandas()             # a pandas DataFrame, sample (a categorical) and entry first
f.to_parquet("out.parquet")
```

Every column asked for at once (`f.read([...])`, `arrays`, `to_*`) is read in parallel. Only
NumPy is required; Awkward, pyarrow and pandas are used when installed
(`pip install "rowver[all]"`). A file whose header does not check out raises `rowver.Error`, a
path that is not there `FileNotFoundError`; a file changed while it is read raises `rowver.Error`
saying so. `rowver.validate(path)` checks every array and works out every derived column, as
`rvr validate` does, and gives `(rows, columns)`. An open `File` holds one open file, through
which it reads, so a file renamed over it meanwhile is read as it was; dropping the `File`
(`del f`) lets it go (a system allows a few hundred open files: 256 in a macOS shell). These, `rowver.open` and `rowver.File` are the
whole package; every other name in it begins with `_`.

## Building

From this folder, with Rust 1.91 or later: `maturin develop --release` installs it into the
active Python; `maturin build --release` writes a wheel (one for every Python from 3.11).

MIT licensed.
