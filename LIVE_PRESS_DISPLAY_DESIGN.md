# Live press display by default

## Outcome

`rowver run STUDY` displays the live press display by default. The display starts
before source files are indexed, remains active while events are skimmed,
and reports the final pack/compress/write phase. Users who want plain output can
run:

```console
rowver run --no-live STUDY
```

The current `--live` option is replaced by `--no-live`.

## Why

The live display currently starts only after `Dashboard.index` returns. Indexing
opens every source file serially and reads the selected tree's entry count so the
skim can show complete progress bars and an ETA. For remote XRootD files, opening
a file may spend a long time resolving a source, authenticating, or retrying.
During that work the user sees only `indexing files…`, which looks like a hang.

Indexing is also a separate pass: the presser later reopens the same files to read
events. This design makes that existing cost visible; it does not try to remove or
parallelize it.

## Command-line behavior

`rowver run` uses these rules:

| Invocation | Output |
| --- | --- |
| `rowver run STUDY` | Live press display when stdout is a TTY |
| `rowver run --no-live STUDY` | Existing plain summary and final `wrote` line |
| `rowver run STUDY | command` | Concise progress lines for every phase, without cursor control |

`--no-live` disables the dashboard and its indexing pre-pass. Without live
metrics, totals and ETA are unnecessary, so the command should proceed directly
to `Cart.build` as the current non-live path does.

The public `--live` flag is removed rather than retained as a second spelling.
This is a small CLI compatibility break, but calls that previously requested
`--live` can simply omit it.

Live metrics are enabled by default regardless of whether stdout is a TTY. TTY
detection selects the rendering style only; it does not select the execution
path. Consequently, redirected default runs still perform the indexing pre-pass
and open each source twice. `--no-live` is the explicit single-pass and
performance-sensitive mode.

### Pre-build study summary

Before indexing begins, retain the study summary and expand it into a quick
verification surface. Unweighted samples are labeled explicitly, and the names
of every column that will be pressed into the cartridge are listed in declaration
order:

```text
jetmet_2018_advanced: 6 samples, 8 columns
  MET 2018A DATA: 1 file(s), unweighted
  MET 2018D DATA: 1 file(s), unweighted
  TTJets DiLept MC: 1 file(s), weighted
  TTTo2L2Nu MC: 2 file(s), weighted
  DYJets M-50 MC: 1 file(s), weighted
  WJets HT 400-600 MC: 3 file(s), weighted
  columns pressed into cartridge:
    MET
    MET_phi
    PT_jet_lead
    AbsEta_jet_lead
    Njet
    Nmu
    Nele
    Npv
```

This summary is printed for both live and `--no-live` runs. It reports only
column names, not expressions or callable representations, so users can verify
the intended cartridge contents without exposing noisy implementation details.

## Display phases

The existing `Dashboard` owns all three phases. No second spinner or rendering
library is introduced.

### 1. Indexing

Before each file open, the dashboard records:

- phase: `indexing`;
- current sample;
- shortened source filename;
- current file number and total file count;
- completed file count and entries discovered so far.

Flatten the ordered source list when constructing the dashboard and initialize
the first source as current. The synchronous initial frame therefore identifies
file `1/total` before the first open rather than briefly displaying an empty
`0/total` state.

The existing ticker redraws every 0.5 seconds, so the spinner and elapsed time
continue to move while a remote open is blocked. A representative frame is:

```text
Indexing files  3/9  /
TTTo2L2Nu MC · 44187D37…root
2 files indexed · 8.42M entries · elapsed 00:18
```

After an open succeeds, the completed count and discovered entry count advance.
If it fails, the existing `CartError` remains the user-facing error and names the
sample and source. Entering `Dashboard.live()` draws the first indexing frame
synchronously before the first file open, starts the indexing clock, and then
starts the 0.5-second ticker.

The heading is always `Indexing files`, including for local, remote, and mixed
filesets; the current source name provides the file-specific context.

TTY frames report cumulative entries discovered across completed files. Each
redirected completion line reports both the entries in the file that just
completed and the cumulative discovered total.

### 2. Skimming

After all files are indexed, the dashboard switches to `skimming` and uses the
current per-sample bars, scanned/kept counts, rate, elapsed time, and ETA.
The skim timer is reset at this transition so indexing latency does not distort
the processing rate or ETA.

Sample rows have no `DATA`, `MC`, or `SIG` decoration and the dashboard does not
interpret `metadata["kind"]` or any other sample metadata. Weighting alone sets
the active bar colour: yellow for weighted samples and cyan for unweighted
samples. A completed bar remains green.

### 3. Pressing

