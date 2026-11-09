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

Cleanup is requested through two different entry points with different types:

```rust
engine.preview(selection: Vec<CandidateId>) -> CleanupPreview   // what, how much, which disposal
engine.execute_confirmed(preview_id)        -> CleanupReport    // after the user confirmed that exact preview
engine.quick_clean(allowed: Vec<QuickCategory>) -> CleanupReport
```

`quick_clean` cannot be handed a worktree or a log: there is no value of
`QuickCategory` for them. The quick-clean switch in Settings therefore cannot
lift the confirmation requirement for other categories.

Other core types: `Candidate` (directory + rule + size + activity),
`ScanState`, `Coverage` (what the analysis could not see), `Freshness`
(when the picture was last true), `WorktreeStatus`, `CleanupReport`
(`found`, `removed`, per-item outcome, measured `freed`).

## 4. Measuring size

* Only metadata is read (`lstat`/`getattrlistbulk`). File contents are never
  opened, with two tiny exceptions used for verification: a worktree's `.git`
  pointer file and a `CACHEDIR.TAG` signature.
* Size is **allocated size**. Sparse and transparently compressed files are
  therefore counted by what they occupy, not by logical length.
* Symlinks are never followed; a symlink counts as itself.
* A file with `nlink > 1` contributes `allocated / nlink` per link, which is
  stateless (works for incremental rescans) and sums to the right total when
  all links are on the scanned volume.
* APFS clones and snapshots cannot be detected from metadata. Sums can exceed
  physical usage, and removing a clone may free nothing. All sizes are shown
  as estimates; only the free-space delta measured after a cleanup is called
  "freed".
* Dataless (cloud placeholder) directories are not descended into, so a scan
  never triggers a download.
* The walk starts at `/` and stays on the system and data volumes. Other
  mounts (`/Volumes/*`, network, disk images) and `/System/Volumes` are
  skipped and reported as skipped.

## 5. Index

SQLite in WAL mode at `~/Library/Application Support/Metla/index.sqlite`,
using the system `libsqlite3`.

* **One writer.** A single thread owns the write connection and receives
  batches through a bounded channel. Readers use their own connections.
* **Directories only.** One row per directory with its own and subtree
  totals. Files are not stored; when the user expands a directory its files
  are listed live by one `readdir` off the UI thread. A Mac with millions of
  files needs hundreds of thousands of rows, not millions, and memory use
  stays bounded by the page cache.

```
dirs(id, parent_id, name, own_bytes, own_files,
     tree_bytes, tree_files, tree_dirs, tree_mtime,
     flags, rule_id, kinds BLOB)
meta(key, value)        -- schema version, root, volume UUID, last FSEvents id,
                        -- last full scan start/finish, completeness
exclusions(path)        -- user "keep this" list
worktrees(dir_id, status_json, checked_at)
```

A candidate is a `dirs` row with a non-null `rule_id`; its size is the row's
`tree_bytes`. There is no second copy of the number.

## 6. Scanning

* A recursive parallel walk on a dedicated thread pool. Each directory is
  read once; children are processed by the pool; a directory's row is
  emitted when its subtree is complete (post-order), so totals are final
  when written and memory is bounded by the active recursion.
* **Progressive results.** Shallow directories (depth ≤ 3) have live atomic
  counters that descendants add to, so the UI shows growing numbers during
  the first scan while deep rows are still being written.
* **Classification is free.** The walker already has each directory's entry
  names; the rule engine is a pure function of those names and the path. No
  extra I/O happens at scan time.
* Errors (`EACCES`, `EPERM`, `ENOENT` races, I/O errors) mark the directory
  and are aggregated into `Coverage`. A scan that met them is "partial".
* Cancellation is cooperative (checked per directory). Parallelism and I/O
  priority come from the *power profile*.

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

* Known tool locations are matched by exact path patterns under the home
  directory (for example `~/Library/Caches/Homebrew`).
* Project build output is matched only when **all** hold: the directory name
  matches, the rule's marker is present (for example a sibling `Cargo.toml`
  or an inner `CACHEDIR.TAG`), the directory is inside a Git repository, and
  — checked again before removal — Git reports the path as ignored.
* A directory named `cache`, `build` or `dist` with no verifiable marker is
  never a candidate.
* Nothing inside a candidate is classified again.
* A user *exclusion* removes a path (and everything under it) from
  candidates.

## 9. Cleaning

1. **Preview.** Selected candidates become preview items: path, estimated
   size, disposal, rule, warnings. The preview has an id.
2. **Revalidate each item right before acting**: still exists, same
   device/inode as scanned, not a symlink, not a mount point, inside the
   engine root, not excluded, rule still matches (markers re-checked, Git
   ignore re-checked for build output). Anything that fails is skipped with
   a reason.
3. **Act.**
   * `DeletePermanently`: descriptor-relative removal (`openat`/`unlinkat`),
     never following symlinks, never crossing devices; errors are collected
     per entry and the rest continues; cancellable between entries.
   * `MoveToTrash`: delegated to the host (`FileManager.trashItem`) through
     a typed callback, so "Put Back" works.
   * `GitWorktreeRemove`: `git worktree remove <path>` without `--force`.
4. **Report.** Per item: estimated (found) bytes, removed bytes, outcome
   (`Removed`, `Partial`, `Skipped(reason)`, `Failed(error)`,
   `AlreadyGone`). Overall: volume free space before and after. Items moved
   to the Trash are reported as "not freed until the Trash is emptied".

Running a cleanup twice, or again after a crash, converges: missing items
are `AlreadyGone`, never an error.

### Git worktrees

Discovered during the scan (a directory whose `.git` is a file pointing into
`…/.git/worktrees/<name>`). Before a worktree can be offered or removed its
status is computed with stock Git commands:

* exact path match against `git worktree list --porcelain` of the main repo;
* `git status --porcelain=v2 --branch --untracked-files=all`: modified,
  staged, untracked, conflicted files;
* unpushed commits: ahead of upstream, or — without an upstream — commits
  not reachable from any remote ref;
* locked, or an operation in progress (rebase, merge, cherry-pick, bisect);
* recent activity (configurable age, default 30 days);
* managed by another tool: Codex Desktop (`~/.codex/worktrees`) and Claude
  Code (`.claude/worktrees`) worktrees are shown but can never be removed by
  Metla.

Only a worktree that passes every check is removable. There is no force path.

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
  directory. The CLI's cleaning command refuses to run unless the root
  contains a `.metla-fixture` marker.
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
