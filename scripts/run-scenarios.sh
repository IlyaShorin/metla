#!/usr/bin/env bash
# Runs the debug app's cleanup scenarios (`--scenario <name>`), each on a
# fixture of its own made by metla-cli: the real view models drive the
# flow end to end, and the app exits non-zero when the fixture's state on
# disk afterwards is not what the flow confirmed. Prints one line per
# scenario and exits non-zero when any failed.
#
#   scripts/run-scenarios.sh [name ...]   default: all four
#
# Environment: METLA_BUILD as for build-app.sh (the debug bundle must be
# built: scripts/build-app.sh --debug);
# METLA_SCENARIO_DIR where the fixtures go (default <build>/scenarios).

set -uo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
build="${METLA_BUILD:-$repo/target/app}"
app="$build/debug/Metla.app/Contents/MacOS/Metla"
work="${METLA_SCENARIO_DIR:-$build/scenarios}"
[[ -x "$app" ]] || { echo "no $app: run scripts/build-app.sh --debug" >&2; exit 2; }
(cd "$repo/core" && cargo build -q -p metla-cli) || exit 2
cli="$repo/core/target/debug/metla-cli"

names=("$@")
[[ ${#names[@]} -gt 0 ]] || names=(default-clean selected-worktree cancel quick-clean)

failed=()
for name in "${names[@]}"; do
    root="$work/$name"
    rm -rf -- "$root" "$root-trash" "$root-data"
    "$cli" fixture create "$root" >/dev/null || exit 2
    METLA_ROOT="$root" METLA_HOME="$root/home" METLA_DATA_DIR="$root-data" METLA_LANGUAGE=en \
        "$app" --scenario "$name" >"$root.log" 2>&1
    code=$?
    grep -E "^scenario |FAILED" "$root.log"
    echo "exit $code"
    [[ $code -eq 0 ]] || failed+=("$name")
done

if [[ ${#failed[@]} -gt 0 ]]; then
    echo "failed: ${failed[*]}"
    exit 1
fi
echo "all scenarios as expected"