The existing `(None, 0, 0)` progress event switches the dashboard to `pressing`.
The current `pressing the cartridge (pack + gzip)…` animation remains unchanged.
After the ticker stops, TTY output retains one non-animated final sample frame
before the existing `wrote` line. Redirected output emits no final box.

## Implementation

### `rowver/cli.py`

1. Replace the `--live` argument with `--no-live`, using `dest="live"`,
   `action="store_false"`, and a default of `True`.
2. Construct a `Dashboard` when live output is enabled.
3. Enter `board.live()` before calling `board.index(cart)` and keep the same
   context active through `cart.build(...)`.
4. Reset the dashboard's skim clock after indexing.
5. Preserve the current non-live path when `--no-live` is present.
6. Remove the static `print("indexing files…")`; the TTY frame or redirected
   first-source line replaces it, while `--no-live` prints neither.

Conceptually:

```python
board = Dashboard(cart) if args.live else None
with board.live() if board else contextlib.nullcontext():
    if board:
        board.index(cart)
        board.begin_skim()
    result = cart.build(..., progress=board.chunk if board else None)
```

`begin_skim()` is preferable to mutating `board.t0` from the CLI because the
phase transition and timer reset belong together.

### `rowver/dashboard.py`

Use one phase value rather than retaining the existing `pressing` boolean. Add
only the state needed by the indexing frame:

- `phase`, initially `indexing`;
- `index_file`, `index_files`, and `indexed_files`;
- `indexed_entries`;
- `current_file`.

Compute `index_files` once from the fileset. In `index`, update the current file
state under the existing lock before each `uproot.open`, then update completed
state under the lock after it succeeds. The ticker performs the redraw; `index`
does not need its own thread.

Capture the output stream and its TTY status per `Dashboard` instance rather than
at module import. Route TTY decisions, colour, cursor control, and writes through
that instance so rendering follows the stream actually owned by the display.

Split `_draw` at its existing phase boundary:

- `indexing` renders the compact indexing frame;
- `skimming` renders the existing sample dashboard;
- `pressing` uses the existing pressing footer.

All phase and timer transitions happen under the existing lock so the ticker
cannot observe a contradictory snapshot. `begin_skim()` owns the
indexing-to-skimming transition and timer reset; `(None, 0, 0)` owns the
skimming-to-pressing transition. Entering each phase triggers a synchronous draw
so even a phase shorter than one ticker interval remains observable.

Do not render unknown totals through the sample bars during indexing: `bar`
currently treats a zero total as full, which would misleadingly show every
sample as complete.

For `root://` paths, indexing should pass the same
`uproot.source.xrootd.XRootDSource` handler used by `Cart.skim`. The pre-pass and
the real read should not choose different remote backends. Keep the pre-pass
orchestration in `Dashboard.index`, but extract the source-opening policy into a
private helper shared with `Cart.skim` so the two paths cannot drift.

### `rowver/press.py`

Extend `Cart.summary()` rather than creating a second summary formatter:

1. Change the unweighted mode text from `data` to `unweighted`.
2. Append `columns pressed into cartridge:` after the sample rows.
3. Append one indented row for each key in `self.columns`, preserving declaration
   order.

The existing first line already reports the column count. The new list makes the
count inspectable without changing the study or cartridge format.

Weighting is independent of sample kind: collision data is unweighted, but an
unweighted sample is not necessarily collision data. The summary therefore does
not infer `data` from a missing weight expression. Update existing Presser
documentation and validation messages that currently describe `None` as being
only for observed data.

### Source names and errors

Progress output shows only a sanitized basename, with query and fragment removed
and the middle elided to at most 28 display cells while preserving as much of the
extension as possible. Use one `…` and divide the remaining cells between the
beginning and end. Reuse the dashboard's control-character and display-width
defenses.
Apply the same control- and format-character sanitization to sample names in all
TTY and redirected progress output. `CartError` retains the original sample name,
quoted so its boundary remains clear.
Errors retain the complete filesystem path or URL path needed for diagnosis, but
redact query and fragment values so credentials and tokens are not exposed.

An indexing failure does not introduce a fourth phase. The live context stops and
joins the ticker, leaves the terminal on a fresh line, and then allows the
existing `CartError` to be printed beneath the last coherent frame.

Preserve the existing rendering-failure boundary. A failure during a synchronous
main-thread draw or progress-callback draw aborts normally, including a closed
output pipe. A draw failure in the background ticker stops animation but does not
abort the Presser or invalidate a correct cartridge; this change adds no
cross-thread exception propagation.

### Non-TTY output

