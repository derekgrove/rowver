# The `.rvr` format

Version 15 (rowver 15.0). This page specifies every byte of a `.rvr` file. `crates/rowver` reads
and writes all of it and is the reference builder; `rvr press` writes pressed files with it.
[The features](FEATURES.md) describe what the tools around the format do.

**The running example.** Every example below is from one real file, `tiny.rvr`, unless it says
otherwise. It is 7,427 bytes, pressed from two copies of a five-entry test ntuple
(`crates/rvr/tests/fixtures/uproot_flat.root`, tree `analysis`; one sample each, `signal` and
`background`) with this recipe, and signed with a throwaway key:

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
00000000: 8952 5652 0d0a 1a0a ab1b 0000 7b22 726f  .RVR........{"ro
          └──────magic──────┘ └length─┘ └─header…
```

- **The magic** is `89 52 56 52 0D 0A 1A 0A` (`\x89RVR\r\n\x1a\n`): a high byte, the letters
  `RVR`, then CR LF, Ctrl-Z and LF. A transfer that strips the eighth bit or rewrites line endings
  therefore breaks the file visibly.
- **The header length** is `AB 1B 00 00`, that is 0x1BAB = 7,083 bytes. A reader parses that many
  bytes of JSON and has every fact about the file without touching any array.
- **The arrays** start at byte 12 + 7,083 = 7,095 and fill the rest of the file: 332 bytes here.
  Each is compressed on its own (§7), so a reader decompresses only the arrays it needs. A program
  that changes only the header writes the new header and copies the arrays byte for byte.

A reader refuses a file whose first 8 bytes are not the magic, whose `rowver` is not 15, or whose
arrays do not fill the rest of the file exactly (the header's array records say where each lies, so
this is checked on opening, with no array read).

**The header's JSON** is one JSON value (RFC 8259) in UTF-8, an object, with these rules:

- no byte-order mark; spaces, tabs and line ends may stand between tokens and around the value;
- no object holds the same key twice, at any depth;
- arrays and objects nest at most 64 deep, the header itself the first;
- a string holds no lone surrogate (`\ud800` without its pair);
- every count the format names (`rows`, `at`, `size`, `count`, `entries`, `stop`, `take`, `n`, a
  hole's entries, and `events_read`, `events_total` and `events_missing`) is a whole number written
  without a fraction or an exponent, from 0 to 2^63 − 1; `seed` from 0 to 2^64 − 1;
- every record holds exactly the fields this page gives it, no more; only the top level may hold
  others, the tool keys (§11). The objects that hold keys of their own instead of fields are
  `made.files` (by sample name), `made.files[sample].files` (by path), a sample's `metadata` (and
  the lists and objects in it), a file record's `totals`, and each tool key's object; every other
  object is a record.

The header length is at most 2^32 − 1, as its four bytes allow.

**An empty file** is a file's header, byte for byte, with nothing after it: the file ends at byte
12 + header length, while its records still name every array, its checksums and its signature as
they were. It keeps the study it was pressed from, which can press the same arrays again, so it gives
back a file's space and keeps its place (`rvr empty`; `rvr refill` presses the arrays again and puts
them back once each has the `sha256` its record states). Its array records are laid out as a whole
file's are (§7: from `at` 0, end to end, each `size` from 1), only the bytes they name are gone. A
reader opens it and checks its header as for any file, says it is empty, and refuses to read a
column of it; nothing else in the format marks it. Full validation of it runs every check but those
of the arrays' bytes (§12). A whole file always has arrays after its header, since even an array of
no values is a compressed frame of a few bytes.

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

The top level has exactly `rowver` (the whole number 15), `data`, `made`, an optional `derived`, an
optional `signature`, and zero or more tool keys (§11). Nothing else. No program rewrites `data`
once the press has written it.

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
| `samples` | every sample in row order, each with exactly `name` (what every tool calls it), `label` (what a plot shows, nonempty text) and `rows` |
| `files` | every input file read, in row order, each with exactly `sample`, `path` (its `at` in the lock, a key of that sample's `made.files[sample].files`) and `rows` (zero included) |
| `sample` | the array `i4[rows]`: each row's index into `samples` |
| `entry` | the array `i8[rows]`: each row's entry number in its file's tree, counted from 0 |
| `cols` | one record per column, in the order `made.recipe.columns` names them (§4) |
| `sha256` | the checksum of all the arrays (§8) |
| `sample_cols` | the sample columns (§5); always present, and never empty: `events_read` and `events_total` are sample columns of every file |

The rules of the layout:

- `samples` lists the lock's samples, in the lock's order, each with its name and label as the lock
  gives them (§6.3), and the number of its rows.
- A sample's rows are contiguous, in the order of `samples`. Within a sample, each file's rows
  are contiguous, in the lock's file order however many files were read at once. The same study
  on the same inputs therefore always lays out the same rows.
- `files` lists, for each sample in order, the files of its lock entry in the lock's order, leaving
  out a file the first N events never reached: one with entries whose `stop` would be 0, as the
  file's own `events` reads them (§6.1). Such a file is in neither `files` nor
  `made.files[sample].files`. A file of no entries is listed, with no `stop`, wherever it falls,
  and so is a file a random choice took nothing from, with 0 rows.
- Within a file, rows are in strictly increasing order of their entries, so each entry number
  appears once, and each is an entry the choice chose and no hole holds (§6.1, §6.2). With `files`,
  `entry` says which event every row is, so a later program can read more branches for the same
  rows without running the selection again.

## 4. Columns

A column is **flat** (one value per row) or **jagged** (a list per row).

| kind | type | bytes per value |
|---|---|---|
| `f8`, `f4` | float | 8, 4 |
| `i8`, `i4`, `i2`, `i1` | signed integer | 8, 4, 2, 1 |
| `u8`, `u4`, `u2`, `u1` | unsigned integer | 8, 4, 2, 1 |
| `b1` | boolean | 1 (0 or 1, not packed bits) |
| `jagged` | a list per row | two arrays: offsets `i8[rows + 1]`, and content of any flat kind |

A copied column keeps its branch's kind (the kind the lock lists, §6.3) unless the recipe stores it
as another. A computed column has the kind its expression gives. There is no promotion rule. A
float, flat or in a list, stored or derived (§10), may be any IEEE value: NaN and infinities
included.

A column's `name` is the name the recipe gives it (`made.recipe.columns`): nonempty, no two columns
alike, and none `sample`, `entry` or `file`, nor the name of a derived column.

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

A flat column's record has exactly `name`, `kind`, `at`, `size`, `sha256`, `by_sample`, `lim`,
`nice`, and `vals` when §4's rule below gives it. A jagged column's has exactly `name`, `kind`
(`"jagged"`), `offsets` (exactly `at`, `size`, `sha256`, `by_sample`) and `content` (exactly `kind`,
`count`, `at`, `size`, `sha256`, `by_sample`).

### Column facts

Every flat column carries facts about its values, so a reader can draw an axis or a legend without
reading the array:

| fact | holds | in `tiny.rvr` |
|---|---|---|
| `lim` | `[min, max]` over the column's finite values (NaN and infinities passed over); for `b1`, of 0 and 1 | `ht`: `[101.703125, 154.046875]` |
| `nice` | `[lo, hi]`: `lim` rounded outward, by the rule below | `ht`: `[100.0, 160.0]` |
| `vals` | every distinct value, ascending; on integer and `b1` columns with at most 64 distinct values, and on no other | `nJet`: `[1, 3, 4]` |

A column with no finite value (an empty one, or one of NaN alone) has `lim: [0, 0]` and
`nice: [0, 1]`, and `vals: []` where `vals` applies.

**`nice`, exactly.** For each end of `lim`, take its shortest decimal: the digits canonical JSON
writes for it (§8), so the double `1.2` is `1.2` and the `f4` value 1.1, widened to a double, is
`1.100000023841858`. A whole-number kind's ends are their digits. Then, in exact decimal arithmetic:

1. an end that is 0 stays 0; for any other, let `e` be the power of ten of its first significant
   digit (`373.67…` has `e = 2`), and `step = 10^(e − 1)`;
2. the low end becomes the largest multiple of `step` at or below it, the high end the smallest at
   or above it (`0.338…` becomes `0.33`; `373.67…` becomes `380`; `-0.037` stays `-0.037`);
3. when the two ends are then equal, the high end is raised by its `step`, or by 1 when it is 0 or
   the kind is a whole-number kind and its `step` is below 1 (`[5, 5]` gives `[5, 6]` on a
   whole-number column and `[5.0, 5.1]` on a float one; `[-1.2, -1.2]` gives `[-1.2, -1.1]`;
   `[0, 0]` gives `[0, 1]`).

A whole-number kind is every kind but `f8` and `f4`: `b1` is one (`[1, 1]` gives `[1, 2]`). On a
whole-number column `nice` holds those decimals, which are whole numbers, exactly (beyond 2^53 too:
`[0, 18446744073709551615]` gives `[0, 19000000000000000000]`); on a float column, the double
nearest to each, or the largest double for an end past it. It always encloses `lim`.

**Spelling.** Facts are compared as numbers: a reader takes each as the double (or, on a
whole-number column, the whole number) it writes, and any JSON spelling of the right value is right
(`380`, `380.0` and `3.8e2` alike, on an `i4` column too); `-0.0` is `0.0`. A writer writes a
whole-number column's `lim`, `nice` and `vals` as whole numbers.

There is no percentile ladder: a reader holding a column can measure a quantile itself.

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
- a key with the name of a column the recipe pressed (`made.recipe.columns`), since the row
  column has that name, and the names `sample`, `entry` and `file`.

(The metadata comes from TOML, whose whole numbers are 64-bit and whose floats are finite, so no
other value can be met.)

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

**The press adds** two keys to every sample's metadata, so that they are sample columns; both are
required, and exact:

- `events_read`: the entries the choice chose of the sample (§6.1), less those in holes;
- `events_total`: the entries of all the sample's files in the lock, those no row comes from
  included.

A file with holes (§6.2) adds a third to every sample: `events_missing`, the entries chosen that
lie in holes (0 for a sample with none), so that `events_read + events_missing` is what the choice
chose. A file with no hole has no `events_missing`. Each total of the recipe's `[totals]` (§6) adds
one more, under its name: the sample's sum of it (§6), a finite double. No key of `samples.toml`'s
metadata has any of these names (SPEC.md §3.5.10, §2.3).

## 6. The study: `made`

```json
"made": {
  "study": "tiny",
  "built": "2026-11-03T20:10:19Z",
  "by": "rvr 15.0.0",
  "toolchain": {"rvr": "15.0.0", "rowver": "15", "commit": "76ab18e2…",
                "rustc": "rustc 1.91.0 (f8297e351 2025-10-28)", "target": "aarch64-apple-darwin"},
  "recipe":  {"sha256": "937f05be", "text": "…the whole recipe.toml…", "format": "rvr-recipe",
              "columns": ["nJet", "Jet_pt", "ht"]},
  "samples": {"sha256": "a960276a", "text": "…the whole samples.toml…"},
  "lock":    {"sha256": "459558ea", "text": "…the whole samples.lock…"},
  "files": {
    "signal": {
      "events": {"choose": "all"},
      "files": {"./signal.root": {"tree": "analysis", "uuid": "31e851e4-b994-11f1-ba5c-1a45bb13c76a",
                                  "entries": 5}},
      "metadata": {"kind": "signal", "xsec": 2.5, "events_read": 5, "events_total": 5}
    },
    "background": { … }
  },
  "sha256": "56c8177d"
}
```

`made` has exactly these fields:

| field | holds |
|---|---|
| `study` | the study folder's name: nonempty, not `.` or `..`, with no `/`, `\` or control character |
| `title` | the recipe's `title`; present exactly when the recipe sets one, and equal to it |
| `built` | when the file was written, UTC to the second: `YYYY-MM-DDTHH:MM:SSZ`, a real date and time |
| `by` | nonempty text naming the program that wrote the file |
| `toolchain` | what built that program: exactly `rvr` (its version, `15.MINOR.PATCH`, each number in digits with no leading zero), `rowver` (`"15"`), `commit` (below), `rustc` (the compiler's `rustc -V`, nonempty) and `target` (the platform, nonempty) |
| `recipe`, `samples`, `lock` | the study's three files, each exactly its whole `text` and the `sha256` of that text as UTF-8; `recipe` also has `format` (`"rvr-recipe"`) and `columns` (every column the recipe stores, in the file's order, each named once) |
| `files` | every sample of `data.samples`, by name (below) |
| `sha256` | the checksum of `made` (§8) |

`commit` is the git commit the program was built from (40 lowercase hexadecimal digits), or
`unknown`. When its code had changes not yet committed, `-dirty-` follows the commit, then 12
lowercase hexadecimal digits of git's hash of those changes, or `-dirty` alone when they could not
be read.

**The three texts are the study,** and each must be valid: write them out as `recipe.toml`,
`samples.toml` and `samples.lock`, and the study can be pressed again. A reader checks that:

- `recipe` is a recipe the [recipe reference](../crates/rvr-recipe/README.md) accepts, with all five
  tables; the columns its `[columns]` stores, in order, are `recipe.columns`; its `[derived]` table
  lists exactly the columns of `derived` (§10); its `title` is `made.title`;
- `samples` is a `samples.toml` [SPEC.md §3](SPEC.md#3-samplestoml) accepts;
- `lock` is a `samples.lock` §6.3 accepts, resolved from that `samples.toml`: its
  `samples_toml_sha256` is the `sha256` of `made.samples` (in `tiny.rvr`, `a960276a…`), and its
  samples are what that `samples.toml` makes (names, labels, trees, metadata, event choices, the
  paths asked);
- `data` and `made.files` agree with the lock, as §3, §6.3 and below say: the samples, their names,
  labels and order; each sample's files, their paths and order; each file's tree, UUID and entries;
  each sample's metadata and `events_total`; and each column's kind (§4). A file's `stop` and `take`
  are not compared with the lock's, since `rvr press --events`, `--random` or `--seed` may have
  chosen otherwise: they follow `events`, as §6.1 says.

**`made.files[sample]`** has exactly three fields:

- `events`: which of its events the press read (§6.1): the lock's choice for the sample, or the one
  `rvr press --events`, `--random` or `--seed` gave;
- `files`: every file of the sample in `data.files`, keyed by its `at` in the lock, in that order
  (which is the lock's);
- `metadata`: the sample's metadata from the lock, with `events_read`, `events_total`,
  `events_missing` when the file has holes, and the sample's totals (§5), as text, whole numbers,
  finite floats, `true` and `false`, and lists and objects of those (never `null`). The order of
  its keys carries no meaning: every checksum is of canonical JSON (§8), whose keys are sorted. The
  keys that hold one number, text or truth value per sample are written again as
  `data.sample_cols` (§5).

Each file record has exactly:

| field | holds |
|---|---|
| `tree` | the tree read: the lock's `tree` for the sample |
| `uuid` | the UUID stored inside that ROOT file, as the lock gives it: 36 characters, lowercase hexadecimal in groups of 8, 4, 4, 4 and 12 joined by `-` |
| `entries` | the tree's entries, as the lock gives them |
| `stop` or `take` | how much of it the event choice read, as §6.1 says when each is present |
| `holes` | only when the press could not read some of the entries it chose (§6.2) |
| `totals` | exactly when the recipe's `[totals]` names at least one total and the press read the file: it read an entry of it, or the file has no entries and the choice is all or the first N (the press opens it all the same). A file every chosen entry of which lies in holes was not read, and nor was one a random choice took none of. Each total by name, a finite double written as one (with a `.` or an exponent), its expression added up over the entries the press read of this file before any cut, one at a time in entry order from +0.0 (or over every entry of the other tree it names); a `--fill` adds the sum of the entries it read. `totals: {}` is refused |

A sample's metadata holds each total as the sum of its files' `totals`, added from +0.0 in the order
of `data.files` (a file with no `totals` adds nothing, so a sample no file of which was read has
0.0), written as a double, so that it does not depend on how many files were read at once; a reader
checks that it is that sum, exactly. A total is what a file of kept
events cannot give afterwards, such as the sum of `genWeight` over every event read, which
normalizes simulation.

### 6.1 The event choice

`events` is one of:

| `events` | the press read | each file records |
|---|---|---|
| `{"choose": "all"}` | every entry of every file | neither `stop` nor `take` |
| `{"choose": "first", "n": N}` | the first N entries, in the files' order | `stop`, the entries read, when it read fewer than the file has |
| `{"choose": "random", "n": N, "seed": S, "algorithm": "splitmix64-selection-sampling"}` | N entries chosen at random over all the files, or all of them when they hold N or fewer | `take`, how many of the chosen entries are in it |

The choice is what the press did: what `samples.toml` asked, unless `rvr press --events`,
`--random` or `--seed` changed it. N is a whole number from 1 to 2^63 − 1; S may be any value from
0 to 2^64 − 1 (a lock's seeds are TOML integers, to 2^63 − 1, but `rvr press --seed` may give any,
and the file records it). `events` has exactly the fields its row above shows.

A sample's files hold at most 2^40 entries in all (about 1.1 × 10^12), so that every reader can make
a random choice again in bounded time, and no sum of entries can overflow. A reader refuses a file
whose samples hold more: on opening, counted over the files the file records (`made.files`), before
any choice is made again; in full validation, over every file of the lock.

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

`total − i` and `needed` are below 2^40, so each is exact as a double; `(total − i) × u` is one IEEE
multiplication, rounded to nearest, and `<` compares the two doubles.

Entry `i` of the run is entry `i − start` of the file it falls in, where `start` is the number of
entries in the files before it. Each file's `take` is how many chosen entries fall in it, and a
row's entry is one of them. A reader makes the choice again from the seed to check every row: the
same seed, files and N always give the same choice.

### 6.2 Holes

A press that cannot read part of a file it chose still writes the `.rvr` file, without those
events, and says so in that input file's record: these are the holes of the `.rvr` file, entries it
lacks (this example is not from `tiny.rvr`, which has no hole):

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
- `why`: text that is not blank (not only spaces), for a person, saying what failed, where, and
  what to do.

| `kind` | the press found |
|---|---|
| `unreachable` | its storage kept failing after every wait (busy, down, not answering), or answered in a way no wait mends |
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

### 6.3 `samples.lock`

`rvr samples resolve` writes the lock (SPEC.md §6); it pins every file the study reads, and nobody
edits it. Every file carries it, as `made.lock.text`. It is TOML 1.0 in UTF-8; comments are free.
The lock of `tiny.rvr`, cut short:

```toml
# samples.lock: written by rvr samples resolve; it pins every file this study reads.
# Review its changes as you review code, and never edit it: resolve again instead.

format = "rvr-samples-lock"
version = 1
resolved = "2026-11-03T20:10:19Z"
samples_toml_sha256 = "a960276a…"

[[sample]]
name = "signal"
label = "Signal"
asked = ["./signal.root"]
tree = "analysis"
branches = "2fccd77d…"
events = "all"
events_total = 5
events_read = 5
metadata = { kind = "signal", xsec = 2.5 }

[[sample.file]]
at = "./signal.root"
bytes = 39262
adler32 = "bbc43743"
uuid = "31e851e4-b994-11f1-ba5c-1a45bb13c76a"
entries = 5

[[branches]]
sha256 = "2fccd77d…"
list = [
  { name = "Jet_eta", kind = "f4", shape = "jagged" },
  { name = "Jet_pt", kind = "f4", shape = "jagged" },
  # … 7 more
  { name = "nJet", kind = "i4", shape = "flat" },
  # … 7 more, among them { name = "pv", kind = "f4", shape = 3 }
]
```

**The top level** has exactly `format = "rvr-samples-lock"`, `version = 1`, `resolved` (when it was
written, `YYYY-MM-DDTHH:MM:SSZ`), `samples_toml_sha256` (the SHA-256 of the `samples.toml` text it
was resolved from), `sample` (one or more) and `branches` (one or more).

**Each `[[sample]]`** has exactly `name`, `label`, `asked`, `tree`, `branches`, `events`,
`events_total`, `events_read`, `metadata` and `file`, and `choose` (`"random"`) and `seed` exactly
when the sample's choice is random. The samples are in the order, and have the name, label, tree,
event choice and metadata, that `samples.toml` makes (SPEC.md §3.7), with `{file}` filled in.
`asked` is the sample's `files` entries, their placeholders filled, in order, each as written (a
bare name stays `a.root`). `events_total` is the sum of its files' `entries`, and `events_read` the
entries its own choice reads. `branches` is the SHA-256 of the branch list of its first file's tree
(the `sha256` of one `[[branches]]`): a press makes a `changed` hole of a file of the sample whose
tree lacks a branch the recipe reads, or reads one as another kind or shape.

**Each `[[sample.file]]`** has exactly `at`, `bytes`, `uuid` and `entries`; `adler32`
when its storage gave one (a file on this computer always has one); `stop` for a first-N choice
exactly when it reads fewer than `entries` of the file (0 for a file it never reaches); `take` for a
random choice, always (how many chosen entries lie in it). The files are in the order SPEC.md §3.8
gives: the matches of each `asked` entry, entry after entry, each entry's in byte order of name,
and every entry matches one file or more. No two name the same file: their `at`s differ once a
local path's `.` and `..` are taken out.

| field | holds |
|---|---|
| `at` | where the file is: its `asked` entry's folder and the matched name, joined by `/`. A local path is relative to the folder of `samples.toml` (an entry that is a bare name, `a.root`, is pinned `./a.root`), under `~/`, or absolute; or the URL |
| `bytes` | its size, a whole number from 1 |
| `adler32` | its adler32, 8 lowercase hexadecimal digits; every file on this computer has one |
| `uuid` | the UUID inside the ROOT file, as §6 writes it |
| `entries` | the tree's entries |

**Each `[[branches]]`** is one branch list, written once however many samples have it: exactly
`sha256` and `list`. `list` holds one inline table per branch of the tree, in byte order of name,
each exactly `name` (as a recipe writes it), `kind` and `shape`, and `why` exactly when `kind` is
`"none"`:

| field | holds |
|---|---|
| `kind` | the kind rvr reads the branch as (§4; for a list, its members' kind), or `"none"` when rvr cannot read it |
| `shape` | `"flat"` (one value per event), `"jagged"` (a list per event), a whole number N from 1 (N values per event), or `"jagged N"`, N from 2 written with no leading zero (a list per event of elements of N values each, as `xy[n][3]/F`); a branch rvr cannot read is `"flat"` |
| `why` | for a branch rvr cannot read, why, in words |

`sha256` is the SHA-256 of the canonical JSON (§8) of `list`, as an array of objects. No two lists
have the same `sha256`, and every list is some sample's.

A branch is read into a column by its shape: `"flat"` as a flat column; `"jagged"` as a jagged
column, one list per event; a whole number N as a jagged column whose every list holds the event's
N values; `"jagged N"` as a jagged column whose list holds, one element after another, the N values
of each of the event's elements (N times their count). The content's kind is the branch's `kind`.

## 7. Arrays

An array reference is `{"at", "size", "sha256"}`, with `by_sample` on every array but `sample`
(§8).

- `at` counts bytes from the start of the arrays (the byte after the header), so a header of
  another length leaves every `at` unchanged.
- `size` is the compressed length, from 1 (a frame is a few bytes even of no values).
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
3. **compressed** as exactly one Zstandard frame (RFC 8878), which fills `size` exactly and
   decompresses to exactly the shuffled bytes:
   - it begins with the magic `28 B5 2F FD`; no skippable frame comes before or after it;
   - it uses no dictionary (its dictionary ID flag is 0);
   - it has a content checksum (its checksum flag is 1), and the checksum (the low 32 bits of
     XXH64 of the decompressed bytes, seed 0) is right;
   - a content size, when the frame gives one, is the decompressed length;
   - its window is at most 2^27 bytes (128 MiB), which every Zstandard library reads by default.

   Zstandard's blocks hold at most 128 KiB each, so a frame of `size` bytes cannot give more than
   32,768 × `size` + 128 KiB; a reader refuses an array whose values would take more before it
   decodes anything.

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
  read back as that double; a number past the largest double has none, and a header holding one in
  `data`, `made` or `derived` is refused. With `e` the decimal exponent of its first significant
  digit, it is written in decimal when `-4 ≤ e < 16`, always with a `.` and at least one digit after
  it. Otherwise it is one digit, a `.` and the rest of the digits when there are more, then `e`, a
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
"signature": {"ed25519": "52b538dd…", "key": "7355b0e5…", "sha256": "1f7b41f3…"}
```

It has exactly these three members:

| member | holds |
|---|---|
| `sha256` | the SHA-256 of the canonical JSON of an object holding the header's `rowver`, `data`, `made` and, when there is one, `derived` |
| `ed25519` | the Ed25519 signature (RFC 8032) of the 32 bytes of that SHA-256, made with the key, as lowercase hexadecimal (128 characters) |
| `key` | the public key, as lowercase hexadecimal (64 characters) |

A reader refuses a signed file whose `sha256` is not that of its header, or whose signature does
not hold for `key`: either means the header changed after it was signed. The signature is checked
strictly (RFC 8032's checks, and a key of small order refused), so that one header and key have one
valid signature. Whether `key` belongs to
someone the reader trusts is for the reader to decide.

## 10. Derived columns

A derived column is kept as the expression that computes it, not as an array. A reader works it
out from the file's own columns when it is asked for; how long it keeps the values is the reader's
choice. It is listed in `derived.columns`, in the order the recipe's `[derived]` table gives;
`derived` has exactly `columns`, and is present exactly when there is at least one:

```json
"derived": {"columns": [
  {"name": "mean_jet_pt", "expression": "ht / nJet", "kind": "f8", "sha256": "e641687e",
   "by_sample": ["e73c3af8", "e73c3af8"], "lim": [33.901041666666664, 120.5], "nice": [33.0, 130.0]}
]}
```

| field | holds |
|---|---|
| `name` | what every tool calls it: a name no stored column, sample column or other derived column has, nor `sample`, `entry` or `file` |
| `expression` | the expression that computes it, as the recipe writes it: not blank |
| `kind` and the rest | exactly what a stored column's record holds, without `at` and `size`: for a flat column `sha256`, `by_sample` and its facts (`lim`, `nice`, and `vals` where it applies, §4); for a jagged one `offsets` (`sha256`, `by_sample`) and `content` (`kind`, `count`, `sha256`, `by_sample`) |

The checksums and facts are of the column's values over every row, as a stored column's are of its
array (§8): the press works each derived column out once, to record them. Its facts are compared as
numbers, as a stored column's are (§4), and its float values, like any float, may be NaN or
infinite. In `tiny.rvr`, `mean_jet_pt` is `ht / nJet` on each row, `[33.901041666666664,
38.51171875, 34.822916666666664, 120.5, …]`, and `e641687e` is the SHA-256 of those eight doubles.

An expression uses only what the file holds: its stored columns, the derived columns above it,
and `sample.KEY` (the metadata of the row's sample, as a press gives it to an expression: without
`events_read`, `events_total` and `events_missing`, with its totals, and with `sample.file` the
path of the row's file in the lock). Its values do not depend on how many rows are worked out at
once, and every function of the recipe language computes the same bits on every platform (its
exponentials, logarithms, trigonometry, roundings, `hypot` and `**` come from the `libm` crate, and
`sqrt` and the remainders of `%` and `delta_phi` are exact in IEEE arithmetic), so it has the same
values whoever works it out. A reader
that works one out checks its values against the record, and refuses the column when they differ.
A later rvr may add functions to the language; a reader that meets one it does not know, in a
file whose `made.toolchain.rvr` is newer than its own version, says that the file needs a newer
reader, and still reads each stored column asked for by name. A function, once released, keeps
its results.

The recipe the file carries (`made.recipe`) lists the same derived columns in its `[derived]`
table, by the same names, with the same expressions. A derived column is added to a pressed file,
dropped from it or renamed with `rvr derive`, which changes the recipe's text, `made.recipe.sha256`
and `made.sha256` with it; an expression is never changed in place. A derived column another uses
is dropped or renamed only with it, or after it.

## 11. Tool keys

Any other top-level key belongs to a tool: one object per tool, named for it (one or more ASCII
letters, digits, `_`, `-` and `.`), holding whatever JSON the tool likes, within
§1's rules. A key that is a slip of one of the format's own (`rowver`, `data`, `made`, `derived`,
`signature`) is refused as a misspelling, naming the key it is near: compared ignoring case, at most
one edit (a character inserted, deleted or replaced) from `data` or `made`, or at most two from
`rowver`, `derived` or `signature` (so `Data`, `mde` and `signatur` are refused, `dash` and `view`
are tool keys). `rvr press` never adds one. Any program that saves a `.rvr` file writes back
every tool key it found: `rvr press` those of the file it replaces at its target, when that file
opens. Tool keys are outside every checksum and the signature, so a tool can
change its own key without breaking either.

## 12. What a reader checks

Every rule on this page is checked by a reader; none is left to trust. The checks fall in three
steps, each refusing the file at the first problem, on one line that says which rule failed, where
(the field's path, as `data.cols["MET"].lim`), and what to do.

**On opening**, a reader checks everything the header says without reading an array or parsing the
study's texts:

- §1: the magic, the header's JSON rules, `rowver: 15`, and the arrays filling the rest of the file
  exactly, from their records (laid out from `at` 0, end to end, in an empty file too);
- §2 and §11: the top level, and the tool keys' names;
- §3: every field of `data`, the samples and files in order, rows adding up;
- §4: every column record's fields, kinds and names; the facts' forms: `lim` in order, `nice` made
  again from `lim`, `vals` only where it applies, ascending, within `lim`, and there whenever `lim`
  spans 64 values or fewer;
- §5: the sample columns, their kinds, values and checksums, against the metadata both ways;
  `events_read`, `events_total` and `events_missing` present as §5 says;
- §6: every field of `made` and its values; each text against its `sha256`; `made.sha256`;
  `made.files` against `data`; each sample's event choice against its files' `stop` and `take`
  (a random choice's `take` summing to its N, or to every entry when fewer), the entry cap; every
  file record's fields and their forms; the holes' fields, kinds, order and bounds; the totals
  present, finite doubles, and their sums;
- §8: `data.sha256` against the arrays' `sha256`s;
- §9: the signature, when there is one;
- §10: every derived record's fields, name, kind and facts.

**When it reads a column**, it checks the array's frame (§7), its length and its `sha256`.

**Full validation** (`rvr validate`, `rowver::rvr::validate_file`, and `rowver.validate` in Python,
all the same checks) does all of the above and:

- §6: parses the three texts: the recipe as the recipe reference reads it, `samples.toml` as
  SPEC.md §3 reads it, the lock as §6.3 reads it; and checks them against each other and the file:
  the recipe's tables, columns, their kinds, derived columns, title and totals; the lock resolved
  from that `samples.toml`; the samples, labels, files, trees, UUIDs, entries and metadata against
  the lock;
- §6.1: each random choice made again from its seed, each file's `take` its share;
- §5, §6 and §6.2: every hole holding a chosen entry; `events_read`, `events_missing` and
  `events_total` exact; totals on exactly the files the press read;
- §7, unless the file is empty: every array, its frame, its length and its `sha256`;
- §3, unless the file is empty: the sample index running through the samples in order; each file's
  entries strictly increasing, each chosen, none in a hole;
- §8, unless the file is empty: every `by_sample`;
- §4, unless the file is empty: jagged offsets starting at 0, never decreasing, ending at `count`;
  `b1` values 0 or 1; `lim`, `nice` and `vals` made again from the values, and equal to the
  record's;
- §10: every derived column worked out, its checksums and facts equal to its record's; in an empty
  file, worked out over no rows, each of its record's kind and shape;
- and it says who signed the file.

An empty file has no arrays to check, so full validation of it runs every check above but those
marked "unless the file is empty"; everything else, the event choice made again and the study's
texts included, is checked as in a whole file.

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
