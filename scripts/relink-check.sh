#!/usr/bin/env bash
# Checks that build-app.sh links the core it just built, even when no
# Swift source changed: builds the debug app, changes the core's sources
# (a comment appended to one file, which changes the core's stamp), builds
# it again, and checks that the new app holds the new stamp and not the
# old one. Then it puts the file back and builds the app again: the app it
# leaves is linked with the core of the sources as they are (checked by
# its stamp), so whatever runs the app next runs the real core. The
# working copy must have no change in that file to begin with.
# fresh-clone-check.sh runs this.
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd -P)"
cd "$repo"
file=core/crates/metla-ffi/src/lib.rs
git diff --quiet -- "$file" || { echo "relink-check: $file has changes of its own" >&2; exit 2; }
trap 'git checkout -q -- "$file"' EXIT
stamp_file=core/target/app-core/plain/lib/stamp

app="$(scripts/build-app.sh --debug)"
before="$(cat "$stamp_file")"
printf '\n// relink-check\n' >>"$file"
app="$(scripts/build-app.sh --debug)"
after="$(cat "$stamp_file")"
binary="$app/Contents/MacOS/Metla"

[[ "$before" != "$after" ]] || { echo "relink-check: the stamp did not change ($after)" >&2; exit 1; }
LC_ALL=C grep -q -a -F "$after" "$binary" || { echo "relink-check: the app lacks the new core ($after)" >&2; exit 1; }
if LC_ALL=C grep -q -a -F "$before" "$binary"; then
    echo "relink-check: the app still holds the old core ($before)" >&2
    exit 1
fi

git checkout -q -- "$file"
app="$(scripts/build-app.sh --debug)"
restored="$(cat "$stamp_file")"
binary="$app/Contents/MacOS/Metla"
[[ "$restored" == "$before" ]] || { echo "relink-check: the restored core has another stamp ($restored)" >&2; exit 1; }
if ! LC_ALL=C grep -q -a -F "$before" "$binary" || LC_ALL=C grep -q -a -F "$after" "$binary"; then
    echo "relink-check: the app left behind is not linked with the restored core ($before)" >&2
    exit 1
fi
echo "relink-check: the app links the core just built ($before -> $after), and is left linked with the restored one ($restored)"
