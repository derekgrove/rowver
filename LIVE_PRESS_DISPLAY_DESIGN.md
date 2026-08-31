# Three press-display modes

## Outcome

`rowver run STUDY` defaults to fast, one-pass live progress. The dashboard
observes work already performed by `Cart.skim`; it does not open a source or
calculate an exact total before the real skim. Minimum time to the completed
`.rvr` is the governing priority.

Two explicit alternatives remain:

```console
rowver run --no-live STUDY
rowver run --live-robust STUDY
```

`--no-live` is the plain one-pass path with no dashboard work.
`--live-robust` preserves the former default indexed display. It opens every
source in a metadata pre-pass, then the Presser opens every source again during
the real skim. That extra I/O buys exact totals, percentage bars, and ETA.

This decision supersedes ADR 0005; see
[`docs/adr/0006-default-to-one-pass-live-progress.md`](docs/adr/0006-default-to-one-pass-live-progress.md).

## Command-line behavior

| Invocation | Source passes | Progress |
| --- | ---: | --- |
| `rowver run STUDY` | one | Live current sample/source, scanned and kept rows, elapsed time, pressing state |
| `rowver run --no-live STUDY` | one | Study summary and final `wrote` line only |
| `rowver run --live-robust STUDY` | two | Indexed live display with exact totals, percentage bars, ETA, and pressing state |

The two flags are mutually exclusive. TTY detection changes rendering only; it
never changes which execution mode runs.

All modes print the same pre-build study summary and final `wrote` line. They use
the same `Cart.build` implementation and therefore retain the same selection,
numerical validation, size limit, cartridge validation, compression, temporary
write, `fsync`, and atomic publication behavior.

## Default fast live mode

The dashboard starts before `Cart.build` so a TTY can immediately show that the
one-pass skim is starting. `Cart.skim` then notifies the progress observer
immediately before it opens each real source, after each processed chunk, and
after each sample completes. These notifications do not open, retain, transform,
or validate source data.

The truthful state available without a pre-pass is:

- current sample and sanitized source basename;
- cumulative rows scanned and kept;
- elapsed wall time;
- sample completion; and
- transition to cartridge packing, compression, and writing.

The fast display never shows an exact total, percentage complete, progress bar,
or ETA. A source's `num_entries` becomes known after the real open, but displaying
a full-study completion estimate would still require totals for sources not yet
opened and would encourage misleading extrapolation.

### TTY rendering

TTY output uses the existing boxed renderer and 0.5-second background redraw.
During a synchronous source open, the spinner and elapsed time can move while the
sample and source remain visible. During skimming, scanned and kept counters
advance from real chunk callbacks. The pressing frame covers packing, cartridge
validation, gzip compression, durable writing, and atomic publication; the final
`wrote` line confirms completion.

### Redirected rendering

Redirected output contains durable lines and no ANSI cursor control:

```text
skimming TTTo2L2Nu MC: 44187D37…root
skimmed TTTo2L2Nu MC: 12,345,678 scanned, 91,204 kept
pressing cartridge…
```

One source line is emitted immediately before each real source open. One sample
line is emitted after each sample completes, including a zero-entry sample.

## Plain mode

`--no-live` constructs no `Dashboard`, passes no progress observer to
`Cart.build`, and performs no indexing. It is the quietest one-pass path.

## Robust indexed live mode

`--live-robust` is the implementation that was the default under ADR 0005.
`Dashboard.index` flattens the fileset and serially opens every source with the
same source policy used by the real skim. It reads `num_entries`, closes the
source, and records exact per-sample and full-study totals. `Cart.skim` later
reopens the sources to perform useful work.

The pre-pass enables:

- file `N/M` indexing progress and discovered entry totals;
- exact per-sample and overall totals;
- percentage-complete bars;
- processing rate and ETA; and
- exact sample-completion boundaries in redirected logs.

TTY behavior, redirected indexing/completion lines, zero-entry handling, source
error translation and redaction, and rendering-failure boundaries remain those
of the former default implementation.

The CLI help calls out that `--live-robust` opens sources twice. This is a
performance tradeoff, not a correctness mode: its cartridge is equivalent to
the fast and plain modes.

## Progress observer contract

The existing callable protocol remains compatible:

```python
progress(sample, scanned_rows, kept_rows)
progress(None, 0, 0)  # skim finished; pressing begins
```

A callable observer may additionally provide:

```python
progress.source(sample, path, tree)  # immediately before the real open
progress.sample_done(sample)         # after the sample is materialized
```

Ordinary functions and lambdas need not provide these methods. Exceptions from
main-thread lifecycle or chunk callbacks abort the build before publication, as
existing callback failures do.

## Errors and rendering failures

All source opens continue through `_open_sample_tree`, including the explicit
XRootD handler for `root://` sources. Existing `CartError` translation and URL
credential/query/fragment redaction are unchanged.

A synchronous draw failure or progress-callback failure propagates and aborts the
press. A failure in the background ticker stops animation but does not abort a
correct press. This preserves the established boundary: a closed redirected
pipe is a main-thread failure, while animation is best-effort.

## Verification surface

Focused tests cover:

- one source open in default and `--no-live` modes versus two in robust mode;
- source-start notification from the real skim and no default pre-pass;
- no dashboard construction in plain mode;
- equivalent cartridge arrays across all modes;
- truthful fast TTY and redirected output without totals, percent, or ETA;
- exact robust TTY and redirected output;
- zero-entry samples, source errors, URL redaction, and rendering failures; and
- the shared explicit XRootD source policy.

Tests use local temporary ROOT files or mocked opens and require no network,
grid proxy, or CMS service.

## Non-goals

- estimating totals or ETA from partial source observations;
- parallel metadata opens;
- persistent entry-count caching;
- retaining pre-pass handles for the skim; or
- changing selection, weighting, chunking, cartridge, compression, or
  publication semantics.
