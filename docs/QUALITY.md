# Quality gates

Two scripts, one per language. Each prints what it checked with numbers
and exits non-zero when a gate fails or a tool analysed nothing.

```
scripts/quality.sh         # Rust: core/ (metla-core, metla-cli, metla-ffi)
scripts/quality-swift.sh   # Swift: app/ (MetlaCoreFFI, MetlaKit, MetlaApp)
```

Run both with `TMPDIR` on a volume with room to spare: the tests write
synthetic trees and indexes there.

## Rust: what the ub-mode tools cover

`scripts/quality.sh` runs, on every crate of the workspace:

* `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test` (the fast suites), and once the slow tests that wait for
  real FSEvents deliveries (`--ignored`);
* **stability**: the fast suite five times in a row must pass five
  times (`STABLE_RUNS`); a failure fails the gate with the test's name.
  The mutator counts any failing run as a killed mutant, so a test that
  fails on its own would kill mutants it has nothing to do with;
* **crapper**: CRAP of every function from its cyclomatic complexity and
  its line coverage (`cargo llvm-cov`); every function below 10;
* **dryer**: structural similarity of every pair of functions; no pair at
  0.8 or above;
* **mutator**: every operator and literal of every source file is
  mutated, the crate's tests run per mutant; no mutant may survive and no
  mutation site may be left uncovered. A mutant's timeout is about
  max(90 s, 3 × a warm run of its crate's tests); the script prints how
  many mutants of each file were killed by that timeout. A run is
  differential: survivors and the sites of functions whose text changed
  run again, the rest keep their verdict from the last run
  (`MUTATE_ALL=1` runs everything);
* **a full disk is not a result**: a test run that dies of `ENOSPC`
  fails, and the mutator counts every failing run as a kill. Before the
  mutator, before each file and after it, the volume of `TMPDIR` must
  have at least 300 MB free (`DISK_FLOOR_MB`), and the per-file mutator
  logs and the test logs must show no full-disk signature (`No space
  left on device`, `Errno 28`, `os error 28`, `ENOSPC`, the index's
  `NoSpace`); otherwise the run stops, says `DISK FULL` and exits 3
  (`scripts/disk-guard.sh`, tested by `scripts/disk-guard-test.sh`,
  which the script runs first).

`metla-ffi` is Rust and gets all of the above. Code that UniFFI's macros
generate is not in the sources the tools read, so it is not measured;
the hand-written conversions, paging and error mapping are. The
`uniffi-bindgen-swift` binary (one line calling UniFFI) is built only
with the `bindgen` feature and has no mutation site.

**Rust numbers at M7.**

* The only complete `MUTATE_ALL=1` run of M7 is the architect's at
  88cd5b2 (a clean clone, 11:30 to 15:24 on 2026-11-11, the machine at a
  load average of 31 to 38): fmt, clippy and the tests (73 + 394 + 23)
  green, the slow group green, stability 5 of 5; crapper 901 functions,
  worst CRAP 9.158; dryer 669 functions, highest score 0.79; mutator:
  every file got a verdict (no failed baseline, no full disk), 852
  sites, 850 killed, 0 uncovered, 2 survivors, both in
  `metla-ffi/src/engine.rs` (a dead offset literal in a page limit and an
  unexercised core-count fallback), fixed in 4cb5d3d. 18 of the kills
  were by timeout, in 13 files: `metla-cli` bench.rs 2, commands.rs 1;
  `metla-core` engine/scheduler.rs 1, engine/worktrees.rs 2, fs/bulk.rs 1,
  fs/dir.rs 3, fs/volume.rs 1, index/codec.rs 1, index/update.rs 1,
  rules/worktree.rs 2, scan/scope.rs 1, scan/walk.rs 1,
  watch/coalescer.rs 1 (at that load a timeout may be the machine, not an
  assertion).
* The Rust sources changed after 88cd5b2 were mutated again one file at
  a time (targeted runs on clean clones, all sites of each file):
  `engine/kept.rs` 17 sites, 17 killed; `clean/act.rs` 9 of 9;
  `metla-cli/src/cleaned.rs` 7 of 7 (at 1703602); `metla-ffi/src/engine.rs`
  2 of 2 (at 4cb5d3d); 0 survived, 0 uncovered. Not run again:
  `engine/mod.rs` (one changed line, the home's canonical path, with no
  operator or literal in it), and the declarations without sites
  (`domain/cleanup.rs`'s new enum, `domain/mod.rs`, `metla-ffi`'s
  `domain.rs` and `lib.rs`).
* crapper on 50c29ae: 904 functions (`metla-core` 708, `metla-cli` 145,
  `metla-ffi` 50, `uniffi-bindgen-swift` 1), one at CRAP 10.08
  (`kept::failure`), split in 1703602 (8.0 on a run over `kept.rs`); no
  later run over the whole workspace. dryer at 1703602: no pair at 0.8 or
  above.
* **There is no complete mutation run on the final commit**: long runs
  were stopped by the user's decision at the end of M7.

final-commit gate: filled in by the architect

## Swift: what the ub-mode tools do not cover

crapper, dryer and mutator do not read Swift. Nothing in the app is
measured by them, and no Swift result is reported as theirs. The Swift
code is held to this instead (`scripts/quality-swift.sh`):

* **the compiler**: Swift 6 language mode with complete strict
  concurrency checking, every warning an error (`Package.swift` sets
  both; the script passes `-warnings-as-errors` again and counts the
  diagnostics of the build, which must be zero). The generated bindings
  compile under the same settings;
* **tests**: `swift test` (Swift Testing) for `MetlaKit`: the view models
  against a real engine on fixtures made by `metla-cli fixture create`
  (each test that cleans has a fixture of its own): the cleanup flow
  (confirmation gating, unchecked groups untouched on disk, items in a
  managed worktree confirmed one by one, set-aside items, the report's
  numbers equal to the engine's and kept across launches, re-running,
  cancel, quick clean of the chosen quick categories only, a debug
  engine on a root without the fixture marker refusing
  `executeConfirmed` and `quickClean`), the four `--scenario` runs on
  fresh fixtures, the hotkey model (keeping, names, the modifier rule,
  a taken combination shown, the opt-in for "clean immediately"),
  exclusions, general settings, the power table, the Full Disk Access
  model with an injected probe, the menu-bar summary; and the pure parts
  (status lines for every engine state, formatting, names, both
  languages' strings and plural rules with the same keys, launch
  settings, the refresh pacer, the main-thread guard);
* **line coverage**: `llvm-cov` per `MetlaKit` file; each view-model and
  text file and the scenarios (`Models/`, `Text/`, `Scenarios/`) needs
  at least 85 % of its lines, the
  report lists the rest;
* **concurrency by construction**: every engine call goes through the
  `EngineClient` actor, off the main thread, and `MainThreadGuard` stops
  a debug build that makes a blocking engine call on the main thread
  (tested).

**Swift numbers at M7** (`scripts/quality-swift.sh` at f76e238; MetlaKit
changed once after it, in 0dc38a6, `AppModel.takesOver` with its test,
and was built and tested but its coverage not measured again): 0
diagnostics in the strict build; 79
tests in 14 suites passed (the scenario test counts its four runs as
one); line coverage per gated file (floor 85 %): Power, RefreshPacer,
ReportModel, Format, StatusLine 100 %, OverviewData 98.4 %, Names
97.7 %, Hotkeys 97.1 %, SettingsModels 96.6 %, CategoryModel 96.3 %,
AppModel 94.2 %, MenuBarSummary 92.0 %, CleanupModel 90.5 %, Scenarios
89.7 %, FoldersModel 87.9 %, Strings 87.9 %, PreviewModel 87.7 %. Below
the floor and not gated: LaunchSettings 81.0 % (`current()` reads the
real process's environment and is only called by the app), EngineClient
94.6 %, MainThreadGuard 66.7 % (the standard guard's report, an
assertion that stops a debug build at a blocking call on the main
thread; the tests check the guard with a reporter of their own). MetlaKit in all: 93.6 % of 2,234 lines (89.5 % of
regions, 90.8 % of functions).

## What no tool checks

* **The SwiftUI views and their layout.** `MetlaApp` has no unit tests
  and no coverage figure. Layout is checked by people: the capture mode
  (`--capture <dir>`) draws every screen, light and dark, in English and
  Russian, from the window server's own image of the app's windows (the
  real Settings window, the real menu-bar panel and the hotkey's real
  panel included), and each milestone's pictures are looked at one by
  one. A title bar band that contradicts its appearance is the only
  thing the capture checks itself.
* **The Carbon hotkey adapter** (`HotkeyCenter`). The app prints what
  the system answered when it registered each combination; a key press
  delivered by the system was never simulated (that needs Accessibility
  access, which is not asked for).
* **The FSEvents adapter beyond the slow group.** `watch::sys` holds the
  foreign declarations; the fast suite starts and stops real streams
  only where nothing is written and decodes batches with the pure
  `watch::events_of`. Delivery itself is exercised by the ignored tests,
  which `quality.sh` runs once per gate; under heavy load fseventsd
  delivers late, so they are not in the suite the mutator runs, and the
  mutator never sees `watch::sys` fail.
* **The rest of the AppKit glue:** the IOKit power monitor (the pure
  power table is tested), the `SMAppService` login item (its error is
  shown; starting at login was not observed), the system Trash host
  (tested only refusing a missing path), the Full Disk Access deep link.
* **Swift duplication, complexity and mutation.** crapper, dryer and
  mutator read Rust only; there is no such measure for Swift.
* **The architecture of the Swift side.** `scripts/architecture.sh`
  scans the Rust crates only; the dependency rule of the Rust core is
  also pinned by `tests/core/architecture.rs`.

The cleanup flows are checked end to end by the scenarios
(`scripts/run-scenarios.sh`: the debug app drives the real view models
on fresh fixtures and checks the disk afterwards), and every script by
`scripts/fresh-clone-check.sh` from a fresh clone. What could not be
checked on the development machine is listed in
[LIMITATIONS.md](LIMITATIONS.md).
