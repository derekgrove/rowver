# The `.rvr` file

Version 14. `crates/rowver` reads and writes everything here and is the reference builder;
`rvr press` writes pressed files with it.

New in 14:

- Arrays are zstd frames instead of zlib streams (see Arrays).
- `made` carries the whole study: the text of `recipe.toml`, `samples.toml` and `samples.lock`,
  each with its SHA-256, so the study can be written back out of the file and pressed again
  (`rvr reproduce`).
- Each sample records which of its events were read (`made.files[sample].events`): all of
  them, the first n, or n chosen at random from a seed, which a reader chooses again to check
  every row's entry.
- `made.toolchain` names what built the file.
- A file may be signed with ed25519 (`signature`).
- Each sample has a label in `data.samples`.
- The checksums' canonical JSON is rowver's own rule, stated here.
- A reader refuses a file whose `made` is missing anything or fails a checksum.

A reader of version 14 reads version 14 only; press an older file's study again.

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
is not 14, or whose arrays do not fill the rest of the file exactly.

## Header

The top level has exactly `rowver`, `data`, `made`, an optional `derived`, an
optional `signature`, and zero or more tool keys. Nothing else.

```json
{
  "rowver": 14,
  "data": { },
  "made": { },
  "derived": { },
  "signature": { },
  "viewer": { }
}
```

A file without `derived` is a pure file: `rvr press` pressed it from a study.
A file with `derived` is a derived file (see `derived`).

### `data` — the rows and their columns

```json
{
  "rows": 45147,
  "samples": [
    {"name": "TTJets_DiLept",  "label": "t t̄ → 2ℓ",         "rows": 45062},
    {"name": "WJets_HT400",    "label": "W + jets, HT 400-600", "rows": 85}
  ],
  "files": [
    {"sample": "TTJets_DiLept", "path": "/data/ttjets_1.root", "rows": 30211},
    {"sample": "TTJets_DiLept", "path": "/data/ttjets_2.root", "rows": 14851},
    {"sample": "WJets_HT400",   "path": "/data/wjets.root",    "rows": 85}
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
  "sha256": "…64 hex…",
  "sample_cols": [
    {"name": "xsec", "kind": "f8",   "values": [87.3, null],               "sha256": "…"},
    {"name": "kind", "kind": "text", "values": ["background", "background"], "sha256": "…"}
  ]
}
```

- `rows` — total kept events. One row is one event.
- `samples` — in order, each with exactly `name` (what every tool calls it),
  `label` (what a plot shows, any text) and `rows`. Rows of a sample are
  contiguous and follow this order.
- `files` — every input file that was read, in row order, each with exactly
  its sample, the path or URL it was opened with (a key of that sample's
  `made.files[sample].files`), and the rows it gave, zero included. A file's
  rows are contiguous. Within a sample, files follow the lock's file order,
  however many files were read at once, so the same study on the same inputs
  always lays out the same rows. A file a press read nothing of because the
  first n events were all in files before it is not listed; a file a random
  choice took nothing from is.
- `sample` — the array `i4[rows]`: each row's index into `samples`.
- `entry` — the array `i8[rows]`: each row's entry number in its file's tree,
  counted from 0. Within a file each number appears once, and every number is
  an entry the press read (see The event choice). With `files`, it says which
  event every row is, so a later program can read more branches for the same
  rows without running the selection again.
- `cols` — one record per column, in the order `made.recipe.columns` names
  them:
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
- `values` — one JSON value per entry of `data.samples`, in that order.
- `sha256` — the SHA-256 of the values as bytes, so that it does not depend on
  how a program spells a number in JSON. For `b1`, `i8` and `f8`: the values as
  little-endian bytes of that kind, one after another, with a missing `f8`
  value as the eight bytes `00 00 00 00 00 00 f8 7f`. For `text`: for each
  value in turn the byte `00` when it is missing, and otherwise the byte `01`,
  the text as UTF-8, and the byte `00`. It is not part of `data.sha256`.

The stored `kind` is what a reader uses; it need not work the kind out again.
The order of `sample_cols` carries no meaning. The value of a sample column on
row `r` is `values[sample[r]]`.

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
64 bits when every sample has one, a number too large for an `f8`, and a key
that no sample has a value for. A key with the name of a column the recipe
pressed (`made.recipe.columns`) is not a sample column either: the row column
has the name. Nor are `sample`, `entry` and `file`.

