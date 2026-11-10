# Performance

Measurements of the Rust core, taken with `metla-cli` from a release build.
Only aggregate numbers are recorded here, never paths or names.

## M1 baseline

**Machine.** Apple M3 Pro (6 performance + 6 efficiency cores), 36 GB RAM,
macOS 26.6.2, internal APFS SSD of 494 GB in one container (system and data
volumes in a volume group). The disk was 99 % full (about 3 GB available).

**Conditions.** `metla-cli scan --root / --index <temporary file> --json`,
release build, 10 scan threads (cores − 2), index on the same volume in a
temporary directory (deleted afterwards). The process had no Full Disk
Access and was not root. The machine was otherwise idle apart from the
desktop session. Directory metadata had been read shortly before by builds
and tests, and there is no way to drop the cache without root (`purge`), so
both runs below are **warm**; the two times agree within 3 %, which supports
that. A cold-cache number is still to be measured.

| | run 1 | run 2 (rescan) |
|---|---:|---:|
| scan wall time (walk + rows written) | 115.0 s | 118.7 s |
| process wall time (incl. index swap and indexes) | 122.2 s | 123.5 s |
| CPU user / system | 20.5 s / 449.8 s | 22.4 s / 508.0 s |
| directories | 2,193,316 | 2,193,320 |
| files (non-directory entries) | 12,706,812 | 12,707,271 |
| throughput | 19,071 dirs/s, 110,486 files/s | 18,479 dirs/s, 107,060 files/s |
| scanned (allocated) | 451.6 GB | 451.8 GB |
| peak RSS | 127.2 MB | 126.6 MB |
| index file after the run | 181.5 MB | 288.0 MB |
| coverage | partial | partial |
| gaps: denied / failed / vanished / cancelled | 373 / 1 / 0 / 0 | 373 / 1 / 0 / 0 |
| skips: other volume / mount point / dataless / excluded | 11 / 0 / 6 / 21 | 11 / 0 / 6 / 21 |

* **Where the time goes.** System time is about 20× user time: the walk is
  bound by the per-entry `fstatat` and directory opens in the kernel, not by
  the Rust code. That is the case for trying `getattrlistbulk` later.
* **Index size.** 181.5 MB after the first scan (about 83 bytes per
  directory). A rescan swaps a new table in and leaves the old picture's
  pages on SQLite's free list, so after the rescan the file was 288.0 MB;
  later scans reuse those pages (not yet measured over more rescans). The
  WAL is empty after a scan.
* **Gaps.** None of the 373 denied directories is a child of `/`; they
  are deeper root-only or privacy-protected directories (not inspected one
  by one), as expected without Full Disk Access. The one failure is
  `EINTR` on a single directory, recorded as a gap and not retried.
* **Skips.** The 11 other volumes are the separate volumes under
  `/System/Volumes`, `/dev`, the automounter and mounted disk images; the
  21 exclusions are the 3 magic root directories and the firmlink targets
  inside `/System/Volumes/Data`.

### Accounting check

* **Subtrees against `du -sk -x`** (run 2's index against `du` right after):

  | subtree | Metla (bytes) | `du` (KiB × 1024) | difference |
  |---|---:|---:|---:|
  | `/Applications` | 36,883,775,782 | 36,883,836,928 | 61 KB (0.0002 %) |
  | `/opt` | 6,246,072,252 | 6,246,072,320 | 68 B |
  | `/usr` | 1,166,362,223 | 1,174,953,984 | 8.6 MB (0.7 %) |

  `/usr` has 4,637 hard-linked files; `du` charges a file fully to the
  first link it meets, Metla charges each link `allocated / nlink`, so links
  that live outside the subtree take their share elsewhere. Over the whole
  volume the two rules give the same total. `du` also reported one
  unreadable directory there.
* **Grand total against `df`.** Used space (total − available, as Finder
  counts it) was 491.3 GB; Metla found 451.8 GB, so **39.5 GB is
  unaccounted**. Of the container, `df` shows the system volume at 12.6 GB
  and the data volume at 468.1 GB used (480.8 GB together, 29 GB more than
  the scan); the remaining ~10 GB are the container's other volumes (Preboot
  alone is 9.1 GB) and APFS overhead. The 29 GB on the two scanned volumes
  are the 374 gaps, APFS snapshots and file-system metadata, none of which a
  metadata walk without Full Disk Access can see.

## M2: the cost of classification

**Question.** Does classifying every directory (rule matching, marker
collection, reading `.git` pointer files and `CACHEDIR.TAG` heads, file-kind
totals) cost noticeable scan time?

