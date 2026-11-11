#!/usr/bin/env bash
# Tests scripts/disk-guard.sh. Exits non-zero, naming the check, when one
# fails.
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
source "$here/disk-guard.sh"
work="$(mktemp -d "${TMPDIR:-/tmp}/disk-guard.XXXXXX")"
trap 'rm -rf "$work"' EXIT
failed=0
check() {
    if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; failed=1; fi
}

free="$(free_mb "$work")"
check "free space is a number of MB" '[[ "$free" =~ ^[0-9]+$ ]]'
check "any volume has 0 MB" 'space_ok "$work" 0'
check "no volume has a petabyte free" '! space_ok "$work" 1000000000'
check "a missing directory has no space" '! space_ok "$work/missing" 0'

printf 'OSError: [Errno 28] No space left on device: /w/x.rs\n' >"$work/python.txt"
printf 'error: failed to write: No space left on device (os error 28)\n' >"$work/rust.txt"
printf 'thread panicked: right: Failed(NoSpace)\n' >"$work/engine.txt"
printf 'Wrote x.edn\nTotal 9 0 0 9\n' >"$work/clean.txt"
found="$(full_disk_logs "$work"/*.txt | xargs -n1 basename | sort | tr '\n' ' ')"
check "every full-disk log is named, no other" '[[ "$found" == "engine.txt python.txt rust.txt " ]]'
check "a clean log is not a full disk" '! full_disk_logs "$work/clean.txt" >/dev/null'
check "no log at all is not a full disk" '! full_disk_logs "$work/none.txt" >/dev/null'

check "a short TMPDIR outside the system areas is fine" '! tmpdir_problem "$work" >/dev/null || [[ ${#work} -gt $TMPDIR_MAX || "$work" == /private/* ]]'
check "a missing TMPDIR is a problem" 'tmpdir_problem "$work/missing" >/dev/null'
check "a TMPDIR under /private/var is a problem" 'tmpdir_problem /private/var >/dev/null'
long="$work/$(printf 'x%.0s' {1..80})"
mkdir -p "$long"
check "a TMPDIR longer than $TMPDIR_MAX characters is a problem" 'tmpdir_problem "$long" >/dev/null'

exit "$failed"
