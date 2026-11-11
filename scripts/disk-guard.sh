# Sourced by scripts/quality.sh: a full disk must never pass for a killed
# mutant or a failing test. A mutant's test run that dies of ENOSPC fails,
# and the mutator counts every failing run as a kill.

# The free megabytes of the volume that holds <dir>.
free_mb() {
    df -Pk "$1" 2>/dev/null | awk 'NR == 2 {print int($4 / 1024)}'
}

# True when the volume that holds <dir> has at least <mb> MB free.
space_ok() {
    local free
    free="$(free_mb "$1")"
    [[ -n "$free" && "$free" -ge "$2" ]]
}

# What a full disk leaves in a log: the OS's words (Python's OSError,
# Rust's io::Error, errno) and the index's own NoSpace.
FULL_DISK='No space left on device|Errno 28|os error 28|ENOSPC|NoSpace'

# Prints each of <file...> that shows a full disk; true when one does.
full_disk_logs() {
    grep -lE "$FULL_DISK" "$@" 2>/dev/null
}

# The longest TMPDIR the tests work in: a fixture's socket path must fit
# in 104 bytes, and the deepest trees they build come near PATH_MAX.
TMPDIR_MAX=64

# Why <dir> (TMPDIR) cannot hold the tests' fixtures, if it cannot: it
# must exist, lie outside the system's own areas (a fixture under
# /private/var is classified as system data, not the user's) and be at
# most TMPDIR_MAX characters long once resolved. Prints nothing and is
# true when it can.
tmpdir_problem() {
    local dir
    dir="$(cd "$1" 2>/dev/null && pwd -P)" || { echo "TMPDIR $1 does not exist"; return 0; }
    case "$dir/" in
        /private/*|/var/*|/tmp/*|/System/*|/Library/*|/usr/*)
            echo "TMPDIR $dir is in a system area: the tests' fixtures would be classified as system data"
            return 0 ;;
    esac
    if (( ${#dir} > TMPDIR_MAX )); then
        echo "TMPDIR $dir is ${#dir} characters long, more than $TMPDIR_MAX: socket paths and the deepest test trees do not fit"
        return 0
    fi
    return 1
}

