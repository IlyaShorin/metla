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

## M4: getattrlistbulk, thread sweep, index growth

**Conditions.** Same machine (Apple M3 Pro, 6 + 6 cores, 36 GB), macOS
26.6.2, the disk 99 % full (about 1.5 GB available). Release build of
`metla-cli` at 0dfcd14, `scan --root / --index <RAM disk> --json` with
the rules for the user's home, read-only, no Full Disk Access, not
root. Indexes on the RAM disk, deleted afterwards. Warm cache (the
disk had been scanned minutes before). The machine was **not** idle:
the 1-minute load average was 7 to 10 during the runs (a music player's
renderer at about 100 % of a core, fseventsd up to 160 %, the window
server about 50 %); runs alternate between the two listings so that
load falls on both, and the spread between equal runs is shown.

| | classic, run 1 | bulk, run 1 | classic, run 2 | bulk, run 2 | bulk, run 3 |
|---|---:|---:|---:|---:|---:|
| threads | 10 | 10 | 10 | 10 | 10 |
| scan wall time | 121.2 s | 88.5 s | 113.1 s | 103.7 s | 109.6 s |
| process wall time (with index swap and Git pass) | 132.6 s | 98.8 s | 123.6 s | 117.7 s | 122.8 s |
| CPU user / system | 27.3 / 551.6 s | 16.4 / 338.2 s | 23.4 / 466.5 s | 20.0 / 408.9 s | 22.0 / 464.4 s |
| directories | 2,230,875 | 2,233,400 | 2,230,876 | 2,233,399 | 2,233,400 |
| files | 12,864,915 | 12,874,708 | 12,864,904 | 12,874,648 | 12,874,662 |
| scanned (allocated) | 449.56 GB | 453.17 GB | 449.56 GB | 453.17 GB | 453.18 GB |
| denied gaps | 409 | 495 | 409 | 495 | 495 |
| peak RSS | 135.2 MB | 136.0 MB | 133.2 MB | 135.6 MB | 130.4 MB |
| index file after the run | 292.6 MB | 292.2 MB | 292.2 MB | 508.8 MB | 509.4 MB |

* **Speed.** With getattrlistbulk the scan's wall time fell from 117.2 s
  (mean of the two classic runs) to 100.6 s (mean of three bulk runs),
  −14 %, and the process's CPU time (scan, index swap and Git pass) from
  534 s to 423 s, −21 % (system time 509 s to 404 s, user time 25.3 s to
  19.5 s). The best bulk run (88.5 s, −25 %)
  came while the load was lowest; with this much other load the means
  are the fairer figure. The walk stays bound by the kernel: system time
  is still about 20 times user time.
* **The two listings on the real disk.** They agree on synthetic trees
  and on `/` and `/Volumes` entry by entry (tests), and two runs of each
  agree with each other here, but they differ from each other in one
  way: 32 directories that `lstat` refuses (EPERM; protected locations,
  without Full Disk Access), in 17 parents. The classic listing fails
  its whole parent on such an entry, so those 17 parents become denied
  gaps and nothing below them is counted; getattrlistbulk describes the
  entry, so the bulk scan reads the 17 parents (2,525 more directories,
  9,800 more files, 3.6 GB more) and records the 32 protected
  directories as denied gaps instead. The 32 refused `lstat`s and their
  17 parents were counted read-only with a script outside the repository
  (no paths kept); that the classic scans recorded exactly those parents
  as gaps is inferred from the code, not read from their indexes, which
  were already deleted. The counts fit: 409 − 17 + 32 = 424, and the
  other 71 of the 495 denied directories lie below the 17 parents, where
  the classic scans never reached. The bulk result is the more complete
  one.
* **Index growth.** Three consecutive scans into one index file: 292.2,
  508.8 and 509.4 MB. A scan writes its rows into a staging table while
  the old picture stays readable, so the file holds two pictures once;
  the third scan reuses the second's free pages. Growth is bounded at
  about 1.75 times one picture; in-place updates reuse free pages as
  well. No change was made.

