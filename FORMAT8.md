# `.rvr` 8 — native dtypes, provenance, digests, jagged copies

Version 8 retains the container, drawers, weighting, sample membership, event
index, and packed booleans of [version 7](FORMAT7.md). This document specifies
its only changes. Readers support 6, 7, and 8; newly pressed cartridges use 8.
Saving an existing cartridge preserves its version, data drawer, and buffer.
The `rvr6` Python module name remains the import path. Version 8 is a fidelity
change: a copied branch is stored exactly, and a reader can prove the buffer is
the one that was pressed.

## Column kinds

`data.cols[].kind` is the storage dtype. Beside `f8`, `i4`, and `b1` it accepts
`f4`, `i8`, `i2`, `i1`, `u8`, `u4`, `u2`, and `u1` (IEEE float or two's-complement
integer, little-endian, width in bytes as named) and `jagged` (below). Every span
is aligned to its width; `b1` spans are byte-aligned as in version 7.

The presser stores a **copy** of a branch in the common NumPy dtype of every
chunk it read, so `kind` and `source.dtype` name the same type for every copy it
writes (`f4` and `float32`, `u8` and `uint64`, `b1` and `bool`). A copy whose
chunks have no exact common dtype (a 64-bit integer beside a float, or `uint64`
beside a signed integer) is refused at press time. A **derived** column keeps the
version 7 promotion to `f8`, `i4`, or `b1`. Readers accept any kind on any
column regardless of `source`.

`lim` is `[min, max]` as float64, `nice` follows the version 6 rules, `vals` is
allowed on any integer or `b1` kind, and the percentiles are computed on a
float64 view of the values. An integer column with a value beyond 2^53 in
magnitude keeps `lim` only as exactly as float64 allows, spells that `nice`
bound as a float, and carries no `vals`.

A version 6 or 7 reader rejects these kinds; a version 8 reader rejects `b1` in
version 6 and the new kinds in 6 and 7, exactly as before.

## Provenance

`data.cols[].source` is optional. When present it is one of:

- `{"class": "copy", "branch": <text>, "dtype": <text>}`: the column is the
  named branch, unchanged, read as the named NumPy dtype.
- `{"class": "derived", "from": [<branch>…], "reproducible": "cartridge" |
  "source", "python": <text>}`: the column was computed from the listed
  branches (distinct, non-empty names; branches, not columns). `cartridge` means
  every input branch is itself a copy in this file, `source` means the ROOT files
  are needed. `python` is the text of the hook or hooks that produced it.

Validators check the shape only. Nothing in `source` is executed, and nothing is
cross-checked against the buffer or against `made`. In a version 6 or 7 header
`source` rides as unknown metadata.

`made.samples[].files[]` records may carry `uuid` (text, the ROOT file's own
UUID) and `entries` (natural number, the tree's entry count). Version 8 types
them; earlier versions carry them as metadata.

## Digests

`data.sha256` is the SHA-256 of the whole column buffer and `data.cols[].sha256`
the SHA-256 of one column's bytes (for a jagged column, the offsets span followed
by the content span), each as 64 lowercase hexadecimal characters. Both are
required in version 8. Validators recompute them on read and on write; a mismatch
is a format error, so a save that preserves the buffer needs no new digests and
one that alters it cannot validate. Version 6 and 7 headers carry no digests and
readers do not synthesize any.

## Jagged columns

A column that holds one list of numbers per event has `kind: "jagged"`, an
`at`, and a `content` object; it is two spans:

- **offsets** at `at`: `i8[rows + 1]`, 8-aligned. `offsets[0]` is 0, the
  sequence never decreases, and `offsets[rows]` equals `content.count`. Event `i`
  owns `content[offsets[i] : offsets[i + 1]]`.
- **content** at `content.at`: `content.count` values of `content.kind`, any
  kind except `jagged`, aligned to its width; `b1` content packs per version 7
  with `content.count` bits and zero tail bits; float content is finite.

Items follow event order, so lists continue across sample boundaries and the
sample index locates each event's list. One level only: an item is a number.
Weights are per event; a consumer that wants a weight per item repeats the
event's weight over its slice. A jagged column carries no `lim`, `nice`,
percentiles, or `vals`; validators ignore any that appear. No cut, plot, grid
variable, or region cut may cite a jagged column: in `view` and `tabs` the name
is not a column. `made.columns`, when present, still describes
it. Readers that plot one value per event skip the kind and preserve its bytes
on save; other consumers rebuild the lists from the two spans.

## Buffer geometry

The buffer length equals:

```text
rows × (4 + (8 if weights are present else 0))
+ sum over flat columns of (rows × width, or ceil(rows / 8) for b1)
+ sum over jagged columns of (8 × (rows + 1) + content bytes by the same rule)
```

All spans must be in bounds and non-overlapping, and together fill the buffer.
There are no padding bytes. The presser emits weights, then every span four
bytes or wider from widest to narrowest (a jagged column's offsets and content
are two spans, placed by their own widths), then the sample index, then spans
narrower than four bytes widest first, then packed bits; spans of equal width
keep the study's declaration order. Any row count therefore leaves every span
aligned. Column descriptors keep declaration order; readers use offsets and
never assume physical block order. A cartridge whose columns are all `f8`,
`i4`, or `b1` has the same buffer as version 7.

The 1 GiB limit applies to the whole packed buffer, jagged content included.

## Reading and saving

`columns()` and `read_by_sample()` return NumPy arrays for flat columns (bits
unpacked to one byte per event) and Awkward arrays for jagged columns. Digests
and provenance need no event arrays: validation hashes the packed bytes as they
are. A conforming save preserves the buffer byte-for-byte, preserves `data`
including every `sha256` and `source`, and preserves unknown metadata as
required by version 6.

The cross-implementation examples are in `contract/v8/cases.json`: a base
header, its buffer, the values every reader must return (lists for the jagged
column), and cases that set header paths or patch buffer bytes and state the
verdict. The version 6 and 7 corpora are unchanged.
