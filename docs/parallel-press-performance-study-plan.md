# Parallel pressing performance study plan

Date: 2026-08-31

## Question

Can RowVer reduce time to a completed cartridge by processing independent
sources concurrently while preserving the exact sequential result, validation,
errors, progress truthfulness, size limit, compression, and atomic publication?

The study must measure before assuming that one Python process means one busy CPU
or that more workers improve remote throughput. Source opening and reading may be
limited by CPU, storage, network, authentication, or the remote server, and the
bottleneck may change between phases.

## Candidate command-line interface

```console
rowver run STUDY --parallel 8
rowver run STUDY --parallel
```

- `--parallel N` requests `N` worker processes.
- Bare `--parallel` requests all but two logical CPUs:
  `max(1, (os.cpu_count() or 1) - 2)`.
- `--parallel 1` is valid and must behave like the sequential implementation.
- Zero, negative, non-integer, and surplus arguments are rejected before sources
  are opened.
- The final worker count is capped by the number of runnable tasks. Documentation
  should call these *workers*, not physical cores.

This is a candidate interface, not a decision to ship. The performance and
failure results below govern whether the option is worthwhile and what its safe
default should be.

## Invariants

Parallel pressing may change scheduling only. It must not change:

- sample order from `Cart.fileset`;
- source order within each sample;
- entry and chunk order within each source;
- selected rows, columns, dtypes, or weights;
- numerical and cartridge validation;
- the 1 GiB cartridge-data limit or its certainty (never reject a cartridge that
  would fit and never publish one that exceeds the limit);
- source-error translation and URL-secret redaction;
- rendering-failure behavior;
- gzip level or cartridge format;
- temporary-file durability, `fsync`, and atomic replacement; or
- cleanup after worker failure or cancellation.

Only the coordinator may pack, compress, and publish the final `.rvr`. A worker
must never write the destination cartridge.

## First measurement: establish the bottleneck

Add temporary study instrumentation, not permanent product logging, around:

1. study load and validation;
2. each source open;
3. source read/decompression;
4. `PREPARE`, `KEEP`, columns, and weights;
5. per-source concatenation or result transfer;
6. ordered merge;
7. `rvr6.pack` and validation;
8. gzip compression;
9. durable write and atomic replacement; and
10. total wall time and peak resident memory.

Record wall time, process CPU time, aggregate worker CPU utilization, bytes read,
rows scanned/kept, worker count, failures/retries, and peak coordinator/worker
memory. Instrumentation must not print credentials or full secret-bearing URLs.

Run the current sequential implementation first. If source reading is not a
material part of end-to-end time, or the machine/network is already saturated,
stop: multiprocessing cannot improve the governing metric enough to justify its
complexity.

## Prototype two scheduling seams

The public interface should remain `Cart.build(..., workers=N)` or its eventual
equivalent. Scheduling, ordered merge, cancellation, and cleanup belong behind
that one seam.

### Prototype A: one task per sample

Each worker processes every source for one sample sequentially and returns that
sample's columns and weights. The coordinator stores results by the sample's
declaration ordinal and passes them to `rvr6.pack` in original order.

Advantages:

- smallest change to current `Cart.skim` semantics;
- simple deterministic merge; and
- only one returned result per sample.

Risks:

- poor load balance when samples differ greatly in file count or event count;
- at most one useful worker per sample; and
- a large sample result must be transferred or materialized as one object.

The JetMET benchmark described below deliberately uses one sample, so this seam
has no useful concurrency there. Its local-fixture result remains valuable as a
control for studies with multiple uneven samples.

### Prototype B: one task per source

Each task is identified by `(sample_ordinal, source_ordinal)` and processes that
source's chunks sequentially. The coordinator buffers completed results by those
ordinals and concatenates them only in original sample/source order, never in
completion order.

Advantages:

- hundreds of sources provide enough tasks for load balancing; and
- a slow source does not pin an entire sample to one worker.

Risks:

- more scheduling and result-transfer overhead;
- more simultaneous source connections;
- higher peak memory while early-ordinal work waits for later completions;
- global size-limit accounting and cancellation become less immediate; and
- remote services may throttle or fail under concurrency.

Source-level tasks are the leading candidate, but the study must retain sample
tasks as the simpler control. Chunk-level tasks are out of scope initially: they
multiply scheduling, remote access, and merge complexity before source-level
results establish a need.

## Worker construction and study recipes

Do not depend solely on `fork`: remote clients may already own unsafe inherited
state, while macOS and Windows use spawn-style process creation. Exercise both
Linux fork and spawn-compatible startup during the prototype.

Study callables loaded dynamically from `cart.py` may not be safely pickleable.
Prototype workers should instead receive the study path plus an explicit resolved
task description, reload and validate the study in the worker, and use the
coordinator's resolved sample/source path and tree. The prototype must detect
recipe drift rather than silently process a different study source.

Start all workers before any source is opened. Confirm that X.509 proxy and CA
configuration remain available in workers without copying credential contents
into task payloads or logs.

## Result transport experiment

Compare at least these internal transports on a representative local fixture:

1. ordinary process-return serialization of NumPy arrays; and
2. per-task temporary result files owned by a run-specific temporary directory.

Measure serialization time, temporary I/O, coordinator copies, cleanup, and peak
memory. Do not add shared memory, a custom binary protocol, or persistent caches
unless both simple transports are measured and inadequate.

Temporary shards are implementation details, not cartridges. They must be
created with private names, removed after success or failure, and never mistaken
for durable output.

## Deterministic ordered merge