**Conditions.** Same machine and macOS as the M1 baseline, release build,
10 scan threads, `metla-cli scan --root / --index :memory: --json`, with
and without `--no-rules`, run alternately (without, with, without, with)
so that background load falls on both sides. The index lived in an
in-memory SQLite database: the disk had about 0.5 GB free, too little for
an on-disk index of the whole volume, so these wall times leave out the
index's disk writes that the M1 numbers include. Background system
services (media analysis, a malware scan, FSEvents) were active; all runs
were warm.

| | no rules, run 1 | rules, run 1 | no rules, run 2 | rules, run 2 |
|---|---:|---:|---:|---:|
| scan wall time | 93.0 s | 97.3 s | 96.2 s | 95.8 s |
| process wall time | 95.1 s | 99.1 s | 98.1 s | 97.7 s |
| CPU user / system | 16.8 s / 395.5 s | 18.0 s / 404.8 s | 16.4 s / 391.8 s | 17.8 s / 396.6 s |
| directories | 2,225,454 | 2,225,457 | 2,225,457 | 2,225,461 |
| files | 12,858,924 | 12,858,931 | 12,858,916 | 12,858,927 |
| peak RSS | 285.9 MB | 279.0 MB | 281.3 MB | 281.4 MB |
| gaps: denied / failed | 398 / 0 | 398 / 0 | 398 / 0 | 398 / 0 |

* **Result.** Classification adds about 1.3 s of user CPU to a scan of
  2.2 million directories (16.6 s → 17.9 s on average, about 0.6 µs per
  directory before parallelism). The wall-time difference of the means,
  94.6 s → 96.5 s (+2 %), is smaller than the spread between the two runs
  without rules (3.2 s); the scan stays bound by system time in the kernel.
* **Failures.** All four runs recorded 0 failed directories. M1 lost one
  directory to `EINTR`, which the fs calls now retry; these runs do not
  show whether an interruption happened and was retried (the retry itself
  is tested deterministically).

## M3: asking Git about project output

**Question.** What does verifying every project-output candidate with Git
after a scan cost, and what does it change?

**Conditions.** Same machine, release build, `metla-cli scan --root /
--json` with the rules for the user's home, 10 threads, the index on a
RAM disk (deleted afterwards). Git is the system's (Apple Git 2.50.1),
called directly rather than through the `/usr/bin/git` shim, with
`GIT_OPTIONAL_LOCKS=0`; both commands it runs only read. Warm cache.

| | |
|---|---:|
| scan wall time | 95.0 s |
| process wall time (scan, index swap, Git pass) | 105.8 s |
| directories | 2,230,628 |
| project-output directories asked about | 2,161 |
| repositories (nearest ancestor holding `.git`) | 204 |
| Git pass wall time | 8.6 s |
| verified / not ignored / tracked inside / unverifiable | 2,103 / 8 / 0 / 50 |
| peak RSS | 127.9 MB |

* **Cost.** Two Git calls per repository (one `check-ignore --no-index
  --stdin -z`, one `ls-files -z` per 512 paths): about 21 ms per call on
  average, 4 ms per directory, 9 % on top of the scan. The macOS shim
  `/usr/bin/git` costs about 40 ms per call on its own (13 ms for the real
  binary with `--version`), which is why `Git::system()` resolves the real
  one once.
* **What it removes.** 58 matches are no longer offered: 8 that Git does
  not ignore, and 50 that sit below a `.git` entry Git does not accept as a
  repository (each in its own, so one bad `.git` never spoils another
  repository's batch).
* **Worktrees.** With classification continuing inside linked worktrees,
  1,873 verified build-output directories (78.2 GB) were found inside the
  232 worktrees (104.3 GB in all); 27 of them (under 0.1 GB) are inside
  Codex-managed and 28 (1.4 GB) inside Claude-managed worktrees and are not
  preselected. Category totals: build output 2,103 candidates, 109.4 GB;
  worktrees 232, 104.3 GB. "Can be freed" (the preselected candidates):
  2,130 candidates, 91.8 GB.

## M3: removal

* `fs::remove_contents` removes 20,000 files from one directory in about
  2.9 s and a directory tree of 1,600 nested directories in about 6.4 s on
  the RAM disk. The second figure is the kernel's: removing a directory
  costs more the deeper it lies (a raw `rmdir` loop in Python over the same
  chain took 5.8 s at 1,600 levels, 0.5 s at 800, 0.03 s at 400), whether
  or not the ancestors' descriptors are held open. Real trees are rarely
  more than a few dozen levels deep.
