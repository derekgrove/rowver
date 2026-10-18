# The `.rvr` file

Version 13. `crates/rowver` reads and writes everything here and is the reference builder.
The Python RowVer (`rowver/rvr.py`, `rowver/derive.py` at tag `v13.0.1`, now retired) wrote
it first, which is why its checksums follow Python's `json.dumps`.

New in 13: every per-row array carries one checksum per sample (`by_sample`),
and a file may hold only columns derived from another file's columns, naming
that parent under its own checksums (`derived`). Each sample's metadata is kept
in `made`, and the metadata that is one number, one text or one truth value per
sample is also a sample column (`data.sample_cols`).

## Container

```
magic (8 bytes) | u32 little-endian header length | UTF-8 JSON header | arrays
```

- The magic is the 8 bytes `89 52 56 52 0D 0A 1A 0A` (`\x89RVR\r\n\x1a\n`):
  a high byte, the letters `RVR`, then CR LF, Ctrl-Z and LF, so a transfer that
  strips the eighth bit or rewrites line ends breaks the file visibly.
- The header is plain JSON, not compressed. A reader takes the length, parses
  that many bytes, and has every fact about the file without touching the data.
- The arrays follow the header with nothing between or after them. Each array
  is compressed on its own (see Arrays), so a reader decompresses only the
  arrays it needs, and a program that changes only the header writes the new
  header and copies the arrays byte for byte.

A reader refuses a file whose first 8 bytes are not the magic, whose `rowver`
is not 13, or whose arrays do not fill the rest of the file exactly. A
version 12 file is refused: press its study again.

## Header

The top level has exactly `rowver`, `data`, `made`, an optional `derived`,
and zero or more tool keys. Nothing else.

```json
{
  "rowver": 13,
  "data": { },
  "made": { },
  "derived": { },
  "viewer": { }
}
```

A file without `derived` is a pure file: `rowver run` pressed it from a
recipe. A file with `derived` is a derived file (see `derived`).

### `data` — the rows and their columns

```json
{
  "sha256": "…64 hex…",
  "rows": 45147,
  "samples": [
    {"name": "TTJets DiLept",    "rows": 45062},
    {"name": "WJets HT 400-600", "rows": 85}
  ],
  "files": [
    {"sample": "TTJets DiLept",    "path": "/data/ttjets_1.root", "rows": 30211},
    {"sample": "TTJets DiLept",    "path": "/data/ttjets_2.root", "rows": 14851},
    {"sample": "WJets HT 400-600", "path": "/data/wjets.root",    "rows": 85}
  ],
  "sample": {"at": 0,     "size": 212,   "sha256": "…"},
  "entry":  {"at": 212,   "size": 40115, "sha256": "…", "by_sample": ["…", "…"]},
  "cols": [
    {"name": "MET",     "kind": "f4", "at": 40327, "size": 120433, "sha256": "…",
     "by_sample": ["…", "…"], "lim": [150.1, 1583.2], "nice": [150, 1600]},
    {"name": "channel", "kind": "u4", "at": 160760, "size": 3014, "sha256": "…",
     "by_sample": ["…", "…"], "lim": [0, 2], "nice": [0, 2], "vals": [0, 1, 2],
     "names": {"0": "hadronic", "1": "one lepton", "2": "two or more leptons"}},
    {"name": "Jet_pt",  "kind": "jagged",
     "offsets": {"at": 163774, "size": 51230, "sha256": "…", "by_sample": ["…", "…"]},
     "content": {"kind": "f4", "count": 352738, "at": 215004, "size": 1101877, "sha256": "…",
                 "by_sample": ["…", "…"]}}
  ],
  "sample_cols": [
    {"name": "xsec", "kind": "f8",   "values": [87.3, null],            "sha256": "…"},
    {"name": "kind", "kind": "text", "values": ["background", "background"], "sha256": "…"}
  ]
}
```

- `rows` — total kept events. One row is one event.
- `samples` — in order. Rows of a sample are contiguous and follow this
  order.
