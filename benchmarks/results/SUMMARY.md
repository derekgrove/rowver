# Parallel press study: results and production handoff

Date: 2026-08-31

The source-task study showed substantial remote speedups while preserving output.
The owner approved an opt-in production feature while keeping sequential pressing
as the default. The 32/64/128-file remote measurements remain unfinished and are
not evidence for a universal safe remote concurrency cap.
The governing study plan is
[`docs/parallel-press-performance-study-plan.md`](../../docs/parallel-press-performance-study-plan.md).

## Interim recommendation

Continue with source-level tasks behind one coordinator-owned build seam. Keep a
shared work queue in declaration order, merge by `(sample ordinal, source
ordinal)`, and let only the coordinator pack, compress, and publish the
cartridge.

Use ordinary process-return serialization as the reference transport. Temporary
shards tied it in wall time and modestly reduced coordinator RSS, but added disk
I/O and cleanup without a demonstrated end-to-end benefit.

Do not add an entry-count indexing pass to ordinary parallel runs. Exact
largest-first ordering was slower even when counts were reused, and paying for
the pre-pass was much slower. If `--live-robust --parallel` is eventually
supported, its already-acquired counts may be reused for display, but the current
evidence does not justify using them to schedule work.

By owner decision, bare `--parallel` uses
`min(runnable_sources, max(1, (os.cpu_count() or 1) - 2))`. This is an explicit
opt-in, not the sequential default or an inferred safe cap for every remote
service. Explicit `--parallel N` remains available when users have stronger
site-specific evidence.

## Results

### Local bottleneck and transport

The deterministic local fixture scanned 3,875,000 rows and kept 1,022,727.

| Mode | Median wall | Main observation |
| --- | ---: | --- |
| Sequential | 1.994 s | Gzip took 1.606 s; source-active work took 0.220 s |
| Sample tasks, 3 workers | 2.263 s | Slower than sequential |
| Source tasks, 4 workers, pickle | 2.262 s | Slower than sequential |
| Source tasks, 4 workers, shards | 2.261 s | 22.1 MB temporary disk; no wall-time gain |

Local multiprocessing fails the first decision gate because source work is not
material. See
[`local-sequential-macos-arm64-20260831.json`](local-sequential-macos-arm64-20260831.json)
and
[`local-transports-macos-arm64-20260831.json`](local-transports-macos-arm64-20260831.json).

### Remote JetMET

All completed outputs opened through `rvr6.read` and contained arrays identical
to sequential output. Remote latency varied by tens of seconds, so medians and
ranges matter more than individual runs.

| Files | Runs | Sequential | Best measured parallel result | Speedup |
| ---: | ---: | ---: | ---: | ---: |
| 4 | 5 per mode | 38.64 s median (35.98–59.73) | 2 workers: 26.19 s median (15.03–42.34) | 1.48× |
| 8 | 3 per mode | 125.52 s median | 8 workers: 34.54 s median | 3.63× |
| 16 | 1 per mode | 172.25 s | 10 workers: 39.34 s | 4.38× |
| 32 | 1 parallel pilot | not run in this continuation | 8 workers: 146.26 s | not established |

The 4-file four-worker median showed no gain despite very fast individual runs;
this is direct evidence against selecting a default from one remote observation.
Detailed results are in
[`remote-jetmet-4-macos-arm64-20260831.json`](remote-jetmet-4-macos-arm64-20260831.json),
[`remote-jetmet-8-pilot-macos-arm64-20260831.json`](remote-jetmet-8-pilot-macos-arm64-20260831.json),
and
[`remote-jetmet-16-pilot-macos-arm64-20260831.json`](remote-jetmet-16-pilot-macos-arm64-20260831.json),
and the single
[`remote-jetmet-32-pilot-macos-arm64-20260831.json`](remote-jetmet-32-pilot-macos-arm64-20260831.json).
The 32-file pilot completed in 146.26 seconds with per-source wall times from
2.02 to 114.44 seconds and a validated 7,400-row cartridge; without repeats or
a sequential control it supplies no stable speedup or worker-cap conclusion.

### Entry-count scheduling experiment

`Dashboard.index` currently retains exact totals per sample, not per source. The
prototype separately indexed all eight sources and submitted whole-source tasks
largest-first.

| Four-worker mode | Wall time |
| --- | ---: |
| Declaration-order shared queue | 21.13 s |
| Largest-first, counts reused | 25.70 s |
| Largest-first, including 19.46 s indexing | 102.43 s |

Entry count did not predict remote duration: an 818k-entry source unexpectedly
took 67.6 seconds while larger sources completed much sooner. A single
200-million-entry source still cannot be balanced by sorting whole-source tasks;
that requires entry-range/chunk tasks, which should be considered only after the
source-level matrix and safety work are complete.

## Correctness and safety evidence

- A fixed build clock proves byte-identical sequential, sample-task, source-task,
  pickle, shard, spawn, and randomized-completion outputs on the exhaustive local
  fixture.
- Empty-source arrays are excluded from dtype inference during ordered merge, so
  they cannot promote integer columns to float64.
- Workers reload the Study, verify its source hash and resolved task ordinals,
  and detect recipe/fileset drift.
- Only the coordinator calls `rvr6.pack` and `rvr6.write`.
- Manual cancellation during active processing exited in about 0.04 seconds,
  published no destination, wrote no result JSON, and left no worker processes.
