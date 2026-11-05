#!/bin/sh
# Fuzz one target of fuzz/fuzz_targets, detached, until its coverage stops growing:
#
#     fuzz/run.sh TARGET [FORKS]        (FORKS: fuzzing processes at once, 1 when not given)
#
# It builds the target (nightly, no sanitizer: the crates hold no unsafe code), then runs it at
# nice 19 over fuzz/corpus/TARGET, which grows from fuzz/seeds/TARGET (made by fuzz/seed.py). Its
# log is fuzz/logs/TARGET.log, whose last line is DONE (no finding), STOPPED (ended early to spare
# the machine; run it again and it goes on from its corpus) or FAILED (findings, kept in
# fuzz/artifacts/TARGET/, or a build that failed). A finding is a panic, a run past 10 s, or
# memory past 1 GB, on an input of 1 MB at most. It ends after STALE_MINUTES (30) without new
# coverage or MAX_HOURS (3) in all, and stops when the disk has under MIN_FREE_GB (10) free, the
# inputs' folder's disk under 64 MB, or macOS reports memory pressure. FUZZ_TMP: where the targets
# that read files write their inputs (a RAM disk spares the disk), each run in a folder of its own
# there (FUZZ_INPUTS), removed at its end. libFuzzer's own job folder stays in the temporary folder
# on the main disk, as a RAM disk is too small for it, and is removed then too. However the run
# ends, the fuzzer and its jobs are stopped before their inputs' folder goes. Before the rehashed
# target fuzzes, the harness's test checks it makes every seed a file full validation accepts.
set -u
cd "$(dirname "$0")" || exit 2
target=${1:-}
forks=${2:-1}
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

inputs="${FUZZ_TMP:-${TMPDIR:-/tmp}}"
inputs="${inputs%/}/rowver-fuzz-$target-$$"
pid=""
# Every process under $1, deepest first, from one listing of the processes: fork mode runs each job
# through a shell, so the fuzzer's children alone are not the jobs.
descendants() {
    ps -axo pid=,ppid= | awk -v root="$1" '
        { kids[$2] = kids[$2] " " $1 }
        END {
            queue[0] = root; head = 0; tail = 1; n = 0
            while (head < tail) {
                m = split(kids[queue[head++]], found, " ")
                for (i = 1; i <= m; i++) { queue[tail++] = found[i]; order[n++] = found[i] }
            }
            for (i = n - 1; i >= 0; i--) print order[i]
        }'
}
# Whether a process is there and not ended (a child of this script ended is a zombie until waited for).
alive() {
    kill -0 "$1" 2>/dev/null && ! ps -o stat= -p "$1" 2>/dev/null | grep -q Z
}
# Stop the fuzzer and every job under it: the fuzzer frozen first, so it starts no job while they
# are found; then each asked to end, and whatever is left after 30 s in all, killed.
stop_fuzzer() {
    [ -n "$pid" ] || return 0
    kill -STOP "$pid" 2>/dev/null
    jobs=$(descendants "$pid")
    kill "$pid" $jobs 2>/dev/null
    kill -CONT "$pid" 2>/dev/null
    waited=0
    while [ "$waited" -lt 30 ]; do
        left=""
        for process in "$pid" $jobs; do
            alive "$process" && left="$left $process"
        done
        [ -z "$left" ] && break
        sleep 1
        waited=$((waited + 1))
    done
    for process in "$pid" $jobs; do
        alive "$process" && kill -9 "$process" 2>/dev/null
    done
}
mkdir -p "$inputs" || { echo "FAILED: the inputs' folder $inputs cannot be made"; exit 1; }
trap 'stop_fuzzer; rm -rf "$inputs"; [ -z "$pid" ] || rm -rf "${TMPDIR:-/tmp}/libFuzzerTemp.FuzzWithFork$pid.dir"' EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
export FUZZ_INPUTS="$inputs"
stale=$(( ${STALE_MINUTES:-30} * 60 ))
most=$(( ${MAX_HOURS:-3} * 3600 ))
least_free=${MIN_FREE_GB:-10}
echo "fuzz/run.sh $target: $(date), $forks forks; it ends after ${STALE_MINUTES:-30} minutes without new coverage or ${MAX_HOURS:-3} hours"
# The harness builds from fuzz/Cargo.lock as it is: a run never rewrites it.
if ! cargo +nightly metadata --locked --format-version 1 > /dev/null; then
    echo "FAILED: fuzz/Cargo.lock is not what fuzz/Cargo.toml needs: run cargo update in fuzz/ and commit the lock"
    exit 1
