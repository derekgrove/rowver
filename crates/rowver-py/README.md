# rowver (Python)

Read `.rvr` files in Python, through the same Rust reader as the `rvr` command, which this
package also installs.

```python
import rowver

f = rowver.open("higgs_tautau.rvr")
f                         # <rowver.File 'higgs_tautau.rvr': 2248 rows, 11 columns, 2 samples>
f.samples, f.columns      # names, in the file's order
f["MET_pt"]               # NumPy array, one value per row (no copy)
f["Muon_pt"]              # Awkward array, a list per row
f["sample"], f["entry"]   # each row's sample index, and its entry in its file's tree
f.derived                 # the derived columns: kept as expressions, worked out when first asked for
f["mean_jet_pt"]          # a derived column, read as any other, checked against what the file records
f.forget("mean_jet_pt")   # let columns go from memory (every one when none is named)
f.labels                  # each sample's label, what a plot shows
f.sample_columns          # per-sample values: events_read, events_total, metadata
f.recipe                  # the recipe.toml that made it
f.study                   # {"recipe.toml": …, "samples.toml": …, "samples.lock": …}
f.signed_by               # the public key that signed it, or None
f.complete                # whether the press read every entry it chose (no holes)
f.holes                   # what it could not read: [{"sample", "file", "entries", "kind", "why"}, …]

f.arrays()                # every column as one Awkward record array
f.to_arrow()              # a pyarrow Table (list columns as Arrow lists)
f.to_pandas()             # a pandas DataFrame
f.to_parquet("out.parquet")
```

Every column asked for at once (`f.read([...])`, `arrays`, `to_*`) is read in parallel. Only
NumPy is required; Awkward, pyarrow and pandas are used when installed
(`pip install "rowver[all]"`). A file whose header does not check out raises `rowver.Error`;
`rowver.validate(path)` checks every array and works out every derived column, as `rvr validate` does.

## Building

From this folder, with Rust 1.91 or later: `maturin develop --release` installs it into the
active Python; `maturin build --release` writes a wheel (one for every Python from 3.11).

MIT licensed.
