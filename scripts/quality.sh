#!/usr/bin/env bash
# Quality gates for the Rust workspace (core/: metla-core, metla-cli,
# metla-ffi):
#   cargo fmt --check, cargo clippy -D warnings, cargo test,
#   the slow tests that wait for real FSEvents deliveries (the ignored
#   group, once), the fast suite five times in a row (it must not fail on
#   its own: the mutator counts every failure as a kill),
#   crapper (every function CRAP < 10), dryer (no pair scoring >= 0.8),
#   mutator (no surviving mutant, no uncovered site).
# Exits non-zero when any gate fails, and when a tool analysed nothing.
#
# Environment:
#   UB_TOOLS                 tool checkout root (default ~/.local/share/ub-tools)
#   CRAPPER, DRYER, MUTATOR  override a single tool's executable
#   MUTATE_ALL=1             rerun every mutant, not only changed functions
#   QUALITY_STEPS=tests      stop after the tests and the stability runs
#                            (fmt, clippy, tests, slow tests, stability):
#                            a quick check, not the gate
#   STABLE_RUNS              how many times in a row the fast suite must pass
#                            before the mutator runs (default 5)
#   MUTATOR_WORKERS          parallel mutants per file (default 2: every
#                            worker builds its own copy of the crates)
#   MUTATOR_TIMEOUT_SECONDS  the shortest mutant timeout (default 90)
#   DISK_FLOOR_MB            the least free space (default 300) the volume of
#                            TMPDIR must have before the mutator, before each
#                            file and after it; below it the run stops with
#                            exit 3: a full disk is not a result
#   MUTATOR_TIMEOUT_FACTOR   mutant timeout / baseline time; by default
#                            max(3, ceil(MUTATOR_TIMEOUT_SECONDS / baseline)),
#                            the baseline being one warm run of the crate's
#                            tests, timed here
#
# Mutant timeouts. A mutant's timeout is the factor times the mutator's own
# baseline, so it comes to about max(90 s, 3 × baseline). The floor covers
# the first mutant of every worker, which builds the crates in the worker's
# own target directory: two workers building at once from empty target
# directories took 9.2 s each for metla-core's tests and 6.5 s for
# metla-cli's (Apple M3 Pro, load average about 7), so 90 s is well above
# 1.5 times a cold build plus a test run. Too short a timeout would count a
# slow, passing mutant as killed and hide a survivor. The mutator reports a
# timed-out mutant as killed; scripts/mutant-test.sh leaves a trace of every
# test run, and a run without an end is a mutant stopped at its timeout,
# counted per file below.
#
# The tools do not analyse Swift; the Swift app has its own gates
# (scripts/quality-swift.sh, docs/QUALITY.md).

set -uo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo" || exit 2

ub_tools="${UB_TOOLS:-$HOME/.local/share/ub-tools}"
crapper="${CRAPPER:-$ub_tools/crapper/crapper}"
dryer="${DRYER:-$ub_tools/dryer/dryer}"
mutator="${MUTATOR:-$ub_tools/mutator/mutator}"
workers="${MUTATOR_WORKERS:-2}"
timeout_floor="${MUTATOR_TIMEOUT_SECONDS:-90}"

source "$repo/scripts/disk-guard.sh"
if problem="$(tmpdir_problem "${TMPDIR:-/tmp}")"; then
    echo "ERROR: $problem (CONTRIBUTING.md, Running the gates)"
    exit 2
fi
disk_floor="${DISK_FLOOR_MB:-300}"
scratch="${TMPDIR:-/tmp}"
disk_full=()

# A full disk makes test runs fail, and the mutator counts every failing
# run as a kill: say so loudly, and no verdict of this run counts.
full() {
    disk_full+=("$1")
    echo "DISK FULL: $1 - no result of this run is valid"
}

crap_limit=10
stable_runs="${STABLE_RUNS:-5}"
dry_limit=0.8
log_dir="$repo/target/quality"
mkdir -p "$log_dir"

failed=()
fail() {
    failed+=("$1")
    echo "FAIL: $1"
}

section() {
    echo
    echo "== $1"
}

for tool in "$crapper" "$dryer" "$mutator"; do
    [[ -x "$tool" ]] || { echo "missing tool: $tool"; exit 2; }
done

section "disk guard (its own tests)"
"$repo/scripts/disk-guard-test.sh" || fail "disk guard tests"

section "cargo fmt --check"
(cd core && cargo fmt --check) || fail "cargo fmt"

section "cargo clippy --all-targets -- -D warnings"
(cd core && cargo clippy --all-targets -- -D warnings) || fail "cargo clippy"

