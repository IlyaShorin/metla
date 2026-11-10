#!/usr/bin/env bash
# The test command quality.sh gives the mutator: `cargo test` in the crate
# directory given as $1 (relative to the mutator's working directory), as
# the mutator's own default runs it; further arguments go to `cargo test`.
# It leaves a trace in $MUTANT_TRACE: "start" before the tests and "end
# <code>" after them. A mutant the mutator stops at its timeout is killed
# with its whole process group, so it leaves a start without an end;
# quality.sh counts those.
trace="${MUTANT_TRACE:-/dev/null}"
dir="$1"
shift
echo "start $$" >>"$trace"
(cd "$dir" && cargo test "$@")
code=$?
echo "end $$ $code" >>"$trace"
exit "$code"
