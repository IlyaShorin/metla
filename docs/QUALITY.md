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

* The only complete `MUTATE_ALL=1` run of M7 is the one at
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

* **The gate on the final Rust sources** (`STABLE_RUNS=3
  scripts/quality.sh`, differential, in the clean clone of the complete
  run, at 0dc38a6, the last commit that changes Rust; 16:12 to 16:24 on
  2026-11-11 with nothing else running): fmt, clippy and the tests
  (73 + 395 + 23) green, the slow group (1 + 10) green, stability 3 of 3
  (491 tests each); crapper 906 functions (`metla-core` 709, `metla-cli`
  145, `metla-ffi` 51, `uniffi-bindgen-swift` 1), worst CRAP 9.158
  (`Scheduler::handle`); dryer 673 functions, highest similarity 0.79, no
  pair at 0.8; mutator 83 files, 850 sites, 850 killed, 0 survived, 0
  uncovered. Of those, 53 mutants were executed in this run, the ones in
  functions changed since 88cd5b2 (`engine/kept.rs` 17, `clean/act.rs` 9,
  `metla-cli/src/cleaned.rs` 7, `metla-ffi/src/engine.rs` 2,
  `engine/mod.rs` 13 of 23, `domain/cleanup.rs` 5 of 16): all killed by a
  failing test, none by timeout. The other 797 verdicts are the complete
  run's, 18 of them kills by timeout under load, not checked again.
* **The gate on the last commit** (the same command in the same clean
  clone at 15027ac, 18:05 to 18:17 on 2026-11-11 with nothing else
  running), after the engine began to count ended full scans (bf7261c):
  fmt, clippy and the tests (73 + 396 + 23) green, the slow group
  (1 + 10) green, stability 3 of 3 (492 tests each); crapper 906
  functions, worst CRAP 9.158; dryer 673 functions, highest similarity
  0.79; mutator 83 files, 850 sites, 850 killed, 0 survived, 0
  uncovered. 26 mutants were executed in this run (`engine/view.rs` 9,
  `engine/mod.rs` 11, `engine/scheduler.rs` 6), all killed by a failing
  test, none by timeout. With the 53 of the run before, at most 79 of
  the 850 verdicts were made after the complete run; the rest are that
  run's.

**Rust numbers at 0.7.1** (parallel removal, live cleanup progress,
the first scan on Interactive's threads, preemption by a cleanup, why a
full scan starts, the cleanup's own events). The ub-mode tools ran on
the changed functions only, not as `scripts/quality.sh`'s differential
run: this working copy has no `.metrics/mutate` snapshot, so a
differential run would have mutated every site. The script's test steps
ran as they are (`STABLE_RUNS=3 QUALITY_STEPS=tests scripts/quality.sh`);
crapper and dryer ran with the script's gates; the mutator ran file by
file with `--lines` set to the functions that changed since the last
gated commit, two workers, the script's timeout factors (6 for
`metla-core`, 45 for `metla-ffi`), `scripts/mutant-test.sh` as the test
command. All on the RAM disk, the machine otherwise in ordinary use.

