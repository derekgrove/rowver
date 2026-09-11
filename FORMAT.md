# `.rvr` 9 — the base specification

A cartridge is a data product: the columns of a study, the statistics that
describe their distributions, and a snapshot of the recipe that produced them.
It knows nothing about any tool that reads it. This document is the whole
standard for version 9. `rowver check` is the reference validator; independent
implementations agree with it through the shared corpus
`contract/v9/cases.json` (ADR 0004): a base header, its buffer, the values
every reader must return, and cases that set header paths or patch buffer
bytes and state the verdict. Where prose and validator disagree, that is a
defect in one of them, pinned by the corpus.

The reasons behind the rules are ADR 0010 and decisions D9 to D21 in
`docs/plan-bare-bones-presser.md`; the handoff
`docs/handoffs/2026-09-11-rvr-is-only-data.md` carries their full text.

Rules marked *presser behaviour* describe what `rowver run` writes; the
validators accept any wire value that satisfies the stated rule, not only the
presser's choice. Rules marked *reader guidance* are advice to consumers and
are not validated.

## 1. Container

```text
gzip( u32 little-endian header length | UTF-8 JSON header | column buffer )
```

- The header length is `1 ≤ len ≤ 8_388_608` bytes (8 MiB). The header is one
  UTF-8 JSON object; every key is a string.
- Number lexemes: an INTEGER-spelled lexeme (no `.`, no `e`) must have
  magnitude at most 2^53. 2^53 itself is legal; anything beyond is refused,
  INCLUDING even integers a double could represent exactly (the cap is the
  rule, not representability). Float-spelled lexemes are IEEE doubles by
  definition (their nearest-double reading IS their value). Any lexeme that
  overflows to infinity is refused, and `NaN`, `Infinity`, and `-Infinity`
  are not JSON. This rule is what makes additive metadata round-trip-safe
  between implementations whose only number type is the double.
- Strings: every string, escaped or not, must decode to Unicode scalar
  values. An escape that spells an unpaired surrogate (`"\ud800"`) is
  refused, since it has no UTF-8 encoding and no canonical digest (§5).
- The column buffer is at most `1_073_741_824` bytes (1 GiB), little-endian
  throughout, and follows the header immediately. Its length is declared in
  `data.bytes` and must match exactly.
- The gzip member's modification time is *presser behaviour*: zero, so that
  pressing the same inputs twice gives the same bytes.

## 2. Header shape

Top level: `rowver: 9` (required), the two drawers `data` and `made` (both
required objects), and any number of consumer keys (§2.1). Nothing else.

| key     | type   | written by | holds |
|---------|--------|------------|-------|
| `rowver`| `9`    | presser    | the version |
| `data`  | object | presser    | buffer geometry, statistics, provenance, digests; never rewritten (W3) |
| `made`  | object | presser    | the recipe snapshot: study identity, samples and their files, hooks, recipe text |

*Presser behaviour*: keys are emitted in the order `rowver`, `data`, `made`.
Key order carries no meaning; writers preserve it (W2).

### 2.1 Consumer keys

State that belongs to a tool rather than to the data lives under ONE top-level
key named for that tool, `"<tool>": {…}`; the presser never writes such a
key, every writer preserves it (W4), and this sentence is the whole of the
format's knowledge of it. The value is a JSON object, so the
consumer owns a namespace rather than a slot. Consumer keys sit outside every
digest in §5, and a consumer's key may not be `rowver`, `data`, `made`, or
one of the retired names in §2.2. Nothing under a consumer key may change what
the data drawer means.

### 2.2 Retired keys

A version 9 header carrying `view`, `tabs`, `opens_on`, or `about` at the top
level is REJECTED. The rejection is what keeps a retired drawer from being
written by habit: tool state belongs under a consumer key (§2.1) and study
identity in `made` (§6).

## 3. `data` — the immutable drawer

Never rewritten by any writer (W3). Keys, all REQUIRED unless marked:

- `rows` — natural number of entries (events). One buffer row is one entry;
  event order is buffer order and never changes.
- `bytes` — exact byte length of the column buffer. It must equal the sum of
  every declared span (§4), and the spans must tile the buffer exactly.
- `sha256` — SHA-256 of the whole column buffer, 64 lowercase hexadecimal
  characters (§5).
