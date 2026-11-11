#!/usr/bin/env bash
# Starts the built Metla.app (scripts/build-app.sh) on a fixture, with its
# index beside it, and passes its arguments on (for example
# `--capture <dir>` or `--scenario <name>`). Without METLA_ROOT it makes a
# fixture with metla-cli first, so a development run never reads or writes
# anything real; a METLA_ROOT that is not a fixture is refused.
#
# Environment: METLA_BUILD as for build-app.sh; METLA_CONFIGURATION debug
# (default), release or release-plain (the bundle build-app.sh made with
# --debug, --release or --release --plain); METLA_ROOT, METLA_HOME and
# METLA_DATA_DIR to choose another fixture or data directory.

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

exec "$app/Contents/MacOS/Metla" "$@"
