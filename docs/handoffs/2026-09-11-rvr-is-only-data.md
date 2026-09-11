# Handoff: strip the `.rvr` file to the data and its provenance

**2026-09-11 · decisions D9 to D19 settled with Derek; FORMAT.md rewrite next.**

*Read with D20 (end of this document) and D21 (the plan). D20 supersedes D11,
"None" under "Open questions", sequence item 2's "stay readable by `rvr6`",
and "versions 6 to 8 readable, never migrated" under "Removed and added";
D21 overturns choice 2 under "Status of step 1". `FORMAT.md` section
numbers cited in the body are those of the version 8 text.*
Supersedes the three handoffs written on 2026-09-10 (interrogation session,
rowver_web v8, Astra agnosticism review), which were deleted unlanded. The
rowver_web handoff is rewritten only after this document is settled.

## The decision

A cartridge is purely the data product. It contains the columns, the snapshot
of the recipe that produced them, and the persistent metadata about the data
itself. It knows nothing about any tool that might read it. The browser viewer
is one consumer among others; when it opens a cartridge that carries no
rowver_web state it says so and offers to start a new save state for the page.

Statistics that describe the data's distribution stay, because every plotting
consumer benefits from them: `lim`, `nice`, the percentile ladder, `vals`, and
value labels. They are not UI state; they are facts about the columns.

## Where the format stands (rowver 7c050ad, `.rvr` 8)

Five required top-level drawers: `about`, `data`, `view`, `tabs`, `made`, plus
`opens_on`. Of these:

| Member | What it is today | Under the decision |
|---|---|---|
| `about` | study name and title | stays (to be honed: which keys) |
| `data` | rows, bytes, samples, columns with kind, offset, statistics, provenance, digests; weights and sample index offsets | stays; this is the file |
| `made` | recipe snapshot: `cart` (path, sha256, whole source text), `keep` and `prepare` hooks, `columns` declarations, `samples[]` with files (path, tree, uuid, entries), metadata, weights declaration, `built` | stays (to be honed: which keys) |
| `view` | the viewer's shared page state: `groups`, `lumis`, `look`, `label`, `fom`, `syst`, `shown`, `prefs` (theme, textScale, exportScale), locked `cuts` and `stage`, and `cats` | leaves the file |
| `tabs` | ordered page setups: cuts, shown samples, a stage of `1d`/`2d`/`3d`/`board`/`map` leaves and blocks with layouts | leaves the file |
| `opens_on` | which tab the page opens on | leaves the file |

### `view.cats`, the one piece of `view` the presser writes

When a recipe passes `labels={"lepton": {11: "electron", 13: "muon", 15: "tau"}}`
the presser writes the values into `data.cols[].vals` and the labels into the
`view` drawer, because version 6 said readers display labels from there:

```json
"data": {"cols": [{"name": "lepton", "kind": "i4", "vals": [11, 13, 15], ...}]},
"view": {"cats": {"lepton": {"names": {"11": "electron", "13": "muon", "15": "tau"},
                             "bands": []}}}
```

`names` maps a stored integer to its display label; `bands` groups values for
cutting and is always empty from the presser (the page fills it). The labels
are facts about the data and stay; they move to `data.cols[].names`, a field
version 6 already defines beside `vals` for exactly this purpose. `bands` is
page state and leaves with the rest of `view`.

## What this removes from rowver

- **From the presser:** `view`, `tabs`, `opens_on` are no longer written;
  labels go to `data.cols[].names`.
- **From the validator (`rowver/rvr6.py`):** the whole page grammar, roughly
  40 percent of the module: `_cuts`, `_cut`, `_shown`, `_stage`, `_leaf`,
  `_plot`, `_board`, `_map`, `_grid`, `_bands`, `_region_cut`,
  `_region_tree`, `_groups`, `_namespaced`, `_lumis`, `_look`, `_style`,
  `_view`, `_tabs`, the `LEAF_KINDS`, `LAYOUTS`, `MAX_GRID_REGIONS`,
  `MAX_REGION_NODES`, `LOCKABLE`, `NEUTRAL`, `NAMESPACES` constants, and the
  helpers `effective`, `lumi_of`, `lock`, `unlock`. `MAX_CHIPS` (25) stays if
  it is kept as the cap on `vals`; that is one of the open questions.
