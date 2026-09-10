# Narrow RowVer to pressing cartridges — execution plan

**DRAFT · 2026-09-10 · for agreement before any step runs.**

## Status

Each session takes the first step below that is not DONE, does it, then marks
it DONE here with the commit hash in the same commit that lands the work.

| Step | Status | Commit |
|---|---|---|
| 0 land v7 | DONE | bc24ff1 |
| 1 ADR 0009 | DONE | e4cdb9b |
| 2 move extraction | DONE | rowver_verification b1f2be9; rowver hash filled by step 3 (a commit cannot carry its own) |
| 3 remove builder | TODO (needs D1) | |
| 4 delete shims | TODO | |
| 5 optional trims | TODO (needs D2, D3) | |
| 6 docs | TODO | |
| 7 readability pass | TODO | |
| 8 format v8 | TODO (needs D6, D7) | |

A step that needs an undecided D-item is skipped, not blocked on: take the next
step and say so in the report.

## 0. What the record says, and what it does not

- `development/rvr-layout.md` (2026-09-07) describes the **current v6 bytes**. It
  proposes nothing.
- `development/type_pilots/REPORT.md` (2026-09-07) records the format decision:
  **retain v6; native ROOT scalar types are not worth it** (0.72 % compressed).
  The one measured win is bit-packed booleans.
- `FORMAT7.md` + `contract/v7/cases.json` (staged, uncommitted) is that win as a
  spec: **v7 = v6 + `b1` columns, nothing else.** This is the "slight format change".
- **The scope decision — RowVer does one thing, make a `.rvr` — is not written
  anywhere yet.** This plan and ADR 0009 (step 1) make it explicit.

## 1. Target state

| | Keeps | Loses |
|---|---|---|
| CLI | `rowver new`, `rowver run` (`--out`, `--step-size`, `--parallel`, progress flags), `rowver check`, `rowver show` | `rowver extract …`, `rowver render` |
| Library | `Cart`, `CartError`, `FormatError`, `study_names`; `rvr6.pack/write/read/check/read_by_sample` | `rowver.extract.*`, `rowver.builder`, `rowver.builder_spec` |
| Format | v7 per `FORMAT7.md`; reads v6 and v7; writes v7 | storage-driven format changes (type pilots closed this); step 8's v8 is provenance and jagged copies, pending D6, D7 |
| Owns | cartridge creation, source-data access, canonical `contract/v6` and `contract/v7` corpora | Python extraction, plot rendering, browser-builder helper, the viewer-resolver twin (`extract/core.py`) and its `contract/core` corpus |
| Deps | numpy, awkward, uproot | aiohttp (`[live]`), matplotlib + mplhep (`[render]`) |

## 2. Inventory and disposition

