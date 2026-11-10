# Metla architecture

Metla is a free, open-source SSD cleaner for macOS. A Rust core scans,
classifies and cleans; a SwiftUI/AppKit app shows the result and integrates
with the system. This document is the contract between the two halves and
between contributors. Terms in *italics* are defined in [GLOSSARY.md](GLOSSARY.md).

## 1. Goals that shape the design

1. **Never lose user work.** Classification is allow-list based: a directory
   is a *candidate* only when a named, verifiable *rule* matches it. Size
   alone never makes anything junk. Everything is re-verified immediately
   before removal.
2. **Speed is a property, not a claim.** Metadata only, bounded memory,
   incremental updates, measurable through the CLI (`metla-cli bench`).
3. **Honest numbers.** An incomplete analysis is shown as incomplete; sizes
   are *allocated-size estimates*; "found", "removed" and "actually freed"
   are three different numbers.
4. **Quiet in the background.** Event-driven refresh, coalesced and
   budgeted; lower intensity on battery; can be switched off.

## 2. Process and layout

One process. The Swift app links the Rust core as a static library.

```
metla/
  core/                         Rust workspace
    crates/metla-core/          everything below "ffi" in the diagram
    crates/metla-ffi/           UniFFI surface (thin; no logic)
    crates/metla-cli/           developer CLI: scan, report, watch, bench, fixture cleaning
  app/                          SwiftPM package
    Sources/MetlaCoreFFI/       generated bindings (not edited by hand)
    Sources/MetlaKit/           engine client, view models, settings, services (unit-tested)
    Sources/MetlaApp/           @main, SwiftUI views, AppKit glue (thin)
  scripts/                      build, quality gates, benchmarks, fixtures
  docs/                         this file, glossary, rule catalogue, measurements
```

### Dependency rule (inner layers first)

```
domain            types only; no I/O, no dependencies on other modules
rules             pure classification over DirFacts            -> domain
fs                directory reading, safe removal, volume info -> domain
git               worktree inspection through the git CLI      -> domain
index             SQLite store (single writer)                 -> domain
scan              parallel walker, live totals, rescan         -> domain, fs, rules
watch             pure event coalescer + FSEvents adapter      -> domain
clean             plan, revalidate, execute, report            -> domain, fs, rules, git
engine            orchestration, scheduling, power profile     -> everything above
--- crate boundary ---
metla-ffi / metla-cli                                          -> engine, domain
--- language boundary ---
MetlaKit -> MetlaCoreFFI;  MetlaApp -> MetlaKit
```

A module may depend only on modules listed to its right. `domain` depends on
nothing. Nothing depends on `engine` except the two outer crates. The UI never
touches the file system for analysis; every listing, size and candidate comes
from the engine, called off the main thread.

The rule is one table in `core/crates/metla-core/tests/core/architecture.rs`,
which fails when a module's source refers to a module (`crate::x`, grouped
imports, or `super::` climbing out of the module) the table does not allow.
Until `engine` exists, `metla-cli` calls `scan`, `index`, `rules`, `fs`,
`git` and `clean` directly; that is the one temporary exception. `clean`
does not read the index: its caller passes the candidates, the exclusions
in force and the time in, and records the outcome in the index itself.

## 3. Domain model

The model makes the dangerous states unrepresentable instead of checking for
them at run time.

```rust
pub struct Bytes(pub u64);                 // allocated bytes (st_blocks * 512), an estimate

pub enum Category { AppCache, DevCache, BuildArtifact, Log, GitWorktree, Temp }

pub enum Disposal { DeletePermanently, MoveToTrash, GitWorktreeRemove }

/// The only categories a hotkey may clean without showing a confirmation.
pub enum QuickCategory { AppCache, DevCache, BuildArtifact }

impl Category {
    pub fn disposal(self) -> Disposal;            // total function, the single place this is decided
    pub fn quick(self) -> Option<QuickCategory>;  // None for Log, GitWorktree, Temp
}
```

| Category        | Disposal             | Quick clean possible | Notes |
|-----------------|----------------------|----------------------|-------|
| `AppCache`      | `DeletePermanently`  | yes (opt-in)         | per-app caches the app rebuilds |
| `DevCache`      | `DeletePermanently`  | yes (opt-in)         | tool caches, re-downloaded or rebuilt |
| `BuildArtifact` | `DeletePermanently`  | yes (opt-in)         | verified, git-ignored build output inside a repository |
| `Log`           | `MoveToTrash`        | never                | always a separate confirmation |
| `Temp`          | `MoveToTrash` (Trash contents: `DeletePermanently`) | never | always a separate confirmation |
| `GitWorktree`   | `GitWorktreeRemove`  | never                | only clean, pushed, unlocked, unmanaged worktrees |

