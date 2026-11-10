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