- The latest completed gate was 186 product tests plus
  `python ../rowver_verification/check_corpus_sync.py`; both passed.

The production seam additionally proves fixed-clock byte identity across worker
counts, reverse-completion ordered merge, per-chunk global size-limit certainty,
source-error redaction at first/middle/last task positions, malformed PREPARE,
KEEP, column, and weight rejection, coordinator callback cancellation, hard
worker-crash cleanup, coordinator-only progress, and compression/atomic replace
failure without partial publication. The existing dashboard tests retain the
established main-thread rendering and background-ticker failure contracts.

## Prototype map

- [`benchmarks/parallel_press.py`](../parallel_press.py): sequential phase
  instrumentation and JSON output.
- [`benchmarks/make_parallel_fixture.py`](../make_parallel_fixture.py):
  deterministic three-sample local fixture generator.
- [`benchmarks/parallel_press_prototype.py`](../parallel_press_prototype.py):
  throwaway sample/source schedulers, pickle/shard transports, exact-entry
  ordering experiment, ordered merge, and cancellation.
- [`tests/test_parallel_press_benchmark.py`](../../tests/test_parallel_press_benchmark.py):
  fixture, instrumentation, prototype transport, production byte-identity, and
  production failure/safety gates.
- [`rowver/press.py`](../../rowver/press.py): production source-task workers,
  coordinator size accounting, ordered merge, cancellation, and cleanup.
- [`docs/adr/0007-offer-source-parallel-pressing-as-opt-in.md`](../../docs/adr/0007-offer-source-parallel-pressing-as-opt-in.md):
  settled production boundary and trade-off.
- [`studies/jetmet_parallel_benchmark`](../../studies/jetmet_parallel_benchmark):
  fixed nested remote source sets.

The cancellation path deliberately uses `ProcessPoolExecutor._processes` because
this is throwaway study code. A production implementation must own worker
termination through a supported process abstraction.

## Resume here

1. Read this summary and the governing study plan. Inspect `git status` and
   preserve unrelated working-tree changes; the benchmark work is uncommitted.
   Completion criterion: the current diff and ownership boundaries are known.
2. Enter the RowVer environment, renew the VOMS proxy if needed, and verify the
   real four-file source path before a timed series. A local expired CERN Root CA
   2 CRL warning occurred during this session, but authenticated RowVer source
   reads succeeded. Completion criterion: one real source opens and the proxy
   validity window is recorded without logging credential contents.
3. Finish the randomized remote matrix: two more repetitions for 8 files, four
   more for 16 files, and five repetitions each for 32, 64, and 128 files. Run
   sequential plus source workers 1, 2, 4, 8, and 10, deduplicating counts after
   the number-of-tasks cap; 8 is this 10-logical-CPU machine's candidate bare
   default. Preserve every JSON result and summarize medians and ranges.
   Completion criterion: every distinct required size/worker cell has five
   successful runs or a recorded, unreplaced external failure.
4. Retain the production safety matrix as a release gate and extend it when new
   source backends expose distinct failure modes.
5. Do not infer a remote-safe concurrency cap from the incomplete matrix. Bare
   automatic mode remains explicitly user-selected and uses the owner-selected
   CPU-minus-two formula capped by runnable sources.

Run a single missing remote cell with:

```bash
ROWVER_JETMET_FILES=32 python -m benchmarks.parallel_press_prototype \
  studies/jetmet_parallel_benchmark --mode source --workers 8 \
  --transport pickle --start-method spawn --step-size '100 MB' \
  --output /tmp/jetmet-32-w8.rvr --json /tmp/jetmet-32-w8.json
```

Run its sequential control with:

```bash
ROWVER_JETMET_FILES=32 python -m benchmarks.parallel_press \
  studies/jetmet_parallel_benchmark --step-size '100 MB' \
  --output /tmp/jetmet-32-sequential.rvr \
  --json /tmp/jetmet-32-sequential.json
```

After code changes, run:

```bash
python -m unittest tests.test_parallel_press_benchmark -v
python gate.py
python ../rowver_verification/check_corpus_sync.py
git diff --check
```

## Owner decision: implement as opt-in

The measured speedup is sufficient to implement source-level parallel pressing.
The current sequential Presser remains the default and the obvious reference
implementation:

```console
rowver run STUDY                 # unchanged sequential default
rowver run STUDY --parallel N    # explicit opt-in with N worker processes
```

The production implementation direction is:

- expose the library seam as `Cart.build(..., workers=N)` or its smallest
  equivalent;
- make `--parallel 1` use sequential behavior and produce the sequential result;
- schedule one task per source from a shared dynamic queue;
- submit in declaration order and merge strictly by sample/source ordinals;
- use ordinary process-return serialization unless later memory measurements
  demonstrate that temporary shards are necessary;
- keep packing, compression, validation, and atomic publication in the
  coordinator only;
- preserve coordinator-owned progress and worker silence;
- add no entry-count pre-pass for scheduling; and
- make the `--live-robust --parallel N` behavior explicit before release rather
  than silently changing its exact-total contract.

Bare `--parallel` is the owner-selected automatic opt-in and uses
`min(runnable_sources, max(1, (os.cpu_count() or 1) - 2))`. Production completion
still does not turn the incomplete larger remote matrix into a general safety or
performance claim.
