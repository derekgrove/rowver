# Extract output schemas

Versioned per kind. An extractor writes only what its kind's schema here
says, and bumps that kind's version when a key, dtype, or meaning changes.
Written before any extractor consumed it (step 1 of the extract plan);
`flattened` (rebased onto the resolver in step 2) is the first consumer.

## The three-file contract

Every extracted object `<name>` produces the three files below — with ONE
exception: kind `flattened` emits a single JSON file (the combine-framework
exchange shape) plus `.cuts.txt`, and no npz/csv.

| File | Holds |
|---|---|
| `<name>.meta.json` | the resolved spec: kind, schema version, variables, bin edges actually used, axis windows and the `--zoom` choice, error mode, stack order, per-source look, lumi ladder results, cuts applied |
| `<name>.data.npz` (+ optional `.csv`) | the numbers (per-kind keys below) |
| `<name>.cuts.txt` | always written: the python code of the cuts applied; an empty ladder is noted explicitly (`# no cuts`) |

Outputs default to `./extracts/<cart-stem>/`; `--out` overrides.

## Output identity

Leaves are unnamed and duplicates are legal, so identity comes from POSITION:
the stable core of `<name>` is `t<tab>_s<stage>` for a top-level stage entry,
`t<tab>_s<stage>_c<cell>` for a block cell. The kind and (for plots) the
sanitized variable(s) are appended for readability only:
`t0_s2_1d_MET`, `t0_s4_c1_2d_MET_nJet`, `t1_s0_map`, `t0_s3_board`.
Consequence, accepted: editing a plot's variable renames its output files;
the positional core is what stays stable, and consumers that need an
edit-stable key should key on it.

- sanitization: `[^A-Za-z0-9_]+` → `_`, runs collapsed, edges stripped
- names cannot collide: every name embeds its entry's own stage path, and
  paths are unique by construction (one entry per position)
- kind `cutflow` is TAB-level — the sankey is a dialog over the tab's
  selection, not a placed stage entry — so its name is `t<tab>_cutflow`,
  with no stage component (one cutflow per tab, by construction)

Region bin names use the viewer's `regionBinName` verbatim: comparison
operators transliterated (`<` → `_lt_`, `≥` → `_ge_`, `≤` → `_le_`), never
stripped — "X < a" and "X ≥ a" are different regions.

## Common policies

- **dtypes**: all npz arrays little-endian; counts and moments `float64`
  (the viewer accumulates in doubles), raw counts are integral values in
  `float64` arrays.
- **non-finite**: NaN/Inf are never sanitized in `.data.npz` — the numbers
  are the numbers. In `.meta.json` (strict JSON) non-finite values are the
  strings `"nan"`, `"inf"`, `"-inf"`.
- **NaN rows**: the selection mask reproduces the page: a NaN value PASSES a
  range cut (`x < lo || x > hi` rejects; NaN compares false) and FAILS a
  chip cut. In LINEAR binning a NaN row counts as kept but fills no bin and
  neither tail; in LOG binning NaN rides with x ≤ 0 (crammed into the low
  bin when overflow is on, dropped when off) and never counts as a tail —
  all of it the page's behavior, pinned in the fixture.
- **tolerance for parity claims**: raw counts exact; weighted sums 1e-8
  rtol, and an expected ZERO is exact. Never bit parity — EXCEPT that every
  weighted reduction accumulates left-to-right in row order (`_seq_sum`,
  cumsum-backed), the page's own order: numpy's pairwise `sum` can differ
  by more than any rtol under negative-weight cancellation. Per-bin fills
  (`bincount`) are already row-order. ONE carve-out on
  "raw exact": a value within ~1 ulp of a LOG-transform bin edge may land
  in the adjacent bin, because `Math.log10` differs by an ulp across
  engines (verified: V8 vs libm at 10^-0.4) — the page has the same drift
  against its own `edgeList` (`bin.ts:31`) and between browsers. Adjacent-
  bin drift only; totals and tails unaffected; the fixture avoids log-edge-
  exact values by policy.
- **string arrays in npz**: fixed-width little-endian numpy unicode
  (`<U...`), never object arrays — every npz must load with
  `allow_pickle=False`.