| Item | Lines / tests | Disposition |
|---|---|---|
| `rowver/press.py`, `rowver/rvr6.py`, `rowver/cli.py`, `rowver/__init__.py` | 2138 | **Keep.** Trim CLI. |
| `rowver/dashboard.py` + `tests/test_live_press_display.py` | 468 / 26 | **Keep** (progress for `run`). Decision D2 below on `--live-robust`. |
| `rowver/extract/{flattened,cutflow,plot1d,plot2d,board,regions,render}.py`, `extract/SCHEMAS.md` | 1964 | **Move to `rowver_verification`**, byte-for-byte, with their tests (`test_flattened`, `test_cutflow`, `test_plot1d`, `test_plot2d`, `test_board`, `test_regions`, `test_render`: 103 tests). |
| `rowver/extract/core.py` + `tests/test_core.py` + `contract/core/cases.json` | 1354 / 21 | **Move to `rowver_verification`.** It is the Python twin of the viewer's authored-state resolver, i.e. a verification artifact by construction (ADR 0004). The corpus copy moves with it; `check_corpus_sync.py` is re-pointed (frozen tool, needs a `FROZEN.md` entry). |
| `rowver/builder.py`, `rowver/builder_spec.py`, `tests/test_builder.py` | 752 / 5 | **Remove from rowver.** Decision D1: delete, or relocate to `rowver_web` (its only client, `web/src/ui/helper.ts`, and its ADR 0004). |
| Root shims `press.py`, `cart.py`, `rvr6.py`, `run_rowver.py` | 4 files | **Delete.** README already calls them a one-transition-period courtesy. |
| `benchmarks/` | empty (pycache only) | **Delete.** |
| `studies/{data_sig_bg_example,jetmet_2018_advanced,jetmet_2018_simple}` | tracked examples | **Keep** (README quick start depends on `jetmet_2018_simple`). |
| `my_studies/` | untracked | Untouched. |
| `gate.py`, `wheel_gate.sh` | | **Rewrite** production list and remove extract wiring. |
| `pyproject.toml`, `environment.yml` | | Drop extras, aiohttp, `extract/SCHEMAS.md` package data; new description. |
| `README.md`, `CONTEXT.md`, `docs/product-status.md`, `docs/agents/verification.md` | | Rewrite to the one-thing scope (step 6). |
| `FORMAT.md`, `FORMAT7.md` | | Keep both; FORMAT7 stays the delta. No module rename (`rvr6` stays the import path, as FORMAT7 promises). |

Tests after the moves: 8 + 7 + 4 + 15 + 26 + 15 = **75 in rowver**, 124 relocated.

## 3. How each step runs

- **One step, one fresh Claude session.** The session reads this plan, ADR 0009,
  and the step's section; it does not carry another step's context.
- **Reviewer: Codex Astra at medium reasoning.** The session runs the
  `AGENTS.md` gate with that model and effort (check `codex exec --help` for the
  exact flags) and records `codex AGREED r<N>` in the commit.
- One commit per step. Steps 2 and 3 touch two or three repositories; land the
  receiving repository first so nothing is ever absent from both.

### Code standard for every diff

- Optimise for speed and simplicity first, then for a human reading it cold.
- No long or run-on comments. A function may carry one short docstring and
  nothing else; if the body needs prose to be understood, restructure the body.
- Prefer the standard library and plain NumPy over new abstractions; no new
  dependencies.
- Moved code (step 2) moves byte-identical so the diff is a move; it is cleaned
  to this standard in its new home, not in transit.
- **Astra has the final say on readability.** In review it may rewrite any
  function wholesale when the rewrite reads better. The Claude session may argue
  the implementation back (speed, a missed edge case, a test that would break),
  and the two go round until both agree; the commit records the round count as
  usual. Readability is Astra's call, correctness is argued on evidence.

## 4. Steps

### Step 0 — land what is already in flight

The v7 work is staged on `dev/packed-booleans-v7`, verified (204 tests, 7/7
viewer gate, corpus sync), and blocked only on the review gate
(`development/boolean_v7/HANDOFF.md`). The worker was Codex, so the reviewer is
Claude.

1. `git diff --cached` review → commit `Press packed boolean columns as .rvr 7`.
2. Commit the unstaged housekeeping separately: `.gitignore` (`/development/`),
   `AGENTS.md` reviewer model, deletion of `LIVE_PRESS_DISPLAY_DESIGN.md` and
   `docs/live-press-performance-study.md`.
3. Merge to `main`. Everything below branches from there.

### Step 1 — write the decision down

- `docs/adr/0009-narrow-rowver-to-pressing-cartridges.md`: RowVer produces
  `.rvr` files and nothing else; extraction and rendering are verification
  tooling; the browser builder leaves this repository. Cite the type-pilot
  report as the reason native scalar types stop at v7; step 8's v8 is limited
  to provenance, jagged copies, and the D7 dtype decision.
- `docs/agents/verification.md` independence boundary: `rowver` no longer owns
  "Python extraction"; `rowver_verification` gains it and the `contract/core`
  corpus.
