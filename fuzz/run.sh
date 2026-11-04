#!/bin/sh
# Fuzz one target of fuzz/fuzz_targets, detached, until 24 CPU-hours bring no new coverage:
#
#     fuzz/run.sh TARGET [FORKS]        (FORKS: fuzzing processes at once, 8 when not given)
#
# It builds the target (nightly, no sanitizer: the crates hold no unsafe code), then runs it at
# nice 19 over fuzz/corpus/TARGET, which grows from fuzz/seeds/TARGET (made by fuzz/seed.py). Its
# log is fuzz/logs/TARGET.log, whose last line is DONE (no finding) or FAILED (findings, kept in
# fuzz/artifacts/TARGET/, or a build that failed). A finding is a panic, a run past 10 s, or
# memory past 1 GB, on an input of 1 MB at most. STALE_HOURS (24) is the CPU-hours (FORKS times
# the time) without new coverage that end it; FUZZ_TMP, a folder for the inputs of the targets
# that read files (a RAM disk spares the disk).
set -u
cd "$(dirname "$0")" || exit 2
target=${1:-}
forks=${2:-8}
if [ ! -f "fuzz_targets/$target.rs" ]; then
    echo "fuzz/run.sh: give a target of fuzz/fuzz_targets: $(ls fuzz_targets | sed 's/\.rs$//' | tr '\n' ' ')"
    exit 2
fi
if [ -z "$(ls "seeds/$target" 2>/dev/null)" ]; then
    echo "fuzz/run.sh: $target has no seeds: make them with mamba run -n rowver python fuzz/seed.py"
    exit 2
fi
mkdir -p "corpus/$target" "artifacts/$target" logs
log=logs/$target.log

if [ "${3:-}" != "--here" ]; then
    nohup "./$(basename "$0")" "$target" "$forks" --here > "$log" 2>&1 &
    echo "fuzzing $target with $forks forks, detached: its log is fuzz/$log"
    exit 0
fi

tmp=${FUZZ_TMP:-${TMPDIR:-/tmp}}
export TMPDIR="$tmp"
stale=$(( ${STALE_HOURS:-24} * 3600 ))
echo "fuzz/run.sh $target: $(date), $forks forks; it ends after ${STALE_HOURS:-24} CPU-hours without new coverage"
# The harness builds from fuzz/Cargo.lock as it is: a run never rewrites it.
if ! cargo +nightly metadata --locked --format-version 1 > /dev/null; then
    echo "FAILED: fuzz/Cargo.lock is not what fuzz/Cargo.toml needs: run cargo update in fuzz/ and commit the lock"
    exit 1
fi
nice -n 19 cargo +nightly fuzz build -s none "$target" || { echo "FAILED: the build"; exit 1; }
host=$(rustc -vV | sed -n 's/^host: //p')
nice -n 19 "target/$host/release/$target" "corpus/$target" "seeds/$target" -fork="$forks" \
    -ignore_crashes=1 -ignore_timeouts=1 -ignore_ooms=1 -timeout=10 -rss_limit_mb=1024 \
    -malloc_limit_mb=1024 -max_len=1048576 -artifact_prefix="artifacts/$target/" -close_fd_mask=3 &
pid=$!
best=0
since=$(date +%s)
while kill -0 "$pid" 2>/dev/null; do
    sleep 60
    cov=$(tail -n 50 "$log" | grep -o 'cov: [0-9]*' | tail -n 1 | cut -d ' ' -f 2)
    now=$(date +%s)
    if [ "${cov:-0}" -gt "$best" ]; then
        best=$cov
        since=$now
    fi
    if [ $(( (now - since) * forks )) -ge "$stale" ]; then
        pkill -P "$pid"
        kill "$pid"
        break
    fi
done
wait "$pid"
rm -f "$tmp"/rowver-fuzz-*.rvr
found=$(ls "artifacts/$target" | wc -l | tr -d ' ')
echo "$(date): coverage $best, last grown $(( ($(date +%s) - since) / 60 )) minutes ago"
if [ "$found" = 0 ]; then echo DONE; else echo "FAILED: $found findings in fuzz/artifacts/$target"; fi
