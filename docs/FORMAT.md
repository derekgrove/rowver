# The `.rvr` format

Version 15 (rowver 15.0). This page specifies every byte of a `.rvr` file. `crates/rowver` reads
and writes all of it and is the reference builder; `rvr press` writes pressed files with it.
[The features](FEATURES.md) describe what the tools around the format do.

**The running example.** Every example below is from one real file, `tiny.rvr`, unless it says
otherwise. It is 6,202 bytes, pressed from two copies of a five-entry test ntuple (one sample
each, `signal` and `background`) with this recipe, and signed with a throwaway key:

```toml
format = "rvr-recipe"
version = 1

[define]

[keep]
jets = "nJet >= 1"

[columns]
branches = ["nJet", "Jet_pt"]
ht = "sum(Jet_pt)"

[totals]

[derived]
mean_jet_pt = "ht / nJet"
```

Checksums are cut to their first 8 hex digits here; a file holds all 64.

## 1. The file at a glance

```
┌─────────────┬──────────────────┬────────────────────────────┬──────────────────────────────┐
│ magic       │ header length    │ header                     │ arrays                       │
│ 8 bytes     │ u32 little-endian│ UTF-8 JSON, not compressed │ each compressed on its own,  │
│             │                  │                            │ end to end, nothing after    │
└─────────────┴──────────────────┴────────────────────────────┴──────────────────────────────┘
 byte 0        byte 8             byte 12                      byte 12 + header length
```

The first bytes of `tiny.rvr`:

```
00000000: 8952 5652 0d0a 1a0a e216 0000 7b22 726f  .RVR........{"ro
          └──────magic──────┘ └length─┘ └─header…
```

- **The magic** is `89 52 56 52 0D 0A 1A 0A` (`\x89RVR\r\n\x1a\n`): a high byte, the letters
  `RVR`, then CR LF, Ctrl-Z and LF. A transfer that strips the eighth bit or rewrites line endings
  therefore breaks the file visibly.
- **The header length** is `E2 16 00 00`, that is 0x16E2 = 5,858 bytes. A reader parses that many
  bytes of JSON and has every fact about the file without touching any array.
- **The arrays** start at byte 12 + 5,858 = 5,870 and fill the rest of the file: 332 bytes here.
  Each is compressed on its own (§7), so a reader decompresses only the arrays it needs. A program
  that changes only the header writes the new header and copies the arrays byte for byte.

A reader refuses a file whose first 8 bytes are not the magic, whose `rowver` is not 15, or whose
arrays do not fill the rest of the file exactly.

**An empty file** is a file's header, byte for byte, with nothing after it: the file ends at byte
12 + header length, while its records still name every array, its checksums and its signature as
they were. It keeps the study it was pressed from, which can press the same arrays again, so it
gives back a file's space and keeps its place (`rvr empty`; `rvr refill` presses the arrays again
and puts them back once each has the `sha256` its record states). A reader opens it and checks its
header as for any file, says it is empty, and refuses to read a column of it; nothing else in the
format marks it. A whole file always has arrays after its header, since even an array of no values
is a compressed frame of a few bytes.

## 2. The header at a glance

```
{
  "rowver": 15,                 the format version
  "data": {                     the rows and their columns (§3 to §5); written once, by the press
    "rows", "samples", "files",
    "sample", "entry",          the two arrays every file has
    "cols",                     one record per column
    "sha256",                   one checksum of all the arrays
    "sample_cols"               one value per sample
  },
  "made": { … },                the study that made the file (§6)
  "derived": { … },             the derived columns, when there are any (§10)
  "signature": { … },           only in a signed file (§9)
  "<tool>": { … }               any number of tool keys (§11)
}
```

The top level has exactly `rowver`, `data`, `made`, an optional `derived`, an optional
`signature`, and zero or more tool keys. Nothing else. No program rewrites `data` once the press
has written it.

## 3. Rows: `data`

One row is one kept event. These are the rows of `tiny.rvr`:

```
               signal (./signal.root)      │ background (./background.root)
row            0      1      2      3      │ 4      5      6      7
sample         0      0      0      0      │ 1      1      1      1
entry          0      1      2      4      │ 0      1      2      4
nJet           3      4      3      1      │ 3      4      3      1
ht             101.7  154.0  104.5  120.5  │ 101.7  154.0  104.5  120.5
```