A value is written as `made` holds it: a whole number in an `f8` column stays
a whole number (`59`, not `59.0`), and the `sha256` is of that text. The
columns are in the order their keys first appear, going through the samples'
metadata in `data.samples` order. `sample_cols` and `made` say the same thing
twice, and a reader checks that they agree in both directions.

`rvr press` adds two keys to every sample's metadata, so that they are sample
columns: `events_read`, the entries it read of the sample (before any cut),
and `events_total`, the entries its files hold.

A viewer never rewrites `data`. The presser writes it once.

### `made` — the study that made the file

```json
{
  "sha256": "…64 hex…",
  "study": "my_study",
  "title": "My study",
  "built": "2026-10-18T18:03:11Z",
  "by": "rvr 0.1.0",
  "toolchain": {
    "rvr": "0.1.0",
    "rowver": "14",
    "commit": "1fe5207a…",
    "rustc": "rustc 1.91.0 (f8297e351 2025-10-28)",
    "target": "aarch64-apple-darwin"
  },
  "recipe":  {"sha256": "…", "text": "…the whole recipe.toml…", "format": "rvr-recipe",
              "columns": ["MET", "channel", "Jet_pt"]},
  "samples": {"sha256": "…", "text": "…the whole samples.toml…"},
  "lock":    {"sha256": "…", "text": "…the whole samples.lock…"},
  "files": {
    "TTJets_DiLept": {
      "events": {"choose": "random", "n": 50000, "seed": 7,
                 "algorithm": "splitmix64-selection-sampling"},
      "files": {
        "/data/ttjets_1.root": {"tree": "Events", "uuid": "…", "entries": 219310, "take": 33912},
        "/data/ttjets_2.root": {"tree": "Events", "uuid": "…", "entries": 104077, "take": 16088}
      },
      "metadata": {"xsec": 87.3, "kind": "background", "events_read": 50000,
                   "events_total": 323387}
    },
    "WJets_HT400": {
      "events": {"choose": "all"},
      "files": {"/data/wjets.root": {"tree": "Events", "uuid": "…", "entries": 4712}},
      "metadata": {"xsec": 48.9, "kind": "background", "events_read": 4712,
                   "events_total": 4712}
    }
  }
}
```

- `study` — the study folder's name. `title` — the recipe's title, present only
  when the recipe sets one.