section "cargo test"
(cd core && cargo test 2>&1) | tee "$log_dir/test.txt" | grep -E "^test result|FAILED|panicked"
[[ ${PIPESTATUS[0]} -eq 0 ]] || fail "cargo test"

section "slow tests: real FSEvents deliveries (ignored group, run once)"
(cd core && cargo test -- --ignored --nocapture 2>&1) | tee "$log_dir/slow.txt" \
    | grep -E "^test result|FAILED|panicked|^fsevents:"
[[ ${PIPESTATUS[0]} -eq 0 ]] || fail "slow tests"

# The mutator counts a failing test run as a killed mutant, so a test that
# fails on its own now and then would "kill" mutants it has nothing to do
# with. The suite it runs must pass every time.
section "stability: the fast suite $stable_runs times in a row"
for run in $(seq 1 "$stable_runs"); do
    (cd core && cargo test 2>&1) >"$log_dir/stable-$run.txt"
    code=$?
    passed=$(grep -E "^test result" "$log_dir/stable-$run.txt" | awk '{n += $4} END {print n + 0}')
    echo "run $run: exit $code, $passed tests passed"
    if [[ $code -ne 0 ]]; then
        names=$(grep -E "^test .* FAILED$" "$log_dir/stable-$run.txt" | sed -E 's/^test (.*) \.\.\. FAILED$/\1/' | paste -sd, -)
        fail "stability run $run: ${names:-cargo test exited $code}"
    fi
done

# A test that failed of a full disk is not a failing test.
while IFS= read -r log; do
    full "a full disk in $(basename "$log")"
done < <(full_disk_logs "$log_dir"/test.txt "$log_dir"/slow.txt "$log_dir"/stable-*.txt)