- `weight_col` — the weight column's name (non-blank text), or `null` when no
  sample is weighted. When non-null, `weight_at` is required: an 8-aligned
  byte offset of a contiguous `f8[rows]` array of finite values. When null,
  `weight_at` must be absent. The weight column name must not equal any data
  column name; it is not listed in `cols`.
- `sample_at` — 4-aligned byte offset of the sample index, `i4[rows]`, each
  value a valid index into `samples`, with the count of each value equal to
  that sample's `rows`. It locates sample membership, not "where the data
  begins"; every column carries its own offset. *Presser behaviour*: the rows
  of each sample are contiguous and follow `samples` order. *Reader
  guidance*: use the index, never assume contiguity.
- `samples` — non-empty array of `{ name, rows, weighted }`:
  - `name`: non-blank text, unique among samples.
  - `rows`: natural; the per-sample values sum to `data.rows`.
  - `weighted`: boolean. `false` means observed counts, never scaled. When
    any sample is weighted the file carries a weight column (*presser
    behaviour*: unweighted samples hold weight 1.0 in it); when no sample is
    weighted the file carries none (`weight_col: null`).
- `cols` — non-empty array of column records (§3.1).

### 3.1 Column records

Every record has:

- `name`: non-blank text, unique among columns and different from
  `weight_col`.
- `kind`: the storage dtype (§3.2).
- `at`: byte offset of the column's span (for `jagged`, of its offsets span),
  aligned to the kind's element width; a `b1` span is byte-aligned.
- `sha256`: SHA-256 of the column's bytes (§5).
- `source` (optional): provenance (§3.4).

A flat record (any kind but `jagged`) also carries the statistics of §3.3. A
`jagged` record carries `content` instead (§3.5). Unknown keys on a column
record ride as metadata (§8).

### 3.2 Kinds

`kind` names the little-endian storage type and its width in bytes:

| kind | type | width |
|---|---|---|
| `f8`, `f4` | IEEE float | 8, 4 |
| `i8`, `i4`, `i2`, `i1` | two's-complement integer | 8, 4, 2, 1 |
| `u8`, `u4`, `u2`, `u1` | unsigned integer | 8, 4, 2, 1 |
| `b1` | one bit per event, packed (§3.2.1) | 1 bit; the span is `ceil(rows / 8)` bytes |
| `jagged` | one list of numbers per event (§3.5) | two spans |

Every span is aligned to its element width; `b1` spans are byte-aligned. Float
payloads are finite: a NaN or infinity anywhere in a float column, in float
jagged content, or in the weights is a format error.

*Presser behaviour*: a **copied** branch is stored in the common NumPy dtype of
every chunk read, so `kind` and `source.dtype` name the same type for every
copy (`f4` and `float32`, `u8` and `uint64`, `b1` and `bool`). A copy whose
chunks have no exact common dtype (a 64-bit integer beside a float, or `uint64`
beside a signed integer) is refused at press time. A **derived** column is
promoted to `f8`, `i4`, or `b1`: booleans in every populated sample give `b1`,
integers within int32 give `i4`, anything else gives `f8`, and an integer
outside int32 must still fit within ±2^53. Empty samples never force a
populated column to widen; if every sample is empty the declared array dtypes
decide. Readers accept any kind on any column regardless of `source`.

#### 3.2.1 Packed booleans

Event `i` of a `b1` span at `at` is bit `i % 8`, least significant bit first,
of byte `at + floor(i / 8)`; false is 0 and true is 1. Bits continue across
sample boundaries. Unused high bits of the final byte MUST be zero. An empty
column occupies zero bytes. A validator checks the tail bits and finds `lim`
and `vals` from the packed bytes without unpacking.

### 3.3 Statistics (flat columns)

These are facts about the column's distribution, written by the presser and
carried for every consumer that draws or cuts on the data. All REQUIRED on a
flat column:

- `lim`: `[min, max]` as float64, finite, `min ≤ max`. Validators compare it
  with the payload: it must equal the observed extremes exactly when the
  column has rows. For `b1` the extremes are the numeric 0 and 1 that occur.
- `nice`: `[lo, hi]`, finite, and it must ENCLOSE `lim`: `lo ≤ lim[0]` and
  `lim[1] ≤ hi`. It is a rounded outer range for display. *Presser behaviour*:
  outward to two significant digits, snapped to integers for integer and `b1`
  kinds, and widened to `lo + 1` when the two ends coincide; only the
  enclosure is normative.
