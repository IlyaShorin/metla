#!/usr/bin/env bash
# Starts the built Metla.app (scripts/build-app.sh) on a fixture, with its
# index beside it, and passes its arguments on (for example
# `--capture <dir>` or `--scenario <name>`). Without METLA_ROOT it makes a
# fixture with metla-cli first, so a development run never reads or writes
# anything real; a METLA_ROOT that is not a fixture is refused. The app
# has the installed app's bundle id: it is started with
# -ApplePersistenceIgnoreState YES, so it neither reads nor writes the
# window state macOS keeps for that id. A capture still running after
# METLA_CAPTURE_LIMIT seconds (default 600) is stopped and fails.
#
# Environment: METLA_BUILD as for build-app.sh; METLA_CONFIGURATION debug
# (default), release or release-plain (the bundle build-app.sh made with
# --debug, --release or --release --plain); METLA_ROOT, METLA_HOME and
# METLA_DATA_DIR to choose another fixture or data directory;
# METLA_CAPTURE_LIMIT as above.

set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
build="${METLA_BUILD:-$repo/target/app}"
configuration="${METLA_CONFIGURATION:-debug}"
app="$build/$configuration/Metla.app"
[[ -x "$app/Contents/MacOS/Metla" ]] || { echo "no $app: build it with scripts/build-app.sh" >&2; exit 1; }

if [[ -z "${METLA_ROOT:-}" ]]; then
    export METLA_ROOT="$build/fixture"
    if [[ ! -e "$METLA_ROOT/.metla-fixture" ]]; then
        (cd "$repo/core" && cargo build -q -p metla-cli)
        "$repo/core/target/debug/metla-cli" fixture create "$METLA_ROOT" >&2
    fi
    export METLA_HOME="${METLA_HOME:-$METLA_ROOT/home}"
fi
[[ -f "$METLA_ROOT/.metla-fixture" ]] || {
    echo "$METLA_ROOT is not a fixture (no .metla-fixture): run-app.sh starts the app on fixtures only" >&2
    exit 1
}
export METLA_DATA_DIR="${METLA_DATA_DIR:-$build/data}"
arguments=("$@" -ApplePersistenceIgnoreState YES)

if [[ " $* " != *" --capture "* ]]; then
    exec "$app/Contents/MacOS/Metla" "${arguments[@]}"
fi
# A capture is started with `open`, so the app is the active one and its
# windows are drawn as the user sees them while working in it (an app
# started from a shell stays in the background: dimmed sidebar, grey
# buttons). `open` passes on only the environment given to it.
log="$(mktemp "${TMPDIR:-/tmp}/metla-capture.XXXXXX")"
environment=()
for name in METLA_ROOT METLA_HOME METLA_DATA_DIR METLA_LANGUAGE TMPDIR; do
    [[ -n "${!name:-}" ]] && environment+=(--env "$name=${!name}")
done
open -n -W -a "$app" --stdout "$log" --stderr "$log" "${environment[@]}" --args "${arguments[@]}" &
opened=$!
# macOS has no timeout(1). The app is not a child of this script: the
# process that holds its log is stopped (TERM, then KILL) once the limit
# is past, and `open` returns.
limit="${METLA_CAPTURE_LIMIT:-600}"
for ((waited = 0; waited < limit; waited++)); do
    kill -0 "$opened" 2>/dev/null || break
    sleep 1
done
if kill -0 "$opened" 2>/dev/null; then
    for signal in TERM KILL; do
        while read -r pid; do
            kill "-$signal" "$pid" 2>/dev/null || true
        done < <(lsof -t -- "$log" 2>/dev/null)
        sleep 5
    done
    echo "capture: FAIL still running after $limit s, stopped" >>"$log"
fi
wait "$opened" || true
cat "$log"
# `open` does not pass the app's exit status on: the log says whether the
# capture wrote its images and whether any of its checks failed.
grep -q '^capture: [0-9]* images' "$log" && ! grep -q '^capture: FAIL' "$log"
code=$?
rm -f -- "$log"
exit "$code"