- **normalized plot spec**: a resolved plot carries `newPlot`'s defaults,
  the tab's `plot` defaults, the leaf's own keys, and the page's placement
  (`sheet`, `quad`, and `layout` for block members). The page's save
  bookkeeping (`blockRaw`, a parked board/map's `at`) is NOT part of the
  resolved content — position lives in the stage path.

## Kind: `flattened` — v1

JSON only (no npz): `{bin: {process: {"nominal": [raw, yield, sqrt(sum w²)]}}}`,
4-space indent, lexically sorted keys. Every process is declared in every
bin, even at zero. A repeated bin name gets `_2`, `_3`, … by occurrence
count, in leaf order — `regionFlat`'s exact behavior, pinned by fixture.
Known shared limit (page and extractor alike): a generated suffix can
collide with a leaf whose OWN name already ends in `_N`, and the later bin
then overwrites the earlier — fixing it means changing the viewer's export
too, so it is deferred and journaled rather than diverged from.

## Kind: `cutflow` — v1

Step labels are the sankey dialog's own (`cutTextFor`): chip cuts as
`name = a/b/c`, ranges as `name ≤ v` / `name ≥ v` / `lo ≤ name ≤ v` with the
viewer's `num()` formatting (en-US grouping — labels can contain commas).
NaN FAILS a ladder step (`lo <= x <= hi`), the opposite polarity of the
selection mask — the page's own split, pinned in the fixture; the `.cuts.txt`
is written in ladder polarity. There is no `--no-cuts`: a cutflow IS its cuts.

One accumulation carve-out from the common `_seq_sum` rule, ported from the
page: the per-source `by` follows `(acc.by[g] || 0) + ww` (select.ts:148), so
a NaN RUNNING total (overflow cancellation) resets to 0 before the next add —
a later weight un-poisons the per-source column, while `w`'s sticky `+=`
stays NaN. Fixture-pinned.

npz keys:

| key | shape | meaning |
|---|---|---|
| `steps` | (S,) str | `"no cuts"` then one label per active cut, ladder order |
| `raw` | (S,) f8 | events reaching each step |
| `w` | (S,) f8 | weighted yield reaching each step |
| `by` | (S, K) f8 | per-SOURCE weighted yield per step |
| `sources` | (K,) str | a source is a group name, or an ungrouped dataset's own name (`select.ts` cutflow's `gOf`) |

Ladder order is `activeCuts`: `cutorder` first, then column order.

## Kind: `plot1d` — v2

v2 (2026-08-30): `colours` became REQUIRED `meta.json` content; the
renderer refuses a record without it rather than guessing with a palette
walk that cannot reproduce the viewer's global slots. npz keys are
unchanged from v1.

npz keys:

| key | shape | meaning |
|---|---|---|
| `edges` | (nb+1,) f8 | the bin edges actually used (`edgeList`) |
| `centres` | (nb,) f8 | bin centres in the binning transform |
| `sources` | (K,) str | source names, FRAME order — see below |
| `h` | (K, nb) f8 | Σw per source per bin |
| `h2` | (K, nb) f8 | Σw² per source per bin |
| `n` | (K, nb) f8 | raw entries per source per bin |
| `under`, `over` | (K,) f8 | raw tail counts |
| `underW`, `overW` | (K,) f8 | weighted tails |
| `err_down`, `err_up` | (K, nb) f8 | per the plot's error mode; absent when mode is `off` |

