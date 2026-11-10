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
`metla-cli` reaches past the engine only for low-level commands kept for
measurement and inspection (`scan`, `top`, `ls`, `candidates`, `summary`,
`exclude`) and for `clean`, which still orchestrates a fixture cleanup
itself; `watch` and `bench` run the engine. `clean` does not read the
index: its caller passes the candidates, the exclusions in force and the
time in, and records the outcome in the index itself. The `watch` module
is split: the coalescer and the resume decision are pure and use
`domain` only; `watch::sys` holds every foreign declaration of the
FSEvents stream.

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
`Verdict` (Git's word on project output), `Reclaimable` (what cleaning
the preselected candidates would free, and what it would only move to
the Trash), `Exclusion` (an absolute
path; `hides(path)` is the one place exclusion semantics live), `FileKind`
and `KindTotals` (bytes per kind for a whole scan),
`Coverage` (what the analysis could not see), `Passed` (what a read
directory hands down to the classification below it), `Stamp` (an
inode's change time in nanoseconds), `RescanTask` (`Shallow(dir)`,
`Recursive(dir)`, `Full`), `Rescanned` (one rescan's rows and removals),
`WorktreeStatus` (`Removable { branch }`
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
  are listed live by one listing off the UI thread (`Engine::children`):
  the entries stream through a bounded heap that keeps the 10,000
  largest files (`LISTED_FILES`; of equal sizes the names that sort
  first), and the rest are one entry with their count and bytes
  (`FolderEntry::MoreFiles`, "N more files", always last). Only the
  subdirectories up to the page's end are read from the index. A Mac with millions of
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

Schema version 4 (M4) keeps in each row what its directory needs to be
rescanned on its own: `mtime` and `changed` (the directory's own
modification time and ctime in nanoseconds, as listed in its parent),
`own_mtime` (its newest file), `kinds` (its own bytes per file kind:
for every kind with bytes, a code byte and the bytes as LEB128), and
what it hands down to the classification below it — `flags` bit 8 in a
repository, bit 9 in a candidate, bits 10-11 its area, and `markers`
(the marker names, each followed by a NUL byte). Both blobs are NULL
when empty. It adds the partial index `dirs_unread` on the rows that
were not read, and `meta` keys `next_id` (ids are never given out twice
within a picture), `picture_updated` (the last in-place update) and
notes (`note:<key>`, the engine's own records: the last applied event
id with the volume's FSEvents UUID, the settings), kept apart from the
index's keys. An index of another schema version is replaced by the
engine (it holds nothing a scan cannot make again); a file that is not
an index is refused.

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
  the low byte, for a failure its errno above it, and for a read
  directory what it hands down (above). The codes and blobs are part of
  the file format and are pinned by tests.
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
  picture stays readable and the index reads as incomplete. A scan whose
  rows could not all be written, or that could not be swapped in, is
  abandoned by the writer before it answers: the staging table goes and
  the index reads as if the scan never began.
* WAL with `synchronous = NORMAL`; automatic checkpoints only (they never
  wait for readers) and `journal_size_limit` to cut the WAL back after
  them.
* **Space.** Schema version 5 (M5) makes the file with
  `auto_vacuum = INCREMENTAL`. While a scan runs the file holds the old
  picture and the staging table; after the swap, and after an abandoned
  scan, the writer gives the free pages back to the disk
  (`incremental_vacuum` in steps of 4,096 pages, each its own
  transaction, then a passive checkpoint), so at rest the file holds one
  picture. The pages of the new picture written past the old one are
  moved into the old one's place, so a scan writes up to a picture's
  size a second time (measured in PERFORMANCE.md). Running out of space
  (`SQLITE_FULL`, on a full disk or past `IndexWriter::open_limited`'s
  limit, which tests use with `max_page_count`) is
  `IndexError::NoSpace`.
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
  `Snapshot::reclaimable` sums the preselected candidates by what
  cleaning them does: `freed` (deleted permanently or removed by Git:
  "can be freed") and `trashed` (moved to the Trash: shown apart, "not
  freed until the Trash is emptied"); `Snapshot::unverified` lists the
  project output Git has not been asked about.
* **After a cleanup** the caller records every item whose outcome may
  have changed the disk (anything but `Skipped`) with `mark_cleaned`; the
  index stops offering those directories and everything below them until
  they are rescanned. The engine then rescans each cleaned item's parent
  shallowly: the cleaned child's stamp moved (or it is gone), so it is
  walked again, its row replaced and its `stale` mark dropped, and every
  total above it is recomputed.
* **In-place updates.** `IndexEditor::apply(picture, Rescanned, now)`
  applies one rescan in one transaction, guarded by the picture id:
  rows of the subtrees that went are deleted (with their cleaned marks;
  the subtree is walked through `dirs_by_parent` and each row looked up
  by id, never by scanning the table),
  re-read rows replace the row with their id, new rows are inserted;
  then the tree totals of every directory above the rescanned one are
  recomputed from its own totals and its children's (a maximum such as
  the newest time cannot be updated by a difference), the bytes by kind
  are adjusted by the removed and added rows' own kinds, and the
  coverage is counted again from the unread rows (`dirs_unread`; a gap's
  depth and path from its parent links). New rows have no verdict.
* **One writer, several handles.** `IndexWriter` keeps the scan session;
  its `IndexEditor` (cloneable) sends edits from other threads to the
  same writer thread: in-place updates, exclusions, verdicts, cleaned
  directories, notes. Dropping the writer stops the thread; editors
  then get `WriterGone`. `ScanWriter::abandon` drops a scan's staging
  table and makes the picture current again.

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
  `fstat`, `readdir`, `read`, `getattrlistbulk`) are repeated instead of
  turning the directory into a gap.

M4 changes:

* **Listing with `getattrlistbulk`.** A directory is listed with
  `getattrlistbulk` (name, error, device, type, modification and change
  time, flags, file id; link count and mount status for directories;
  link count and allocated data size for everything else), packed with
  `FSOPT_PACK_INVAL_ATTRS` and read by the returned attribute set. A
  mount point (the call describes the directory underneath), a firmlink
  (it describes the link, not its target), an entry with an error of its
  own and an entry with an attribute the call could not give are asked
  with `fstatat`, exactly as before, so their errors are classified as
  the classic listing classifies them; a file system without the call
  (`ENOTSUP`) is listed with `readdir` and `fstatat`, which stays
  available (`ScanOptions::classic`, `scan --classic`) for comparison. A
  directory's own link count is not used and is one in both listings.
  Reading a record is a pure function (`fs::decode_bulk`), tested on
  records packed as the kernel packs them; tests also compare both
  listings field by field on synthetic trees and on `/` and `/Volumes`.
* **Origins.** A walk starts at an origin: the scan root, or one
  directory of a picture, which inherits from the stored rows above it
  what an unbroken walk would have handed down (`scan::Inherited`: its
  parent's `Passed` and its grandparent's markers). The root's times are
  read (`lstat`) when each scan starts.
* **Rescans.** `scan::rescan(target)` opens the target from the root
  down without following a symlink and reads its metadata as listed in
  its parent; a target that is no longer a directory there is reported,
  and its parent is rescanned instead. A *recursive* rescan walks the
  subtree again (the target keeps its id; everything below gets new
  ids). A *shallow* rescan re-reads the directory and compares it with
  its stored row and children: if it now hands down something else (its
  rule, markers, repository, candidate or area changed) every child is
  walked again; otherwise a child that is the same directory (device,
  inode) with the same change stamp and modification time, read or
  skipped for the same reason as before, keeps its stored subtree, and
  only new, replaced, unread or changed children are walked. The change
  stamp (ctime, nanoseconds) moves with every change of a directory's
  entries, permissions or owner, so a permission change is seen even
  when the event names only the parent, and changes within one second
  are not missed. A rescan's rows are held in memory until they are
  written in one transaction; a recursive rescan of more than
  `MAX_SUBTREE` (100,000) directories, or of the root, is a full scan
  instead.
* **Throttling.** `ScanOptions::throttled` runs the scan threads at
  background priority (`PRIO_DARWIN_BG`: CPU and I/O yield to everything
  else).

## 7. Watching and incremental refresh

* **The stream** (`watch::sys`, every FFI declaration of the module):
  one FSEvents stream on the root, directory granularity, `WatchRoot`,
  not `NoDefer`, with the profile's latency, delivered on a serial
  dispatch queue of its own. A profile of another latency replaces the
  stream with one that replays from the id after which nothing is
  pending, so nothing the old stream held back is lost. On `/` an event in a firmlinked area is
  reported by its path under `/`, which is why the coalescer may ignore
  the firmlink targets inside `/System/Volumes/Data` that the scan scope
  excludes.
* **Resuming.** After every batch the engine stores the id after which
  nothing is pending (`Coalescer::settled`, or the id the stream started
  after when no event came) with the volume's FSEvents UUID, as an index
  note. On opening, and whenever watching starts again, the stream
  starts after that id when `watch::resume_after` allows it: the same
  UUID, and an id not ahead of the current one. Otherwise — or without a
  picture, or when the picture's latest scan never finished — the engine
  needs a full scan, and says so in the first snapshot.
* **The coalescer** is a pure state machine (`watch::Coalescer`): events
  `{ path, flags, id }` at a time passed in, due `RescanTask`s out. It
  * keeps events per directory; a change is due after a *settle time* of
    quiet; each later event (events handed over together count once)
    restarts the wait and multiplies the settle time by the factor, up to
    the cap and never below the profile's own settle time (so a profile
    change retimes what waits), and nothing waits longer than one cap
    after its first event,
    so a directory that never stops changing is rescanned once per cap; a
    directory rescanned less than a cap ago starts from its last settle
    time multiplied again (remembered for as many directories as are
    tracked);
  * makes `MustScanSubDirs` a recursive task, which absorbs everything
    below it; turns more than `collapse` changed children of one
    directory into one recursive task for it; past `max_dirs` tracked
    directories collapses the deepest into their parents; a recursive
    task at the root, lost events (`UserDropped`, `KernelDropped`),
    wrapped ids and a changed root are one full rescan, which replaces
    the rest;
  * re-reads the directory holding a mount point when a volume is
    mounted or unmounted;
  * drops events outside the root, inside the engine's data directory
    and inside the scope's excluded paths;
  * hands out at most a batch of due tasks at a time, deepest first, and
    says when the next one is due (`next_deadline`); `Off` forgets
    everything and hands out nothing.
* **Applying a task** (`engine::refresh`): the task's directory is found
  in the picture (one the picture does not have yet is found by a
  shallow rescan of its nearest ancestor that it has; one that was not
  read is walked whole), rescanned with what the picture says about it,
  and the change applied in one transaction. Applying the tasks produced
  by any sequence of changes leaves exactly what a full scan of the
  changed tree writes — rows, totals, bytes by kind, coverage and
  candidates (a seeded property test). Events inside ignored paths are
  the exception, by design.

### The engine

`engine::Engine::open(EngineConfig { root, home, user, data_dir, git,
production, trash, watching, profiles, profile })` opens (or replaces)
the index in `data_dir` and answers `snapshot()` at once from the saved
picture. API calls read through a connection of their own and return at
once; everything that changes the picture runs on one scheduler thread,
one thing at a time:

* full scans: the user's (`start_scan`) with the `Interactive` profile's
  threads, the engine's own when it needs one and the profile allows
  it; a cancelled scan with a picture to fall back on is given up (the
  picture stays current), the first one becomes a partial picture; after
  the user cancels, no full scan starts on its own until the user starts
  one;
* batches of due tasks, the profile's pause apart; a task that needs a
  full scan makes one;
* cleanups (`execute_confirmed`, `quick_clean`): nothing else runs
  meanwhile; then the cleaned directories are marked and their parents
  rescanned shallowly;
* after each of these: Git's verdicts on project output that has none
  yet (the only rows without one are new or re-read), worktree statuses
  where something changed, the stored event id, and a notification.
  After a batch or a cleanup's rescans both passes look only at what it
  wrote: the candidate rows a rescan wrote (`Refreshed::Applied`) and
  the worktrees holding a rescanned directory (`Snapshot::enclosing`);
  only a full scan has them look at every row.

The scheduler sleeps on its command channel until the coalescer's next
deadline; with nothing due it does not wake up (`Counters::wakeups`).
The observer is told what changed (`Changed::{Picture, Activity,
Worktrees, Cleanup, Settings, Exclusions}`), never the data.

`snapshot()` answers the Overview; `freshness()` only how fresh the
picture is (it reads no candidate), for a status line that follows
every batch. `Freshness` is explicit: the last full scan, the last update, pending
changes, `EngineState` (`Idle`, `Scanning { progress, interactive }`,
`Updating { pending }`, `Cleaning`, `Paused(profile)` when a needed full
scan waits for the profile, `Failed(reason)`, `Off`), the picture's
completeness (`Missing`, `Partial`, `Complete`), whether changes are
followed, and whether a full scan is needed; `is_current()` holds only
when all of them say so.

A full scan or an update that fails keeps the picture as it was and
records why (`Failure::NoSpace`: "not enough disk space to refresh";
`Index`; `Scan`); the state is `Failed(reason)` once nothing else is
due, and no full scan starts on its own until the user starts one
(which forgets the failure), so a full disk never makes the engine scan
again and again. `EngineConfig::index_limit` caps the index file.

Worktree statuses for lists are computed after a scan and after changes
below a worktree on a background-priority thread, two Git calls at a
time, abandoned when a newer request comes or the engine closes, kept
per picture and served with the candidate page (`Removability::{Unknown,
Removable, Blocked}`). They are for display only: a preview and an
execution compute them again.

Settings (background analysis, battery behaviour, ages, hotkey mode,
quick categories) are kept as an index note, one `key=value` line each.

### Power profiles

One table, `Profiles::standard(cores)`; the app picks the profile from
the power state, the engine owns what each means. The thread counts
come from the thread sweep in PERFORMANCE.md.

| Profile       | Threads | Latency | Settle / cap | Batch | Tracked dirs | Pause | Starts full scans | Priority |
|---------------|---------|---------|--------------|-------|--------------|-------|-------------------|----------|
| `Interactive` | cores−2, at most 8 | 1 s | 1 s / 30 s | 256 | 4,096 | none | yes | normal |
| `Background`  | 2       | 15 s    | 30 s / 300 s | 1,024 | 4,096        | 60 s  | yes               | normal   |
| `Saver`       | 1       | 15 s    | 60 s / 900 s | 256   | 1,024        | 180 s | no (`Paused`)     | background, throttled I/O |
| `Off`         | 0       | —       | —            | —     | —            | —     | no                | — (nothing is watched) |

The pause between batches sets how often Background and Saver change
the picture: about once a minute and every three minutes. Their streams
hold events back for 15 s; macOS was measured to deliver every 15 s or
so whatever longer latency is asked for (30 s and 600 s alike), so the
table does not promise more. While nobody looks, a change on disk can
therefore wait up to a latency plus a pause before it is applied, and
`pending` counts only what has been delivered.

**Attention.** `Engine::set_attention(true)` says someone looks (the
app's window or panel is visible): changes are followed as
`Interactive` follows them (its latency, settle times, batches and
threads), what waits is due at once (`Coalescer::expedite`), and the
pause is forgotten; full scans the engine starts on its own still
follow the power profile. `set_attention(false)` returns to the power
profile. Attention watches nothing that `Off` leaves alone.

Every profile multiplies the settle time by 2 per event and collapses
more than 32 changed children. `Off` stops watching at once and cancels
a full scan the engine started on its own (never the user's); turning
it on again resumes the stored history or asks for a full scan. With
background analysis switched off in the settings the profile in force
is `Off`.

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
* any commit reachable from HEAD and from no remote ref (`rev-list
  --count HEAD --not --remotes`), which covers commits ahead of a remote
  upstream, a detached HEAD, a missing or local upstream, and a
  repository without remotes — `Unpushed`;
* Codex Desktop and Claude Code worktrees — `Managed(…)`;
* any Git failure or unparsable output — `InspectionFailed`.

Only a worktree with no reason is removable. There is no force path.

**Trust model.** Metla runs Git in repositories it did not make, so a
repository must not be able to make Metla run anything. Git is called
only with stock commands (`worktree list`, `status`, `rev-list`,
`check-ignore`, `ls-files`, `worktree remove`), never with `--force`,
never with a command that talks to the network (no fetch, push or
remote query), and every run starts with `--no-pager -c
core.fsmonitor=false`: a repository's configuration can name a program
for `git status` to start (`core.fsmonitor`), and that is switched off.
Metla never sets `safe.directory`; a repository Git refuses because
another user owns it stays `InspectionFailed` (a worktree) or
`Unverifiable` (project output), so nothing in it is offered.

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
* The engine can run on the real root for measurements (scan, watch,
  snapshot): without the `Production` capability its cleaner refuses a
  root without the fixture marker, and the CLI's engine moves nothing to
  the Trash. `bench churn` and `bench refresh` write only into a root
  marked as a fixture.
* Scanning the real disk is read-only; real paths never go into the
  repository, test fixtures or screenshots.

## 13. Quality gates

Rust: `cargo test`, `clippy -D warnings`, and the ub-mode tools — crapper
(every function CRAP < 10), dryer (no duplication ≥ 0.8), mutator (no
uncovered sites, no survivors), uml-viewer (dependency rule). These tools do
not understand Swift. Swift is checked with the Swift 6 compiler in strict
concurrency mode, warnings as errors, `swift test` and llvm-cov coverage of
`MetlaKit`; this boundary is stated in every quality report.
