# Live pressing flow and performance study

Date: 2026-08-31

## Executive assessment

The live path studied here was not optimized for the commissioning priority,
"get the user their `.rvr` file as soon as possible." It performed a serial
metadata pre-pass over every source solely to obtain complete progress bars and
an ETA, then the Presser reopened all sources for the useful skim. That
implementation and its measured cost remain available as `--live-robust`. The
former decision is recorded in
[`docs/adr/0005-default-to-the-live-press-display.md`](adr/0005-default-to-the-live-press-display.md)
and is superseded by
[`docs/adr/0006-default-to-one-pass-live-progress.md`](adr/0006-default-to-one-pass-live-progress.md).

The implemented default fast path is now:

```console
rowver run jetmet_2018_advanced
```

It reports source and chunk activity from `Cart.skim` and does not pre-open
sources merely to calculate an ETA. `--no-live` retains plain one-pass execution
without dashboard work. `--live-robust` selects the measured indexed behavior
when exact totals, bars, and ETA are worth the additional source opens.

The observed `indexing 7/8` wait was inside the seventh source's metadata open.
It was not caused by numerical, column, weight, size, cartridge-validation,
compression, or atomic-write safety checks; none of those run during indexing.
At that moment six sources had completed and RowVer was blocked somewhere in
the synchronous `_open_sample_tree` / `uproot.open` / `num_entries` / close
operation for source seven. RowVer supplies no timeout and does not parallelize,
cache, or reuse those opens.