**Thread sweep** (bulk listing, same conditions, one run each):

| threads | 2 | 4 | 6 | 8 | 10 (mean of 3) |
|---|---:|---:|---:|---:|---:|
| scan wall time | 223.1 s | 130.6 s | 115.5 s | 104.5 s | 100.6 s |
| CPU user + system (process) | 215.6 s | 236.2 s | 312.7 s | 360.8 s | 423.3 s |
| peak RSS | 127.9 MB | 126.6 MB | 130.3 MB | 131.3 MB | 134.0 MB |

* Past 6 threads the scan gains little (115.5 → 100.6 s) and burns much
  more CPU (313 → 423 s): threads wait on each other in the kernel.
  **Interactive** therefore uses `min(cores − 2, 8)` threads: 8 here,
  104.5 s, 15 % less CPU than 10 for 4 % more time. **Background** keeps
  2 threads: the cheapest scan in CPU (216 s), at the cost of time
  (223 s), which a background scan can afford; **Saver** keeps 1.

## M4: the engine

**Conditions.** Same machine, macOS and load as above (load average 4.5
to 9). Release build of `metla-cli` (commit given per row), FSEvents
latency 1 s, the standard profile table. The engine's index of `/` is
the third bulk scan's index copied into a data directory on the RAM
disk (deleted afterwards). Read-only on `/`; churn and refresh write
only inside a fixture (`fixture create`) on the RAM disk, with the
engine's data directory beside it.

**Opening.** `bench open`: opening the engine on the saved picture of
the full disk (2,233,400 directories) and reading the first snapshot
(totals, categories, what can be freed, kinds, coverage, freshness):
0.19 s wall, 0.12 s CPU, peak RSS 21.2 MB (0dfcd14); 0.2 s and
23.7 MB at 64afccd. No scan runs: the snapshot comes from the index.

**Idle.** `watch --root / --profile background --seconds 600`: the
engine follows the whole disk for ten minutes of ordinary desktop use
(a music player, a browser, chat and editor windows, this work's own
file edits; not quiet), resuming from the event id an earlier run
stored, so no full scan runs. Wake-ups are the scheduler's turns.

| build | events | rescans (batches) | full scans | wake-ups | CPU user + system | peak RSS |
|---|---:|---:|---:|---:|---:|---:|
| 0dfcd14 | 3,858 | 937 (465) | 0 | 974 | 64.5 + 39.6 s = 104.1 s (17 % of a core) | 54.1 MB |
| 64afccd | 13,162 | 1,122 (519) | 0 | 1,372 | 9.0 + 8.6 s = 17.6 s (2.9 % of a core) | 51.3 MB |

The first run was too expensive. A sample of the process (`sample`,
45 s) showed the index writer reading the whole table of 2.2 million
rows for every rescan that removed a subtree: SQLite planned the
subtree query as a scan of `dirs` probing the recursive CTE. With
`CROSS JOIN` (walk the subtree, look each row up) the writer's share
fell from 2,215 samples to 70, and the second run, with three times as
many events, cost a sixth of the CPU. What remains is spread over
re-reading directories (`getattrlistbulk`), reading the candidates for
worktree statuses and Git's verdicts after each batch, and Git itself
for worktrees under changed paths (its own processes, not counted
above). The RSS is the page cache of the index and the rescans' rows.

**Churn.** `bench churn --seconds 120 --files 250` on a fixture: a
thread creates 250 files in one of 16 directories, then removes the
directory with its files, every 50 ms or so, while the engine follows
the fixture (Background profile); then it waits until the engine is
idle and compares its picture with a fresh full scan.