- **From `FORMAT.md`:** sections 4 and 5 and every sentence that names the
  browser, a slider, a chip, a zoom, or a theme as the reason for a rule.
- **From the corpora:** every v6 case that exercises page grammar. The data
  cases stay.

Files of versions 6, 7, and 8 continue to be readable: their `view` and `tabs`
ride as metadata a data-only reader ignores. New presses write the new version.

## What this changes elsewhere

- **rowver_web** validates and owns its own state. It writes it under a single
  namespaced top-level key on save (name to be decided), which the extension
  policy in `FORMAT.md` section 9 already admits and which the writer contract
  (W1 to W4) already preserves. First open of a data-only cartridge: the page
  reports no rowver_web state and offers to create one. ADR 0004's
  independent implementations then cover the data contract only.
- **rowver_verification:** `extract/core.py` (the Python twin of the viewer's
  authored-state resolver) and `extract/flattened.py` (yields from a saved
  region map) read `view` and `tabs`; they would read the viewer's key. The
  `contract/core` corpus is unaffected in content, only in where the state is
  found.
- **Older cartridges** whose `view` and `tabs` hold real analyst work: a
  migration path (the viewer lifts them into its key on first save) or a
  statement that they stay as they are.

## Decided so far (2026-09-11)

**D9. `view`, `tabs`, and `opens_on` leave the file.** The cartridge is the
data, the recipe snapshot, and metadata about the data. Distribution
statistics (`lim`, `nice`, percentiles, `vals`, labels as `data.cols[].names`)
stay. Consumers keep their own state outside the presser's drawers.

**D10. `made.samples` becomes a dict keyed by sample name, its files a dict
keyed by ROOT file UUID, and absolute paths are not stored.** A ROOT UUID is
the version 1 `TUUID` written into every ROOT file header at creation: unique
across all files, preserved by a byte copy, replaced by any re-skim or merge,
readable for free through uproot (`file.uuid`). CMS names its `/store` files
by it. It identifies the file, not its content and not its location. Shape:

```json
"made": {
  "samples": {
    "TTJets": {
      "files": {"6f0c1a2e-…": {"tree": "Events", "entries": 2000000}},
      "weights": {"kind": "branch", "value": "genWeight"},
      "metadata": {"kind": "mc"}
    },
    "Data": {
      "files": {"b91e…": {"tree": "Events", "entries": 5509985}},
      "weights": null,
      "metadata": {"kind": "data"}
    }
  }
}
```

`name` and `rows` drop out of each record because `data.samples` already
holds them; the validator checks that the dict's keys equal the sample names.
Whether a file record keeps the basename (no directory) for human reading is
still open. Two files with the same UUID in one sample are refused by the
presser (decided 2026-09-11).

**D11. Legacy cartridges are not migrated.** Versions 6 to 8 with analyst
state in `view` and `tabs` stay as they are; the new version is separate and
the web viewer refuses to load older versions.

**D12. The 25-value cap on `vals` is dropped from the format.** The presser
warns the analyst at press time when a column enumerates many distinct values;
consumers decide what is small enough to show.

**D13. `FORMAT.md` is rewritten as the base specification of the new version**,
for Derek's review, rather than a fourth delta document.

**D14. A new-version cartridge that carries `view`, `tabs`, or `opens_on` is
rejected by `rowver check`.**

**D15. `about` folds into `made`.** The study name becomes the first key of
`made`, `study_name`; an optional title follows it. The header is then
`rowver`, `data`, `made`, and any consumer keys.

**D16. Consumer state lives under one top-level key named for the consumer**,
for example `"rowver_web": {...}`. The presser never writes such a key, every
writer preserves it, and the specification names the convention in one
sentence. `made.cart.path` and `made.cart.mtime` are dropped with D10;
`made.skimmed` duplicates `made.built` and one goes.