- `files` — every input file that was read, in row order: its sample, the
  path or URL it was opened with (a key of that sample's `made.files`), and
  the rows it gave, zero included. A file's rows are contiguous. Within a
  sample, files follow the recipe's file order, however many files were read
  at once, so the same recipe on the same inputs always lays out the same rows.
- `sample` — the array `i4[rows]`: each row's index into `samples`.
- `entry` — the array `i8[rows]`: each row's entry number in its file's tree,
  counted from 0. Within a file each number appears once; the rows follow the
  order `prepare` and `keep` left the events in, which is the entry order unless
  a recipe reorders them. With `files`, it says which
  event every row is, so a later program can read more branches for the same
  rows without running the selection again.
- `cols` — one record per column, in the recipe's `columns` order:
  - flat columns: `name`, `kind`, the array reference (`at`, `size`,
    `sha256`, `by_sample`) of `kind[rows]`, then the column facts: `lim` and `nice`, and on
    integer and boolean columns `vals` and `names` when the distinct values
    are few.
  - jagged columns: `name`, `kind: "jagged"`, `offsets` (the array
    `i8[rows + 1]`) and `content` (its `kind`, its `count`, and the array
    `kind[count]`). Event `i` owns `content[offsets[i] : offsets[i + 1]]`.
- `sample_cols` — the sample columns, a list that is always there and may be
  empty (see Sample columns).
- `sha256` — see Checksums.

There is no percentile ladder. A viewer holding a column can measure a
quantile itself.

#### Sample columns

A sample column holds one value per sample, not one per row. It is a key of
the samples' metadata (`made.files[sample].metadata`) written a second time
as a column, so that a cross-section or a luminosity can be multiplied with a
per-row weight without being stored on every row.

Each record has exactly `name`, `kind`, `values` and `sha256`:

- `name` — the metadata key.
- `kind` — `b1`, `i8`, `f8` or `text`.
- `values` — one JSON value per entry of `data.samples`, in that order. A
  sample left out of the file is not in `data.samples` and has no value.
- `sha256` — the SHA-256 of the values as bytes, so that it does not depend on
  how a program spells a number in JSON. For `b1`, `i8` and `f8`: the values as
  little-endian bytes of that kind, one after another, with a missing `f8`
  value as the eight bytes `00 00 00 00 00 00 f8 7f`. For `text`: for each
  value in turn the byte `00` when it is missing, and otherwise the byte `01`,
  the text as UTF-8, and the byte `00`. It is not part of `data.sha256`.

The stored `kind` is what a reader uses; it need not work the kind out again.
The order of `sample_cols` carries no meaning.

The value of a sample column on row `r` is `values[sample[r]]`, with `sample`
the array of each row's sample index. `rvr.sample_values(header, name)` gives
the values as stored, and `rvr.sample_column(header, section, name)` gives one
value per row: a NumPy `f8` array with NaN where the value is `null`, an `i8`
array, a `bool` array, or for `text` an object array of `str` and `None`.

Which keys are sample columns is decided over the samples in the file. A
sample has a value for a key when its metadata holds the key and the value
is not `null`; otherwise it lacks it. With the values the samples have:

| the values | every sample has one | kind |
|---|---|---|
| all `true` or `false` | yes | `b1` |
| all whole numbers that fit in a signed 64-bit integer | yes | `i8` |
| whole numbers or finite floats, at least one a float | yes or no | `f8` |
| all whole numbers | no | `f8` |
| all text | yes or no | `text` |

A sample that lacks the key has `null`, which only `f8` and `text` allow. A
truth value is not a number. Every other key is not a sample column and stays
only in `made`: a list or an object, values of more than one of these types,
truth values that a sample lacks, whole numbers one of which does not fit in
64 bits when every sample has one, a number too large for an `f8`, a key
that no sample has a value for, and a key that `metadata_omitted` names in
any sample. A key with the name of a column in `cols` is not a sample column
either: the row column has the name. Nor are `sample`, `entry` and `file`.

A value is written as `made` holds it: a whole number in an `f8` column stays
a whole number (`59`, not `59.0`), and the `sha256` is of that text. The
columns are in the order their keys first appear, going through the samples'
metadata in `data.samples` order. `sample_cols` and `made` say the same thing
twice, and `rowver validate` checks that they agree in both directions.

A viewer never rewrites `data`. The presser writes it once, and may later
add columns to it (`rowver run --onto`): the rows, `samples`, `files`,
`sample`, `entry` and `sample_cols` stay as they are, and an array already held is moved
as the bytes it is, never made again, so its `sha256` and `by_sample` do
not change. A derived file is refused: nothing is added to it. So is a
new column with the name of a sample column the file holds: a press of that
recipe gives the name to the row column and writes no such sample column, so
press again. The
new columns take their places in the recipe's `columns` order, every `at`
is counted again, and `data.sha256` changes, because it names every array.
The file that results has the same `data` as a press of the new recipe,
as far as the marks of `made.cart.hooks` can tell that nothing else changed.

### `made` — the recipe snapshot

```json
{
  "sha256": "…64 hex…",
  "study": "my_study",
  "title": "My study",
  "note": "Two local UL18 NanoAOD samples with a generator weight column.",
  "built": "2026-09-12T18:03:11Z",
  "by": "rowver 13",
  "cart": {
    "sha256": "…64 hex…",
    "source": "…the whole cart.py…",
    "modules": {"helpers.py": {"sha256": "…64 hex…", "source": "…"}},
    "hooks": {
      "keep": "…64 hex…",
      "prepare": "…64 hex…",
      "metadata": "…64 hex…",
      "columns": {"MET": "…64 hex…", "channel": "…64 hex…", "Jet_pt": "…64 hex…"}
    }
  },
  "files": {
    "TTJets DiLept": {
      "files": {
        "/data/ttjets_1.root": {
          "tree": "Events",
          "uuid": "…ROOT file UUID…",
          "entries": 219310
        }
      },
      "metadata": {"xsec": 87.3, "lumi": 59.8, "kind": "background"}
    }
  }
}
```

`title` and `note` are copied out of the cart, because the viewer
cannot run Python to find them. Present only when the recipe set them.
Nothing else is reconstructed from the snapshot.

`cart.modules` holds every Python file inside the study folder, other
than `cart.py`, that was imported when the recipe loaded: a `fileset.py`
or `helpers.py` beside it, or `util/more.py` below it. Keys are paths
relative to the study folder with `/` separators, in sorted order; each
value holds the file's text, decoded as Python decodes source, and the
SHA-256 of that text as UTF-8. It is present only when there is at least
one such module. `cart.py` and its modules are read as the recipe loads,
before any events, so a sequential and a parallel press record the same
snapshot. Not copied: installed packages (anything under a
`site-packages`, or a Python environment kept inside the study folder),
files the recipe opens as data, and modules a hook imports only while
events are read.

`cart.hooks` holds a mark of what chose the rows and made each column:
`keep`, `prepare` when the recipe has one, `metadata` for the samples'
metadata (every hook is given it), and under `columns` every column by name.
A mark is a SHA-256. For a column copied from a branch or field it is of
`branch <name>`. For a function it is of the function's code as Python
writes it back from its syntax tree (`ast.unparse`), so a comment, the
layout, and whatever else shares a lambda's line are not part of it, then of
every plain value (a number, text, or a list or dict of those) and every
function of the study that it names or closes over, those functions marked
the same way. An installed package is not part of a mark, nor is a value
that is not plain. The presser reads the marks when it adds columns to a
file (`rowver run --onto`): it adds nothing unless `keep`, `prepare`, the
metadata and every column the file holds have the marks they had, and unless
every input file, read again, gives exactly the rows `data.entry` holds for
it. A file written before the marks existed has no `cart.hooks`; it is
proven by its rows alone, and stays without marks when columns are added.

`files` records the inputs actually processed, grouped by their recipe
sample names. Each sample holds a `files` dict keyed by the full path or
URL used to open the ROOT file. Its value contains the tree name, the UUID
stored inside that ROOT file, and the tree's entry count before selection.
When the recipe reads only the first entries of a file, written
`{path: {"tree": "Events", "stop": 1000000}}`, the record also holds
`stop`, the number of entries read.
These facts come from the already-open input, including files with no kept
events. They preserve the resolved inputs even when `cart.py` imports or
builds its fileset. Callable objects are not copied.

Each sample's `metadata` is the recipe's metadata for that sample, kept so that
facts such as a cross-section or luminosity travel with the file. It holds every
value that is plain: text, a whole number, a finite float, `true`, `false`,
`null`, or a list or object of those with text keys. The keys that hold one
number, text or truth value per sample are written again as `data.sample_cols`
(see Sample columns). A value that is not plain
is left out, and its key is then named in `metadata_omitted`, a list that is
written only when something was left out. The `file` key the presser adds for a
hook is not stored. `made.sha256` covers the metadata like the rest of `made`,
and a derived file carries it unchanged inside its parent's `made`.

### `derived` — columns made from another file's columns

A derived file holds only columns computed from the columns of another
`.rvr` file, its parent. It does not copy the parent's columns; it names
the parent by its checksums.

- `data.rows`, `data.samples` and `data.files` are the parent's, and the
  `sample` and `entry` arrays hold the parent's values. Both arrays are
  stored in this file too; they are small. Row `i` of this file is row `i`
  of the parent.
- `data.cols` holds only the derived columns. No derived column has the
  name of a column or a sample column its parent holds. `data.sha256` covers
  this file's own arrays by the usual rule.
- `data.sample_cols` is the parent's, unchanged.
- `made` is the direct parent's `made`, equal to it as a JSON value, so
  `made.sha256` is the parent's. It still records the recipe, the marks of
  `keep` and `prepare`, and the ROOT files that made the rows.
  `made.cart.hooks.columns` therefore names the pressed file's columns, not
  this file's.

```json
{
  "sha256": "…64 hex…",
  "built": "2026-10-06T19:12:44Z",
  "by": "rowver 13.0.1",
  "parents": [
    {"data_sha256": "…", "made_sha256": "…", "derived_sha256": null,
     "rows": 45147, "path": "/data/my_study.rvr",
     "read": {"MET": "…", "Jet_pt": "…"}}
  ],
  "source": "…the exact Python text that computed the columns…",
  "columns": {"mT2": "…64 hex…", "soft_jets": "…64 hex…"},
  "parameters": {"cut": 30},
  "runtime": "Python 3.13.5 / NumPy 2.1.3 / Awkward 2.7.2"
}
```

- `sha256` — see Checksums.
- `built` — when the file was written, UTC, as in `made`.
- `by` — free text naming the program that wrote the file.
- `parents` — a list of exactly one parent, a list so that more can be
  allowed later:
  - `data_sha256`, `made_sha256` — the parent's `data.sha256` and
    `made.sha256`.
  - `derived_sha256` — the parent's `derived.sha256`, or `null` when the
    parent is a pure file.
  - `rows` — the parent's `data.rows`.
  - `path` — the path the parent was opened with. A hint only: the
    checksums say which file the parent is.
  - `read` — the parent columns and sample columns that were read, by name.
    A flat column's value is its array's `sha256`. A jagged column's is the
    SHA-256 of the ASCII text of its offsets' `sha256` followed by its
    content's `sha256`, with nothing between them. A sample column's is its
    `sha256`.
- `source` — the exact Python text that computed the columns.
- `columns` — every column of the file by name, in `data.cols` order, each
  with a mark: the SHA-256 of the function that made it, by the rule of
  `made.cart.hooks` for a function. A function not written in a file that
  can be read at the time, such as one compiled from `source`, is marked by
  the SHA-256 of the whole of `source`, a newline, and the function's name. Any
  change to `source` therefore changes the mark of every such column.
- `parameters` — an object of plain JSON values, empty when there are none.
- `runtime` — free text naming what ran `source`.

A parent may itself be derived. A derive step reads the columns its direct
parent holds and no others: a column of the parent's own parent is not
looked up. `made` is then the same object down the whole chain, and each
file's `derived.parents[0].derived_sha256` names the step before it.

`rowver.derive.derive(parent, target, source, columns, by, parameters=None,
labels=None, runtime=None, replace=False)` writes a derived file and returns
its header. `columns` is `{name: function}`; each function is called once
with a mapping of the parent's own columns, NumPy for flat and Awkward for
jagged, which decompresses a column, and checks it against its `sha256`,
only when it is asked for, so `read` names exactly the columns used. The
mapping also holds the parent's sample columns by name, each as one value per
row (as `rvr.sample_column` gives it), checked against its `sha256` and
recorded in `read` when it is asked for; `name in columns` reads nothing. A
result must be one number per row or one list of numbers per row, of a
kind in the table under Arrays; a flat float column must be finite.
`labels` is `{column: {value: label}}` as in a recipe. Nothing is written
when there is no column, when a name is one the parent holds as a column or
a sample column, or when a result is not a column. The file is written whole beside its target and
then put in place, and a target that exists is left alone unless `replace`
is true.

### Checksums

Every array reference carries `sha256`: the SHA-256 of that array's
uncompressed little-endian bytes (after undoing the shuffle), as 64
hexadecimal characters.

`data.sha256` is the SHA-256 of the ASCII text made by joining every array's
`sha256`, with nothing between them, in file order (see Arrays). It
fingerprints all the data without decompressing any of it. It does not cover
the `data` JSON describing the arrays.

Every array with one value, or one offset step, per row also carries
`by_sample`: a list of SHA-256 values, 64 hexadecimal characters each, one
per entry of `data.samples`, in that order. These are `entry`, every flat
column, and both arrays of every jagged column. `sample` has none: it is
constant within a sample. With sample `s` on rows `[r0, r1)`:

- `entry` and a flat column: the SHA-256 of the uncompressed little-endian
  bytes of `values[r0:r1]`.
- jagged `offsets`: the SHA-256 of the little-endian `i8` bytes of
  `offsets[r0 : r1 + 1] - offsets[r0]`, which is `r1 - r0 + 1` values starting
  at 0. Counting from the sample's own first offset means a change in an
  earlier sample does not alter a later sample's checksum.
- jagged `content`: the SHA-256 of the little-endian bytes of
  `content[offsets[r0] : offsets[r1]]`.
- a sample with no rows: the SHA-256 of zero bytes for `entry`, a flat column
  and jagged content, and of the single `i8` value 0 for jagged offsets.

`by_sample` is not part of `data.sha256`: it can be worked out from the
arrays, and `rowver validate` checks it. Two headers are enough to say which
column of which sample changed between two presses, with no array
decompressed; `rowver diff A.rvr B.rvr` does that. It matches samples and
columns by name, and reports the rows of each file, the samples and columns
only one file holds, the samples whose row counts differ, the columns whose
kinds differ, and each (column, sample) whose `by_sample` differs. For the
sample columns it reports the ones only one file holds, the ones whose kinds
differ, and each sample whose value differs, with both values.

`made.sha256` covers the complete `made` object, including `files`, except
its own top-level `sha256` field. Serialize that object with
`json.dumps(body, sort_keys=True, separators=(",", ":"), ensure_ascii=False)`,
encode it as UTF-8, and take its SHA-256 as 64 hexadecimal characters.
`made.cart.sha256` covers the saved recipe text alone, and each
`made.cart.modules[path].sha256` covers that module's text alone.
`derived.sha256` covers the complete `derived` object except its own
top-level `sha256` field, serialized exactly as `made` is.

`rowver run` and `rowver.derive.derive` create every checksum when building
a file. Reading and writing preserve them without recomputing them;
`rowver validate` checks them. Tool keys are outside every checksum.

### Tool keys

One top-level object per tool, named for the tool. `rowver run` never writes
one. Any program that saves an `.rvr` writes back every tool key it found.

## Arrays

An array reference is `{"at", "size", "sha256"}`, with `by_sample` on every
array but `sample` (see Checksums). `at` counts bytes from the
start of the arrays (the byte after the header), so a new header of another
length leaves every `at` unchanged. `size` is the compressed length. The
uncompressed length is the number of values times the kind's width: `rows`
values for `sample`, `entry` and flat columns, `rows + 1` for jagged offsets,
`count` for jagged content.

Each array is stored as:

1. its values as little-endian bytes;
2. byte-shuffled, when the kind is wider than one byte: every value's first
   byte, then every value's second byte, and so on. With `n` values of width
   `w`, output byte `j * n + i` is input byte `i * w + j`. Shuffling puts the
   slowly changing bytes of neighbouring values together, which compresses
   better;
3. compressed as one zlib stream (RFC 1950: deflate with a 2-byte header and
   an Adler-32 check), which Python's `zlib`, a browser's
   `DecompressionStream("deflate")` and Rust's `flate2` all read.

The arrays are written in file order, one after another from `at` 0, with no
gap between them and nothing after the last: `sample`, `entry`, then each
column in `cols` order (a jagged column is offsets then content).

Kinds and widths:

| kind | type | bytes per value |
|---|---|---|
| `f8`, `f4` | float | 8, 4 |
| `i8`, `i4`, `i2`, `i1` | signed integer | 8, 4, 2, 1 |
| `u8`, `u4`, `u2`, `u1` | unsigned integer | 8, 4, 2, 1 |
| `b1` | boolean | 1 |
| `jagged` | offsets `i8[rows + 1]` plus content of any flat kind | two arrays |

`b1` is one byte per value, 0 or 1, not packed bits.

A column's kind is whatever NumPy dtype the skim produced. A copied
column keeps its branch dtype; a derived column keeps what the hook returned.
No promotion rule.

## What `rowver validate` checks

One line per check, on any number of files:

- the magic, `rowver: 13`, and `data` and `made` at the top level;
- the arrays fill the file in file order, end to end, and each decompresses
  to its declared length and has its `sha256`; `data.sha256` matches;
- sample rows and file rows sum to `rows`; the sample index runs through the
  samples in order; each file's rows are its sample's, with distinct entry
  numbers inside the entries read; `data.files` matches `made.files`;
- every `by_sample` has one checksum per sample, each matching that sample's
  values;
- jagged offsets start at 0, never decrease and end at `count`; flat floats
  are finite; `lim`, `nice`, `vals` and `names` agree with the values;
- `sample_cols` is a list; each sample column has `name`, `kind`, `values`
  and `sha256`, a kind of the four, and one value per sample that its kind
  allows; the names are distinct and none is a column's, `sample`, `entry` or
  `file`; each `sha256` matches its values; and the list is exactly what the
  metadata in `made.files` gives by the rules of Sample columns. In
  a derived file the names left to row columns are the pressed file's, which
  `made.cart.hooks.columns` holds;
- `made` has its fields, its file records are complete, `made.cart.sha256`
  and each module's `sha256` match their text, and `made.sha256` matches;
- in a pure file with `made.cart.hooks`: the hooks mark `keep` and exactly
  the file's columns, in order;
- in a derived file instead: `derived` has every field of its kind and
  exactly one parent; `derived.columns` names exactly the file's columns, in
  order; `made.sha256` and `data.rows` are the `made_sha256` and `rows`
  recorded for the parent; `derived.sha256` matches.

A derived file's parent is checked only when it is named:
`rowver validate DERIVED.rvr --parent PARENT.rvr`. The parent is then
validated as a file itself, and compared with the derived file from its
header: its `data.sha256`, `made` and `made.sha256`, and `derived.sha256`
(absent for `null`) are the ones recorded; its rows, `samples` and `files`
are the derived file's; its `sample` and `entry` arrays have the derived
file's `sha256`s; its `sample_cols` are the derived file's; every column and
sample column in `read` has the checksum recorded; and it holds no column or
sample column of the derived file's names. Without `--parent`, one line
says the parent was not checked, and the file can still be valid. With
`--parent` on a file that is not derived, one line says the parent was not
compared, and the file can still be valid.

## Column facts

- `lim` — `[min, max]` of the values. For `b1`, the 0 and 1 that occur.
- `nice` — `[lo, hi]` enclosing `lim`, rounded outward to two significant
  digits, snapped to integers for integer kinds, widened by one when the two
  ends coincide.
- `vals` — every distinct value, ascending, on integer and `b1` columns.
  Written when there are at most 64 distinct values.
- `names` — `{"value": "label"}` from the recipe's `labels`, only for
  members of `vals`.

An empty column gets `lim: [0, 0]`, `nice: [0, 1]`, and `vals: []` where
`vals` applies.

A reader that loads a column later than the header can check it then: the
sha256, the facts against the values, finite floats, and 0 or 1 in `b1`.

## Selection

Before `prepare` runs, every event gets a hidden field holding its entry
number, and after `keep` each surviving event's field becomes its row's `entry`.
So `prepare` may add fields, select events or reorder them, and `keep` may
select them any way awkward allows; the entry numbers follow the events. A
`prepare` that returns new events without the fields it was given stops the
press with a recipe error.

## Weights

The file stores no weights. A weight is an ordinary column the recipe
declares, such as the ntuple's `weight` branch or a hook that multiplies
several factors. A factor that is one number per sample, such as a
cross-section, can stay in the metadata and be read as a sample column. A
viewer decides which columns weight events and how.

## Not in the file

Stored weights or a per-sample weighted flag. Per-column provenance. A
structured copy of hooks. Sample metadata per row. Percentiles. Any older version:
a reader of version 13 reads version 13 only. A parent's columns inside a
derived file.