| build | files created + deleted | churn CPU | engine batches / rescans / full scans | events | engine CPU | idle after the churn | picture |
|---|---:|---:|---:|---:|---:|---:|---|
| 0dfcd14 | 330,000 + 330,000 | 38.7 s | 31 / 37 / 0 | 662,670 | 4.5 s | 59.8 s | exactly right |
| 64afccd | 326,500 + 326,500 | 38.4 s | 16 / 21 / 1 | 654,537 | 3.1 s | 60.0 s | exactly right |

Engine CPU is the process's CPU minus the churn thread's. The back-off
holds the busy directories for up to one cap (60 s in Background), so
the picture is right a cap after the churn ends. In the second run the
engine made one full scan of the fixture; which event or task asked
for it was not recorded (lost-event flags under 650,000 events are the
likely cause, a collapse into the root the other possibility). The first run's figure includes the bench polling a full
snapshot every 20 ms; the second polls only the freshness.

**Latency of a small change.** `bench refresh`: a 1 MB file written
into one directory of a fixture, three times, each after the engine is
idle; the time until the root's total shows it.

| profile (settle / cap) | first | second | third |
|---|---:|---:|---:|
| Interactive (1 s / 30 s) | 1.3 s | 2.0 s | 5.0 s |
| Background (2 s / 60 s) | 2.4 s | 5.0 s | 9.0 s (8.0 s at 0dfcd14) |
| Saver (10 s / 300 s) | 10.7 s | 20.0 s | 41.0 s |

The first change shows after about the FSEvents latency plus the
settle time. The same directory changing again within a cap of its
last rescan starts from its last settle time multiplied again, so the
second and third show later: that is the back-off, which keeps a
directory that keeps changing from being rescanned often.

**Not measured.** Wake-ups are counted as the scheduler's turns (each
command, event batch or deadline), not as CPU wake-ups of the whole
process; cold-cache scans (no `purge` without root); the app's own
cost (M5); a Saver run on battery (profiles were set by hand, on AC
power); idle with a quiet desktop (the desktop was busy throughout).

## M5: idle cadence, index size, the app

**Conditions.** Same machine, macOS 26.6.2, the disk about 98 % full
(7.6 GB available). Release build of `metla-cli` at d73b7fd, the
standard profile table (Background: stream latency 15 s, settle 30 s,
cap 300 s, 60 s between batches). Read-only on `/`, no Full Disk
Access, not root; indexes and data directories on the RAM disk,
deleted afterwards. Warm cache. No other build or test ran; the
desktop was in ordinary use (1-minute load average 5.6 to 8.4, with
one spike to 27 in the last minute of the idle run, not from this
work). One run of each.

**Idle.** As in M4: a first `watch --profile interactive` (240 s) made
the picture and stored the event id; then `watch --root / --profile
background --seconds 600` resumed from it, so no full scan ran.

| build | events | rescans (batches) | full scans | wake-ups | CPU user + system | peak RSS |
|---|---:|---:|---:|---:|---:|---:|
| 64afccd (M4) | 13,162 | 1,122 (519) | 0 | 1,372 | 9.0 + 8.6 s = 17.6 s (2.9 % of a core) | 51.3 MB |
| d73b7fd | 5,023 | 406 (10) | 0 | 231 | 1.1 + 1.5 s = 2.6 s (0.43 % of a core) | 35.8 MB |

The target (at most 3 s per 600 s) is met by the engine's process, the
figure M4 measured (its CPU while it watches, Git's own processes not
counted). Batches came once a minute (10 in 600 s instead of 519).
Events depend on the desktop's activity and are not comparable between
the runs.