- The percentile ladder: `p1`, `p5`, `p10`, `p20`, `p30`, `p40`, `p50`,
  `p60`, `p70`, `p80`, `p90`, `p95`, `p99`. ALL THIRTEEN are REQUIRED on
  every flat column, whatever its kind; a record missing any one of them is
  a format error, and no subset (a median alone, a `p50` and `p90` pair) is
  a conforming ladder. Each is a finite percentile over EVERY row of the
  column (not a sample), computed on a float64 view of the values, and the
  ladder is ordered: `lim[0] ≤ p1 ≤ p5 ≤ … ≤ p95 ≤ p99 ≤ lim[1]`. Validators
  check presence, finiteness, and the chain, not the values. *Presser
  behaviour*: linear interpolation between order statistics. The whole ladder
  is stored so that a tool that needs a quantile reads it rather than
  re-measuring the column; the header cost is thirteen numbers per column.

An EMPTY column (zero rows) carries exactly `lim: [0, 0]`, `nice: [0, 1]`,
thirteen zero percentiles, and for integer and `b1` kinds `vals: []`. The stub
is normative for every kind: validators enforce it in place of the observed
extremes.

An integer column with a value beyond 2^53 in magnitude keeps `lim` only as
exactly as float64 allows, spells that `nice` bound as a float, and carries no
`vals` (§3.3.1 requires integer-spelled members, which §1 caps at 2^53).

#### 3.3.1 `vals` and `names`

- `vals` (optional; integer and `b1` kinds only, never on a float kind): the
  COMPLETE list of distinct values in the column, unique integers in ascending
  order. Validators compare it with the payload. Its presence declares that
  the column takes values from a known finite set, which a consumer may treat
  as categories. The format sets no cap on its length beyond the header limit
  of §1; *presser behaviour*: the presser enumerates an integer or `b1`
  column only when its distinct count is at most a threshold of the presser's
  own, and warns at press time about each column it therefore left without
  `vals`. Consumers decide what is small enough to show.
- `names` (optional; requires `vals`): labels keyed by value, each key the
  decimal spelling of a member of `vals` and each label text, for example
  `{"11": "electron", "13": "muon", "15": "tau"}`. Every member of `vals` is
  named exactly once. `names` is the one place labels live; there is no other.

### 3.4 Provenance

`source` is optional on any column, for columns the presser could not
observe. When present it is one of:

- `{"class": "copy", "branch": <text>, "dtype": <text>}`: the column is the
  named branch, unchanged, read as the named NumPy dtype.
- `{"class": "derived", "from": [<branch>…], "reproducible": "cartridge" |
  "source", "python": <text>}`: the column was computed from the listed
  branches (distinct, non-empty names; branches, not columns). `cartridge`
  means every input branch is itself a copy in this file; `source` means the
  ROOT files are needed. `python` is the text of the hook or hooks that
  produced it, including which branches the hook actually read.

Validators check the shape only. Nothing in `source` is executed, and nothing
is cross-checked against the buffer or against `made`. The recipe's own
declarations are in `made.cart.source` (§6), verbatim; `source` records what
the presser observed.

### 3.5 Jagged columns

A column that holds one list of numbers per event has `kind: "jagged"`, an
`at`, and a `content` object; it is two spans:

- **offsets** at `at`: `i8[rows + 1]`, 8-aligned. `offsets[0]` is 0, the
  sequence never decreases, and `offsets[rows]` equals `content.count`. Event
  `i` owns `content[offsets[i] : offsets[i + 1]]`.
- **content** at `content.at`: `content.count` values of `content.kind`, any
  kind except `jagged`, aligned to its width; `b1` content packs per §3.2.1
  with `content.count` bits and zero tail bits; float content is finite.

Items follow event order, so lists continue across sample boundaries and the
sample index locates each event's list. One level only: an item is a number.
Weights are per event; a consumer that wants a weight per item repeats the
event's weight over its slice. A jagged column carries no `lim`, `nice`,
percentiles, `vals`, or `names`; validators ignore any that appear. *Reader
guidance*: a consumer that wants one value per event skips the kind and
preserves its bytes on save; others rebuild the lists from the two spans.

## 4. The column buffer

Contiguous little-endian arrays at the declared offsets: the optional weight
column (`f8[rows]` at `weight_at`), the sample index (`i4[rows]` at
`sample_at`), every flat column, and both spans of every jagged column. The
buffer length equals:

