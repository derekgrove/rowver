# `.rvr` 6 — the normative specification

This document is the standard. `rvr6.py check` and the browser's `check6` are the
two reference validators; where prose and validator disagree, that is a defect in
one of them, pinned by the shared corpus `contract/v6/cases.json` (byte-identical
in both repositories). Rules marked *reader behaviour* describe what conforming
readers DO with a value (clamping, defaulting) — the validators accept the wire
value as written.

Version 6 is a FREEZE CANDIDATE (2026-08-28). Once the owner declares the lock,
future changes may only ADD metadata under the extension policy below — no key
may change meaning, type, or default.

## 1. Container

```text
gzip( u32 little-endian header length | UTF-8 JSON header | column buffer )
```

- Header length: `1 ≤ len ≤ 8_388_608` bytes (8 MiB). The header is UTF-8 JSON.
- Number lexemes: an INTEGER-spelled lexeme (no `.`/`e`) must have magnitude
  at most 2^53 — 2^53 is legal, anything beyond is refused, INCLUDING
  even-valued integers a double could represent exactly (the cap is the rule,
  not representability). Float-spelled lexemes are IEEE doubles by
  definition (their nearest-double reading IS their value). Any lexeme
  overflowing to infinity is refused. This is what makes additive metadata
  round-trip-safe.
- Column buffer: at most `1_073_741_824` bytes (1 GiB), little-endian throughout.
- Compressed container cap in the browser: 512 MiB.

## 2. Header shape

Top level: `rowver: 6` (required), optional `opens_on` (natural number; with
empty `tabs` it must be `0`; otherwise `< tabs.length`), and exactly these five
drawers, all REQUIRED (empty allowed where noted):

| drawer  | type    | may be empty | holds |
|---------|---------|--------------|-------|
| `about` | object  | no (needs non-empty `name`) | study identity |
| `data`  | object  | no  | immutable buffer geometry |
| `view`  | object  | yes | settings shared by every tab |
| `tabs`  | array   | yes | ordered page setups |
| `made`  | object  | yes | provenance; emptying it never changes a plotted number |

Unknown keys: see §9 (extension policy).

## 3. `data` — the immutable drawer

Never rewritten by any writer (W3). Keys:

- `rows` — natural number of entries (events). One buffer row = one entry.
- `bytes` — exact byte length of the column buffer. Must equal the sum of all
  declared spans, and every span must fit inside it without overlap.
- `weight_col` — string name of the weight column, or `null` for none.
  When non-null, `weight_at` is required: an 8-aligned byte offset of a
  contiguous `f8[rows]` array. The weight column name must not collide with a
  data column name.
- `sample_at` — 4-aligned byte offset of the per-row sample-index column:
  `i4[rows]`, each value a valid index into `samples`. (It locates the
  sample-membership array, NOT "where the data begins" — every column carries
  its own offset.)
- `samples` — non-empty array of `{ name, rows, weighted }`:
  - `name`: non-empty string, unique across samples AND across every
    group/subgroup name (stage references are strings, so the namespace is one).
  - `rows`: natural; the per-sample values must sum to `data.rows`.
  - `weighted`: boolean. `false` means observed counts, never scaled.
- `cols` — non-empty array of column records:
  - `name`: non-empty unique string (same shared namespace).
  - `kind`: `"f8"` or `"i4"` only. Width 8 or 4 bytes.
  - `at`: byte offset, aligned to the column width; span `width × rows` must
    fit the buffer and overlap nothing.
  - `lim`: `[min, max]`, finite, `min ≤ max` — the measured extremes.
  - `nice`: `[lo, hi]`, finite, must ENCLOSE `lim` — slider endpoints.
  - `p1`, `p5`, `p10`, `p20`, `p30`, `p40`, `p50`, `p60`, `p70`, `p80`,
    `p90`, `p95`, `p99`: finite exact whole-cart percentiles, computed by
    the packer, all REQUIRED, ordered:
    `lim[0] ≤ p1 ≤ … ≤ p99 ≤ lim[1]`. (Owner decision 2026-08-30: the
    percentile ladder is wire content — the zoom feature reads
    p80/p90/p95/p99 and readers no longer measure a missing quantile; the
    rest are packed for future features at negligible header cost.)
    An EMPTY column stamps thirteen zeros (with `lim` `[0, 0]`) — the
    packer's stub, normative.
  - `vals` (optional, `i4` only): the distinct values of a small categorical
    column — unique integers, at most 25 (`MAX_CHIPS`). Its presence makes the
    column categorical: cuts select values, plots bin one-per-value.
  - `names` (optional beside `vals`): the packer's labels keyed by value —
    carried metadata. Readers DISPLAY labels from `view.cats[col].names`
    (§4), never from here; writers copy these into `cats` when creating the
    view.