Entry 3 of each file has no jet, so the cut `nJet >= 1` drops it. The two samples hold the same
rows because their files are copies of one ntuple.

```json
"rows": 8,
"samples": [{"name": "signal", "label": "Signal", "rows": 4},
            {"name": "background", "label": "Background", "rows": 4}],
"files":   [{"sample": "signal", "path": "./signal.root", "rows": 4},
            {"sample": "background", "path": "./background.root", "rows": 4}],
"sample":  {"at": 0,  "size": 50, "sha256": "eeb59ffe"},
"entry":   {"at": 50, "size": 51, "sha256": "02ea099d", "by_sample": ["045487dc", "045487dc"]},
```

| field | holds |
|---|---|
| `rows` | the number of kept events |
| `samples` | every sample in row order, each with exactly `name` (what every tool calls it), `label` (what a plot shows, any text) and `rows` |
| `files` | every input file read, in row order, each with exactly `sample`, `path` (its `at` in the lock, a key of that sample's `made.files[sample].files`) and `rows` (zero included) |
| `sample` | the array `i4[rows]`: each row's index into `samples` |
| `entry` | the array `i8[rows]`: each row's entry number in its file's tree, counted from 0 |
| `cols` | one record per column, in the order `made.recipe.columns` names them (§4) |
| `sha256` | the checksum of all the arrays (§8) |
| `sample_cols` | the sample columns (§5); always present, possibly empty |

The rules of the layout:

- A sample's rows are contiguous, in the order of `samples`. Within a sample, each file's rows
  are contiguous, in the lock's file order however many files were read at once. The same study
  on the same inputs therefore always lays out the same rows.
- Within a file, rows keep the order of their entries. Each entry number appears once, and each
  is an entry the press read (§6.1). With `files`, `entry` says which event every row is, so a
  later program can read more branches for the same rows without running the selection again.
- A file the press read nothing of, because the first N events all lay in files before it, is
  not listed. A file a random choice took nothing from is listed.

## 4. Columns

A column is **flat** (one value per row) or **jagged** (a list per row).

| kind | type | bytes per value |
|---|---|---|
| `f8`, `f4` | float | 8, 4 |
| `i8`, `i4`, `i2`, `i1` | signed integer | 8, 4, 2, 1 |
| `u8`, `u4`, `u2`, `u1` | unsigned integer | 8, 4, 2, 1 |
| `b1` | boolean | 1 (0 or 1, not packed bits) |
| `jagged` | a list per row | two arrays: offsets `i8[rows + 1]`, and content of any flat kind |

A copied column keeps its branch's kind unless the recipe stores it as another. A computed column
has the kind its expression gives. There is no promotion rule.

**A flat column** is its name, its kind, the reference to its array `kind[rows]` (§7), and its
facts:

```json
{"name": "nJet", "kind": "i4", "at": 101, "size": 50, "sha256": "4c0f18a1",
 "by_sample": ["af095d9c", "af095d9c"], "lim": [1, 4], "nice": [1, 4], "vals": [1, 3, 4]}
```

**A jagged column** is two arrays. Row `i` owns `content[offsets[i] : offsets[i + 1]]`:

```json
{"name": "Jet_pt", "kind": "jagged",
 "offsets": {"at": 151, "size": 52, "sha256": "1e8928fc", "by_sample": ["fa00c623", "fa00c623"]},
 "content": {"kind": "f4", "count": 22, "at": 203, "size": 80, "sha256": "89d29a1d",
             "by_sample": ["a5cf09fd", "a5cf09fd"]}}
```

```
offsets   [0, 3, 7, 10, 11, 14, 18, 21, 22]           i8[rows + 1] = i8[9]
content   [65.3125, 19.96875, 16.421875,              row 0 = content[0:3]
           51.75, 49.3125, 31.90625, 21.078125,       row 1 = content[3:7]
           37.03125, 34.03125, 33.40625,              row 2 = content[7:10]
           120.5,                                     row 3 = content[10:11]
           … the same again for rows 4 to 7]          f4[count] = f4[22]
```

### Column facts

Every flat column carries facts about its values, so a reader can draw an axis or a legend without
reading the array:

| fact | holds | in `tiny.rvr` |
|---|---|---|
| `lim` | `[min, max]` of the values; for `b1`, the 0 and 1 that occur | `ht`: `[101.703125, 154.046875]` |
| `nice` | `[lo, hi]` enclosing `lim`, rounded outward to two significant digits, snapped to integers for integer kinds, widened by one when the two ends coincide | `ht`: `[100.0, 160.0]` |
| `vals` | every distinct value, ascending, on integer and `b1` columns with at most 64 distinct values | `nJet`: `[1, 3, 4]` |
| `names` | `{"value": "label"}` for members of `vals` only; optional (`rvr press` writes none) | |

An empty column has `lim: [0, 0]`, `nice: [0, 1]`, and `vals: []` where `vals` applies. There is
no percentile ladder: a reader holding a column can measure a quantile itself.

## 5. Sample columns

A sample column holds one value per sample, not one per row. It is a key of the samples' metadata
(`made.files[sample].metadata`) written a second time as a column. A cross-section or a
luminosity can then multiply a per-row weight without being stored on every row. The sample
columns of `tiny.rvr`:

```
kind           text  signal: "signal", background: "background"
xsec           f8    signal: 2.5,      background: null
events_read    i8    signal: 5,        background: 5
events_total   i8    signal: 5,        background: 5
```

```json
{"name": "xsec", "kind": "f8", "values": [2.5, null], "sha256": "e98f4aa4"}
```

Each record has exactly `name` (the metadata key), `kind`, `values` (one JSON value per entry of
`data.samples`, in that order) and `sha256`. The value on row `r` is `values[sample[r]]`. The
stored kind is what a reader uses; it need not work the kind out again.

**Which keys are sample columns** is decided over the samples in the file. A sample *has* a value
for a key when its metadata holds the key and the value is not `null`; otherwise it *lacks* it.

| the values the samples have | every sample has one | kind |
|---|---|---|
| all `true` or `false` | yes | `b1` |
| all whole numbers that fit in a signed 64-bit integer | yes | `i8` |
| whole numbers or finite floats, at least one a float | yes or no | `f8` |
| all whole numbers | no | `f8` |
| all text | yes or no | `text` |

A sample that lacks the key has `null`, which only `f8` and `text` allow; `xsec` above is `null`
for `background`. A truth value is not a number. Every other key is not a sample column and stays
only in `made`:

- a list or an object;
- values of more than one of these types;
- truth values that some sample lacks;
- whole numbers, every sample having one, of which one does not fit in 64 bits;
- a number too large for an `f8`;
- a key that no sample has a value for;
- a key with the name of a column the recipe pressed (`made.recipe.columns`), since the row
  column has that name, and the names `sample`, `entry` and `file`.

**Order and agreement.** The sample columns are in the order their keys first appear, going
through the samples' metadata in `data.samples` order, but the order carries no meaning. A value
is written as `made` holds it: a whole number in an `f8` column stays a whole number (`59`, not
`59.0`). `sample_cols` and `made` say the same thing twice, and a reader checks that they agree in
both directions.

**The checksum** is of the values as bytes, so it does not depend on how a program writes a number
in JSON. It is not part of `data.sha256`.

| kind | each value, one after another | a missing value |
|---|---|---|
| `b1` | one byte, `00` or `01` | not allowed |
| `i8` | 8 bytes, little-endian | not allowed |
| `f8` | the double, 8 bytes, little-endian (`59` is hashed as `59.0`) | `00 00 00 00 00 00 f8 7f` |
| `text` | `01`, the text as UTF-8, `00` | `00` |

For `tiny.rvr`, `xsec` is the SHA-256 of `2.5` as 8 bytes followed by `00 00 00 00 00 00 f8 7f`
(`e98f4aa4`), and `kind` that of `01 "signal" 00 01 "background" 00` (`023c71f9`).

**The press adds** two keys to every sample's metadata, so that they are sample columns:
`events_read` (the entries it read of the sample, before any cut) and `events_total` (the entries
its files hold). A file with holes (§6.2) adds a third to every sample: `events_missing`, the
entries chosen that lie in holes (0 for a sample with none). `events_read + events_missing` is
then what the event choice chose. A file with no hole has no `events_missing`. Each total of the
recipe's `[totals]` (§6) adds one more, under its name: the sample's sum of it, a number.

## 6. The study: `made`

```json
"made": {
  "study": "tiny",
  "built": "2026-10-31T07:03:04Z",
  "by": "rvr 15.0.0",
  "toolchain": {"rvr": "15.0.0", "rowver": "15", "commit": "ad9d3e6a…",
                "rustc": "rustc 1.91.0 (f8297e351 2025-10-28)", "target": "aarch64-apple-darwin"},
  "recipe":  {"sha256": "937f05be", "text": "…the whole recipe.toml…", "format": "rvr-recipe",
              "columns": ["nJet", "Jet_pt", "ht"]},
  "samples": {"sha256": "aa22df20", "text": "…the whole samples.toml…"},
  "lock":    {"sha256": "8c29824e", "text": "…the whole samples.lock…"},
  "files": {
    "signal": {
      "events": {"choose": "all"},
      "files": {"./signal.root": {"tree": "analysis", "uuid": "31e851e4-b994-11f1-ba5c-1a45bb13c76a",
                                  "entries": 5}},
      "metadata": {"kind": "signal", "xsec": 2.5, "events_read": 5, "events_total": 5}
    },
    "background": { … }
  },
  "sha256": "d5e24c11"
}
```

| field | holds |
|---|---|
| `study` | the study folder's name |
| `title` | the recipe's title; present only when the recipe sets one |
| `built` | when the file was written, UTC to the second |
| `by` | free text naming the program that wrote the file |
| `toolchain` | what built that program, each as text: `rvr` (its version), `rowver` (the format version it writes), `commit`, `rustc` (the compiler's `rustc -V`) and `target` (the platform) |
| `recipe`, `samples`, `lock` | the study's three files, each as its whole `text` and the `sha256` of that text as UTF-8; `recipe` also has `format` (the recipe language, `rvr-recipe`) and `columns` (every column the recipe stores, in the file's order, each named once) |
| `files` | every sample of `data.samples`, by name (below) |
| `sha256` | the checksum of `made` (§8) |

`commit` is the git commit the program was built from, or `unknown`. When its code had changes
not yet committed, `-dirty-` follows, then 12 hex digits of git's hash of those changes, or
`-dirty` alone when they could not be read.

Together, the three texts are the study: write them out as `recipe.toml`, `samples.toml` and
`samples.lock`, and the study can be pressed again. The lock's own record of the `samples.toml` it
was resolved from matches `samples`: in `tiny.rvr` the lock says
`samples_toml_sha256 = "aa22df20…"`, the `sha256` of `made.samples`.

**`made.files[sample]`** has exactly three fields:

- `events`: which of its events the press read (§6.1);
- `files`: every file of the sample in `data.files`, keyed by its `at` in the lock (the path or
  URL it was opened with; for a file of a catalog dataset, its global name
  `root://cms-xrd-global.cern.ch//store/…`, whichever copy was read);
- `metadata`: the sample's metadata, from the lock, as text, whole numbers, finite floats,
  `true`, `false`, `null`, and lists and objects of those. The keys that hold one number, text or
  truth value per sample are written again as `data.sample_cols` (§5).

Each file record holds:

| field | holds |
|---|---|
| `tree` | the tree read |
| `uuid` | the UUID stored inside that ROOT file (or `null`, §6.2) |
| `entries` | the tree's entries |
| `stop` or `take` | how much of it the event choice read (§6.1) |
| `served` | for a file of a catalog dataset, the copy it was read from: exactly `rse` (the storage element, `T2_US_Purdue`) and `host` (its HTTPS door, `eos.cms.rcac.purdue.edu:9000`), both nonempty |
| `holes` | only when the press could not read some of the entries it chose (§6.2) |
| `totals` | only when the recipe has `[totals]` and the press read the file: each total by name, a finite number, its expression added up over the entries the press read of this file before any cut, one at a time in entry order (or over every entry of the other tree it names); a `--fill` adds the sum of the entries it read |

When a read moved to another copy part way, `served` names the copy that served the last read.
A sample's metadata holds each total as the sum of its files' `totals`, added in the order of
`made.files`, so that it does not depend on how many files were read at once; a reader checks
that it does. A total is what a file of kept events cannot give afterwards, such as the sum of
`genWeight` over every event read, which normalizes simulation.
Every copy holds the same bytes: each is checked against the lock's size, and its adler32 when the
storage gives one, before it is read.

### 6.1 The event choice

`events` is one of:

| `events` | the press read | each file records |
|---|---|---|
| `{"choose": "all"}` | every entry of every file | neither `stop` nor `take` |
| `{"choose": "first", "n": N}` | the first N entries, in the files' order | `stop`, the entries read, when it read fewer than the file has |
| `{"choose": "random", "n": N, "seed": S, "algorithm": "splitmix64-selection-sampling"}` | N entries chosen at random over all the files, or all of them when they hold N or fewer | `take`, how many of the chosen entries are in it |

The choice is what the press did: what `samples.toml` asked, unless `rvr press --events`,
`--random` or `--seed` changed it. N and S are whole numbers; S may be any value from 0 to
2^64 − 1.

**First N.** The files, in `data.files` order, read `min(entries, left)` each, with `left`
starting at N and going down by what each file read. A row's entry is below what its file read.

**Random.** The files, in `data.files` order, count as one run of `total` entries, file after
file. The algorithm `splitmix64-selection-sampling` chooses `m = min(N, total)` of them: SplitMix64
gives uniform doubles, and Knuth's selection sampling (Algorithm S) uses them. All arithmetic on
`state` and `z` is mod 2^64.

```
state = S

draw():
    state = state + 0x9E3779B97F4A7C15
    z = state
    z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9
    z = (z ^ (z >> 27)) * 0x94D049BB133111EB
    z = z ^ (z >> 31)
    return (z >> 11) / 2^53                         a double in [0, 1)

needed = m
for i = 0, 1, 2, … while needed > 0:
    u = draw()
    if the double (total − i) × u < needed:
        entry i is chosen; needed = needed − 1
```

Entry `i` of the run is entry `i − start` of the file it falls in, where `start` is the number of
entries in the files before it. Each file's `take` is how many chosen entries fall in it, and a
row's entry is one of them. A reader makes the choice again from the seed to check every row: the
same seed, files and N always give the same choice.

### 6.2 Holes

A press that cannot read part of a file it chose still writes the file, without those events, and
says so in the file's record (this example is not from `tiny.rvr`, which has no hole):

```json
"holes": [
  {"entries": [50000, 100000], "kind": "damaged",
   "why": "basket 12 of branch Muon_pt (entries 50,000 to 53,999) does not decompress: …"}
]
```

Each hole has exactly:

- `entries`: `[first, last]`, two whole numbers with `first < last`, meaning the entries `first`
  to `last − 1` of the file's tree;
- `kind`: one word of the list below, so a tool can group holes and act on them;
- `why`: nonempty text for a person, saying what failed, where, and what to do.

| `kind` | the press found |
|---|---|
| `tape` | every copy of the dataset's file is on tape only |
| `unreachable` | its storage kept failing after every wait (busy, down, not answering) |
| `damaged` | a basket failed to decompress, or another error reading the file's bytes |
| `gone` | the file is not there any more |
| `changed` | the file differs from the lock (size, adler32, ROOT UUID, entries) |
| `refused` | its storage refuses the file with a valid grid proxy |

The list is closed: a reader refuses a kind it does not know, and a new kind is a new format
version.

The rules:

- The holes are in entry order and apart (one ends at or before the next begins). They lie within
  what the file's choice read: below `stop` when there is one, otherwise below `entries`.
- A hole holds at least one entry the choice chose. For a random choice, it is missing just the
  chosen entries inside it.
- Holes do not change the event choice: `stop` and `take` are what they would be without them, so
  a reader still makes the choice again and checks it. No row's entry lies in a hole of its file.
- The key is absent when a file has no hole; an empty list is refused.
- A file the press could not open at all has one hole of every entry it chose (`[0, stop]`, or
  `[0, entries]`), no rows, and the `entries` and `uuid` the lock gave.
- `uuid` is `null` only in two cases. One is a file the press could not open at all whose lock
  gives no UUID for it (a catalog dataset's file). The other is a file the press chose no entry of
  (`take` 0, or no entries) and could not open for its UUID.

## 7. Arrays

An array reference is `{"at", "size", "sha256"}`, with `by_sample` on every array but `sample`
(§8).

- `at` counts bytes from the start of the arrays (the byte after the header), so a header of
  another length leaves every `at` unchanged.
- `size` is the compressed length.
- The uncompressed length is the number of values times the kind's width: `rows` values for
  `sample`, `entry` and flat columns, `rows + 1` for jagged offsets, `count` for jagged content.

The arrays are written in file order, one after another from `at` 0, with no gap between them and
nothing after the last: `sample`, `entry`, then each column in `cols` order, a jagged column as
its offsets then its content. In `tiny.rvr`:

| array | `at` | `size` |
|---|---|---|
| `sample` | 0 | 50 |
| `entry` | 50 | 51 |
| `nJet` | 101 | 50 |
| `Jet_pt` offsets | 151 | 52 |
| `Jet_pt` content | 203 | 80 |
| `ht` | 283 | 49 |
| | 332 | the end of the file |

Each array is stored in three steps:

1. its values as little-endian bytes;
2. **byte-shuffled**, when the kind is wider than one byte: every value's first byte, then every
   value's second byte, and so on. With `n` values of width `w`, output byte `j × n + i` is input
   byte `i × w + j`. Shuffling puts the slowly changing bytes of neighbouring values together,
   which compresses better;
3. **compressed** as Zstandard data (RFC 8878), which decompresses to exactly the shuffled bytes
   and fills `size` exactly. `rvr press` writes one frame per array.

The `sample` array of `tiny.rvr`, step by step:

```
values      0  0  0  0  1  1  1  1                                    i4[8]
bytes       00000000 00000000 00000000 00000000 01000000 01000000 01000000 01000000
shuffled    00 00 00 00 01 01 01 01   then 24 bytes of 00             the first bytes, then the rest
stored      28 b5 2f fd 04 38 …                                       a zstd frame, 50 bytes
```

## 8. Checksums

| checksum | of | on |
|---|---|---|
| array `sha256` | the array's uncompressed little-endian bytes, after undoing the shuffle | every array |
| `data.sha256` | the ASCII text of every array's `sha256` joined, with nothing between them, in file order | `data` |
| `by_sample` | one SHA-256 per sample, of that sample's part of the array (below) | `entry`, every flat column, both arrays of every jagged column |
| `made.sha256` | the canonical JSON (below) of `made` without its own `sha256`, as UTF-8 | `made` |
| a derived column's `sha256` and `by_sample` | its values as they are worked out, as a stored column's are of its array | each derived column |
| `made.recipe.sha256`, `made.samples.sha256`, `made.lock.sha256` | that text alone | the three texts |
| sample column `sha256` | its values as bytes (§5) | each sample column |
| `signature.sha256` | §9 | `signature` |

Every checksum is SHA-256, written as 64 lowercase hexadecimal characters. `rvr press` creates
every checksum when it builds a file. Reading and writing preserve them without recomputing them,
and readers check them. Tool keys are outside every checksum, and outside the signature.

**`data.sha256`** fingerprints all the data without decompressing any of it. It does not cover the
`data` JSON describing the arrays. In `tiny.rvr` it is the SHA-256 of
`"eeb59ffe…" + "02ea099d…" + "4c0f18a1…" + "1e8928fc…" + "89d29a1d…" + "ce0457c7…"` (the full 64
characters of each), which is `1325524b`.

**`by_sample`.** With sample `s` on rows `[r0, r1)`:

- `entry` and a flat column: the SHA-256 of the bytes of `values[r0:r1]`;
- jagged `offsets`: the SHA-256 of the `i8` bytes of `offsets[r0 : r1 + 1] − offsets[r0]`, which
  is `r1 − r0 + 1` values starting at 0;
- jagged `content`: the SHA-256 of the bytes of `content[offsets[r0] : offsets[r1]]`;
- a sample with no rows: the SHA-256 of zero bytes for `entry`, a flat column and jagged content,
  and of the single `i8` value 0 for jagged offsets.

Counting the offsets from the sample's own first offset means a change in an earlier sample does
not change a later sample's checksum. In `tiny.rvr`, `Jet_pt`'s offsets give the same values, and
so the same checksum `fa00c623`, for both samples:

```
offsets      [0, 3, 7, 10, 11, 14, 18, 21, 22]
signal       rows 0–3: offsets[0:5] − 0   = [0, 3, 7, 10, 11]
background   rows 4–7: offsets[4:9] − 11  = [0, 3, 7, 10, 11]
```

`by_sample` is not part of `data.sha256`: it can be worked out from the arrays, and full validation
checks it. `sample` has none, since it is constant within a sample. Two headers are enough to say
which column of which sample changed between two presses, with no array decompressed.

### Canonical JSON

The canonical JSON of a value is:

- **an object**: `{`, its members with their keys sorted by Unicode code point, each as the key's
  canonical string, `:` and the value, with `,` between members, then `}`;
- **an array**: `[`, the members in order with `,` between them, then `]`;
- **a string**: `"` and the text, then `"`. Within it, `"` is written `\"` and `\` is written
  `\\`. The control characters U+0008, U+0009, U+000A, U+000C and U+000D are written `\b`, `\t`,
  `\n`, `\f` and `\r`, and the other characters below U+0020 as `\u00xx` (lowercase hexadecimal).
  Every other character is itself, in UTF-8;
- **`true`, `false` and `null`**: as themselves;
- **a whole number** (written without a fraction or an exponent): its digits as the header writes
  them, however many there are, with a leading `-` when negative and `-0` written `0`;
- **any other number**: the double nearest to it, written with the fewest significant digits that
  read back as that double. With `e` the decimal exponent of its first significant digit, it is
  written in decimal when `-4 ≤ e < 16`, always with a `.` and at least one digit after it.
  Otherwise it is one digit, a `.` and the rest of the digits when there are more, then `e`, a
  sign and at least two exponent digits. Zero is `0.0`, or `-0.0` when negative.

There is no space anywhere outside strings. A whole number is never turned into a double, so a
64-bit seed or byte count is checksummed exactly.

| written | canonical |
|---|---|
| `{ "b": 1, "a": [true, null] }` | `{"a":[true,null],"b":1}` |
| `18446744073709551615` | `18446744073709551615` |
| `-0` | `0` |
| `1.5e2` | `150.0` |
| `0.00010` | `0.0001` |
| `0.00001` | `1e-05` |
| `1234567890123456.0` | `1234567890123456.0` |
| `15000000000000000.0` | `1.5e+16` |

## 9. The signature

A press signs the file when the person pressing it has a key. A file without a `signature` is
unsigned, and still valid.

```json
"signature": {"ed25519": "228e7cf8…", "key": "7c10dc33…", "sha256": "df704d52…"}
```

It has exactly these three members:

| member | holds |
|---|---|
| `sha256` | the SHA-256 of the canonical JSON of an object holding the header's `rowver`, `data`, `made` and, when there is one, `derived` |
| `ed25519` | the Ed25519 signature (RFC 8032) of the 32 bytes of that SHA-256, made with the key, as lowercase hexadecimal (128 characters) |
| `key` | the public key, as lowercase hexadecimal (64 characters) |

A reader refuses a signed file whose `sha256` is not that of its header, or whose signature does
not hold for `key`: either means the header changed after it was signed. Whether `key` belongs to
someone the reader trusts is for the reader to decide.

## 10. Derived columns

A derived column is kept as the expression that computes it, not as an array. A reader works it
out from the file's own columns when it is asked for; how long it keeps the values is the reader's
choice. It is listed in `derived.columns`, in the order the recipe's `[derived]` table gives:

```json
"derived": {"columns": [
  {"name": "mean_jet_pt", "expression": "ht / nJet", "kind": "f8", "sha256": "e641687e",
   "by_sample": ["e73c3af8", "e73c3af8"], "lim": [33.901041666666664, 120.5], "nice": [33.0, 130.0]}
]}
```

| field | holds |
|---|---|
| `name` | what every tool calls it: a name no stored column, sample column or other derived column has, nor `sample`, `entry` or `file` |
| `expression` | the expression that computes it, as the recipe writes it |
| `kind` and the rest | what a stored column's record holds, without `at` and `size`: for a flat column `sha256`, `by_sample` and its facts (`lim`, `nice`, and `vals` and `names` where they apply, §4); for a jagged one `offsets` (`sha256`, `by_sample`) and `content` (`kind`, `count`, `sha256`, `by_sample`) |

The checksums and facts are of the column's values over every row, as a stored column's are of
its array (§8): the press works each derived column out once, to record them. In `tiny.rvr`,
`mean_jet_pt` is `ht / nJet` on each row, `[33.901041666666664, 38.51171875, 34.822916666666664,
120.5, …]`, and `e641687e` is the SHA-256 of those eight doubles.

An expression uses only what the file holds: its stored columns, the derived columns above it,
and `sample.KEY` (the metadata of the row's sample, as a press gives it to an expression: without
`events_read`, `events_total` and `events_missing`, with its totals, and with `sample.file` the
path of the row's file in the lock). It is worked out a file's rows at a time, as the press reads
them, and every function of the recipe language computes the same bits on every platform (none
calls the platform's own math library), so it has the same values whoever works it out. A reader
that works one out checks its values against the record, and refuses the column when they differ.

The recipe the file carries (`made.recipe`) lists the same derived columns in its `[derived]`
table, by the same names, with the same expressions. A derived column is added to a pressed file,
dropped from it or renamed with `rvr derive`, which changes the recipe's text, `made.recipe.sha256`
and `made.sha256` with it; an expression is never changed in place. A derived column another uses
is dropped or renamed only with it, or after it.

## 11. Tool keys

Any other top-level key belongs to a tool: one object per tool, named for it, holding whatever
JSON the tool likes. `rvr press` never writes one. Any program that saves a `.rvr` file writes back
every tool key it found. Tool keys are outside every checksum and the signature, so a tool can
change its own key without breaking either.

## 12. What a reader checks

**On opening**, a reader checks everything the header says without reading an array, and refuses
the file at the first problem:

- the magic, `rowver: 15`, and the top level;
- `made`: its fields, the toolchain, each of the three texts against its `sha256`,
  `recipe.columns`, and `made.sha256`;
- `data.samples` and `data.files` against `made.files`: the same samples and files, each record's
  fields, and every sample's `events` against its files' `stop` and `take`;
- each file's `holes`: their fields, a kind from the list, in order and apart, and within what the
  choice read. `uuid: null` only on a file that is one whole hole or that the choice took nothing
  of. In a file with holes whose samples carry `events_read`, `events_missing` on every sample;
- each file's `totals`, finite numbers, and each sample's metadata holding their sums, in order;
- that `data.cols` names exactly `made.recipe.columns`, in order;
- each derived column's record: its fields, a name no other column has, a nonempty expression,
  a kind the format defines, one checksum per sample, and its facts;
- the sample columns: each has `name`, `kind`, `values` and `sha256`; its kind is one of the four;
  it has one value per sample, of a type its kind allows; its name is no column's, nor `sample`,
  `entry` or `file`; its `sha256` matches its values; and the list is exactly what the metadata
  gives;
- that `data.sha256` matches the arrays' `sha256`s;
- the signature, when there is one.

**When it reads a column**, it checks the column against its `sha256`. It can also check then that
the facts agree with the values, that floats are finite, and that a `b1` holds only 0 and 1.

**Full validation** (`rvr validate`, and `rowver.validate` in Python) checks all of the above and,
unless the file is empty (§1), every array:

- the arrays fill the file in file order, end to end, and each decompresses to its declared length
  and has its `sha256`;
- sample rows and file rows each sum to `rows`; the sample index runs through the samples in
  order; each file's rows are its sample's, with distinct entry numbers that the event choice read
  (a random choice being made again from its seed), and none in a hole;
- every hole holds an entry the choice chose. `events_missing`, when present, is the entries
  chosen that lie in holes, and `events_read` is the entries chosen less those;
- every `by_sample` has one checksum per sample, each matching that sample's values;
- jagged offsets start at 0, never decrease, and end at `count`; flat floats are finite; `lim`,
  `nice`, `vals` and `names` agree with the values;
- every derived column, worked out, has the checksums and facts its record holds, and the
  recipe's `[derived]` table lists them as `derived.columns` does (`rvr validate`);
- and it says who signed the file.

## 13. Weights, and what is not in the file

The file stores no weights. A weight is an ordinary column the recipe declares, such as the
ntuple's `weight` branch or an expression that multiplies several factors. A factor that is one
number per sample, such as a cross-section, can stay in the metadata and be read as a sample
column, and a sum over every event read, such as of `genWeight`, is a total (`[totals]`, §6),
which a derived column can divide by. Whoever reads the file decides which columns weight events,
and how.

Not in the file, on purpose:

- stored weights, or a per-sample weighted flag;
- per-column provenance beyond the recipe's text;
- sample metadata per row;
- percentiles;
- a derived column's values: only its expression, its checksums and its facts;
- any other version: a reader of version 15 reads version 15 only, and a file of another version
  is pressed again from its study.

## 14. Versions

This is version 15, the first released version of the format. The header says `rowver: 15`, the
major version of the `rowver` crate that reads and writes it, and a reader refuses any other. A
reader also refuses a field it does not know, so an addition in a later 15.x opens only in a reader
that has it; each will be listed here with the version that added it.