```text
rows × (4 + (8 if weights are present else 0))
+ sum over flat columns of (rows × width, or ceil(rows / 8) for b1)
+ sum over jagged columns of (8 × (rows + 1) + content bytes by the same rule)
```

All spans must be in bounds and non-overlapping, and together fill the buffer.
There are no padding bytes.

*Presser behaviour*: weights first, then every span four bytes or wider from
widest to narrowest (a jagged column's offsets and content are two spans,
placed by their own widths), then the sample index, then spans narrower than
four bytes widest first, then packed bits; spans of equal width keep the
study's declaration order. Any row count therefore leaves every span aligned.
Column descriptors keep declaration order. *Reader guidance*: use offsets and
never assume physical block order. Readers may copy the buffer to obtain
8-byte alignment; writers pass it through untouched (W3).

The 1 GiB limit applies to the whole packed buffer, jagged content included.
Pressing, including parallel pressing, accounts for packed spans in its running
size check.

Validators read the payload against its descriptors: sample index range and
per-sample counts, finite weights and floats, zero tail bits, `lim` and `vals`
of every flat column against the observed values, and the digests of §5.

## 5. Digests

Four digests are simple proofs of three things: the bytes (whole and per
column), the recipe text, the provenance record. Each is 64 lowercase
hexadecimal characters of SHA-256.

- `data.sha256` — of the whole column buffer. REQUIRED.
- `data.cols[].sha256` — of one column's bytes; for a jagged column, the
  offsets span followed by the content span. REQUIRED on every column.
- `made.cart.sha256` — of the UTF-8 encoding of `made.cart.source`, the
  recipe text. REQUIRED when `made.cart` is an object.
- `made.sha256` — of the canonical JSON of `made` with the `sha256` key
  itself absent. REQUIRED. Canonical JSON is: object keys sorted by Unicode
  code point at every level, no whitespace (`,` and `:` as the only
  separators), strings escaped only where JSON requires it with non-ASCII
  characters kept as UTF-8, integers as their decimal digits, floats in the
  grammar of Python's `repr(float)`: the shortest digits that round-trip,
  written without an exponent when `1e-4 ≤ |x| < 1e16` or `x` is a zero
  (`0.0`, `-0.0`), and otherwise as `d.ddde±XX` with a signed exponent of at
  least two digits, an integral value keeping `.0` (`1.0` stays `1.0`,
  `1e16` is `1e+16`, `1e-5` is `1e-05`), the whole encoded as UTF-8. This is
  Python's
  `json.dumps(made, sort_keys=True, separators=(",", ":"), ensure_ascii=False)`;
  a serializer of another language reproduces this grammar rather than its
  own (JavaScript's `JSON.stringify` writes `10000000000000000` and
  `0.00001` for the two examples).
  The digest covers every key in `made`, including extension keys (§8).
  An implementation whose parsed numbers lose the integer/float lexeme
  distinction (`1.0` read as `1`) cannot recompute the digest from a parsed
  header when `made` holds a float-valued field; it verifies the digest from
  the raw header text instead. `contract/v9/cases.json` carries a `made`
  whose `metadata` holds float-valued fields: an integral float, one
  outside the no-exponent range, and a zero, so that every implementation
  meets each case.

Validators recompute all four on read and on write; a mismatch is a format
error. A save that preserves the buffer needs no new data digests. The
digests prove that a file agrees with itself, not that it is the file the
presser wrote: a writer that alters the buffer, its statistics, and every
digest together produces a self-consistent file, and only comparison with the
original's `data.sha256` detects it. That is why W3 forbids it rather than a
validator refusing it. A writer that changes anything in `made` recomputes
`made.sha256`. Consumer keys (§2.1) are outside all four.

## 6. `made` — the recipe snapshot

`made` is what the recipe declared and what the presser did, enough to lay the
snapshot beside the original `cart.py` and judge them consistent. Nothing in
it changes the meaning of a stored number. The snapshot is REQUIRED in full:
every key below is required unless marked optional, and a `made` missing one
is a format error. A writer that changes any of them recomputes the digest
(§5).

- `study_name` — REQUIRED, non-blank text. *Presser behaviour*: the first key
  of `made`.