`Category::disposal()` and `Category::quick()` are exhaustive `match`es, so a
new category cannot exist without both decisions; a test pins the table.
The one exception in the Temp row lives next to them: `RuleId::disposal()`
returns `DeletePermanently` for `temp.trash` (the Trash's contents cannot be
moved to the Trash) and the category's disposal for every other rule.

A rule id (`RuleId`) is the category's name, a dot and the rule's name
(`dev-cache.npm`); the category is part of the id, so an id and its
category cannot disagree, and the index needs no catalogue to group
candidates. `Category::preselected(activity, recency, managed)` is false for build
output active within `Recency`'s age (default 14 days; `now` is passed in,
there is no clock in `domain`, `rules` or `index`), for worktrees, and for
anything inside a worktree another tool manages (`Manager::of(path,
home)`: Codex Desktop below `~/.codex/worktrees`, Claude Code below a
`.claude/worktrees` directory — the one place this is decided).

Cleanup is requested through two different entry points with different types
(`clean::Cleaner`; the engine will wrap it):

```rust
cleaner.preview(candidates: Vec<Candidate>, now) -> CleanupPreview          // what, how much, which group, warnings
cleaner.execute(id, &Confirmation, &Context, &CleanControl, &dyn TrashHost) -> Result<CleanupReport, CleanError>
cleaner.quick_clean(&[QuickCategory], candidates, ...) -> Result<CleanupReport, CleanError>
```

`quick_clean` cannot be handed a worktree or a log: there is no value of
`QuickCategory` for them, and it takes only preselected candidates and
confirms nothing beyond the preview. The quick-clean switch in Settings
therefore cannot lift the confirmation requirement for other categories.

Confirmation is typed. `Group::of(rule)` is `Quick` for the quick-eligible
categories, otherwise `Separate(MoveToTrash | EmptyTrash |
RemoveWorktrees)`. A `Confirmation` carries the separate groups the user
confirmed and the items confirmed one by one; executing a preview confirms
its quick group; an item inside a managed worktree needs its own entry. An
item not covered is `Skipped(NotConfirmed)`.

Other core types: `Candidate` (id, path, rule, size, file count, activity,
device and inode for revalidation, the managing tool if any, preselected),
`Verdict` (Git's word on project output), `Exclusion` (an absolute
path; `hides(path)` is the one place exclusion semantics live), `FileKind`
and `KindTotals` (bytes per kind for a whole scan),
`ScanState`, `Coverage` (what the analysis could not see), `Freshness`
(when the picture was last true), `WorktreeStatus` (`Removable { branch }`
or `Blocked(Reasons)`, never both, the reasons never empty),
`CleanupPreview` (id, items with warnings, rejected candidates with the
reason), `Outcome` (`Removed`, `Partial(Leftovers)`, `Skipped(SkipReason)`,
`Failed`, `AlreadyGone`, `MovedToTrash`) and `CleanupReport` (per-item
found and removed bytes and outcome, free space before and after; found,
removed and measured freed are three sums of three measurements).

## 4. Measuring size

* Only metadata is read (`lstat`/`getattrlistbulk`). File contents are never
  opened, with two tiny exceptions used for verification: a worktree's `.git`
  pointer file and a `CACHEDIR.TAG` signature. The walker reads them during
  the scan, only outside candidates, only as regular files that are not
  cloud placeholders, with `O_NOFOLLOW | O_NONBLOCK` and at most 2048 bytes
  (`DirHandle::read_head`).
* Size is **allocated size**. Sparse and transparently compressed files are
  therefore counted by what they occupy, not by logical length.
* Symlinks are never followed; a symlink counts as itself.
* A file with `nlink > 1` contributes `allocated / nlink` per link, which is
  stateless (works for incremental rescans) and sums to the right total when
  all links are on the scanned volume. The division rounds down, so a file
  with three links can be a few bytes short.
* A directory's own inode contributes nothing. APFS reports no allocated
  blocks for directories (their entries live in the file-system tree), and
  what a directory holds is counted through its entries.