if [[ "${QUALITY_STEPS:-all}" == tests ]]; then
    section "result (QUALITY_STEPS=tests: crapper, dryer and mutator not run)"
    if [[ ${#disk_full[@]} -gt 0 ]]; then
        printf 'disk full: %s\n' "${disk_full[@]}"
        exit 3
    fi
    if [[ ${#failed[@]} -gt 0 ]]; then
        printf 'failed: %s\n' "${failed[@]}"
        exit 1
    fi
    echo "tests green"
    exit 0
fi

section "crapper (CRAP < $crap_limit)"
"$crapper" core >"$log_dir/crap.txt" 2>&1 || fail "crapper run"
python3 -I - "$repo/.metrics/crap.edn" "$crap_limit" <<'PY' || fail "crapper gate"
import re, sys
text = open(sys.argv[1]).read()
limit = float(sys.argv[2])
entries = re.findall(r'\{:name "([^"]*)", :namespace "([^"]*)", '
                     r':complexity (\d+), :coverage (\S+), :crap (\S+)\}', text)
crates = {}
bad = []
worst = (0.0, "")
for name, ns, cc, cov, crap in entries:
    crates[ns.split("::")[0]] = crates.get(ns.split("::")[0], 0) + 1
    if crap == "nil":
        bad.append(f"{ns}::{name} has no coverage")
        continue
    score = float(crap)
    worst = max(worst, (score, f"{ns}::{name}"))
    if score >= limit:
        bad.append(f"{ns}::{name} CRAP {score}")
print(f"functions analysed: {len(entries)} "
      + ", ".join(f"{c}: {n}" for c, n in sorted(crates.items())))
print(f"worst CRAP: {worst[0]} ({worst[1]})")
for line in bad:
    print("  " + line)
sys.exit(1 if bad or not entries else 0)
PY

section "dryer (score < $dry_limit)"
"$dryer" --threshold 0 --edn core >"$log_dir/dry-all.edn" 2>/dev/null || fail "dryer census"
"$dryer" --threshold "$dry_limit" core >"$log_dir/dry.txt" 2>&1 || fail "dryer run"
python3 -I - "$log_dir/dry-all.edn" "$repo/.metrics/dry.edn" <<'PY' || fail "dryer gate"
import re, sys
every = open(sys.argv[1]).read()
forms = set(re.findall(r'\{:file "([^"]*)", :start-line (\d+)', every))
scores = [float(s) for s in re.findall(r':score (\S+)', every)]
gate = open(sys.argv[2]).read()
pairs = re.findall(r':score (\S+)', gate)
print(f"functions compared: {len(forms)}")
print(f"highest similarity: {max(scores) if scores else 0:.2f}")
print(f"pairs at or above the limit: {len(pairs)}")
sys.exit(1 if pairs or not forms else 0)
PY
[[ -s "$log_dir/dry.txt" ]] && grep -A2 "^DUPLICATE" "$log_dir/dry.txt"

section "mutator (no survivors, no uncovered sites)"
space_ok "$scratch" "$disk_floor" \
    || full "$(free_mb "$scratch") MB free on the volume of $scratch before the mutator (floor $disk_floor MB)"
mutate_all=()
[[ "${MUTATE_ALL:-0}" == "1" ]] && mutate_all=(--mutate-all)

# Seconds one warm run of a crate's tests takes.
baseline() {
    (cd "core/crates/$1" && cargo test --no-run >/dev/null 2>&1)
    python3 -I - "$repo/core/crates/$1" <<'PY'
import subprocess, sys, time
started = time.monotonic()
subprocess.run(["cargo", "test"], cwd=sys.argv[1], capture_output=True)
print(f"{time.monotonic() - started:.1f}")
PY
}

# The timeout factor for a crate: the override, or max(3, ceil(floor / baseline)).
factor_for() {
    [[ -n "${MUTATOR_TIMEOUT_FACTOR:-}" ]] && { echo "$MUTATOR_TIMEOUT_FACTOR"; return; }
    python3 -I -c 'import math, sys; print(max(3, math.ceil(float(sys.argv[2]) / float(sys.argv[1]))))' \
        "$(baseline "$1")" "$timeout_floor"
}

factors=""
for crate in $(ls core/crates); do
    factor=$(factor_for "$crate")
    factors="$factors $crate=$factor"
    echo "$crate: timeout factor $factor (floor ${timeout_floor} s)"
done

total_sites=0
while IFS= read -r file; do
    if [[ ${#disk_full[@]} -gt 0 ]] || ! space_ok "$scratch" "$disk_floor"; then
        [[ ${#disk_full[@]} -gt 0 ]] || full "$(free_mb "$scratch") MB free before $file (floor $disk_floor MB)"
        break
    fi
    out="$log_dir/mutate-$(echo "$file" | tr '/' '_').txt"
    trace="$log_dir/trace-$(echo "$file" | tr '/' '_').txt"
    crate=$(echo "$file" | cut -d/ -f3)
    factor=$(echo "$factors" | tr ' ' '\n' | sed -n "s/^$crate=//p")
    : >"$trace"
    MUTANT_TRACE="$trace" "$mutator" --use-existing-coverage --max-workers "$workers" \
        --timeout-factor "$factor" ${mutate_all[@]+"${mutate_all[@]}"} \
        --test-command "$(printf '%q %q' "$repo/scripts/mutant-test.sh" "core/crates/$crate")" \
        "$file" </dev/null >"$out" 2>&1
    code=$?
    read -r killed survived uncovered sites < <(awk '$1 == "Total" {print $2, $3, $4, $5}' "$out")
    killed=${killed:-0}; survived=${survived:-0}; uncovered=${uncovered:-0}; sites=${sites:-0}
    timeouts=$(( $(grep -c '^start' "$trace") - $(grep -c '^end' "$trace") ))
    printf '%-52s sites %3s  killed %3s (by timeout %s)  survived %s  uncovered %s\n' \
        "${file#core/crates/}" "$sites" "$killed" "$timeouts" "$survived" "$uncovered"
    total_sites=$((total_sites + sites))
    grep -E "^(SURVIVED|UNCOVERED)" "$out" | sed 's/^/    /'
    if [[ $code -ne 0 || $survived -ne 0 || $uncovered -ne 0 ]]; then
        fail "mutator $file (exit $code)"
    fi
done < <(find core/crates/*/src -name '*.rs' | sort)
echo "mutation sites analysed: $total_sites"
[[ $total_sites -gt 0 ]] || fail "mutator found no sites"
rm -rf "$repo/target/mutation-workers"
space_ok "$scratch" "$disk_floor" \
    || full "$(free_mb "$scratch") MB free on the volume of $scratch after the mutator (floor $disk_floor MB)"
while IFS= read -r log; do
    full "a full disk in $(basename "$log")"
done < <(full_disk_logs "$log_dir"/mutate-*.txt)

section "result"
if [[ ${#disk_full[@]} -gt 0 ]]; then
    printf 'disk full: %s\n' "${disk_full[@]}"
    echo "the disk was full: rerun when it has room; no verdict of this run counts"
    exit 3
fi
if [[ ${#failed[@]} -gt 0 ]]; then
    printf 'failed: %s\n' "${failed[@]}"
    exit 1
fi
echo "all gates green"