## 4. `view` — shared page state

All optional unless noted.

- `groups` — `{ of: {sample→group}, sub: {sample→subgroup}, order: [group…] }`.
  Referenced samples must exist; `order` lists top-level groups (may include an
  empty group, so a destination can exist before members). Group and subgroup
  names obey the one-namespace uniqueness rule. Nesting is exactly two levels:
  a sample may carry a group, a subgroup, or both; subgroups never nest.
- `cats` — per-column categorical metadata:
  `{ colName: { names?: { value → label }, bands?: [{ label, vals, at? }] } }`.
  `names` are the display labels for declared values (readers take labels from
  HERE, not from `data.cols`); `bands` group declared values for cutting, with
  per-column-disjoint `vals`.
- `lumis` — `{ sample|sub|group: { name: number|null } }`. Positive finite or
  `null`; names must exist in their namespace. The outermost configured tier
  wins (group over subgroup over sample).
- `look` — `{ sample|sub|group: { name: style } }`. A style is an object; the
  modeled keys are:
  - `colour`: `"#rrggbb"` exactly (6 hex digits).
  - `draw`: `"bars"` | `"points"`.
  - `dash`: `"solid"` | `"dash"` | `"dot"` (bars only; absent = solid).
  - `lw`: number, `0 < lw ≤ 32` — this source's own line width.
  - `errstyle`: `"ticks"` | `"none"` (points only).
  - `binwidth_indicator`: boolean. An explicit value wins. When absent, the
    Viewer defaults points with `errstyle: "none"` to `false` and points with
    absent/`"ticks"` errstyle to `true`; bars preserve the value latently but
    never render the indicator. Python matplotlib rendering is deferred.
  - `role`: `"signal"` | `"background"` | `"data"` — stored and preserved,
    read by nothing since 2026-08-28 (kept for round-trip fidelity).
  Unknown keys inside a style object currently RIDE THROUGH unvalidated (an
  open interior; see §9's pending decision).
- `label` — `{ exp, tag, right }` strings: the experiment strip. *Reader
  behaviour*: malformed shapes are ignored, not refused.
- `fom` — `[numerator, denominator]` group names for the figure-of-merit
  pair. *Reader behaviour*: malformed shapes read as unset.
- `syst` — the ZBi relative systematic, number or numeric string; *reader
  behaviour*: a garbled value reads as the default `0.05`, never as `0`.
- `shown` — array of sample names to draw; absent = all shown. (Also valid
  per-tab; a locked `view.shown` is the shared truth.)
- `cuts`, `stage` — when present in `view`, these LOCK the concern: every tab
  shows the shared copy and each tab's own copy lies dormant
  (promote-and-shadow; unlocking restores the dormant copies).
- `prefs`:
  - `theme`: `"professional"` | `"graphite"` | `"cgb"` | `"pocket"`. The
    retired wire value `"retro"` reads as `"cgb"` and migrates on an edited
    save while surviving a no-op save byte-for-byte. Unknown theme ids ride
    through saves untouched.
  - `textScale`: display size hint (string).
  - `exportScale`: PNG export scale, number 1–8.

## 5. `tabs`

Each entry: `{ name, cuts?, chips?/shown?, stage?, plot?, … }`.

- `name`: non-empty string, unique among tabs.
- `cuts`: keyed by column NAME (the page keys its internal state by index;
  the wire never does): `{ colName: [lo, hi] }` for continuous columns
  (inclusive; `null` opens one end) and `{ colName: { "in": [values…] } }`
  for categorical columns (declared values only). Present-but-`null` cuts
  are invalid.
- `shown`: as in `view`.
- `plot`: DEFAULTS merged into every plot leaf of this tab's stage. Any plot
  key is legal here; `srcs`/`mute` obey the same rules as on a leaf (below).
- `stage`: ordered array of leaves and blocks (never `null`):
  - Leaf kinds: `1d`, `2d`, `3d`, `board`, `map`.
  - `block`: `{ kind: "block", cells: [leaf|null ×1–6], layout }` with layout
    one of `[6] [3,3] [2,2,2] [3,3,6] [6,3,3] [3,3,3,3] [2,2,2,2,2,2]`;
    blocks never nest; at least one cell is non-null.
  - At most one `board` and one `map` per effective stage.

### 5.1 Plot leaves (`1d`/`2d`/`3d`)

- `varx` required, and `vary` for `2d`/`3d` — must name declared columns.
- `srcs` (optional): the sources THIS plot draws — unique known source names
  (sample/subgroup/group namespace). Absent = every shown source (the meaning
  of every file from before the key). `[]` is a real state: an empty skeleton.
  A name that is not currently a top-level drawn source is INERT, not invalid
  (grouping is runtime-mutable; the name may return).
- `mute` (optional): sources kept in the plot's panel but drawn as nothing.
  Same validation as `srcs`.
- Binning: `nb` (*reader behaviour*: clamped to 2–200, default 40), `nby`
  (2D/3D; absent = follows `nb`, same clamp), `binspace`/`binspacey`:
  `"lin"`|`"log"`; absent = follow the log-axis flag. Validators accept any
  number on the wire; the clamps are how readers draw them.
- Axes: `logx/logy/logz` booleans; `xauto/yauto/zauto` booleans (absent =
  auto); `xmin/xmax/ymin/ymax/zmin/zmax` numbers or numeric strings — a manual
  range is the HISTOGRAM range (overflow folds into end bins per `oflow`).
- `oflow`: absent/`true` piles overflow into end bins; `false` drops it.
- `norm`: `"yield"` (default) | `"shape"`.
- `err`: `"off"` | `"sumw2"` | `"sqrtn"` | `"poisson_neff"` | `"poisson_raw"`.
- Pull (1D only): `pull` boolean — draws only when a points source faces
  bars; `pullref` (bars-source name or absent = the stack), `pullh` (*reader
  behaviour*: clamped 12–45, % of figure), `pmax` (σ window, default 5),
  `pulltitle`, `pullband` (absent = on).
- Dressing: `stack`, `legend`, `legendmax` (≥2), `legendpos`
  (`below`|`right`|`float`), `legendxy`, `frame`, `inward`, `gridx/gridy`,
  `xtitle/ytitle`, `x/yticksize`, `x/ytitlesize`, `padauto/padl/padb`,
  `lw`, `ms`, `fillop` (0–100), `explabel`, `showcuts`, `cmt`, `backend`
  (`root`|`plotly`), `aspect` (`free`|`16:9`|`3:2`|`4:3`|`1:1`), `wfrac`
  (30–100), `hpx` (200–900), `rootui`.
- Block membership: `sheet`, `quad`, `layout`, `shared` — see §5 block rules;
  `shared` broadcasts the whitelisted styling keys across the block.
- Carried legacy keys — modeled by readers, unvalidated, preserved on save:
  `span`, `pmin`, `decouple`, `compare`, `d3style`, `layer`, `layer2`, `view`,
  `cmpmode`, `titles`, `anno`, `drawcol`, `draw`, `lines`, `merge`.

### 5.2 `board`

`rows`/`cols` integers 1–4 (default 2×2); `cells` at most `rows × cols`
entries, each `null` or a known source; `bkgs` unique known sources;
`channel`: `"yield"`|`"metrics"`; `err`: the uncertainty enum above (default
`poisson_neff`); `met`: unique subset of `["sob","sig","zbi"]` (absent = all).

### 5.3 `map`

`srcs`/`bkg` unique known sources; `sig` a known source; `floor` ≥ 0;
`metric`: `"yields"`|`"sb"`|`"zbi"`|`"neff"`; `flip` boolean; `view`: a free
string riding as metadata — `"bubbles"` selects the map face, anything else
reads as the yields table. `grid`: `{ vars: [{col, bands}], exhaustive? }`
with ≤ 32 variables, ≤ 64 bands each, product of `(bands+1)` ≤ 4096 regions.
A band entry is EITHER a finite numeric threshold (thresholds strictly
increase) or a list — a disjoint non-empty set of declared categorical values
(≤ 25); the validators type-check each entry by its own shape, so the editor,
not the wire, keeps thresholds off categorical columns. `root`: a binary
region tree of `{ cut: {col, lt|in}, yes, no }` nodes, depth ≤ 64, ≤ 8192
nodes; an absent or `null` child is a leaf (a node may have none).

## 6. `about` and `made`

`about` requires a non-empty string `name`; `title` and other keys are free
identity metadata. `made` is provenance with a PARTIALLY VALIDATED core:
when present, `samples` must be records whose `name` and `rows` MATCH
`data.samples` one-to-one (each with `files` `[{path, tree}]`, `metadata`,
and a `weights` declaration — `{kind: "branch", value}`, `{kind: "python",
name}`, or none), and the source-carrying fields (`keep`, `prepare`,
`columns`, `cart`) are shape-checked. `built` is type-checked
(text) when present; the remaining conventional keys — `skimmed` (ISO
timestamp by convention), `by`, `merged` — ride as metadata, NOT validated. Additional keys ride as metadata. Neither drawer may influence a plotted number.

## 7. The column buffer

Contiguous little-endian arrays at the declared offsets: each data column
(`f8`/`i4`), the optional weight column (`f8` at `weight_at`), and the sample
index (`i4[rows]` at `sample_at`, values `0 ≤ v < samples.length`, with the
per-sample row counts matching `samples[].rows`). Spans must tile `data.bytes`
exactly — no gaps, no overlaps. Readers may copy the buffer to obtain 8-byte
alignment; writers pass it through untouched (W3).

## 8. The writer contract

- **W1 — merge, never rebuild.** A writer starts from the header exactly as it
  arrived and merges edits into it. Rebuilding any object from a literal is
  the defect class that silently drops keys.
- **W2 — a no-op save is a fixed point.** Saving without edits re-emits the
  writer's canonical serialization of the UNCHANGED header: every key and
  value preserved (even the retired `"retro"` theme; it migrates only on an
  edited save), and saving a writer's own output again is byte-identical.
  Member ORDER is preserved up to the host's canonical enumeration —
  JavaScript sorts integer-like object keys first, so such keys may reorder
  on the FIRST save; after that the order is fixed.
  Third-party formatting (pretty-printing, key spacing) canonicalizes on the
  first save; strengthening W2 to raw-byte retention is on the pre-lock list.
- **W3 — `data` is never written.** The drawer is immutable and the column
  buffer passes through as the same bytes.
- **W4 — unknown keys survive.** A key the writer has never heard of survives
  a save wherever the extension policy (§9) admits it.

## 9. Extension policy (additive metadata)

Frozen-format changes are ADDITIVE ONLY, in these places:

- Any new key at the top level, inside `about`, `made`, `view.prefs`, on a tab
  entry, or on any stage leaf/block — readers preserve and ignore them.
- New OPTIONAL keys on existing modeled objects require a spec revision here
  plus corpus cases in both repos before any writer emits them (as `srcs`,
  `mute`, `dash`, `lw` were added; the percentile ladder entered this way
  before being promoted to REQUIRED on 2026-08-30).
- Closed interiors: `view.lumis` tiers, region-tree nodes, and categorical
  cut objects validate strictly — new keys there are a spec revision, not
  free metadata. `view.look` styles validate their KNOWN keys strictly but
  let unknown keys ride (§4) — an open interior until the owner decision
  below lands.
- [OWNER DECISION PENDING 2026-08-28]: whether to additionally freeze one
  designated `extensions` container at top level as the sole future home for
  free-form metadata. Until decided, put new free metadata at top level.
- Extension values must obey §1's number-safety rule.

## 10. Limits (both validators)

| limit | value |
|---|---|
| header bytes | 8 MiB |
| column buffer | 1 GiB |
| categorical values / band members | 25 |
| block cells | 1–6, layouts as listed |
| board | ≤ 4 × 4 |
| grid variables / bands / regions | 32 / 64 / 4096 |
| region tree depth / nodes | 64 / 8192 |
| bins per axis | 2–200 (*reader clamp*, not validated — §5.1) |
| tabs `opens_on` | `< tabs.length` (0 when empty) |