* APFS clones and snapshots cannot be detected from metadata. Sums can exceed
  physical usage, and removing a clone may free nothing. All sizes are shown
  as estimates; only the free-space delta measured after a cleanup is called
  "freed".
* Dataless (cloud placeholder) directories are not descended into, so a scan
  never triggers a download.
* **Scan scope, as data.** A scan stays on its root's device and records,
  without descending, every directory on another device (another volume, a
  disk image, a network share, `/dev`, autofs), every dataless placeholder
  and every path on the scope's exclusion list. These are *skips*: counted
  in coverage, but they do not make a scan partial. Only *gaps* (denied,
  failed, vanished, cancelled) do.
* **The root `/`.** On current macOS the sealed system volume and the data
  volume form a volume group. Files on both report the data volume's
  device; the data volume appears inside `/` through firmlinks (`/Users`,
  `/Applications`, `/Library`, `/private`, ..., listed in
  `/usr/share/firmlinks`) and is mounted a second time at
  `/System/Volumes/Data`. Walking both would count the data volume twice;
  skipping the second mount would miss what only it holds (the Spotlight
  index, `.fseventsd`, document revisions, update staging). So the scope
  walks `/System/Volumes/Data` but excludes each firmlink target inside it
  whose firmlink source lies under the root (a scan of `/System` keeps
  `/System/Volumes/Data/Users`). Without a firmlink list such a second
  mount is skipped whole. The other `/System/Volumes/*` (VM, Preboot,
  Update, ...) are separate devices and are skipped, like `/Volumes/*`.
  `/.vol`, `/.nofollow` and `/.resolve` are views of the file system rather
  than directories and are excluded. The policy is `ScanScope::plan`, a pure
  function of the root, its device, the mount table and the firmlink list.
* **Unaccounted.** For a volume root the CLI reports used space (total −
  available, as Finder counts it) minus the scanned total. The difference
  holds the container's other volumes, APFS snapshots, purgeable space,
  metadata and gaps; it is negative when clones are counted more than once.

## 5. Index

SQLite in WAL mode at `~/Library/Application Support/Metla/index.sqlite`,
using the system `libsqlite3`.

* **One writer.** A single thread owns the write connection and receives
  commands through a bounded channel: start a scan, rows, finish the scan,
  add or remove an exclusion. `IndexWriter::open` starts it; a scan is a
  session (`start_scan(&mut self) -> ScanWriter`), so only one scan can be
  written at a time. Readers use their own connections.
* **Directories only.** One row per directory with its own and subtree
  totals. Files are not stored; when the user expands a directory its files
  are listed live by one `readdir` off the UI thread. A Mac with millions of
  files needs hundreds of thousands of rows, not millions, and memory use
  stays bounded by the page cache.

```
dirs(id, parent_id, name BLOB, own_bytes, own_files,
     tree_bytes, tree_files, tree_dirs, tree_mtime,
     flags, device, inode, rule_id TEXT)
meta(key, value)        -- schema version, root, volume UUID, last FSEvents id,
                        -- last full scan start/finish, completeness
coverage_counts(code, count)                  -- gaps and skips by kind
coverage_gaps(rank, depth, path BLOB, flags)  -- the shallowest gaps
kinds(code, bytes)      -- the picture's bytes per file kind (whole scan)
exclusions(path BLOB)   -- user "keep this" list
stale(dir_id)           -- directories cleaned since the scan
```

A candidate is a `dirs` row with a non-null `rule_id`; its size is the row's
`tree_bytes`. There is no second copy of the number. File kinds are totals
of the whole scan, stored with the picture, not per directory.

Schema version 2 (M2) adds `device`, `inode` (the directory's identity when
listed, stored with their bit pattern) and `rule_id` (the stable rule id)
to `dirs`, the partial index `dirs_candidates ON dirs(rule_id) WHERE
rule_id IS NOT NULL`, and the `kinds` and `exclusions` tables. Opening a
file whose `schema_version` is not 2 (an M1 index included), or that has
no Metla metadata, fails with `IndexError::UnknownSchema` instead of
misreading it; the CLI's indexes are disposable and are rebuilt by a scan.