One discrepancy matters: the tracked `jetmet_2018_advanced` currently contains
nine sources, not eight: `1 + 1 + 1 + 2 + 1 + 3` across its six samples
([`studies/jetmet_2018_advanced/cart.py:27`](../studies/jetmet_2018_advanced/cart.py#L27)).
Therefore the old `7/8` display came from a different resolved fileset. Possible
causes include a different installed checkout, working-tree state, study revision,
or (for studies with local patterns) a different set of glob matches at load time.
The last possibility does not apply to this all-remote study. Without the captured
source name, it is not sound to identify which file was seventh in that run. In
the current nine-source study,
source 7 is the first WJets file, `71AB3D1B-...root`, but that does not establish
that it was the source in the earlier eight-source run.

## Measurement

On 2026-08-31 I ran the exact source helper used by indexing and skimming,
`_open_sample_tree(sample, path, tree)`, read `source.num_entries`, and closed the
context. One Python process traversed all nine sources three times, matching the
serial in-process shape of `Dashboard.index` and allowing client-level reuse.
The workstation was macOS 26.1 arm64 with Python 3.13.14, Uproot 5.7.5, and a
valid X.509 grid proxy. This measures only the index operation, not event reads or
the full press. It is a three-repeat remote snapshot, not a benchmark guarantee.

| Current source | Entries | R1 | R2 | R3 | Per-source median |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1/9 MET 2018A | 1,086,853 | 7.604 s | 3.740 s | 4.185 s | 4.185 s |
| 2/9 MET 2018D | 2,669,674 | 6.175 s | 3.445 s | 5.404 s | 5.404 s |
| 3/9 TTJets DiLept | 410,350 | 5.191 s | 3.041 s | 3.112 s | 3.112 s |
| 4/9 TTTo2L2Nu, file 1 | 1,374,000 | 10.503 s | 6.426 s | 6.178 s | 6.426 s |
| 5/9 TTTo2L2Nu, file 2 | 1,359,000 | 5.861 s | 4.424 s | 3.237 s | 4.424 s |
| 6/9 DYJets M-50 | 1,938,185 | 6.442 s | 3.348 s | 8.066 s | 6.442 s |
| 7/9 WJets HT 400-600, file 1 | 803,678 | 4.991 s | 4.442 s | 2.506 s | 4.442 s |
| 8/9 WJets HT 400-600, file 2 | 684,551 | 3.233 s | 4.184 s | 3.611 s | 3.611 s |
| 9/9 WJets HT 400-600, file 3 | 618,811 | 3.909 s | 16.120 s | 2.691 s | 3.909 s |
| **Serial pass total** | **10,945,102** | **53.909 s** | **49.169 s** | **38.991 s** | **49.169 s median of totals** |

The observed totals span 39.0–53.9 seconds; individual opens span 2.5–16.1
seconds. The reported roughly 20-minute tail was not reproduced and remains
uncharacterized. The measurements establish a material display-only pre-pass and
show ordinary remote variability, but they do not establish why the earlier open
stalled or bound future tail latency. The repository design warns that remote
opens may spend a long time resolving a source, authenticating, or retrying
([`LIVE_PRESS_DISPLAY_DESIGN.md:21`](../LIVE_PRESS_DISPLAY_DESIGN.md#L21)).

Index handles are not reused by the skim. Some open cost may nevertheless carry
over through XRootD client connection pools, redirector resolution, or server-side
caches; this study did not measure that carry-over. A full end-to-end live versus
`--no-live` comparison was not inferred from this probe, because skim throughput
and post-skim CPU costs require separate measurements.

The probe was an inline read-only diagnostic, not a committed product or
verification tool. Its essential loop was:

```python
cart = Cart.load("jetmet_2018_advanced")
sources = [(sample, path, tree)
           for sample, spec in cart.fileset.items()
           for path, tree in spec["files"].items()]
for repeat in range(3):
    for sample, path, tree in sources:
        started = time.perf_counter()
        with _open_sample_tree(sample, path, tree) as source:
            entries = source.num_entries
        print(repeat, sample, entries, time.perf_counter() - started)
```

Independent verification was not run: this report changes no pressing behavior,
and the private verifier currently provides no independent source-open latency
check. The local product tests are recorded under Verification below.

## What `indexing N/M` means

`Dashboard.__init__` flattens the ordered fileset into `(sample, path, tree)`
tuples and initializes the first source
([`rowver/dashboard.py:97`](../rowver/dashboard.py#L97)). `Dashboard.index` then
processes that list serially. Before an open it sets `index_file = number`, the
current sample, and the current path. Only after the context-managed open,
`source.num_entries`, and close have succeeded does it update `indexed_files`
and the entry totals ([`rowver/dashboard.py:157`](../rowver/dashboard.py#L157)).

Consequently:

```text
Indexing files  7/8
...
6 files indexed ...
```

means "attempting source 7; six are complete," not "seven sources are complete"
and not "87.5% of the indexing work is complete." There is no progress signal
inside one open. On a TTY, redraws include a spinner and a separate ticker forces
a redraw every half second; main-thread draws also advance it. Redirected output
has no spinner or ticker. A moving TTY spinner therefore proves display liveness,
not remote-operation progress
([`rowver/dashboard.py:233`](../rowver/dashboard.py#L233),
[`rowver/dashboard.py:267`](../rowver/dashboard.py#L267)).

The open chain is:

```text
Dashboard.index
  -> _open_sample_tree(sample, path, tree)
     -> _open_tree(path, tree)
        -> uproot.open({path: tree}, handler=XRootDSource)
  -> source.num_entries
  -> context-manager close
```

The explicit XRootD handler is shared with the real skim
([`rowver/press.py:62`](../rowver/press.py#L62),
[`tests/test_live_press_display.py:329`](../tests/test_live_press_display.py#L329)).
Errors are translated into `CartError` and URL credentials/query/fragment values
are redacted. Redaction runs only on failure and is not a plausible steady-state
cost ([`rowver/press.py:69`](../rowver/press.py#L69)).

## Full call and data flow

### 1. Command dispatch and study loading

1. The installed `rowver` command enters `rowver.cli:main`
   ([`pyproject.toml:19`](../pyproject.toml#L19)).
2. `main` parses three mutually exclusive progress modes: fast live by default,
   plain one-pass with `--no-live`, or indexed live with `--live-robust`. It then
   dispatches to `_run`.
3. `_run` normalizes `--step-size` to either an integer entry count or an Uproot
   byte-size string, then calls `Cart.load` ([`rowver/cli.py:25`](../rowver/cli.py#L25)).
4. `Cart.load` resolves and imports the study's `cart.py`, retrieves its exported
   `CART`, attaches the study name/directory/source, expands local paths and globs
   while retaining remote URLs, and calls `Cart.validate`
   ([`rowver/press.py:212`](../rowver/press.py#L212)).
5. `Cart.validate` checks study structure, hook signatures, callable source
   recoverability, explicit weight declarations, metadata JSON compatibility,
   labels, and title/note types before any ROOT source is opened
   ([`rowver/press.py:261`](../rowver/press.py#L261)). It returns no value and
   marks the `Cart` validated.
6. `cart.summary()` produces a string containing the study, sample/file counts,
   weighted status, and column names; `_run` prints it
   ([`rowver/press.py:503`](../rowver/press.py#L503)).

### 2. Display-mode dispatch

1. Default fast live mode creates `Dashboard(cart)` and passes it directly to
   `Cart.build`. It performs no indexing call. TTY output gets the existing
   0.5-second ticker; redirected output gets durable source, sample, and pressing
   lines.
2. `--live-robust` creates `Dashboard(cart, robust=True)`, enters the same live
   context, calls `Dashboard.index`, and then calls `begin_skim`. Index totals are
   display-only and are not inputs to `Cart.build`.
3. `--no-live` constructs no dashboard and calls
   `Cart.build(..., progress=None)`.

TTY detection chooses rendering, not execution semantics, in both live modes.

### 3. Real skim

`_run` calls `Cart.build`, which first calls `Cart.skim`
([`rowver/press.py:474`](../rowver/press.py#L474)). `Cart.skim` returns two
in-memory mappings:

```text
rows = {sample: {column: one-dimensional NumPy array}}
weights = {weighted_sample: float64 NumPy weight array}
```

The work for each sample and source is sequential
([`rowver/press.py:327`](../rowver/press.py#L327)):

1. Create per-column chunk lists and a weight-chunk list.
2. Build metadata `{weights, ..., sample, file, tree}` with `Cart._meta`.
3. Notify a lifecycle-aware progress observer immediately before opening the
   source, then open it through `_open_sample_tree`. This is the only source open
   in default and plain modes. Robust mode does not reuse its indexing handle.
4. Choose entries per chunk. An integer `--step-size` is used directly. For a
   remote source, RowVer estimates bytes per entry from file bytes and total tree
   entries, avoiding `tree.num_entries_for`, whose basket-header reads are
   recorded in the code as thousands of remote round trips and minutes of delay.
   Local trees use `num_entries_for` when available; the fallback is 100,000
   entries ([`rowver/press.py:375`](../rowver/press.py#L375)).
5. Read the chunk with `tree.arrays(..., library="ak", virtual=True)`. Uproot
   virtual arrays materialize only branches touched by the recipe. If unsupported,
   retry without virtual mode ([`rowver/press.py:383`](../rowver/press.py#L383)).
6. Invoke optional `PREPARE`; the advanced study has none.
7. Invoke `KEEP`, require a one-dimensional Boolean mask of the prepared length,
   and select events. The advanced study keeps `nJet >= 1` and `MET_pt > 175`
   ([`studies/jetmet_2018_advanced/cart.py:23`](../studies/jetmet_2018_advanced/cart.py#L23)).
8. For every declared column, `Cart._evaluate` either selects a branch or invokes
   a callable. `_one_dimensional` converts the result to NumPy and rejects
   missing, jagged, nonnumeric, nonfinite, or wrong-length results. The advanced
   study produces eight columns. Its data chunks touch eight unique event
   branches; weighted MC additionally touches `genWeight`
   ([`studies/jetmet_2018_advanced/cart.py:81`](../studies/jetmet_2018_advanced/cart.py#L81)).
9. Evaluate weights for weighted samples, convert them to float64, and reject a
   wrong length or nonfinite values.
10. Track whether each output column needs 4 or 8 bytes and refuse at the first
    chunk whose definitely required buffer would exceed the 1 GiB cartridge data
    limit. This avoids completing a doomed long skim
    ([`rowver/press.py:343`](../rowver/press.py#L343)).
11. If live, call the dashboard with `(sample, raw_rows, kept_rows)`. It updates
    and occasionally draws counters; it does not transform data.
12. Concatenate the chunk arrays per sample and notify lifecycle-aware observers
    that the sample completed. After all samples, return `rows` and `weights`.

### 4. Pack, validate, compress, and publish

1. `Cart.build` emits progress `(None, 0, 0)`. `Dashboard.chunk` changes the
   display to `pressing`; no data is returned from this callback.
2. `Cart._provenance` returns a metadata dictionary containing RowVer version,
   timestamps, sample/source descriptions, hook and column source descriptions,
   and the study source text/hash ([`rowver/press.py:447`](../rowver/press.py#L447)).
3. `rvr6.pack(rows, weighted, weights, labels, about, made)` normalizes the arrays,
   checks matching columns and lengths, fills unit weights for unweighted samples
   in mixed cartridges, concatenates each column, chooses `i4` or `f8`, computes
   min/max, nice ranges, 13 percentiles, and small integer value sets, creates the
   sample-index column, joins the byte slabs, constructs the v6 metadata, validates
   it with `rvr6.check`, and returns `(meta: dict, buf: bytes)`
   (`_stats` at [`rowver/rvr6.py:792`](../rowver/rvr6.py#L792), `pack` at
   [`rowver/rvr6.py:812`](../rowver/rvr6.py#L812)).
4. `rvr6.write(target, meta, buf)` runs `rvr6.check` again, frames header plus
   buffer, gzip-compresses the complete frame at level 9, writes a temporary file,
   flushes and `fsync`s it, atomically replaces the target, and returns
   `{file, rows, bytes, compressed}`
   ([`rowver/rvr6.py:733`](../rowver/rvr6.py#L733)).
5. The live context stops and joins its ticker. `_run` draws one final TTY frame
   and prints `wrote <file> — <rows> rows, <MiB>`
   ([`rowver/cli.py:50`](../rowver/cli.py#L50)). The durable artifact is the target
   `.rvr`; the large `rows` and `weights` mappings are transient build memory.

## Where time is spent and what is safety

### Indexing

The expensive operation is the extra serial remote open, not safety work.
Indexing reads only the tree entry count and updates display counters. The shared
source policy and error redaction are sensible consistency/security features and
do not add material cost to successful opens. The 2 Hz ticker and small string
formatting are expected to be negligible relative to multi-second XRootD opens;
that is an inference from code shape, not a profiler result.

The pre-pass was an explicit product tradeoff, not an accidental consequence of
validation. The design listed parallel opens, persistent count caching, and
handle reuse as non-goals
([`LIVE_PRESS_DISPLAY_DESIGN.md:355`](../LIVE_PRESS_DISPLAY_DESIGN.md#non-goals)).
The measured 39.0–53.9 second snapshots and reported unbounded tail are evidence
that the observability tradeoff should be revisited if the commissioning request's
time-to-file priority is adopted.

### Study-load checks

`Cart.validate` is CPU-only over the small study declaration and runs before any
source open. These checks prevent ambiguous weights, malformed hooks, and invalid
metadata. Their cost is not credible as a significant part of an advanced remote
press.

### Skim

The skim contains real speed-conscious choices: bounded chunks, virtual arrays
that materialize touched branches, and a cheap remote chunk-size estimate instead
of basket-by-basket sizing. Its safety checks are per-chunk scans/conversions and
therefore scale with kept rows and columns. They should be profiled before being
removed because they enforce the cartridge's numerical contract.

There are optimization candidates that do not weaken that contract:

- `_invoke` repeats `inspect.signature` through `_call_shape` for each callable
  invocation, even though validation already inspected every hook. Cache the
  accepted arity if profiling shows many small chunks make this visible.
- Avoid redundant conversions or min/max/finiteness scans only after a profiler
  establishes which array operations dominate; preserve equivalent rejection.

Neither item can explain an `indexing` stall.

### Packing and writing

Post-skim work has several potentially material CPU/memory costs:

- `_stats` invokes `np.percentile` separately for 13 percentiles on every column.
- Packing concatenates and encodes full columns and then joins all slabs into a
  new `bytes` object.
- `pack` calls the full `rvr6.check`, and `write` immediately calls the same full
  check again. The check rescans sample indices, weights, column finiteness,
  min/max, and categorical unique values
  ([`rowver/rvr6.py:475`](../rowver/rvr6.py#L475)). This is genuine duplicated
  validation work on the just-produced buffer.
- `gzip.compress(..., compresslevel=9)` explicitly chooses maximum compression
  effort rather than fastest production.
- Temporary-file writing, flush, `fsync`, and atomic replacement provide crash
  safety. They should normally remain; the compression level and duplicate
  validation are better first benchmarking targets.

These are credible later bottlenecks, but this study did not run a representative
post-skim profile, so it would be wrong to rank their wall-time impact from source
inspection alone.

## Recommendation and implementation status

ADR 0006 adopts the study's primary recommendation and supersedes ADR 0005.

1. **Implemented: default live execution is one-pass.** Lifecycle notifications
   identify real source opens and exact sample completion; chunk callbacks provide
   scanned and kept counts. The fast renderer never presents totals, percentage,
   bars, or ETA.
2. **Implemented: retain both explicit alternatives.** `--no-live` is plain and
   one-pass. `--live-robust` preserves the old metadata pre-pass and exact display.
3. **Do not lead with parallel pre-indexing, caching, or nine retained handles.**
   They complicate invalidation/resource behavior and still preserve work that
   exists only for presentation. Delete the pre-pass from the critical path first.
4. **Add phase timings and a repeatable benchmark.** Record study load, each
   source open, read/select/column work, concatenate, pack/statistics/check,
   compression, `fsync`, and total time. Compare default and one-pass modes on a
   fixed local fixture plus a separately reported remote smoke run. Remote results
   must record their variability rather than present one timing as a guarantee.
5. **Profile the real post-skim path before weakening safety.** First evaluate
   combined percentile calls, a trusted/skip-validation option used only when
   `Cart.build` passes `write` the buffer it just received from `pack`, a
   speed-oriented gzip level, and cached callable arity. Keep `write` validating
   by default because it is a public entry point that accepts arbitrary metadata
   and buffers. Any change needs format checks and output-equivalence verification.

## Verification

- `python -m unittest tests.test_live_press_display tests.test_cart`: 28 tests
  passed on 2026-08-31. The displayed argparse errors are expected assertions for
  the removed `--live` option and mutually exclusive display-mode flags.
- `python gate.py`: 176 tests passed and the release gate completed on
  2026-08-31.
- `python ../rowver_verification/check_corpus_sync.py`: both shared contract
  corpora matched their independent copies on 2026-08-31.
- `git diff --check -- docs/live-press-performance-study.md`: passed.

## Bottom line

The earlier wait is consistent with exactly what the indexed design permits: one
synchronous remote metadata open can wait indefinitely while the TTY spinner
keeps moving. The many numerical safety checks are not responsible for that
phase. Three serial index passes took 39.0–53.9 seconds; their output was progress
totals and ETA, not cartridge data. ADR 0006 removes those opens from the default
critical path and keeps them available only through `--live-robust`.