* Round 1 at 2c8da0b (21:59 to 22:42 on 2026-11-11, functions changed
  since aa29acd): fmt, clippy, tests (73 + 419 + 23), the slow group
  (1 + 11) and stability 3 of 3 (515 tests each) green. crapper: 935
  functions, one over the limit, `refresh::Attempt::attempt` at 10.02;
  dryer: 701 functions, two pairs at or above 0.8 (`CleanControl`'s two
  lock helpers, 1.00; `Flags::lost_history` and `must_scan`, 0.85).
  Mutator: 129 mutants run (the sites of the changed functions in 11
  files; 9 changed files had none): 113 killed (2 by timeout: one in
  `engine/scheduler.rs`, one in `watch/coalescer.rs`), 13 survived, 3
  uncovered: `fs/remove.rs` 8 and 2 (most equivalent by construction:
  the parallel and the iterative walks give the same result, so the
  depth where one hands over to the other could not be seen),
  `clean/control.rs` 2, `engine/refresh.rs` 2, `engine/shared.rs` 1,
  `engine/scheduler.rs` 1 uncovered (the stream's sink). Fixed in
  f1fb786 (see its message).
* Round 2 at f1fb786 (22:50 to 23:02, functions changed since
  20889c4): tests (73 + 423 + 23), slow group (1 + 11), stability 3 of
  3 (519 tests each) green; crapper 940 functions, worst CRAP 9.18
  (`fs::remove::parallel`), none at 10; dryer 701 functions, highest
  similarity 0.79, no pair at 0.8. Mutator: 36 mutants run, 32 killed
  (none by timeout), 4 survived, 2 uncovered: `clean/control.rs` 1 (the
  live count's reset, unseen while it was subtracted with saturation;
  fixed in c272bf6, which counts finished and live bytes apart) and
  `fs/remove.rs` 3 and 2 (a pool of zero threads, which rayon sizes
  itself, unseen where its automatic size was one; the fallback of
  `threads()`; the negation that picks the parallel walk, which gives
  the same result on one thread: fixed in 9bdb2ec, NonZeroUsize thread
  counts and a test that holds each removal until a second thread has
  removed something). The two uncovered sites are one now (`unread`): a
  directory that opens but cannot be listed needs an I/O error after a
  successful open, which no test causes.
* Round 3 at 9bdb2ec (23:07 to 23:16, functions changed since
  f1fb786): tests, slow group and stability 3 of 3 (520 tests each)
  green; crapper 941 functions, worst CRAP 9.16
  (`Scheduler::handle`); dryer 701 functions, highest similarity 0.79,
  no pair at 0.8. Mutator: 10 sites, 9 killed (none by timeout), 0
  survived, 1 uncovered: `fs::remove::unread`, the directory that opens
  but cannot be listed (above). That one uncovered site is what the
  gate does not close.
* Round 4 at afc805d (23:25 to 23:47, functions changed since 32fe381:
  deferred full scans and the attention at opening): stability 3 of 3
  (524 tests each) green; crapper 945 functions, worst CRAP 9.16
  (`Scheduler::handle`); dryer 705 functions, highest similarity 0.79,
  no pair at 0.8. Mutator: 58 mutants run, 57 killed (1 by timeout, in
  `engine/scheduler.rs`), 0 uncovered, 1 survived: `metla-cli`'s live
  mode starts its engine with `attention: false`, and no CLI test sees
  the scan threads that would change.
* Round 5, functions changed since afc805d (the stream's exclusions,
  the picture kept after a killed scan, the replay of lost events and
  the loss kinds, the stamped core, the gate items of round 4):
  - A first run on d0cde7b (00:23 to 00:45 on 2026-11-12): stability 3
    of 3 (533 tests each); dryer 719 functions, 0.79; crapper failed
    (`ScanReason::says`, 13.0, a match of twelve arms; fixed in
    fa1830e). Its mutator completed `metla-cli/src/live.rs` (1 killed),
    `engine/mod.rs` (1 killed, 3 survived) and `engine/scheduler.rs` (36
    killed, 1 of them by timeout, 3 survived, 2 uncovered), and was
    stopped on the coordinator's word in `fs/remove.rs`; its
    `engine/view.rs` verdicts were void (the file changed during the
    run). Its findings were fixed in ff2f814 (`Replays`, `reopen`,
    `exclusion_line` and tests of their edges); no verdict of that run
    is carried over.
  - The final gate on ff2f814 (00:46 to 01:12): fmt, clippy, tests
    (74 + 439 + 23), the slow group (1 + 12) and stability 3 of 3 (536
    tests each) green; crapper 967 functions, worst 9.02
    (`Scheduler::handle`); dryer 723 functions, highest 0.79, no pair at
    0.8. Mutator on every file changed since afc805d: 68 mutants run in
    7 files (`metla-cli/src/live.rs` 1, `engine/mod.rs` 1,
    `engine/scheduler.rs` 43, `engine/view.rs` 1, `fs/remove.rs` 7,
    `watch/coalescer.rs` 12, `watch/sys.rs` 3; 5 changed files have no
    site in their changed functions), 66 killed (1 by timeout, in
    `engine/scheduler.rs`), 2 survived, 0 uncovered.
  - The two survivors (the guard that set the stream's exclusions only
    when there were any, and `exclusion_line`'s `left > 0`) were fixed
    in 63b2f52; crapper again on 63b2f52: 967 functions, worst 9.02, none
    at 10; the mutator again on what the fix touched: `watch/sys.rs` 2
    killed, `engine/scheduler.rs` 43 killed (the scheduler's snapshot
    let the mutator run every site of its changed functions again), 0
    survived, 0 uncovered, no timeout. The rest of the final gate's
    verdicts are carried over to 63b2f52 unchanged (no other source
    changed).
  - The fresh-clone check (`scripts/fresh-clone-check.sh`) on b46d783
    (01:20 to 01:50 on 2026-11-12; the clone on the SSD, `TMPDIR` on the
    RAM disk, the screen locked) did not pass: build-core, build-debug
    and relink passed, run-app (the capture) stopped at a Swift trap in
    `Capture.composed`, the scenarios passed but quick-clean, which hung
    until it was stopped after 28 minutes, and the last three steps were
    not reached. It found three things. The capture took the first
    visible window, which can be the status item's, whose number (2^32)
    does not fit the window server's 32-bit id: 6224480 takes the main
    window by its id and paints a window whose number is not a positive
    32-bit id. The debug app has the installed app's bundle id, so its
    runs read and wrote the window state macOS keeps for that id: after
    the trap, launches of the id stopped at macOS's "quit unexpectedly
    while reopening windows; reopen them?" alert, which comes before
    AppKit finishes launching and before any of the app's code (its
    window and text were read from inside the app); with the screen
    locked nobody answered it, and a scenario hung with an empty log (by
    02:42 the alert no longer came, though nobody had answered it; why
    was not established). The scripts now start the app with
    `-ApplePersistenceIgnoreState YES` (2ac670a) and stop a scenario
    still running after 180 s, or a capture after 600 s, as a failure
    (d90cce9). And the app started its engine from a view's `.task` and
    opened no main window when the windows macOS restored did not include
    it (after a run that ended abnormally with the main window closed, or
    a quit with it closed and "Close windows when quitting" off): on a
    copy of the debug app under a bundle id of its own, b46d783's build
    then showed no window and 92aec8f's the main window. 92aec8f starts
    the app from `applicationDidFinishLaunching` and ends a launch with
    the main window open; it cannot get past the alert above.
  - The fresh-clone check on d90cce9 (02:51 to 03:01, 600 s; the same
    places, the screen locked): every step exit 0. build-core 53 s,
    build-debug 18 s, relink 17 s, run-app 121 s (30 images, 0 failures,
    painted in-process at 1x: the window server gives no image with the
    screen locked), scenarios 18 s (30, 9, 19 and 12 checks),
    build-release 31 s (0 strings with a local path), quality-swift 83 s
    (0 diagnostics, 87 tests in 14 suites, MetlaKit 93.10 % of 2,319
    lines), quality-tests 256 s (fmt, clippy, tests 74 + 439 + 23, the
    slow group 1 + 12, stability 5 of 5 with 536 tests each). No Rust
    source changed after 63b2f52: crapper, dryer and the mutator were not
    run again, and the round's Rust verdicts carry over.
* Verdicts not executed in either round are carried over from the
  runs above (the complete run at 88cd5b2 and the differential gates at
  0dc38a6 and 15027ac) for every function that did not change.

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

**Swift numbers at M7** (`scripts/quality-swift.sh` at 15027ac, the
last commit; eight runs in a row on 2026-11-11, five in one clean clone
and three in another, each with at least 1,500 MB free on the volume of
`TMPDIR`): all eight green. 0 diagnostics in the strict build; 84 tests
in 14 suites passed (the scenario test counts its four runs as one);
line coverage per gated file (floor 85 %): Power, RefreshPacer,
ReportModel, Format, StatusLine 100 %, OverviewData 98.4 %, Names
97.7 %, Hotkeys 97.1 %, SettingsModels 96.6 %, CategoryModel 96.3 %,
AppModel 93.2 %, MenuBarSummary 92.0 %, CleanupModel 89.5 %, Scenarios
88.8 %, Strings 87.9 %, PreviewModel 87.7 %, FoldersModel 86.4 %. Below
the floor and not gated: LaunchSettings 81.0 % (`current()` reads the
real process's environment and is only called by the app), EngineClient
94.6 %, MainThreadGuard 66.7 % (the standard guard's report, an
assertion that stops a debug build at a blocking call on the main
thread; the tests check the guard with a reporter of their own). MetlaKit in all: 93.1 %
of 2,301 lines (88.5 % of regions, 90.2 % of functions).

The gate was not stable before that. On d348f86 and 3c12f54 it failed
in some runs, each time for a timing defect in a model, and each was
fixed test-first: the Folders outline lost what was expanded when a
reload overlapped (7b2cf73); the status kept the user's scan profile
after the scan had ended (3c12f54); and the model guessed that the
user's scan was over from a picture change or a full-scan time in whole
seconds, which a fixture that scans in milliseconds defeats (bf7261c,
8062680: the engine counts the full scans that ended, and the model
compares that count). One earlier run on d348f86 failed ten tests at
once after a 26-second stall; the RAM disk was close to full then, the
cause was not established, and the script has no full-disk guard of its
own. Eight green runs do not prove that no timing defect is left.

**Swift numbers at 0.7.1** (`scripts/quality-swift.sh` in this
working copy at f1fb786, 23:02 to 23:03 on 2026-11-11, once, 1,314 MB
free on the volume of `TMPDIR`; MetlaKit last changed in 2c8da0b, and
20889c4 changed only the app target and the capture): green. 0
diagnostics in the strict build; 87 tests in 14 suites passed; line
coverage per gated file (floor 85 %): Power, RefreshPacer,
ReportModel, Format, StatusLine 100 %, OverviewData 98.4 %, Names
97.7 %, Hotkeys 97.1 %, SettingsModels 96.6 %, CategoryModel 96.3 %,
AppModel 93.7 %, MenuBarSummary 92.0 %, CleanupModel 90.1 %, Scenarios
88.8 %, Strings 87.9 %, PreviewModel 87.7 %, FoldersModel 86.4 %; not
gated: LaunchSettings 75.8 % (lower than at M7: the preferences suite
of aa29acd is read only by the app), EngineClient 94.6 %,
MainThreadGuard 66.7 %. MetlaKit in all: 93.06 % of 2,319 lines. One
run only, not the five in a row of M7. The capture (`scripts/run-app.sh
--capture`), which is not part of the gate, now fails when a main
screen changes the window's frame or lays its content out at another
size than the window's; it passed with 0 failures at 20889c4. Run
again at afc805d (23:47 to 23:49, after the engine options gained the
attention at opening): green, 0 diagnostics, 87 tests in 14 suites,
every gated file at or above 85 %, MetlaKit 93.14 % of 2,319 lines. And at 63b2f52, the final commit
(01:17 to 01:18 on 2026-11-12): green, 0 diagnostics, 87 tests in 14
suites, every gated file at or above 85 %, MetlaKit 93.19 % of 2,319
lines. And at d90cce9, after the capture, launch and script changes
(02:49 to 02:51; MetlaKit unchanged): green, 0 diagnostics, 87 tests in
14 suites, every gated file at or above 85 %, MetlaKit 93.10 % of 2,319
lines.

**0.7.2** (with Full Disk Access granted, the folders still denied are
closed to every app: no banner and no access button for them; the probe
at start, on activation and after a full scan; access offered only when
a folder is denied; a picture made before access was granted is scanned
again once). No Rust source changed: the Rust gate and the mutator were
not run, and the verdicts above carry over. On 2026-11-12, `TMPDIR` on
the RAM disk, the screen locked from 12:30:

* At 17afe39 (the first round): `scripts/quality-swift.sh` (12:37:17 to
  12:38:22, 65 s) green, 0 diagnostics, 94 tests in 14 suites, MetlaKit
  93.37 % of 2,353 lines; `scripts/run-scenarios.sh` 17 s, the four
  scenarios pass; the capture 124 s, 30 images, 0 failures;
  `scripts/build-app.sh --release` 26 s.
* At 0db8699 (the final code):
  - `scripts/quality-swift.sh` (13:04:01 to 13:05:06, 65 s): green. 0
    diagnostics; 100 tests in 14 suites passed; every gated file at or
    above 85 % (OverviewData 98.69 %, AppModel 94.16 %, SettingsModels
    96.75 %, Power 100 %, StatusLine 100 %, the lowest FoldersModel
    86.41 %); MetlaKit 93.47 % of 2,405 lines. One run; the engine suite
    alone eight times in a row before it, 18 tests passed each time.
  - `scripts/run-scenarios.sh` (15 s): the four scenarios pass (30, 9, 19
    and 12 checks).
  - `scripts/build-app.sh --release` (17 s): built, version 0.7.2 (9), 0
    strings with a local path. Not installed.
  - `scripts/run-app.sh --capture` on a fresh fixture (127 s, in Russian,
    the system's language): 30 images, 0 failures, painted in-process at
    1x (the window server gives no image with the screen locked; the
    light Overview's title bar band came out dark, which the capture
    reports without counting it as a failure). The fixture has no denied
    folder and the development build has no Full Disk Access: the
    Overview has no banner and no closed-folder line; the first-run sheet
    and Settings → Access show the not-granted text.
* Swift has no mutator: the new decisions were mutated by hand, one
  change at a time, against the suites that test them. First round, 10
  mutants (the start probe and the probe after a scan removed, `>` made
  `>=`, the count of ended scans never kept, "cannot tell" read as
  granted, closed folders with other gaps, the unread count changed, the
  status ignoring `closedOnly`, the model never passing it, the Overview
  ignoring the access): all killed; the removed start probe had first
  survived on its own commit's tree, and
  `fullDiskAccessIsProbedBeforeTheFirstPicture` was added for it. Second
  round, 11 mutants (the rescan not once per launch, the once-flag never
  set, an outdated picture read as closed, the denied gap not looked
  for, no place looked up, the access banner with no folder denied, an
  outdated picture saying closed, the count not taken at the check, no
  check before the first picture, denied places counted as listed, and
  the count advanced before a check after a scan): 10 killed, 1
  survived, the last, which only keeps two refreshes running at once
  from probing twice; no test makes that interleaving.
* The branches with access granted, and the one full scan for an
  outdated picture, are exercised by the tests only (OverviewDataTests,
  StatusLineTests, SettingsTests, and EngineTests on a fixture with a
  folder of mode 000 and a probe that answers granted, claiming to have
  listed that folder or not); the new views were drawn once by a capture
  changed for the purpose and not kept.

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
