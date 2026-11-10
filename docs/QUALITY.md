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

**Swift numbers at M6** (`scripts/quality-swift.sh` on the final tree,
42cb5a0 and later, with a mutation gate running beside it): 0
diagnostics in the strict build; 70 tests in 13 suites passed (the
scenario test counts its four runs as one); line coverage per gated
file: Power, RefreshPacer, Format, StatusLine 100 %, OverviewData
98.4 %, Hotkeys 97.1 %, SettingsModels 96.6 %, CategoryModel 96.3 %,
Names 95.6 %, AppModel 93.6 %, ReportModel 92.5 %, MenuBarSummary
92.0 %, CleanupModel 89.5 %, Scenarios 88.8 %, FoldersModel 87.9 %,
Strings 87.1 %, PreviewModel 86.4 %; not gated: EngineClient 94.6 %,
LaunchSettings 76.6 %, MainThreadGuard 66.7 %; MetlaKit in all 87.6 %
of lines.

Not covered by any gate: `MetlaApp` (the SwiftUI views and the AppKit
glue: the Carbon hotkey adapter, the HUD panel, the IOKit power monitor,
the `SMAppService` login item) has no unit tests and no coverage figure;
it is checked by building it under the strict settings, by running the
app on a fixture (`scripts/run-app.sh`; it prints what the system
answered when the hotkeys were registered), by the scenarios
(`scripts/run-scenarios.sh`, the debug app driving the cleanup flows on
fresh fixtures) and by its capture mode (`--capture <dir>`), whose
images are looked at by a person. What an agent cannot check on a
development machine is listed with each milestone's report (a physical
hotkey press, the login item after a reboot, Full Disk Access granted).
There is no mutation testing of Swift and no duplication or complexity
measure for it.