**D17. `made.columns` is dropped; `data.cols[].source` is the column
provenance.** The declaration list (`{"kind": "branch", "value": "MET"}` or a
hook's text) said what the recipe wrote; `source` (`{"class": "copy", "branch":
"MET", "dtype": "float64"}` or `{"class": "derived", "from": [...],
"reproducible": ..., "python": ...}`) says what the presser observed, including
which branches a hook actually read. The recipe text in `made.cart.source`
keeps every declaration verbatim. `source` stays optional in the validator
for columns the presser could not observe.

**D18. `made.sha256` is added.** The SHA-256 of the canonical JSON of `made`
(sorted keys, compact separators, UTF-8) with the `sha256` key itself absent,
computed by the presser and recomputed by `rowver check`. Beside `data.sha256`
and `made.cart.sha256` that is three simple proofs: the bytes, the recipe text,
the provenance record. Consumer keys sit outside all three.

**D19. File records keep the bare filename.** A file is
`"<uuid>": {"file": "TTJets_1.root", "tree": "Events", "entries": 2000000}`;
no directory is ever stored.

## The resulting header, in full

```json
{
  "rowver": 9,
  "data": {
    "rows": 7, "bytes": 330, "sha256": "…",
    "weight_col": "weight", "weight_at": 0, "sample_at": 288,
    "samples": [{"name": "mc", "rows": 3, "weighted": true},
                {"name": "data", "rows": 4, "weighted": false}],
    "cols": [
      {"name": "MET", "kind": "f4", "at": 232, "sha256": "…",
       "source": {"class": "copy", "branch": "MET_pt", "dtype": "float32"},
       "lim": [90.0, 320.75], "nice": [90, 330], "p1": …, "p99": …},
      {"name": "lepton", "kind": "i4", "at": …, "sha256": "…",
       "source": {"class": "copy", "branch": "lepton", "dtype": "int32"},
       "lim": [11.0, 15.0], "nice": [11, 15], "p1": …, "p99": …,
       "vals": [11, 13, 15], "names": {"11": "electron", "13": "muon", "15": "tau"}},
      {"name": "jet", "kind": "jagged", "at": 168, "sha256": "…",
       "content": {"kind": "f4", "at": 260, "count": 7},
       "source": {"class": "copy", "branch": "Jet_pt", "dtype": "float32"}}
    ]
  },
  "made": {
    "study_name": "study", "title": "optional",
    "by": "RowVer 9.0.0", "built": "2026-09-11T03:57:31Z",
    "sha256": "…",
    "samples": {
      "mc":   {"files": {"6f0c1a2e-…": {"file": "mc.root", "tree": "Events", "entries": 10}},
               "weights": {"kind": "branch", "value": "genWeight"}, "metadata": {"kind": "mc"}},
      "data": {"files": {"b91e…": {"file": "data.root", "tree": "Events", "entries": 12}},
               "weights": null, "metadata": {"kind": "data"}}
    },
    "keep":    {"kind": "python", "name": "keep", "doc": "…", "source": "def keep(events): …"},
    "prepare": null,
    "cart":    {"sha256": "…", "source": "<the whole cart.py>"}
  },
  "rowver_web": {"…": "written only by the viewer; the presser never emits it"}
}
```

Gone from version 8: `about` (into `made`), `view`, `tabs`, `opens_on`,
`made.columns`, `made.skimmed`, `made.cart.path`, `made.cart.mtime`, the
per-sample `name` and `rows` in `made`, file paths, the 25-value cap.

### What D10 changes in code

- `rowver/press.py`: `trace.files` is keyed by `(path, tree)` and
  `_provenance` writes the list of records with paths; it keys by UUID and
  drops the path. The recipe still needs paths to open files; they simply stop
  being written. Two files with the same UUID in one sample (a copy pressed
  twice) would collide and must be refused or deduplicated; decide which.
- `rowver/rvr6.py`: `_made_samples` and `_made_file` validate the dict shape
  (keys equal `data.samples` names; each file record has `tree` text and
  natural `entries`; UUID keys are text).
- `made.cart.path` (the absolute path of `cart.py`) is the same leak and goes
  with it; `made.cart` keeps `sha256` and `source`.
- Readers: nothing in rowver, rowver_verification, or the viewer reads
  `made.samples[].files` today. The viewer validates the list shape
  (`validateMade` in `rvr6.ts`) and reads a per-sample `weightSource` from a
  `cart.samples` stamp for export; both follow the new shape in the viewer
  handoff.
- Corpus: `contract/v8` carries the list shape and must be regenerated for the
  new version.

### Acceptance test for the recipe snapshot

Write a `cart.py`, press it, then from the cartridge's `made` drawer alone
reconstruct everything the recipe declared (samples, files by UUID with tree
and entries, weights declarations, metadata, hooks, column declarations, the
recipe text and its sha) and lay it beside the original `cart.py`. Derek
judges the two side by side for consistency and reproducibility. This is the
last step of the format work, after the presser change lands.

## Open questions for Derek

None. D9 to D19 above are the complete decision record as of 2026-09-11;
they go into `docs/plan-bare-bones-presser.md` and ADR 0010 with the step
that lands them.

## Sequence

1. Rewrite `FORMAT.md` as the version 9 base specification for Derek's review
   (D13). `FORMAT7.md` and `FORMAT8.md` become history of what 7 and 8 added.
2. ADR 0010 and D9 to D19 in the plan, with a step 9: presser and validator
   changes, `contract/v9/cases.json`, the press-time `vals` warning, the gate,
   Astra rounds as usual. Versions 6 to 8 stay readable by `rvr6`.
3. The recipe snapshot acceptance test (above), judged by Derek.
4. The rowver_web handoff, rewritten for version 9.

## Status of step 1 (2026-09-11, later the same day)

`FORMAT.md` is rewritten as the version 9 base specification and sits in the
working tree UNSTAGED, awaiting Derek's review. `FORMAT7.md` and `FORMAT8.md`
are untouched; the version 6 text is `git show 7c050ad:FORMAT.md`. No code,
corpus, or test changed. The draft states every rule D9 to D19 settle, carries
forward the rest of versions 6 to 8, and says explicitly that all thirteen
percentiles (`p1` to `p99`) are required on every flat column (Derek,
2026-09-11: the full ladder, not a subset, for future tools).

### Choices made where D9 to D19 were silent — Derek rules on each

The draft takes the first position in every line; the next session applies
Derek's verdicts to `FORMAT.md` before ADR 0010 and step 9 begin.

Ruled 2026-09-11 (plan step 9.1): every choice stands as drafted except 2
(the full snapshot is required) and 14 (moot under D20); the presser's
`about.note` becomes optional `made.note`. The verdicts are D21 in the plan.

1. **`about` in a version 9 header is rejected**, alongside `view`, `tabs`,
   `opens_on` (D14 named only the three). Alternative: let it ride as an
   unknown key.
2. **Required `made` keys are only `study_name` and `sha256`**; everything
   else is validated when present, so a redacted snapshot stays valid once
   re-digested. Alternative: require the full snapshot the presser writes.
3. **`made.cart.sha256` is recomputed by validators** against the UTF-8 of
   `made.cart.source` (D18 calls it a proof). Version 8 only type-checked it.
4. **Canonical JSON for `made.sha256`** is Python `json.dumps(made,
   sort_keys=True, separators=(",", ":"), ensure_ascii=False)`, `sha256` key
   absent, encoded UTF-8. Hazard: a language whose parsed numbers lose the
   integer/float lexeme distinction cannot recompute this from a parsed header
   when `made` holds a float-valued field (a cross section in `metadata`).
   Either the v9 corpus carries such a case, or `made` forbids float-spelled
   integers, or independent implementations verify this digest from the raw
   header text.
5. **`names` must label every `vals` member exactly once** (the presser's
   existing `labels` check). Partial labelling is refused.
6. **`file` may not contain `/` or `\`** — "no directory is ever stored"
   became a validated rule. UUID keys are unconstrained text.
7. **A consumer key's value is an object**, and the names `rowver`, `data`,
   `made`, `about`, `view`, `tabs`, `opens_on` are reserved.
8. **Sample names and column names are two namespaces.** Version 6 prose
   claimed one shared namespace; the validator never enforced it.
9. **Weight direction rules are now prose**: no weighted sample means
   `weight_col: null` and no `weight_at`. The validator already did this.
10. **Sample contiguity is presser behaviour, not a rule**; readers use the
    sample index.
11. **`by` is typed as text when present** (v8 typed only `built`).
12. **Key order** (`rowver`, `data`, `made`; `study_name` first in `made`) is
    presser behaviour, not validated.
13. **The consumer example is written `"<tool>"`**, not `"rowver_web"`, so the
    specification names no consumer. Put the real name back if preferred.
14. **`FORMAT7.md` and `FORMAT8.md` still link "[version 6](FORMAT.md)"**,
    which now resolves to the version 9 text. Options: a `FORMAT6.md` copy of
    `7c050ad:FORMAT.md` with the two links repointed, or leave them and rely
    on the git-history pointer in the new FORMAT.md's introduction.

### Removed and added, in one place

Removed from the v8 documents: `about`/`view`/`tabs`/`opens_on` and the whole
page grammar; the rule that labels display from `view.cats`; the 25 cap on
`vals` and band members; `made.columns`, `made.skimmed`, `made.cart.path`,
`made.cart.mtime`, file `path`, per-sample `name`/`rows` in `made`; the
browser 512 MiB compressed cap; the one-namespace rule across groups; the
"no cut or plot may cite a jagged column" sentence; the page-grammar limit
rows; the pending `extensions`-container decision (closed by D16).

Added: top level is exactly `rowver`, `data`, `made`, consumer keys; retired
keys rejected; the consumer-key convention; `made.study_name` and `title`;
`made.samples` and `files` as keyed objects with `{file, tree, entries}`;
`made.sha256` and its canonical JSON; `made.cart.sha256` verified;
`data.cols[].names` as the home of labels; `vals` unbounded with a press-time
warning; W5 (a changed `made` is re-digested); versions 6 to 8 readable, never
migrated.

## D20 (2026-09-11, evening): older versions are abandoned, not kept readable

Derek: the project has no users yet, so nobody's rug is pulled. Versions 6, 7,
and 8 are deleted from the repository and the live documents stop referencing
them; the `.rvr` standard is developed and polished as one current version
until it is ready. This supersedes D11 (which kept legacy files "as they are")
and the sentence above that says "files of versions 6, 7, and 8 continue to be
readable". Concretely:

- `FORMAT7.md` and `FORMAT8.md` are deleted; `FORMAT.md` drops its history
  pointers (the intro, §2.2's "versions 6 to 8 used", §8's "entered earlier
  versions this way", and the whole of §10 Versions).
- `contract/v6`, `contract/v7`, `contract/v8` and their pins in
  `tests/test_contract.py` are deleted; `contract/v9/cases.json` is the only
  corpus. `check_corpus_sync.py` pairs only `v9` (and `core` per ADR 0009).
- The reader accepts `rowver: 9` only: `READS`, the per-version `KINDS`
  table, and every `version` parameter in the validator go. The module is
  renamed from `rvr6.py` to a version-free name (`rowver/rvr.py`), since the
  old name itself is a reference to version 6; `from rowver import rvr6`
  import paths are not kept.
- The retired-key rejection of §2.2 stays (it guards against habit, not
  against old files), with the sentence about what versions 6 to 8 used
  those keys for removed.
- ADRs 0001 to 0009, the plan's Status table, and the dated handoffs are
  history and are NOT rewritten. Only live documents (`FORMAT.md`,
  `README.md`, `CONTEXT.md`, `docs/agents/*.md`, module docstrings) drop the
  references.
- The version number stays `rowver: 9` unless Derek renumbers; nothing is
  gained by restarting at 1 while D9 to D19, ADR 0010, and the plan all say 9.
- The rowver_web copies of `contract/v6`, `v7`, `v8` and its v6 to v8 reader
  are the viewer handoff's business (sequence item 4), not this repository's.
