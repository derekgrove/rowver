# The `.rvr` file

Version 11. `rowver/rvr.py` is the code for everything here.

## Container

```
gzip( u32 little-endian header length | UTF-8 JSON header | buffer )
```

One gzip member. To read: gunzip, take 4 bytes as the header length, parse
that many bytes as JSON, and everything after is the buffer.

## Header

The top level has exactly `rowver`, `data`, `made`, and zero or more tool
keys. Nothing else.

```json
{
  "rowver": 11,
  "data": { },
  "made": { },
  "viewer": { }
}
```

`rowver: 11`, so a viewer can tell this file from a version 10 one, which
stored weights, and no old file ever collides with a new number.

### `data` — written once, never rewritten

```json
{
  "sha256": "…64 hex…",
  "rows": 45147,
  "samples": [
    {"name": "TTJets DiLept",    "rows": 45062},
    {"name": "WJets HT 400-600", "rows": 85}
  ],
  "sample_at": 0,
  "cols": [
    {"name": "MET",     "kind": "f4", "at": 541768,
     "lim": [150.1, 1583.2], "nice": [150, 1600]},
    {"name": "channel", "kind": "u4", "at": 722016,
     "lim": [0, 2], "nice": [0, 2], "vals": [0, 1, 2],
     "names": {"0": "hadronic", "1": "one lepton", "2": "two or more leptons"}},
    {"name": "Jet_pt",  "kind": "jagged", "at": 902608,
     "content": {"kind": "f4", "at": 1263792, "count": 352738}}
  ]
}
```

- `sha256` — SHA-256 of the complete raw buffer, as 64 hexadecimal characters.
- `rows` — total kept events. One buffer row is one event.
- `samples` — in order. Rows of a sample are contiguous and follow this
  order.
- `sample_at` — offset of `i4[rows]`, the index into `samples` for each row.
- `cols` — one record per column, in the recipe's `columns` order:
  - `name`, `kind`, `at` always.
  - flat columns add `lim` and `nice`; integer and boolean columns add
    `vals` and `names` when the distinct values are few.
  - jagged columns: `at` is the offsets span, `i8[rows + 1]`, and `content`
    says where the values are, their kind, and how many. Event `i` owns
    `content[offsets[i] : offsets[i + 1]]`.

There is no percentile ladder. A viewer holding the buffer can measure a
quantile itself.

### `made` — the recipe snapshot

```json
{
  "sha256": "…64 hex…",
  "study": "my_study",
  "title": "My study",
  "note": "Two local UL18 NanoAOD samples with a generator weight column.",
  "built": "2026-09-12T18:03:11Z",
  "by": "rowver 11",
  "cart": {"sha256": "…64 hex…", "source": "…the whole cart.py…"},
  "files": {
    "TTJets DiLept": {
      "files": {
        "/data/ttjets.root": {
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

`data.sha256` covers every byte of the uncompressed buffer: sample indices,
flat columns, jagged offsets and content, and zero padding. It
does not cover the `data` JSON describing those arrays.

`made.sha256` covers the complete `made` object, including `files`, except
its own top-level `sha256` field. Serialize that object with
`json.dumps(body, sort_keys=True, separators=(",", ":"), ensure_ascii=False)`,
encode it as UTF-8, and take its SHA-256 as 64 hexadecimal characters.
The existing `made.cart.sha256` still covers the saved recipe text alone.

`rowver run` creates both checksums when building the file. Reading and
writing preserve them without recomputing them; `rowver validate` checks them.
Tool keys are outside both checksums.

### Tool keys

One top-level object per tool, named for the tool. `rowver run` never writes
one. Any program that saves an `.rvr` writes back every tool key it found.

## Buffer

Little-endian arrays at the declared offsets.

Layout: sample index, then each column in `cols` order
(a jagged column is offsets then content). Every span starts on a multiple of
8 bytes; the gap before it is zero. That is the whole rule. A reader uses the
offsets and never the order.

No end offsets are stored. A span ends at its start plus its number of
values times the kind's byte width; that end is exclusive. Flat spans have
`rows` values, jagged offsets have `rows + 1`, and jagged content has `count`.

Kinds and widths:

| kind | type | bytes per value |
|---|---|---|
| `f8`, `f4` | float | 8, 4 |
| `i8`, `i4`, `i2`, `i1` | signed integer | 8, 4, 2, 1 |
| `u8`, `u4`, `u2`, `u1` | unsigned integer | 8, 4, 2, 1 |
| `b1` | boolean | 1 |
| `jagged` | offsets `i8[rows + 1]` plus content of any flat kind | two spans |

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

## Weights

The file stores no weights. A weight is an ordinary column the recipe
declares, such as the ntuple's `weight` branch or a hook that multiplies
several factors. A viewer decides which columns weight events and how.

## Not in the file

Stored weights or a per-sample weighted flag. Checksums of individual
columns. Per-column provenance. A structured copy of sample metadata or hooks. Percentiles. Reading older versions.