Schema version 3 (M3) adds `dirs.verdict` (Git's verdict on project
output: 0 `Verified`, 1 `NotIgnored`, 2 `HasTrackedFiles`, 3
`Unverifiable`; NULL until Git was asked), the `stale` table and the
picture's home directory in `meta`. Verdicts (`IndexWriter::set_verdicts`)
and cleaned directories (`mark_cleaned`) name directories of one picture
and carry its `PictureId`; the writer applies them only while that picture
is current. A worktree's status is not stored: it is computed with Git
when a preview is made and again before removal.

* **Rows.** Names are stored as raw bytes. `own_bytes`/`own_files` are NULL
  for a directory that was not read. `flags` holds the directory's state:
  0 when read, a gap code (1 denied, 2 failed, 3 vanished, 4 cancelled) or a
  skip code (16 other volume, 17 mount point, 18 dataless, 19 excluded) in
  the low byte, and for a failure its errno above it. The codes are part of
  the file format and are pinned by a test.
* **Atomic replacement.** `IndexWriter::start_scan` creates a staging table
  `dirs_next` and increments `scan_seq` in one committed transaction before
  any row is written. Rows arrive in batches over a bounded channel and are
  inserted, one transaction per batch, by the writer thread, which owns the
  only read-write connection. `finish` drops `dirs`, renames `dirs_next`,
  builds the indexes, stores the coverage and sets `picture_seq =
  scan_seq`, all in one transaction. A reader runs its queries inside one
  read transaction (`IndexReader::read`), so it sees the old picture or the
  new one, never a mix.
* **Completeness is derived, not stored**: the index is complete when
  `picture_seq = scan_seq` and the picture has no gaps. A scan that was
  cancelled finishes as a partial picture; a scan that was interrupted (the
  writer dropped, the process killed) never sets `picture_seq`, so the old
  picture stays readable and the index reads as incomplete.
* WAL with `synchronous = NORMAL`; automatic checkpoints only (they never
  wait for readers) and `journal_size_limit` to cut the WAL back after
  them.
