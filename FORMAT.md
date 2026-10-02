# The `.rvr` file

Version 12. `rowver/rvr.py` is the code for everything here.

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
is not 12, or whose arrays do not fill the rest of the file exactly.

## Header

The top level has exactly `rowver`, `data`, `made`, and zero or more tool
keys. Nothing else.

```json
{
  "rowver": 12,
  "data": { },
  "made": { },
  "viewer": { }
}
```

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
  "entry":  {"at": 212,   "size": 40115, "sha256": "…"},
  "cols": [
    {"name": "MET",     "kind": "f4", "at": 40327, "size": 120433, "sha256": "…",
     "lim": [150.1, 1583.2], "nice": [150, 1600]},
    {"name": "channel", "kind": "u4", "at": 160760, "size": 3014, "sha256": "…",
     "lim": [0, 2], "nice": [0, 2], "vals": [0, 1, 2],
     "names": {"0": "hadronic", "1": "one lepton", "2": "two or more leptons"}},
    {"name": "Jet_pt",  "kind": "jagged",
     "offsets": {"at": 163774, "size": 51230, "sha256": "…"},
     "content": {"kind": "f4", "count": 352738, "at": 215004, "size": 1101877, "sha256": "…"}}
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
    `sha256`) of `kind[rows]`, then the column facts: `lim` and `nice`, and on
    integer and boolean columns `vals` and `names` when the distinct values
    are few.
  - jagged columns: `name`, `kind: "jagged"`, `offsets` (the array
    `i8[rows + 1]`) and `content` (its `kind`, its `count`, and the array
    `kind[count]`). Event `i` owns `content[offsets[i] : offsets[i + 1]]`.
- `sha256` — see Checksums.

There is no percentile ladder. A viewer holding a column can measure a
quantile itself.

A viewer never rewrites `data`. The presser writes it once, and may later
add columns to it (`rowver run --onto`): the rows, `samples`, `files`,
`sample` and `entry` stay as they are, and an array already held is moved
as the bytes it is, never made again, so its `sha256` does not change. The
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
  "by": "rowver 12",
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
      }
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
builds its fileset. Sample metadata and callable objects are not copied.

### Checksums

Every array reference carries `sha256`: the SHA-256 of that array's
uncompressed little-endian bytes (after undoing the shuffle), as 64
hexadecimal characters.

`data.sha256` is the SHA-256 of the ASCII text made by joining every array's
`sha256`, with nothing between them, in file order (see Arrays). It
fingerprints all the data without decompressing any of it. It does not cover
the `data` JSON describing the arrays.

`made.sha256` covers the complete `made` object, including `files`, except
its own top-level `sha256` field. Serialize that object with
`json.dumps(body, sort_keys=True, separators=(",", ":"), ensure_ascii=False)`,
encode it as UTF-8, and take its SHA-256 as 64 hexadecimal characters.
`made.cart.sha256` covers the saved recipe text alone, and each
`made.cart.modules[path].sha256` covers that module's text alone.

`rowver run` creates every checksum when building the file. Reading and
writing preserve them without recomputing them; `rowver validate` checks them.
Tool keys are outside every checksum.

### Tool keys

One top-level object per tool, named for the tool. `rowver run` never writes
one. Any program that saves an `.rvr` writes back every tool key it found.

## Arrays

An array reference is `{"at", "size", "sha256"}`. `at` counts bytes from the
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
several factors. A viewer decides which columns weight events and how.

## Not in the file

Stored weights or a per-sample weighted flag. Per-column provenance. A
structured copy of sample metadata or hooks. Percentiles. Any older version:
a reader of version 12 reads version 12 only.