- Commit this alone. It is the reference every later diff points at.

### Step 2 — move extraction to `rowver_verification`

In `rowver_verification` (receiving side first):

1. Create a package `extract/` (or `rowver_extract/`, Derek's call) holding the
   eight modules, `__init__.py` (it defines `ExtractError`), and `SCHEMAS.md`
   byte-identical. Change the one relative
   import: `from .. import rvr6` → `from rowver import rvr6`. Byte access via
   the product reader is already sanctioned by the README ("only
   `rvr6.read_by_sample` for byte access").
2. Add `tests/` with the eight test files and the checked-in fixture
   `tests/golden.rvr` (`test_flattened` reads it); replace
   `from rowver.extract import …` with the new package path, and retarget the
   two `cli.main(["render", …])` calls in `test_render` to the new CLI entry.
3. Add `contract/core/cases.json`. Re-point `check_corpus_sync.py` so the
   `core` pair is `rowver_verification` ↔ `rowver_web` and the `v6`/`v7` pairs
   stay `rowver` ↔ `rowver_web`. Record the new sha in `FROZEN.md` with the
   reason.
4. Add a CLI entry (a `__main__` or small script) so `python -m extract flattened …`
   replaces `rowver extract flattened …`. `render` needs matplotlib and mplhep:
   add them to the verification environment, not to rowver.
5. Retire the "historical reference" `rvr_to_flattened.py` note in the README
   in favour of the living module now that it lives here.
6. Update `README.md` tools list; reassign GitLab rowver#3 (flattened vs external
   framework) to the verification tracker, since its subject moved.

Gate: verification test suite green with rowver installed `pip install -e
../rowver`; `check_corpus_sync.py` green against `rowver_web`.

In `rowver` (sending side):

7. `git rm -r rowver/extract tests/test_{flattened,cutflow,plot1d,plot2d,board,regions,render,core}.py tests/golden.rvr contract/core`.
8. `cli.py`: drop `extract` and `render` subparsers, `_extract`, the `extra`
   pass-through, and the module docstring line.
9. `pyproject.toml`: drop `[render]` extra and `extract/SCHEMAS.md` package data.
10. `gate.py`: drop extract entries from the production list and the import
    loop. `wheel_gate.sh`: drop the `extract flattened … bins source` check.

Gate: `python gate.py` (expect 75 tests), `sh wheel_gate.sh`,
`python ../rowver_verification/check_corpus_sync.py`.

### Step 3 — remove the browser builder (needs D1)

If D1 = delete: `git rm rowver/builder.py rowver/builder_spec.py
tests/test_builder.py`; drop both modules from `gate.py`'s production list;
drop `[live]` extra and `aiohttp` from
`environment.yml`; delete the builder paragraphs in `README.md`. Then in
`rowver_web`: retire `web/src/ui/helper.ts` and the Experimental Builder UI,
supersede its ADR 0004.

If D1 = relocate: same removals in rowver; the two modules and test land in
`rowver_web` as a Python helper next to `build_page.py`, importing `Cart` from
the installed `rowver` package. The worker launch at `builder.py` line 137
(`python -m rowver.builder`) and the generated recipe import at
`builder_spec.py` line 259 (`from press import Cart`) are retargeted to the new
module path and `from rowver import Cart`, and the relocated job-build test
must pass with the old module gone. `rowver_web` README takes over the
`python -m …builder` instructions.

Gate: `python gate.py`, `sh wheel_gate.sh`; `rowver_web` gate.

### Step 4 — delete the shims and dead directories

`git rm press.py cart.py rvr6.py run_rowver.py`; `rm -r benchmarks`. Update
`gate.py` production list and the README sentence about shims. Two tests still
import the shim: `tests/test_builder.py` line 14 (if step 3 has not run yet)
and the recipe fixture in `tests/test_cart.py` line 37; change both to
`from rowver import Cart`. If step 3 has not run, `rowver/builder_spec.py`
line 259 still generates `from press import Cart`: change it too. Recipes older
than 2026-08-28 that `from cart import Cart` stop working from a checkout;
`studies/*/cart.py` already import from `rowver`, verify with
`grep -L "from rowver" studies/*/cart.py`.

Gate: `python gate.py`, `sh wheel_gate.sh`.

### Step 5 — optional trims (needs D2, D3)

- D2 `--live-robust`: the indexed pre-pass opens every source twice and carries
  `Dashboard.index/begin_skim` and roughly a third of `test_live_press_display`.
  Dropping it reverses ADR 0005/0006's "two explicit alternatives" wording.
- D3 `--parallel`: ADR 0007. Keep unless Derek says otherwise; it is still
  "make a `.rvr`", just faster.

### Step 6 — docs to the new scope

- `README.md`: title "RowVer presser"; quick start unchanged; "Useful commands"
  reduced to `run/check/show/gate`; drop Programmatic-use extraction, builder,
  and shim paragraphs; "What is shared" stays; format paragraph stays.
- `pyproject.toml` description and `rowver/__init__.py` docstring: "Press ROOT
  ntuples into .rvr cartridges."
- `docs/product-status.md`: mark Combine extraction as moved; note the 2026-09-10
  scope baseline.
- `CONTEXT.md`: language is unchanged (Study, Cartridge, Presser, …); add one
  sentence under Presser that this repository is the presser only.
- Memory note in `MEMORY.md` for future sessions: scope decided 2026-09-10,
  extraction lives in `rowver_verification`.

### Step 7 — readability pass on what stays

Apply the code standard to `press.py`, `rvr6.py`, `cli.py`, `dashboard.py`:
cut comments to one short docstring per function, remove explanatory prose that
restates the code, and split any function that needed the prose. Behaviour
unchanged; `python gate.py` and `sh wheel_gate.sh` are the proof.

Astra reviews this step function by function and may replace any function with
its own version. The Claude session accepts readability verdicts and argues
only implementation facts (a benchmark on the tau skims, a failing test, a
dropped edge case). Rounds continue until AGREED.

### Step 8 — format v8: provenance, native dtypes, jagged copies (needs D6, D7)

The worked example and presser sketch are in `development/rvr8-column-classes.md`.
In order, each its own commit:

1. `data.cols[k].source` (copy or derived, inputs, reproducibility, source
   text), `data.cols[k].sha256`, `data.sha256`, `made.samples[].files[].uuid`
   and `entries`. Metadata only; the buffer is unchanged.
2. Native dtypes if D7 = yes: `kind` gains `f4`, `i8`, `u8`, `u1`, `i2`, … and
   `_kind` stops widening copies. Derived columns keep the current promotion.
3. Jagged copies: `kind: "jagged"` with int64 offsets and a `content` block, via
   `_column`, `_pack_jagged`, and the `check`/`read_by_sample` branches in the
   sketch. One level only. The viewer's reader and validator accept and skip the
   kind and preserve it on save, proven by an open/save test in `rowver_web`.
4. `FORMAT8.md` as a delta on v7, `contract/v8/cases.json`, and the sync into
   `rowver_web` and `rowver_verification` per step 2's corpus layout.

Gate per commit: `python gate.py`, `sh wheel_gate.sh`, corpus sync, and a
press of the 18-file tau skim set timed against the 82 s sequential baseline.

## 5. Decisions Derek owns

- **D1** Browser builder: delete, or relocate to `rowver_web`?
- **D2** Keep `--live-robust`?
- **D3** Keep `--parallel`? (recommended: yes)
- **D4** Name of the receiving package in `rowver_verification` (`extract` vs `rowver_extract`).
- **D5** GitLab rowver#2 (hosted credentialed pressing): still open, or closed as
  out of scope now that the product is a local presser?

## 6. Order and size

| Step | Repos | Rough diff | Blocks |
|---|---|---|---|
| 0 | rowver | already staged | everything |
| 1 | rowver | 2 files | 2–6 |
| 2 | verification, web (corpus), rowver | ~3.3k lines moved, ~150 changed | 3, 4 |
| 3 | rowver, web | ~750 removed | — |
| 4 | rowver | ~40 removed | — |
| 5 | rowver | up to ~200 removed | — |
| 6 | rowver | README rewrite | — |
| 7 | rowver | comments only | after 2–5 |
| 8 | rowver, web, verification | ~400 added, 4 commits | after 7, D6, D7 |

Steps 3, 4, and 6 are independent once 2 has landed. Step 7 runs after them;
step 8 after 7 so the format work lands on clean code.

## 7. Format questions raised 2026-09-10 (feeds step 8)

Derek's framing: first-class columns are literal, immutable copies of ROOT
branches in their exact order, protected by a stored SHA; second-class columns
are built from first-class ones, possibly with the construction logic stored in
the file. What the current format already has, and where it falls short:

- **Provenance exists but is prose.** `made.columns` stores each column
  declaration's source text and `made.cart` stores `cart.py` with its SHA-256.
  A copy (`"MET": "MET_pt"`) and a derivation (`lambda events: ...`) become
  indistinguishable `f8`/`i4`/`b1` columns in the buffer.
- **Copies are value-exact, not type-exact.** Every column is widened or
  narrowed to `f8`, `i4`, or `b1`; 64-bit integer branches cannot be copied
  exactly. The type pilots measured native dtypes as a storage non-event and
  explicitly deferred them as "a fidelity/design decision". This is that decision.
- **No payload hash.** Immutability of the data drawer is policy (FORMAT7: saves
  preserve it), not something a reader can verify. The pilots measured a
  whole-payload SHA-256 check at ~82 ms for a 150 MB cartridge.
- **The viewer cannot run Python**, so stored construction logic is either
  documentation (already there) or a new expression language implemented twice
  under ADR 0004. That is the part that is beyond this project.

Candidate v8 (metadata-only unless native dtypes are adopted):
`data.cols[k]` gains `source` (`{file_uuid?, tree, branch, dtype}` for copies,
`{from: [...], text: "..."}` for derived), `sha256` of the stored bytes, and
`data.sha256` over the whole buffer. Decision D6: land v7 as staged and design
v8 separately, or hold v7 and ship one reader change.

- **Jagged branches are storable but not stored.** A jagged array is an
  offsets array plus a flat content array, both plain bytes. The format
  excludes them because a column means one value per event; the viewer's cuts,
  histograms, and statistics assume that. Decided 2026-09-10: v8 stores jagged
  branches as first-class copies (`kind: "jagged"`) that the viewer ignores for
  plotting and selections, so other `.rvr` consumers get object-level data.
  The viewer's reader and validator must learn the kind: accept it, skip it,
  and preserve its bytes on save.
  Worked example in `development/rvr8-column-classes.md`.

- **D6** v7 now and v8 later, or fold provenance into the pending format change?
- **D7** Native dtypes in, or keep `f8`/`i4`/`b1` and accept value-exact copies?
- **D8** Jagged copies: decided yes (option 2 above). Open sub-choices are the
  offsets width (int64 is safe; int32 caps content at 2^31 objects) and whether
  weights are duplicated per object or left to the consumer via offsets.

## 8. Side exercise: Astra from a blank directory

Separate from steps 0–8 and not on their critical path. Astra receives only
`development/astra-from-scratch.md` (the brief: what the project is for, what
a cartridge is, how to keep it slim, and where the ROOT files are) and builds
the same product in `~/Code/rowver_astra` without reading this repository. The
point is to see what a clean-slate design looks like next to ours. Its result
is read, not merged; anything worth stealing becomes a normal step.
