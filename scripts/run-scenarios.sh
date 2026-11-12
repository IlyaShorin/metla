#!/usr/bin/env bash
# Runs the debug app's cleanup scenarios (`--scenario <name>`), each on a
# fixture of its own made by metla-cli: the real view models drive the
# flow end to end, and the app exits non-zero when the fixture's state on
# disk afterwards is not what the flow confirmed. Prints, per scenario, the
# app's first line (its version and the stamp of the core it links) and
# what the scenario says, and exits non-zero when any failed; a scenario
# still running after its limit is stopped and fails.
#
# The debug app has the installed app's bundle id: it is started with
# -ApplePersistenceIgnoreState YES, so it neither reads nor writes the
# window state macOS keeps for that id (a state left by a crash once held
# every launch at the "reopen windows?" alert, which nobody answers here).
#
#   scripts/run-scenarios.sh [name ...]   default: all five (update: the
#                                         in-app update on fixture bundles
#                                         in <fixture>-update; only the
#                                         relaunch is observed as a request)
#
# Environment: METLA_BUILD as for build-app.sh (the debug bundle must be
# built: scripts/build-app.sh --debug);
# METLA_SCENARIO_DIR where the fixtures go (default <build>/scenarios);
# METLA_SCENARIO_LIMIT the seconds one scenario may run (default 180).

set -uo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
build="${METLA_BUILD:-$repo/target/app}"
app="$build/debug/Metla.app/Contents/MacOS/Metla"
work="${METLA_SCENARIO_DIR:-$build/scenarios}"
limit="${METLA_SCENARIO_LIMIT:-180}"
[[ -x "$app" ]] || { echo "no $app: run scripts/build-app.sh --debug" >&2; exit 2; }
(cd "$repo/core" && cargo build -q -p metla-cli) || exit 2
cli="$repo/core/target/debug/metla-cli"

names=("$@")
[[ ${#names[@]} -gt 0 ]] || names=(default-clean selected-worktree cancel quick-clean update)

# Waits for the app (pid $2) and gives its exit status. macOS has no
# timeout(1): after $1 seconds this stops the app (TERM, then KILL) and
# says so in its log ($3), so a hung app is a failed scenario, not a hung
# script.
stop_after() {
    local limit="$1" pid="$2" log="$3" waited
    for ((waited = 0; waited < limit; waited++)); do
        kill -0 "$pid" 2>/dev/null || break
        sleep 1
    done
    if kill -0 "$pid" 2>/dev/null; then
        echo "FAILED: still running after $limit s, stopped" >>"$log"
        kill "$pid"
        sleep 5
        kill -KILL "$pid" 2>/dev/null
    fi
    wait "$pid"
}

failed=()
for name in "${names[@]}"; do
    root="$work/$name"
    rm -rf -- "$root" "$root-trash" "$root-data" "$root-update"
    "$cli" fixture create "$root" >/dev/null || exit 2
    METLA_ROOT="$root" METLA_HOME="$root/home" METLA_DATA_DIR="$root-data" METLA_LANGUAGE=en \
        "$app" --scenario "$name" -ApplePersistenceIgnoreState YES >"$root.log" 2>&1 &
    stop_after "$limit" $! "$root.log"
    code=$?
    grep -E "^metla |^scenario |FAILED" "$root.log"
    echo "exit $code"
    [[ $code -eq 0 ]] || failed+=("$name")
done

if [[ ${#failed[@]} -gt 0 ]]; then
    echo "failed: ${failed[*]}"
    exit 1
fi
echo "all scenarios as expected"