Not in that figure, and new in M5: **the worktree status pass at
opening.** An engine opened on a saved picture computes the status of
every linked worktree in it (three Git calls each, two at a time; M5
added it so that the list does not say "Checking…" forever). On this
disk that costs about 38 s of CPU in Git processes once per opening
(`/usr/bin/time` of the whole process: 5.8 + 35.2 s for the 600 s run;
a 60 s run cost 39 s and a 300 s run 43 s, the difference being the
engine's own CPU). It is a launch cost, not an idle one, but the app
pays it at every launch; computing the statuses only when the worktree
list is first asked for would avoid it. Not done in M5.

**Churn.** `bench churn --seconds 120 --files 250 --profile background`
on a fixture, as in M4.

| build | files created + deleted | churn CPU | engine batches / rescans / full scans | events | engine CPU | idle after the churn | picture |
|---|---:|---:|---:|---:|---:|---:|---|
| 64afccd (M4) | 326,500 + 326,500 | 38.4 s | 16 / 21 / 1 | 654,537 | 3.1 s | 60.0 s | exactly right |
| d73b7fd | 325,250 + 325,250 | 39.0 s | 2 / 2 / 1 | 652,015 | 3.6 s | 180.1 s | exactly right |

The engine's CPU is about the same (3.6 s against 3.1 s; one run
each). The picture is right 180 s after the churn ends instead of 60 s:
Background now waits a settle time of 30 s, backs off to 300 s and
pauses 60 s between batches, by design; with the app's window visible
(attention) the engine follows as Interactive. As in M4 one full scan
of the fixture ran; its cause was not recorded.

**Index size on the full disk.** Three consecutive `scan --root /
--index <file>` runs into one index file (8 threads, bulk listing),
with `auto_vacuum = INCREMENTAL` and the space given back after each
swap:

| | run 1 | run 2 | run 3 |
|---|---:|---:|---:|
| scan wall time | 96.8 s | 89.5 s | 91.4 s |
| Git pass (verdicts) | 10.7 s | 8.5 s | 10.1 s |
| rest of the process (open, swap, giving back, exit) | 2.7 s | 3.1 s | 3.2 s |
| CPU user / system | 15.4 / 293.3 s | 14.4 / 274.7 s | 14.3 / 273.4 s |
| directories | 2,237,853 | 2,237,860 | 2,237,862 |
| files | 12,875,927 | 12,876,000 | 12,875,950 |
| denied gaps | 498 | 498 | 498 |
| peak RSS | 133.2 MB | 134.7 MB | 130.2 MB |
| index file after the run | 293.1 MB | 293.5 MB | 293.5 MB |
| free pages after the run | 0 | 0 | 0 |

M4's second and third scans left 508.8 and 509.4 MB; now the file
returns to one picture after every scan. Giving back the old picture's
pages (about 71,000 of 4 KiB, in steps of 4,096) costs about 0.4 s: the
rest of the process takes 3.1 to 3.2 s when there is a picture to give
back and 2.7 s when there is none. During a scan the file still holds
two pictures for a while (the old one stays readable); the peak was not
sampled.

**The app.** `Metla.app` (release build at d73b7fd, ad-hoc signed)
started six times on the fixture (`scripts/run-app.sh`, English, a
saved picture in its data directory), with
`METLA_PRINT_LAUNCH_TIMING=1`: time from the process's start (kernel
start time) to the first window and to the first picture shown, and the
resident memory then and 10 s later.

| run | first window | first picture | RSS at first picture | RSS after 10 s |
|---:|---:|---:|---:|---:|
| 1 | 809 ms | 966 ms | 111.2 MB | 111.3 MB |
| 2 | 436 ms | 604 ms | 109.7 MB | 109.2 MB |
| 3 | 460 ms | 627 ms | 111.3 MB | 109.7 MB |
| 4 | 452 ms | 601 ms | 111.1 MB | 109.6 MB |
| 5 | 392 ms | 554 ms | 111.1 MB | 109.7 MB |
| 6 | 380 ms | 547 ms | 109.7 MB | 109.2 MB |

The first run is the first after a rebuild (caches cold). The fixture
is small; on the full disk's picture the first window comes before the
engine is asked anything, and the first picture after one read of the
index (0.1 s, `bench open`). Not measured: the app on the full disk
(every app run in M5 uses a fixture), its memory over hours, and the
worktree status pass at launch on the full disk from inside the app.

