#!/usr/bin/env bash
# Quality gates for the Swift app (app/):
#   the core and the bindings (scripts/build-core.sh) and metla-cli, which
#   the tests use to make a fixture;
#   swift build: Swift 6 language mode, complete strict concurrency, every
#   warning an error (Package.swift; -warnings-as-errors again here);
#   swift test with code coverage;
#   llvm-cov line coverage per MetlaKit file, with the floor for the view
#   models and formatting (Models/, Text/): SWIFT_COVERAGE_FLOOR percent.
#
# crapper, dryer and mutator do not analyse Swift (docs/QUALITY.md).
#
# Environment:
#   SWIFT_SCRATCH          SwiftPM's build directory (default target/swift-build
#                          in the repository: keep it off a full disk)
#   SWIFT_COVERAGE_FLOOR   line coverage each view-model and text file needs
#                          (default 85)

set -uo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
scratch="${SWIFT_SCRATCH:-$repo/target/swift-build}"
floor="${SWIFT_COVERAGE_FLOOR:-85}"
failed=()
fail() {
    failed+=("$1")
    echo "FAIL: $1"
}

echo "== core, bindings and metla-cli"
lib="$("$repo/scripts/build-core.sh")" || { echo "FAIL: build-core"; exit 1; }
(cd "$repo/core" && cargo build -q -p metla-cli) || { echo "FAIL: metla-cli"; exit 1; }
export METLA_CLI="$repo/core/target/debug/metla-cli"

cd "$repo/app" || exit 2
link=(-Xlinker -L"$lib")

echo
echo "== swift build (Swift 6, strict concurrency, warnings as errors)"
swift build --build-tests --scratch-path "$scratch" -Xswiftc -warnings-as-errors "${link[@]}" \
    >"$scratch.build.log" 2>&1
code=$?
diagnostics=$(grep -cE "(warning|error):" "$scratch.build.log")
echo "exit $code, diagnostics $diagnostics"
[[ $code -eq 0 && $diagnostics -eq 0 ]] || { grep -E "(warning|error):" "$scratch.build.log" | head -20; fail "swift build"; }

echo
echo "== swift test --enable-code-coverage"
swift test --scratch-path "$scratch" --enable-code-coverage -Xswiftc -warnings-as-errors "${link[@]}" \
    >"$scratch.test.log" 2>&1
code=$?
grep -E "^✔ Test run|^✘ Test run|recorded an issue" "$scratch.test.log"
[[ $code -eq 0 ]] || fail "swift test"

echo
echo "== line coverage of MetlaKit (llvm-cov)"
data="$(swift test --scratch-path "$scratch" --show-codecov-path "${link[@]}" 2>/dev/null)"
profile="$(dirname "$data")/default.profdata"
binary="$(dirname "$(dirname "$data")")/MetlaPackageTests.xctest/Contents/MacOS/MetlaPackageTests"
xcrun llvm-cov report "$binary" -instr-profile "$profile" "$repo/app/Sources/MetlaKit" \
    >"$scratch.coverage.txt" 2>&1 || fail "llvm-cov"
python3 -I - "$scratch.coverage.txt" "$floor" <<'PY' || fail "coverage floor"
import sys
floor = float(sys.argv[2])
rows, low = [], []
for line in open(sys.argv[1]):
    parts = line.split()
    if not parts or not parts[0].endswith(".swift"):
        continue
    name = parts[0]
    lines, missed, cover = int(parts[7]), int(parts[8]), parts[9]
    rows.append((name, lines, missed, cover))
    gated = name.startswith(("Models/", "Text/"))
    if gated and float(cover.rstrip("%")) < floor:
        low.append(name)
for name, lines, missed, cover in rows:
    mark = "  *" if name.startswith(("Models/", "Text/")) else ""
    print(f"{name:40} lines {lines:4}  missed {missed:4}  {cover:>8}{mark}")
total = [l for l in open(sys.argv[1]) if l.startswith("TOTAL")]
print(total[0].rstrip() if total else "no total")
print(f"* view models and text: at least {floor:.0f}% each")
for name in low:
    print(f"  below the floor: {name}")
sys.exit(1 if low or not rows else 0)
PY

echo
if [[ ${#failed[@]} -gt 0 ]]; then
    printf 'failed: %s\n' "${failed[@]}"
    exit 1
fi
echo "all Swift gates green"
