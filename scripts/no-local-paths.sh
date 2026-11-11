#!/usr/bin/env bash
# Fails when a Mach-O file under <dir> (an app bundle) holds a local path:
# the user's name, the home directory, the repository's path or any other
# path given, as `strings` finds them. scripts/build-app.sh runs it on
# every release bundle before signing it, with its build directory.
#
#   scripts/no-local-paths.sh <dir> [path ...]

set -uo pipefail

dir="${1:?usage: $0 <dir>}"
repo="$(cd "$(dirname "$0")/.." && pwd)"
needles=("$(id -un)" "$HOME" "$repo" "$(cd "$repo" && pwd -P)" "${@:2}")

checked=0
found=0
while IFS= read -r -d '' file; do
    file -b "$file" | grep -q 'Mach-O' || continue
    checked=$((checked + 1))
    text="$(strings -a "$file")"
    for needle in "${needles[@]}"; do
        hits=$(grep -cF -- "$needle" <<<"$text")
        if [[ $hits -gt 0 ]]; then
            echo "local path in ${file#"$dir"/}: $hits strings hold the $([[ "$needle" == "$(id -un)" ]] && echo "user's name" || echo "path $needle")"
            found=$((found + hits))
        fi
    done
done < <(find "$dir" -type f -print0)

echo "no-local-paths: $checked Mach-O files checked, $found strings with a local path"
[[ $checked -gt 0 && $found -eq 0 ]]