fi
nice -n 19 cargo +nightly fuzz build -s none "$target" || { echo "FAILED: the build"; exit 1; }
if [ "$target" = rehashed ] && ! nice -n 19 cargo +nightly test --lib; then
    echo "FAILED: the harness's test fails: it no longer makes each seed a file full validation accepts, so its chunk and checksum rules are not rowver's (fuzz/src/rehash.rs)"
    exit 1
fi
host=$(rustc -vV | sed -n 's/^host: //p')
nice -n 19 "target/$host/release/$target" "corpus/$target" "seeds/$target" -fork="$forks" \
    -ignore_crashes=1 -ignore_timeouts=1 -ignore_ooms=1 -timeout=10 -rss_limit_mb=1024 \
    -malloc_limit_mb=1024 -max_len=1048576 -artifact_prefix="artifacts/$target/" -close_fd_mask=3 &
pid=$!
best=0
why=""
start=$(date +%s)
since=$start
while kill -0 "$pid" 2>/dev/null; do
    sleep 60
    cov=$(tail -n 50 "$log" | grep -o 'cov: [0-9]*' | tail -n 1 | cut -d ' ' -f 2)
    now=$(date +%s)
    if [ "${cov:-0}" -gt "$best" ]; then
        best=$cov
        since=$now
    fi
    free=$(df -Pk "corpus/$target" | awk 'NR == 2 { print int($4 / 1048576) }')
    room=$(df -Pk "$inputs" | awk 'NR == 2 { print int($4 / 1024) }')
    pressure=$(sysctl -n kern.memorystatus_vm_pressure_level 2>/dev/null || echo 1)
    if [ $(( now - since )) -ge "$stale" ]; then why=stale
    elif [ $(( now - start )) -ge "$most" ]; then why=time
    elif [ "$free" -lt "$least_free" ]; then why="STOPPED: the disk has $free GB free, under $least_free"
    elif [ "$room" -lt 64 ]; then why="STOPPED: the inputs' folder $inputs has $room MB free, under 64: give FUZZ_TMP a folder with more room"
    elif [ "$pressure" -gt 1 ]; then why="STOPPED: macOS reports memory pressure: close programs and run it again"
    fi
    if [ -n "$why" ]; then
        break
    fi
done
stop_fuzzer
wait "$pid"
status=$?
# Reaped: its pid may be another process's now, which the exit trap must not signal.
fuzzer=$pid
pid=""
# What the run left: its inputs' folder, and libFuzzer's fork-mode folder, named for the fuzzer.
rm -rf "$inputs"
tmp=${TMPDIR:-/tmp}
rm -rf "${tmp%/}/libFuzzerTemp.FuzzWithFork$fuzzer.dir"
found=$(ls "artifacts/$target" | wc -l | tr -d ' ')
echo "$(date): coverage $best, last grown $(( ($(date +%s) - since) / 60 )) minutes ago"
if [ "$found" != 0 ]; then
    echo "FAILED: $found findings in fuzz/artifacts/$target"
elif [ -z "$why" ]; then
    # It runs until stopped here: a fuzzer that ended itself (a full disk, a broken corpus) proves nothing.
    echo "FAILED: the fuzzer stopped by itself (exit $status) before its coverage went stale: see the lines above"
elif [ "$why" = stale ]; then
    echo DONE
elif [ "$why" = time ]; then
    echo "DONE: ${MAX_HOURS:-3} hours, coverage still growing"
else
    echo "$why"
fi
