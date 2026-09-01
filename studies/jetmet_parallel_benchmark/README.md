# JetMET parallel benchmark

This Study supplies nested remote-I/O workloads of 4, 8, 16, 32, 64, and 128
distinct CMS 2018 MET NanoAOD files. Every larger case is the exact prefix of
the next case, so file count is the only input change.

A valid CMS VOMS proxy and CA configuration are required. Choose a size with:

```console
ROWVER_JETMET_FILES=4 rowver run --no-live studies/jetmet_parallel_benchmark --out jetmet-4.rvr
ROWVER_JETMET_FILES=128 rowver run --no-live studies/jetmet_parallel_benchmark --out jetmet-128.rvr
```

Allowed values are `4`, `8`, `16`, `32`, `64`, and `128`; the default is `4`.
Use `--no-live` for timing so terminal rendering is not part of the measurement.
The tight keep cut makes the output small while every selected source is still
opened and scanned.

Run the full single-worker input-size series with:

```bash
for files in 4 8 16 32 64 128; do
  ROWVER_JETMET_FILES=$files rowver run --no-live \
    studies/jetmet_parallel_benchmark --out "jetmet-$files.rvr"
done
```

The 128-file manifest was checked on 2026-08-31 from the development machine
through RowVer's real source-opening helper and the CMS global XRootD redirector.
Seventeen DBS-listed files that timed out under concurrency were excluded; all
seventeen replacements opened successfully and exposed their `Events` trees.
Remote availability can change, so record failures and proxy/network context
with each benchmark run rather than silently substituting files.