Cursor animation and boxed frames remain TTY-only. When stdout is redirected,
use concise, durable lines throughout: one line after each file completes, one
line after each sample finishes skimming, and one line when pressing begins.
Immediate liveness comes from one pre-open line for the first source only:

```text
indexing 1/9 MET 2018A DATA: 44187D37…root
```

Subsequent sources do not add a separate attempt line. An indexing completion is:

```text
indexed 3/9 TTTo2L2Nu MC: 44187D37…root (1.28M entries; 8.42M total)
```

This gives logs durable progress without printing twice per second or emitting
the current boxed sample-boundary snapshots. `--no-live` suppresses these lines
along with all other live metrics.

Completed samples and the pressing transition use stable, exact messages:

```text
skimmed TTTo2L2Nu MC: 12,345,678 scanned, 91,204 kept
pressing cartridge…
```

Sparse redirected updates do not report rate or ETA.

Keep the existing progress callback protocol unchanged. A sample normally
completes when its scanned count reaches its indexed total. Because a zero-entry
sample emits no chunk callback, flush its single completion line when the next
sample begins or, for trailing zero-entry samples, when pressing begins. Every
sample produces exactly one completion line.

## Tests

Add focused tests around the observable seams:

1. `Cart.summary()` labels samples without a weight expression as `unweighted`
   without inferring their kind, and lists all column names
   in declaration order for both live and non-live callers.
2. CLI parsing defaults `live` to true and `--no-live` sets it false.
3. The live context begins before `Dashboard.index` and remains active through
   `Cart.build`.
4. A delayed mocked `uproot.open` causes indexing-phase redraws in TTY mode.
5. Indexing reports file `N/total`, the sample, and discovered entries.
6. A remote path uses the same explicit XRootD handler as the skim.
7. Non-TTY mode emits concise per-file, per-sample, and pressing-transition lines,
   with no ticker or boxed frames.
8. `--no-live` skips `Dashboard` construction and the metadata pre-pass.
9. An indexing failure still raises a `CartError` containing the sample and the
   full filesystem path or redacted URL path.
10. The initial indexing frame is drawn before the first source is opened.
11. Phase transitions are lock-protected and use a single phase value.
12. Displayed source names are sanitized and elided; error URLs redact query and
    fragment values.
13. Dashboard instances derive TTY behavior from their own output stream rather
    than module import state.
14. Redirected sample and pressing lines use the stable concise formats above.
15. Sample rows contain no kind tags or metadata-dependent presentation; active
    bars distinguish only weighted from unweighted samples.
16. The first TTY frame identifies source `1/total` before its open begins.
17. Every sample produces exactly one redirected completion line, including
    zero-entry samples, without changing the progress callback protocol.
18. Displayed source basenames occupy at most 28 display cells.
19. Redirected mode emits one pre-open line for the first source, before it can
    block, without doubling attempt and completion lines for later sources.
20. Indexing, skimming, and pressing each receive a synchronous initial draw in
    TTY mode.
21. Sample names are sanitized in every progress format while errors retain the
    original quoted name.
22. The legacy static `indexing files…` line is absent from every output mode.
23. Main-thread rendering failures propagate, while background ticker failures do
    not abort pressing.

Use local temporary ROOT files or mocked opens; tests must not require CMS,
XRootD, a grid proxy, or network access.

## Acceptance criteria

- Running the advanced JetMET study shows visible motion within 0.5 seconds of
  entering the indexing phase.
- Before indexing, the summary labels unweighted samples explicitly and lists the
  eight pressed column names in study declaration order.
- The display identifies which of the nine files is currently being opened.
- The same display transitions cleanly from indexing to skimming to pressing.
- Skim rate and ETA exclude indexing time.
- `rowver run --no-live` performs no totals pre-scan and retains concise output.
- A redirected live run produces readable per-file progress without ANSI cursor
  control, including a first-source line before any blocking open, followed by
  concise per-sample and pressing progress.
- Default execution semantics do not depend on TTY detection; only `--no-live`
  disables the metadata pre-pass.
- A successful TTY run leaves one completed sample frame before the final `wrote`
  line; redirected runs never emit boxed frames.
- When remote credentials and a reachable XRootD source are available, the
  implementation handoff includes a manual smoke check. Otherwise it explicitly
  reports remote behavior as unverified without weakening the local test suite.
- No new runtime dependency is added.

## Non-goals

- Parallel metadata opens.
- Persistent entry-count caching.
- Reusing open remote file handles between indexing and skimming.
- Changing selection, weighting, chunking, or cartridge semantics.

Those optimizations should be considered only if measurement shows that making
the pre-scan visible is insufficient.