* **Candidate queries** read only candidate rows (`INDEXED BY
  dirs_candidates`), largest first, inside one read transaction: tallies per
  rule and per category (every category present), one page of a category,
  and one candidate by id. Exclusions are applied while reading — a
  candidate's path is rebuilt from its parent links with one prepared
  statement and checked with `Exclusion::hides` — so changing an exclusion
  needs no rescan. `preselected` is computed from `tree_mtime` and the
  `Recency` the caller passes; it is never stored. A candidate is offered
  only if no exclusion hides it, it is not at or below a directory cleaned
  since the scan, and — for project output — Git verified it.
  `Snapshot::reclaimable` sums the preselected candidates ("can be
  freed"); `Snapshot::unverified` lists the project output Git has not
  been asked about.
* **After a cleanup** the caller records every item whose outcome may
  have changed the disk (anything but `Skipped`) with `mark_cleaned`; the
  index stops offering those directories and everything below them until
  the next scan replaces the picture. Ancestors' totals are left as they
  were until then (M4's incremental rescans will refresh them); this is the
  simplest option that never advertises a removed candidate.

## 6. Scanning

* A recursive parallel walk on a dedicated thread pool. Each directory is
  read once; children are processed by the pool; a directory's row is
  emitted when its subtree is complete (post-order), so totals are final
  when written and memory is bounded by the active recursion.
* **Progressive results.** Shallow directories (depth ≤ 3) have live atomic
  counters that descendants add to, so the UI shows growing numbers during
  the first scan while deep rows are still being written.
* **Classification is free.** The walker already has each directory's entry
  names; the rule engine is a pure function of those names and the path. The
  only extra I/O is the bounded read of a `.git` pointer file or a
  `CACHEDIR.TAG` where a directory holds one (outside candidates); measured
  in PERFORMANCE.md.
* Errors (`EACCES`, `EPERM`, `ENOENT` races, I/O errors) mark the directory
  and are aggregated into `Coverage`. A scan that met them is "partial".
* Cancellation is cooperative (checked per directory). Parallelism and I/O
  priority come from the *power profile*.

How the walk is built (M1):

* Every directory is one task on a rayon pool built per scan. A task opens
  its directory with `openat` relative to the parent's descriptor
  (`O_NOFOLLOW`), lists it (`readdir` + `fstatat(AT_SYMLINK_NOFOLLOW)`),
  and spawns one task per subdirectory; nothing recurses on the stack, so
  depth is limited only by descriptors and memory. Opening relative to the
  parent also works where the full path is longer than `PATH_MAX`.
* A parent's descriptor stays open while some child task has not opened
  itself yet, so descriptors in use are about threads × depth; the scan
  raises the soft `RLIMIT_NOFILE` (up to `OPEN_MAX`) because the default
  of 256 is too low for that.
* A directory's row is emitted when its last child has been delivered;
  then its totals are delivered to its parent, iteratively up the chain.
  Rows go to the caller's sink in batches of 1024.
* An unreadable directory, a cancelled task and a skipped subdirectory each
  become a row with no own totals and a reason, so the picture is always a
  whole tree.
* `ScanControl` is shared with the caller: a cancel flag, atomic progress
  counters, and the live tree (directories with depth ≤ 3, at most a given
  number, each with counters every descendant adds its own totals to).
* The walker hands an optional observer the *directory facts* of every
  directory it reads, before its subdirectories are read: path, entries,
  the marker entries of the parent and of the grandparent, whether an
  ancestor contains `.git`, and the probed heads of `.git` / `CACHEDIR.TAG`.
* With a rule set (`ScanOptions::rules`) the walker classifies each read
  directory from those facts and records the rule in the row. What a
  directory passes to its children travels down the way totals travel up:
  its marker entries (`RuleSet::is_marker`, kept only outside candidates),
  whether it is in a repository, whether it is or lies inside a candidate
  other than a linked worktree (then nothing below is classified or
  probed; sizes are still counted),
  and its file-kind *area*. Files are totalled per kind into the scan
  summary. The scan root inherits the area of the directories above it but
  not their repository or candidate status.
* System calls that fail with `EINTR` (`open`, `openat`, `fstatat`,
  `fstat`, `readdir`, `read`) are repeated instead of turning the
  directory into a gap.

## 7. Watching and incremental refresh

* One FSEvents stream on the root, directory granularity, with a latency of
  a few seconds. The last processed event id and the volume UUID are stored
  in `meta`. On the next launch the stream resumes from that id, so changes
  made while Metla was not running are replayed instead of rescanning.
* The **coalescer** is a pure state machine: events in, due rescan tasks
  out. It
  * merges events per directory and drops children of a recursive task;
  * waits for a directory to be quiet (*settle time*) and backs off when a
    directory keeps changing, with a maximum latency, so an active build is
    not rescanned continuously;
  * collapses many dirty siblings into one recursive task and, past a hard
    cap, into "needs full rescan" — its memory is bounded;
  * turns `MustScanSubDirs`, dropped-event flags, id wrap and a changed
    volume UUID into the corresponding recursive or full rescans.
* The engine processes due tasks in budgeted slices and yields between them.
* A shallow rescan re-reads one directory and applies the delta to its
  ancestors. Added subdirectories are walked; removed ones are deleted.
* Metla ignores events under its own data directory.

### Power profiles

| Profile       | When                              | Threads | Priority                | Tick / settle |
|---------------|-----------------------------------|---------|-------------------------|---------------|
| `Interactive` | user pressed Scan                 | cores−2 | user-initiated          | n/a           |
| `Background`  | on AC power                       | 2       | utility                 | short         |
| `Saver`       | on battery or Low Power Mode      | 1       | background, throttled I/O | long        |
| `Off`         | background analysis disabled      | 0       | —                       | no watching   |

The Swift side observes power state and tells the engine; the engine owns
what each profile means.

## 8. Rules

See [RULES.md](RULES.md) for the catalogue. Principles:

* The catalogue is data (`rules/catalogue.rs`); `RuleSet::new(home, user)`
  compiles it for one home directory, which is always passed in. The
  module is pure: no file system, no environment, no clock.
* Known tool locations are matched by exact path patterns under the home
  directory (for example `~/Library/Caches/Homebrew`), compiled into a
  component trie: a directory costs one map lookup per path component, and
  paths deeper than the deepest pattern are rejected without a lookup.
  Literal components win over `*`; a last `*` never claims a name another
  pattern spells out; per-rule deny lists; `[a|b]` conditions one or two
  levels up.
* Project build output is matched only when **all** hold: the directory name
  matches, the rule's marker is present (for example a sibling `Cargo.toml`
  or an inner `CACHEDIR.TAG` with the standard signature), the directory is
  inside a Git repository, it is not inside an installed package tree
  (below `node_modules`, `site-packages`, `dist-packages` or `Cellar`), and
  — checked again before removal — Git reports the path as ignored.
* A linked worktree is recognised from its `.git` pointer file alone.
* After every scan the caller asks Git about all project output not yet
  judged (`git::verify_outputs`: directories grouped by repository — the
  nearest ancestor holding `.git` — and one `check-ignore --no-index
  --stdin -z` plus one `ls-files -z` batch of at most 512 paths per
  repository) and stores the verdicts; only `Verified` output is offered.
* Precedence: location, then worktree, then project output; then catalogue
  order.
* A directory named `cache`, `build` or `dist` with no verifiable marker is
  never a candidate.
* Nothing inside a candidate is classified again — except inside a linked
  worktree (`Category::classifies_within`), whose build output is offered
  on its own because most worktrees cannot be removed whole. A preview
  drops an item nested in another item of the same preview.
* A user *exclusion* removes a path, everything under it and every
  candidate that holds it from the candidates.

## 9. Cleaning

`clean::Cleaner` makes previews and executes each at most once. It is made
for one root; without the release app's `Production` capability that root
must hold a `.metla-fixture` regular file (§12). The root's device and
inode, and the marker, are checked again every time it executes.

1. **Preview.** `preview(candidates, now)`: duplicates are dropped; a
   worktree whose status (computed now with Git) is blocked is set aside
   with its reasons; an item inside another item of the preview is set
   aside with the id of the one that holds it. Items carry warnings
   (recently active build output; inside a managed worktree, which must be
   confirmed by itself) and are listed largest first; totals per group.
2. **Execute.** `execute(id, confirmation, context, control, trash)`: an
   unknown or already executed preview id is an error, not a removal. Per
   item, in order: cancelled → `Skipped(Cancelled)`; not covered by the
   confirmation → `Skipped(NotConfirmed)`; then revalidation; then the
   action. Free space is measured (`statfs`) before and after.
3. **Revalidate each item right before acting** (a failed check changes
   nothing): the path lies inside the root (no `..`); no exclusion in force
   hides it (itself, beneath it or above it); every component from the
   root down is opened relative to its parent without following a symlink
   and must be a directory (`AncestorChanged` otherwise); the item is still
   a directory (`Symlink`, `NotADirectory`), on the scanned device
   (`MountPoint`: another volume is there) with the scanned inode
   (`Replaced`), checked on the name and again on the opened descriptor;
   its rule still matches the facts read now — its entries, the parent's
   and grandparent's markers, whether a directory from the root down holds
   `.git`, the `.git` and `CACHEDIR.TAG` probes (`RuleMismatch`); project
   output is still ignored and holds nothing tracked (`NotIgnored`,
   `HasTrackedFiles`, `Unverifiable`). A vanished item or ancestor is
   `AlreadyGone`; one that cannot be read is `Unreadable(errno)`. The
   opened descriptor is what the action works on.
4. **Act.**
   * `DeletePermanently`: `fs::remove_contents` empties the opened
     directory with `openat(O_NOFOLLOW|O_DIRECTORY)` and `unlinkat`,
     iteratively; symlinks are unlinked, never followed; a directory on
     another device is left and reported; nothing is chmodded — an entry
     that cannot be removed is recorded and the rest continues (`Partial`
     with the count, the first 16 paths with their errno, the volumes
     left); cancellation is checked before every entry; removed bytes use
     the scan's hard-link share rule. Then the candidate directory itself
     is removed — only if nothing stayed and its rule does not keep it
     (RULES.md, "Keeps").
   * `MoveToTrash`: through the `TrashHost` the caller supplies (the app:
     `FileManager.trashItem`, so "Put Back" works; tests and the CLI: a
     fixture's own trash directory) — the directory, or its entries when
     the rule keeps it. `MovedToTrash` counts in found, not in removed; a
     host refusal is `Failed`.
   * `GitWorktreeRemove`: the tree is surveyed (bytes, newest change), the
     status is recomputed with that fresh activity, and only `Removable`
     proceeds, through `git worktree remove` in the main repository; Git's
     refusal is `Failed` with its message. Removed bytes are the survey
     before minus a survey after.
5. **Report.** Per item: found (the estimate), removed (measured as
   unlinked), outcome. Overall: found, removed, moved to the Trash, and
   freed — the measured change in free space, which may differ either way.

Running a cleanup twice, or again after a crash, converges: missing items
are `AlreadyGone`, never an error; a half-emptied directory is emptied; a
kept directory that is already empty is `Removed` with 0 bytes.

### Git worktrees

Discovered during the scan (a directory whose `.git` is a file pointing into
`…/.git/worktrees/<name>`). `git::inspect_worktree(path, activity, now,
settings)` decides with stock Git commands, run with
`GIT_OPTIONAL_LOCKS=0`, `LC_ALL=C`, no pager, no prompts, a time limit that
also bounds collecting the output, and the binary injected
(`Git::system()` calls the real binary behind macOS's `/usr/bin/git`
shim, which costs about 40 ms a call). It collects every reason at once:

* the repository from the `.git` pointer; exactly one entry of `git
  worktree list --porcelain -z` (run in the repository) is this directory
  after canonicalisation, and not the main worktree — else `NotListed`;
* that entry's `locked` and `prunable`;
* `rebase-merge`, `rebase-apply`, `MERGE_HEAD`, `CHERRY_PICK_HEAD`,
  `REVERT_HEAD` or `BISECT_LOG` in the worktree's git directory —
  `OperationInProgress`; stash entries are repository-wide and not a
  reason;
* `index.lock` there, or the newest of the tree's activity and the git
  directory's `index`, `HEAD` and `logs/HEAD` within the staleness age
  (default 30 days; unknown activity counts as recent) — `InUse`;
* `git status --porcelain=v2 --branch --untracked-files=all -z`: staged,
  modified or conflicted — `Uncommitted`; untracked — `Untracked`;
* ahead of the upstream, or any commit reachable from HEAD and from no
  remote ref (`rev-list --count HEAD --not --remotes`, which also covers a
  detached HEAD, a missing or local upstream, and a repository without
  remotes) — `Unpushed`;
* Codex Desktop and Claude Code worktrees — `Managed(…)`;
* any Git failure or unparsable output — `InspectionFailed`.

Only a worktree with no reason is removable. There is no force path.

## 10. The Rust/Swift boundary

* UniFFI (proc-macro mode). Domain types derive the UniFFI traits behind the
  `ffi` cargo feature, so each type is defined once and the core builds and
  tests without UniFFI.
* `metla-ffi` exports one object, `Engine`, plus two foreign-implemented
  traits: `EngineObserver` (coalesced change notifications) and `TrashHost`.
* Calls are synchronous and short on the Rust side. Swift wraps the engine
  in an actor; nothing is called on the main thread. Long work (scan,
  cleanup) runs on engine threads and reports through the observer.
* Observer events carry no data, only "what changed"; the UI then asks for
  the snapshot it shows. This keeps one source of truth and makes dropped or
  merged notifications harmless.

## 11. The app

* `NavigationSplitView` main window: Overview, cleanup categories, Folders,
  last report. Standard controls, light and dark, full keyboard navigation.
* `MenuBarExtra` panel: disk gauge, reclaimable estimate, scan state and
  freshness, quick actions.
* Settings scene: General (background analysis, battery behaviour, launch at
  login), Hotkeys (recorders, confirmation mode, quick categories),
  Exclusions, Permissions (Full Disk Access).
* Global hotkeys use `RegisterEventHotKey`, which needs no Accessibility
  permission.
* Without Full Disk Access the app says so, skips protected locations
  rather than triggering a prompt per folder, and marks the analysis
  partial. It never asks for root.

## 12. Development safety

* Deletion is only ever exercised on synthetic trees under a temporary
  directory. A `Cleaner` refuses a root without a `.metla-fixture` regular
  file unless it is given `clean::Production`, which only
  `Production::release_app()` makes, and that function exists only in a
  release build (`not(debug_assertions)`) with metla-core's `production`
  feature. The CLI never names it (a test reads its sources), so
  `metla-cli clean` works only on fixtures; `metla-cli fixture create
  <dir>` builds one.
* The app honours `METLA_ROOT` and `METLA_DATA_DIR` so it can be driven
  against a fixture with a separate index.
* Scanning the real disk is read-only; real paths never go into the
  repository, test fixtures or screenshots.

## 13. Quality gates

Rust: `cargo test`, `clippy -D warnings`, and the ub-mode tools — crapper
(every function CRAP < 10), dryer (no duplication ≥ 0.8), mutator (no
uncovered sites, no survivors), uml-viewer (dependency rule). These tools do
not understand Swift. Swift is checked with the Swift 6 compiler in strict
concurrency mode, warnings as errors, `swift test` and llvm-cov coverage of
`MetlaKit`; this boundary is stated in every quality report.