## M6: opening without Git, looking at the picture

**Conditions.** Same machine and disk (about 97 % full). Release build
of `metla-cli` at 66877a2 (the measured code is that of b3662d7 for
opening and of 42195b9 for `--look`). Read-only on `/`, no Full Disk
Access, not root; data directories on the RAM disk, deleted afterwards.
Busier than M5: a full mutation gate ran on two workers, and after a
full-disk incident on the RAM disk `fseventsd` itself used a whole core
and handed events over seconds late. One picture of `/` made with
`watch --root / --profile interactive` (200 s: one full scan of
2,238,911 directories; CPU 27.4 s user + 582.9 s system; index
292.7 MB).

**Opening.** M5 computed every linked worktree's status when the
engine opened on a saved picture: about 38 s of CPU in Git processes at
every launch (a 60 s `watch` of the saved picture cost 39 s). M6
computes statuses only when the worktree list is first asked for
(b3662d7; a test with a Git that logs its calls pins that opening and
`snapshot()` start none).

| measurement (whole process, Git included, `/usr/bin/time -l`) | M5 (d73b7fd) | M6 |
|---|---:|---:|
| `bench open` (open + first snapshot), three runs | — ¹ | 0.16 / 0.50 / 0.22 s wall, 0.04 s user + 0.02 s system each, 22.2 MB |
| `watch --profile background --seconds 60` on the saved picture | 39 s of CPU | 0.57 s user + 0.64 s system = 1.2 s, 23.1 MB |

¹ Not measured this way in M5: `bench open` exits right after the
snapshot, before a background status pass would have spent much; the
60 s `watch` shows what an opening cost. The status
pass now runs when the app first shows the worktree table, as before
two Git calls at a time at the profile's priority.

**Looking at the picture.** `set_attention(true)` (the app's window or
panel opens) makes what waits due at once. Two measurements:

* *What waits.* A change reported to an engine on a fixture
  (`Watching::Manual`, so fseventsd's delay is not in it) while the
  Background profile holds it (settle 30 s, pause 60 s); 2 s later
  `set_attention(true)`: the picture was current after 3 to 4 ms, five
  rounds of five (a probe program against metla-core at 42cb5a0, not in
  the repository; release build).
* *After a churn, on real FSEvents.* `bench churn --seconds 120 --files
  250 --profile background --look` on a fixture: the picture exactly
  right 30.9 s after the churn ended at 42195b9, and 30.8 s at 42cb5a0
  (after `expedite` forgets the busy directories' back-off; one run
  each). Both runs had fseventsd at a full core, handing the churn's
  events over seconds late and dropping some: the engine did 2 full
  scans of the fixture, and a full rescan's wait, like any other, backs
  off with each late event up to Interactive's cap of 30 s, which is
  what the 30.8 s is. Without attention (M5) the same picture was right
  after 180 s.

| run | files created + deleted | churn CPU | engine batches / rescans / full scans | events | engine CPU | right after the churn |
|---|---:|---:|---:|---:|---:|---:|
| M5 d73b7fd, not looked at | 325,250 + 325,250 | 39.0 s | 2 / 2 / 1 | 652,015 | 3.6 s | 180.1 s |
| 42195b9, looked at | 305,750 + 305,750 | 39.0 s | 17 / 109 / 2 | 570,655 | 2.1 s | 30.9 s |
| 42cb5a0, looked at | 301,000 + 301,000 | 39.1 s | 2 / 2 / 2 | 556,509 | 1.8 s | 30.8 s |

So a change that waits when someone looks shows at once; a churn that
fseventsd itself is still delivering shows once it has been delivered
and has settled, within Interactive's 30 s cap. The bench now waits
for a directory made after the churn to appear before it calls the
engine quiet (66877a2): with fseventsd late, being idle right after the
churn did not mean that the churn's events had arrived.