Dimension order is always (source, bin). Frame order is the viewer's
`sources()` (groups first, then ungrouped samples) filtered to the plot's
own `srcs`/`mute` lists and to sources with at least one SHOWN dataset —
the order ROOT stacks in; plotly's largest-at-bottom re-sort is draw-time
layout, not record content. The window is frameFor's: an integer column
(≤200 values, non-log spacing) gets one bin per value; AUTO tracks the
`--zoom` quantile — the packed whole-cart p80/p90/p95/p99 (required wire
content since 2026-08-30), 4 significant digits; MANUAL
bounds parse like the page (`parseFloat`, longest numeric prefix) and an
inverted window falls back to the column's own range; nb clamps to
[2, 200] with 40 the default. The choice and resolved window are stamped
into `meta.json` (`window.zoom`, `window.from`: vals/auto/manual/lim;
`window.log` is the spacing REQUEST, `window.log_active` whether log
spacing actually engaged — a log request over a lo ≤ 0 window silently
stays linear, the page's own fallback). `meta.json` also carries `edges`
(the contract's "bin edges actually used") and `centres`, mirroring the
npz keys in strict JSON, and `colours` — the RESOLVED colour per drawn
source, stamped at extract time because the viewer's assignment walks
every global source and look namespace, which the record alone cannot
reconstruct. `colours` is REQUIRED as of v2; the render pass consumes
these.
A SHAPE-normalized plot is written normalized (`h`/`h2` scaled to unit
area, `n` raw), and the error bands ride the scaled moments — what the
page draws. Tails are per source, counted like `binned()`; a NaN row
fills no bin and neither tail (linear spacing).

## Kind: `plot2d` — v1 (reserved; built in step 6)

The parity object is the viewer's aggregate: `h` (nby, nbx) f8 — row-major,
y outer, exactly `binned2D`'s flat grid reshaped — plus scalar `kept`.
Per-source moment grids may ride as extras: keys `x_h_<source>`,
`x_h2_<source>`, `x_n_<source>`, each (nby, nbx) f8, `<source>` sanitized by
the output-identity rule (collision-suffixed in stack order). Extensions,
not a parity claim.

## Kind: `plot3d` — v1 (reserved; built in step 6, after 2D is proven)

Identical to plot2d: the viewer's 3D mode renders the SAME two-variable
`binned2D` grid as a surface — there is no third binning axis. Same keys:
`h` (nby, nbx) f8, scalar `kept`, `x_*_<source>` extras. Only the
`meta.json` `kind` differs.

## Kind: `board` — v1

Identity `t<tab>_s<stage>_board` — the board is a stage singleton. One
pass over the kept rows (`boardNumbers`), the shelf summed FIRST in
`bkgs` order (every square's B). npz keys (exact shapes so two
implementations cannot both "satisfy" this schema and disagree):

| key | shape | meaning |
|---|---|---|
| `rows`, `cols` | scalar i8 | the declared board shape |
| `cell_sources` | (Ncells,) str | cell sources in cells order; `""` for an empty cell |
| `raw`, `yield`, `w2` | (Ncells,) f8 | per cell; NaN for an empty cell |
| `err_down`, `err_up` | (Ncells,) f8 | per the board's error mode; ABSENT when the mode is `off` (same convention as plot1d) |
| `sob`, `sig`, `zbi` | (Ncells,) f8 | one array per metric SHOWN. Absent or empty `met` means ALL THREE (`board.ts:168`), not none |
| `bkgs` | (S,) str | the background shelf's source names |
| `bkg_raw`, `bkg_w`, `bkg_w2` | (S,) f8 | per shelf source |
| `bkg_total_raw`, `bkg_total_w`, `bkg_total_w2` | scalar f8 | the pooled background object (`BoardNumbers`, `board.ts:53`) |
| `weighted` | scalar bool | whether yields are w × lumi |
| `err_fallback_count` | scalar i8 | how often a Poisson error mode fell back to the symmetric error (`errFor`'s `fellBack`) |
| `overlap` | (P, 2) str | pairs of sources sharing a dataset |

The ZBi systematic actually used goes in `meta.json` only (a malformed value
normalizes to 0.05 — recorded there, never counted as a fallback).
`err_fallback_count` counts per READ, squares then shelf — a source on
both a square and the shelf that falls back counts twice, the viewer's
own tally. `meta.json` carries `err` (the mode actually used, wire
default `poisson_neff`), `syst`, `mets` (the shown metric keys), plus
the usual spec/look/active/cuts/lumi/weighted context.

## Kind: `regions` — v1

Identity `t<tab>_s<stage>_map` — the map is a stage singleton. What is
counted is the map's saved `root` only (a grid with no root is one
undivided region — the page's regionCount rule; deriving a tree from the
grid stays `extract flattened --from-map`'s explicit-request face). The
leaf partition, tree order:

| key | shape | meaning |
|---|---|---|
| `bins` | (L,) str | `regionBinName` per leaf, duplicate-suffixed |
| `sources` | (K,) str | the map's sources (its `srcs`, or every source) |
| `n` | (L, K) f8 | raw entries |
| `w` | (L, K) f8 | Σw |
| `w2` | (L, K) f8 | Σw² |
| `total` | scalar f8 | the viewer's normalization total (bubble radii): Σw over kept rows feeding ≥1 source, each row counted ONCE globally — NOT the sum over source columns, which double-counts overlapping membership |
| `neff` | (L, K) f8 | effective entries per cell, w²/Σw²; NaN when either moment is not positive (`region.ts` neff) |
| `asimov_z` | scalar f8 | Asimov Z, no systematic: sqrt(max(q0, 0)), q0 = 2·Σ[(s+b)ln(1+s/b) − s]; NaN when significance is not ready (falsy `sig` or empty `bkg` — the page's own gate and placeholder) |
| `asimov_dropped` | scalar i8 | bins with s ≠ 0 whose likelihood is undefined — s < 0 (or NaN), b ≤ 0 (or NaN), or a non-finite term: DROPPED AND COUNTED, never silently skipped |
| `asimov_thin` | scalar i8 | contributing bins whose pooled RAW background is under 5 |

The flattened face of the same numbers is kind `flattened`. The Asimov
`bins` count and the view's ZBi systematic (format fallback 5%) go in
`meta.json` (`asimov.bins`, `syst`), with the usual
spec/look/active/cuts/lumi/weighted context.
