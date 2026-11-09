#!/usr/bin/env bash
# Quality gates for the Rust core (core/):
#   cargo fmt --check, cargo clippy -D warnings, cargo test,
#   crapper (every function CRAP < 10), dryer (no pair scoring >= 0.8),
#   mutator (no surviving mutant, no uncovered site).
# Exits non-zero when any gate fails, and when a tool analysed nothing.
#
# Environment:
#   UB_TOOLS                 tool checkout root (default ~/.local/share/ub-tools)
#   CRAPPER, DRYER, MUTATOR  override a single tool's executable
#   MUTATE_ALL=1             rerun every mutant, not only changed functions
#   MUTATOR_WORKERS          parallel mutants per file (default 2: every
#                            worker builds its own copy of the crates)
#   MUTATOR_TIMEOUT_FACTOR   mutant timeout / baseline time (default 30:
#                            each worker builds its own copy of the crate)
#
# The tools do not analyse Swift; the Swift app has its own gates.

set -uo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo" || exit 2

ub_tools="${UB_TOOLS:-$HOME/.local/share/ub-tools}"
crapper="${CRAPPER:-$ub_tools/crapper/crapper}"
dryer="${DRYER:-$ub_tools/dryer/dryer}"
mutator="${MUTATOR:-$ub_tools/mutator/mutator}"
workers="${MUTATOR_WORKERS:-2}"
timeout_factor="${MUTATOR_TIMEOUT_FACTOR:-30}"

crap_limit=10
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

section "cargo fmt --check"
(cd core && cargo fmt --check) || fail "cargo fmt"

section "cargo clippy --all-targets -- -D warnings"
(cd core && cargo clippy --all-targets -- -D warnings) || fail "cargo clippy"

section "cargo test"
(cd core && cargo test 2>&1) | tee "$log_dir/test.txt" | grep -E "^test result|FAILED|panicked"
[[ ${PIPESTATUS[0]} -eq 0 ]] || fail "cargo test"

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
mutate_all=()
[[ "${MUTATE_ALL:-0}" == "1" ]] && mutate_all=(--mutate-all)
total_sites=0
while IFS= read -r file; do
    out="$log_dir/mutate-$(echo "$file" | tr '/' '_').txt"
    "$mutator" --use-existing-coverage --max-workers "$workers" \
        --timeout-factor "$timeout_factor" ${mutate_all[@]+"${mutate_all[@]}"} "$file" \
        </dev/null >"$out" 2>&1
    code=$?
    read -r killed survived uncovered sites < <(awk '$1 == "Total" {print $2, $3, $4, $5}' "$out")
    killed=${killed:-0}; survived=${survived:-0}; uncovered=${uncovered:-0}; sites=${sites:-0}
    printf '%-52s sites %3s  killed %3s  survived %s  uncovered %s\n' \
        "${file#core/crates/}" "$sites" "$killed" "$survived" "$uncovered"
    total_sites=$((total_sites + sites))
    grep -E "^(SURVIVED|UNCOVERED)" "$out" | sed 's/^/    /'
    if [[ $code -ne 0 || $survived -ne 0 || $uncovered -ne 0 ]]; then
        fail "mutator $file (exit $code)"
    fi
done < <(find core/crates/*/src -name '*.rs' | sort)
echo "mutation sites analysed: $total_sites"
[[ $total_sites -gt 0 ]] || fail "mutator found no sites"
rm -rf "$repo/target/mutation-workers"

section "result"
if [[ ${#failed[@]} -gt 0 ]]; then
    printf 'failed: %s\n' "${failed[@]}"
    exit 1
fi
echo "all gates green"