Assign every task an ordinal before submitting it. A result carries its ordinal,
validated columns, optional weights, row counts, and width information needed for
the global size lower bound. Completion order affects only when a result becomes
available.

The coordinator reconstructs:

```text
sample declaration order
  -> source declaration order
     -> chunk/entry order within the worker result
```

It then calls the existing `rvr6.pack` once. No parallel reduction may sum,
sort, or otherwise reorder floating-point weights or rows.

## Byte-identical correctness proof

`rvr6.write` already fixes gzip `mtime=0`, but `Cart.build` records the current
UTC time in `made.built` and `made.skimmed`. Two ordinary invocations at different
times are therefore expected to differ even when their data are identical.

The equivalence test must inject one fixed build clock for all runs; it must not
delete provenance or weaken production metadata. With identical study source,
resolved fileset, step size, build time, and destination-independent inputs:

1. press the fixture sequentially;
2. press it with worker counts `1..(os.cpu_count() or 1)`;
3. repeat with randomized worker delays to force different completion orders;
4. compare SHA-256 and complete `.rvr` bytes, not only decoded arrays;
5. independently open every result through `rvr6.read`; and
6. on mismatch, compare the uncompressed frame, metadata, and each column to
   identify ordering, dtype, provenance, or compression drift.

The tiny exhaustive fixture should include:

- at least three samples and multiple sources per sample;
- weighted and unweighted samples;
- integer and floating-point columns;
- a callable column, callable weight, `PREPARE`, and `KEEP`;
- empty sources, zero-kept sources, and uneven source sizes; and
- enough chunks to prove within-source ordering.

CI may additionally run a bounded matrix for time, but the local study gate must
exercise every worker count supported by the test machine.

## Failure and safety matrix

For sequential, sample-parallel, and source-parallel prototypes, inject:

- failure while opening the first, middle, and last source;
- a lazy error after a source context is entered;
- malformed `PREPARE`, `KEEP`, column, and weight outputs;
- NaN/infinity and wrong-length outputs;
- certain crossing of the cartridge size limit;
- worker crash and forced cancellation;
- main-thread progress rendering failure;
- background ticker failure; and
- write/compression failure after all workers finish.

Assert the established user-facing exception and redaction behavior, prompt
cancellation of outstanding work, removal of temporary shards, and absence of a
new or partially replaced destination cartridge.

## Progress behavior

Default fast live progress remains truthful without a display-only pre-pass. It
may report active workers/sources, completed sources/samples, scanned and kept
rows received by the coordinator, elapsed time, and pressing/writing state. It
must not infer exact totals, percentage, or ETA from incomplete tasks.

`--no-live --parallel [N]` performs no dashboard work. The study must decide
explicitly whether `--live-robust --parallel [N]` keeps its indexing pre-pass
sequential, indexes concurrently, or is initially rejected as an unsupported
combination; do not silently change its exact-total behavior.

Progress callbacks run in the coordinator. Workers send structured events or
results; they do not write terminal output.

## Benchmark matrix

Use two classes of input:

1. fixed local ROOT fixtures for repeatable CPU, ordering, memory, and transport
   measurements; and
2. the checked-in `studies/jetmet_parallel_benchmark` Study for remote behavior.

The remote Study contains 128 distinct CMS 2018 MET NanoAOD files in a fixed
order and selects a nested prefix with `ROWVER_JETMET_FILES`. Run every size:

```text
4, 8, 16, 32, 64, and 128 files
```

The default is 4 files. The benchmark rejects every other value, uses one
unweighted sample, scans every selected file, and applies a tight keep cut so
the final cartridge remains small. Its manifest was opened through the real
RowVer source-opening path with the development machine's VOMS proxy before the
study was recorded. Availability is still external state, so do not replace a
failed file during a timed series.

For each prototype, run worker counts:

```text
1, 2, 4, 8, bare --parallel default, and the machine's logical-CPU count
```

Deduplicate equal worker counts. Cross every worker count with every JetMET file
count. Run at least five repeats locally. Randomize test order and report medians
plus ranges. For remote runs, record source set, machine, network/site, proxy
state, time window, and per-source variability; do not present one remote result
as a stable benchmark. Use `--no-live` so dashboard cost is not timed.

Compare against both the current sequential default and `--no-live`. Report:

- total time to atomically published `.rvr`;
- skim-only time and speedup;
- CPU utilization and efficiency per added worker;
- peak memory and temporary-disk use;
- source failures/throttling; and
- output byte identity.

## Decision gates

Recommend implementation only if one prototype:

1. is byte-identical to sequential output under the fixed-clock proof;
2. passes the complete failure/safety matrix;
3. materially reduces median end-to-end time on representative work;
4. does not cause unacceptable remote failure or throttling;
5. keeps peak memory and temporary storage within documented bounds; and
6. has a simple enough seam that sequential pressing remains the obvious,
   maintainable reference implementation.

Choose the bare `--parallel` default only from measured scaling. “All but two
logical CPUs” is the initial candidate, not an entitlement to open that many
remote sources concurrently. If remote scaling peaks earlier, cap or require an
explicit worker count and document why.

## Deliverables

- a benchmark harness with frozen local inputs and machine-readable results;
- throwaway sample-task and source-task prototypes;
- byte-identity and failure-matrix results;
- local and remote performance tables with memory measurements;
- a recommendation: no feature, sample tasks, or source tasks;
- the recommended `--parallel` interface and safe default, if any; and
- an ADR only after the study resolves the scheduling seam and tradeoff.
