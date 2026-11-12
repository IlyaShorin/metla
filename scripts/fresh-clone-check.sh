#!/usr/bin/env bash
# Clones the repository's HEAD into <dir>, a fresh clone with no build
# output and no target/ directory, and runs the scripts there as
# README.md and CONTRIBUTING.md tell a newcomer to, each with its exit
# status: a script that only works in a used working copy fails here.
#
#   scripts/fresh-clone-check.sh <dir>
#
# Steps: build-core.sh; build-app.sh --debug; relink-check.sh (a change
# to the core alone reaches the app, and the app is left linked with the
# real core); run-app.sh --capture (a fixture made by metla-cli, the
# pictures in <dir>/shots); run-scenarios.sh; stamps (the capture and
# every scenario ran an app whose first line names the stamp of this
# clone's core, not the one relink-check made);
# build-app.sh --release (with its local-path check); quality-swift.sh;
# quality.sh with QUALITY_STEPS=tests (fmt, clippy, tests, slow tests,
# stability: the whole gate takes hours). Put <dir> on a volume with a few
# GB free and TMPDIR there too. Prints one line per step and exits
# non-zero when one failed.

set -uo pipefail

dir="${1:?usage: $0 <dir>}"
repo="$(cd "$(dirname "$0")/.." && pwd)"
[[ ! -e "$dir" ]] || { echo "$dir exists: give a new directory" >&2; exit 2; }
git clone -q "$repo" "$dir" || exit 2
clone="$(cd "$dir" && pwd -P)"
logs="$clone.logs"
mkdir -p "$logs"
echo "fresh clone of $(git -C "$clone" rev-parse --short HEAD) in $clone"

failed=()
step() {
    local name="$1"
    shift
    local started=$SECONDS
    (cd "$clone" && "$@") >"$logs/$name.txt" 2>&1
    local code=$?
    printf '%-16s exit %s  %4s s  %s\n' "$name" "$code" "$((SECONDS - started))" "$(tail -1 "$logs/$name.txt")"
    [[ $code -eq 0 ]] || failed+=("$name")
}

# The app's first line on stderr in the capture's and the scenarios'
# logs names the core it links; each must be this clone's plain core.
stamps() {
    local expected found
    expected="$(cat core/target/app-core/plain/lib/stamp)"
    found="$(grep -h "^metla .*, core " "$logs/run-app.txt" "$logs/scenarios.txt")"
    printf '%s\n' "$found"
    if [[ -z "$found" ]] || grep -v -q -F ", core $expected" <<<"$found"; then
        echo "an app ran with another core than this clone's ($expected)"
        return 1
    fi
    echo "$(sort -u <<<"$found") (each of $(wc -l <<<"$found" | tr -d ' ') runs: this clone's core)"
}

step build-core scripts/build-core.sh
step build-debug scripts/build-app.sh --debug
step relink scripts/relink-check.sh
step run-app scripts/run-app.sh --capture "$clone/target/shots"
step scenarios scripts/run-scenarios.sh
step stamps stamps
step build-release scripts/build-app.sh --release
step quality-swift scripts/quality-swift.sh
step quality-tests env QUALITY_STEPS=tests scripts/quality.sh

if [[ ${#failed[@]} -gt 0 ]]; then
    echo "failed: ${failed[*]} (logs in $logs)"
    exit 1
fi
echo "every script ran from a fresh clone (logs in $logs)"