- `title` — optional text.
- `note` — optional text, free prose the recipe attached to the study.
- `by` — REQUIRED text naming the presser and its version, for example
  `"RowVer 9.0.0"`.
- `built` — REQUIRED text, an ISO 8601 UTC timestamp by convention.
- `sha256` — REQUIRED (§5).
- `samples` — REQUIRED object keyed by sample name. Its keys must equal the
  set of `data.samples[].name`. Each value is an object with three REQUIRED
  keys:
  - `files`: object keyed by ROOT file UUID (text; the version 1 `TUUID`
    every ROOT file carries in its header, unique across files, preserved by
    a byte copy, replaced by any re-skim or merge). Each value is
    `{ file, tree, entries }`: `file` the bare filename (non-blank text with
    no `/` or `\`; no directory is ever stored), `tree` the tree name
    (non-blank text), `entries` the tree's entry count (natural). The object
    shape makes two files with one UUID in one sample unrepresentable;
    *presser behaviour*: such a pair is refused at press time. The same UUID
    may appear under two samples.
  - `weights`: a source declaration (below) or `null` for unweighted.
  - `metadata`: object, the recipe's per-sample metadata minus `weights`.
    Free content under §1's number rule.
- `keep` — REQUIRED; a source declaration, never `null`.
- `prepare` — REQUIRED; a source declaration, or `null` when the recipe has
  none.
- `cart` — REQUIRED; `{ sha256, source }` with `source` the whole recipe text
  and `sha256` its digest (§5), or `null` when the recipe was not loaded from
  a file. No path or modification time is stored.

A **source declaration** is `{"kind": "branch", "value": <non-empty text>}`
or `{"kind": "python", "name": <text>, "doc": <text>, "source": <text>}`,
the latter carrying a callable's name, docstring, and source text. Nothing in
`made` is executed.

Unknown keys in `made` and in its records ride as metadata (§8) and are
covered by `made.sha256`.

## 7. The writer contract

- **W1 — merge, never rebuild.** A writer starts from the header exactly as
  it arrived and merges edits into it. Rebuilding any object from a literal is
  the defect class that silently drops keys.
- **W2 — a no-op save is a fixed point.** Saving without edits re-emits the
  writer's canonical serialization of the UNCHANGED header, every key and
  value preserved, and saving a writer's own output again is byte-identical.
  Member order is preserved up to the host's canonical enumeration: a host
  whose object enumeration sorts integer-like keys first (as `names` keys are)
  may reorder them on the FIRST save; after that the order is fixed.
  Third-party formatting (pretty-printing, key spacing) canonicalizes on the
  first save.
- **W3 — `data` is never written.** The drawer is immutable and the column
  buffer passes through as the same bytes, so `data.sha256` and every column
  digest are constant for the life of the file.
- **W4 — unknown keys survive.** A key the writer has never heard of survives
  a save wherever the extension policy (§8) admits it. Consumer keys (§2.1)
  are the first case: a writer that is not that consumer carries the key
  through untouched.
- **W5 — a changed `made` is re-digested.** A writer that edits `made`
  recomputes `made.sha256`; one that does not touch `made` leaves the digest
  alone and re-emits `made` with every number lexeme as it arrived, so the
  digest still verifies. A host whose serializer cannot keep `1.0` distinct
  from `1` (W2's canonicalization) splices the original JSON text of `made`
  into its output instead of re-serializing it.

## 8. Extension policy (additive metadata)

Changes within version 9 are ADDITIVE ONLY, in these places:

- A new top-level key is a consumer key (§2.1) and needs no spec change.
- New keys inside `made`, on a `made.samples` record, on a file record, or on
  a column record ride as metadata: readers preserve and ignore them. Making
  such a key MODELED (validated, or read by the presser or `rowver check`)
  requires a revision of this document plus cases in `contract/v9/cases.json`
  before any writer emits it.
- No key may change meaning, type, or default within the version.
- A revision that removes or redefines a key is a new version number.
- Extension values obey §1's number-safety rule.
- The 2026-08-28 question of a designated `extensions` container is closed by
  §2.1: free metadata that belongs to a tool lives under that tool's key;
  free metadata about the data belongs in `made`.

## 9. Limits

| limit | value |
|---|---|
| header bytes | 8 MiB |
| column buffer | 1 GiB |
| integer lexeme magnitude | 2^53 |
| `vals` length | unbounded by the format |
| samples, columns | at least one each; otherwise unbounded |