- `built` — when the file was written, UTC to the second.
- `by` — free text naming the program that wrote the file.
- `toolchain` — what built that program, each as text: `rvr` (its version),
  `rowver` (the format version it writes), `commit` (the git commit it was
  built from, with `-dirty` after it when the checkout had changes, or
  `unknown`), `rustc` (the compiler's `rustc -V`) and `target` (the platform).
- `recipe`, `samples`, `lock` — the study's three files, each as its whole
  `text` and the `sha256` of that text as UTF-8. Together they are the study:
  write them out as `recipe.toml`, `samples.toml` and `samples.lock` and the
  study can be pressed again. `recipe` also has `format` (the recipe
  language, `rvr-recipe`) and `columns`: every column the recipe stores, in
  the file's order, each named once. The lock's own record of the
  samples.toml it was resolved from matches `samples`.
- `files` — every sample of `data.samples`, by name, each with exactly:
  - `events` — which of its events the press read (see The event choice);
  - `files` — every file of the sample in `data.files`, by the path or URL it
    was opened with: its `tree`, the `uuid` stored inside that ROOT file, the
    tree's `entries`, and `stop` or `take` as The event choice says;
  - `metadata` — the sample's metadata, from the lock: text, whole numbers,
    finite floats, `true`, `false`, `null`, and lists and objects of those.
    The keys that hold one number, text or truth value per sample are written
    again as `data.sample_cols`.

A derived file carries its parent's `made` unchanged.

#### The event choice

`events` is one of:

| `events` | the press read | each file records |
|---|---|---|
| `{"choose": "all"}` | every entry of every file | neither `stop` nor `take` |
| `{"choose": "first", "n": N}` | the first N entries, in the files' order | `stop`, the entries read, when it read fewer than the file has |
| `{"choose": "random", "n": N, "seed": S, "algorithm": "splitmix64-selection-sampling"}` | N entries chosen at random over all the files, or all of them when they hold N or fewer | `take`, how many of the chosen entries are in it |

The choice is what the press did, which is what `samples.toml` asked unless
`rvr press --events`, `--random` or `--seed` changed it. N and S are whole
numbers; S may be any value from 0 to 2^64 − 1.

For `first`, the files in `data.files` order read `min(entries, left)` each,
with `left` starting at N and going down by what each file read. A row's
entry is below what its file read.

For `random`, the files in `data.files` order are counted as one run of
`total` entries, file after file. The algorithm `splitmix64-selection-sampling`
chooses `m = min(N, total)` of them:

1. Start SplitMix64 at the state S. Each draw adds `0x9E3779B97F4A7C15` to
   the state (mod 2^64), then mixes a copy `z` of it:
   `z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9`,
   `z = (z ^ (z >> 27)) * 0x94D049BB133111EB`,
   `z = z ^ (z >> 31)` (all mod 2^64), and gives `z`.
2. A uniform number is `(z >> 11) / 2^53`, a double in [0, 1).
3. Knuth's selection sampling (Algorithm S): with `needed = m`, for
   `i = 0, 1, 2, …` while `needed > 0`, draw `u`; entry `i` is chosen when the
   double `(total − i) × u` is less than `needed`, which then goes down by 1.

Entry `i` of the run is entry `i − start` of the file it falls in, where
`start` is the number of entries in the files before it. Each file's `take`
is how many chosen entries fall in it, and a row's entry is one of them. A
reader chooses the entries again from the seed to check every row, so the
same seed, files and N always give the same choice.

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
  `made.sha256` is the parent's. It still records the study and the ROOT
  files that made the rows, and `made.recipe.columns` names the pressed
  file's columns, not this file's.

```json
{
  "sha256": "…64 hex…",
  "built": "2026-10-06T19:12:44Z",
  "by": "…",
  "parents": [
    {"data_sha256": "…", "made_sha256": "…", "derived_sha256": null,
     "rows": 45147, "path": "/data/my_study.rvr",
     "read": {"MET": "…", "Jet_pt": "…"}}
  ],
  "source": "…the exact text that computed the columns…",
  "columns": {"mT2": "…64 hex…", "soft_jets": "…64 hex…"},
  "parameters": {"cut": 30},
  "runtime": "…what ran source…"
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
- `source` — the exact text that computed the columns.
- `columns` — every column of the file by name, in `data.cols` order, each
  with a mark: the SHA-256 of the whole of `source`, a newline, and the
  column's name. Any change to `source` changes the mark of every column.
- `parameters` — an object of plain JSON values, empty when there are none.
- `runtime` — free text naming what ran `source`.

A parent may itself be derived. A derive step reads the columns its direct
parent holds and no others: a column of the parent's own parent is not
looked up. `made` is then the same object down the whole chain, and each
file's `derived.parents[0].derived_sha256` names the step before it.

### Checksums

Every array reference carries `sha256`: the SHA-256 of that array's
uncompressed little-endian bytes (after undoing the shuffle), as 64
lowercase hexadecimal characters.

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
arrays, and `rvr validate` checks it. Two headers are enough to say which
column of which sample changed between two presses, with no array
decompressed; `rvr diff A.rvr B.rvr` does that. It matches samples and
columns by name, and reports the rows of each file, the samples and columns
only one file holds, the samples whose row counts differ, the columns whose
kinds differ, and each (column, sample) whose `by_sample` differs. For the
sample columns it reports the ones only one file holds, the ones whose kinds
differ, and each sample whose value differs, with both values.

`made.sha256` is the SHA-256 of the canonical JSON (below) of the complete
`made` object without its own top-level `sha256`, encoded as UTF-8.
`derived.sha256` is taken the same way of `derived`. The `sha256` of each of
`made.recipe`, `made.samples` and `made.lock` covers that text alone.

The canonical JSON of a value is:

- an object: `{`, its members with their keys sorted by Unicode code point,
  each as the key's canonical string, `:` and the value, with `,` between
  members, then `}`;
- an array: `[`, the members in order with `,` between them, then `]`;
- a string: `"` and the text, with `"` written `\"`, `\` written `\\`, the
  control characters U+0008, U+0009, U+000A, U+000C and U+000D written `\b`,
  `\t`, `\n`, `\f` and `\r`, the other characters below U+0020 written
  `\u00xx` (lowercase hexadecimal), and every other character as itself in
  UTF-8; then `"`;
- `true`, `false` and `null` as themselves;
- a number written without a fraction or an exponent (a whole number): its
  digits as the header writes them, with a leading `-` when negative and
  `-0` written `0`, however many digits it has;
- any other number: the double nearest to it, written with the fewest
  significant digits that read back as that double. With the decimal
  exponent `e` of its first significant digit, it is written in decimal when
  `-4 ≤ e < 16` (`0.0001`, `150.0`, `1234567890123456.0`), always with a `.`
  and at least one digit after it, and otherwise as one digit, a `.` and the
  rest of the digits when there are more, `e`, a sign and at least two
  exponent digits (`1e-05`, `1.5e+16`). Zero is `0.0`, or `-0.0` when
  negative.

There is no space anywhere outside strings. A whole number is never turned
into a double, so a 64-bit seed or byte count is checksummed exactly.

`rvr press` creates every checksum when building a file. Reading and
writing preserve them without recomputing them; readers check them. Tool
keys are outside every checksum, and outside the signature.

### `signature` — who pressed the file

A press signs the file when the person pressing it has a key (`rvr key new`
makes one in `~/.config/rvr/key`). A file without a key is unsigned and still
valid.

```json
"signature": {"ed25519": "…128 hex…", "key": "…64 hex…", "sha256": "…64 hex…"}
```

It has exactly these three members:

- `sha256` — the SHA-256 of the canonical JSON of an object holding the
  header's `rowver`, `data`, `made` and, when there is one, `derived`.
- `ed25519` — the Ed25519 signature (RFC 8032) of the 32 bytes of that
  SHA-256, made with the key, as lowercase hexadecimal.
- `key` — the public key, as lowercase hexadecimal.

A reader refuses a signed file whose `sha256` is not that of its header, or
whose signature does not hold for `key`: either says the header changed after
it was signed. Whether `key` belongs to someone the reader trusts is for the
reader to say; `rvr validate` names the signer from `~/.config/rvr/trusted`.

### Tool keys

One top-level object per tool, named for the tool. `rvr press` never writes
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
3. compressed as Zstandard data (RFC 8878), which decompresses to exactly the
   shuffled bytes and fills `size` exactly. `rvr press` writes one frame per
   array.

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

A copied column keeps its branch's kind unless the recipe stores it as
another; a computed column has the kind its expression gives. No promotion
rule.

## What a reader checks

Opening a file checks everything its header says without reading an array,
and refuses the file on the first problem:

- the magic, `rowver: 14`, and the top level;
- `made`: its fields, the toolchain, each of the three texts against its
  `sha256`, `recipe.columns`, and `made.sha256`;
- `data.samples` and `data.files` against `made.files`: the same samples and
  files, each record's fields, and every sample's `events` against its
  files' `stop` and `take`;
- in a pure file, `data.cols` names exactly `made.recipe.columns`, in order;
  in a derived file, `derived` has every field and exactly one parent,
  `derived.columns` names exactly the file's columns in order, `made.sha256`
  and `data.rows` are the ones recorded for the parent, and `derived.sha256`
  matches;
- the sample columns: each has `name`, `kind`, `values` and `sha256`, a kind
  of the four, one value per sample that its kind allows, a name that is no
  column's, `sample`, `entry` or `file`, and the `sha256` of its values; and
  the list is exactly what the metadata gives;
- `data.sha256` matches the arrays' `sha256`s;
- the signature, when there is one.

A column read later is checked against its `sha256` when it is read.

`rvr validate` (and `validate_file`) checks all of that and every array, one
line per file:

- the arrays fill the file in file order, end to end, and each decompresses
  to its declared length and has its `sha256`;
- sample rows and file rows sum to `rows`; the sample index runs through the
  samples in order; each file's rows are its sample's, with distinct entry
  numbers that the event choice read, a random choice being made again from
  its seed;
- every `by_sample` has one checksum per sample, each matching that sample's
  values;
- jagged offsets start at 0, never decrease and end at `count`; flat floats
  are finite; `lim`, `nice`, `vals` and `names` agree with the values;
- and it says who signed the file.

A derived file's parent is checked only when it is named:
`rvr validate DERIVED.rvr --parent PARENT.rvr`. The parent is then
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

`rvr dump` shows a file whose header fails these checks, after a line saying
what is wrong, so that a damaged file can still be looked into.

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

`rvr press` keeps each event's entry number with it through every cut, and a
row's `entry` is the entry its event came from. Rows keep the order of their
entries within each file.

## Weights

The file stores no weights. A weight is an ordinary column the recipe
declares, such as the ntuple's `weight` branch or an expression that
multiplies several factors. A factor that is one number per sample, such as
a cross-section, can stay in the metadata and be read as a sample column. A
viewer decides which columns weight events and how.

## Not in the file

Stored weights or a per-sample weighted flag. Per-column provenance beyond
the recipe's text. Sample metadata per row. Percentiles. Any older version:
a reader of version 14 reads version 14 only. A parent's columns inside a
derived file.
