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
  real FSEvents latency (`--ignored`);
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
  (`MUTATE_ALL=1` runs everything).

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
  against a real engine on a fixture made by `metla-cli fixture create`,
  and the pure parts (status lines for every engine state, formatting,
  names, both languages' strings and plural rules, launch settings, the
  refresh pacer, the main-thread guard);
* **line coverage**: `llvm-cov` per `MetlaKit` file; each view-model and
  text file (`Models/`, `Text/`) needs at least 85 % of its lines, the
  report lists the rest;
* **concurrency by construction**: every engine call goes through the
  `EngineClient` actor, off the main thread, and `MainThreadGuard` stops
  a debug build that makes a blocking engine call on the main thread
  (tested).

Not covered by any gate: `MetlaApp` (the SwiftUI views and AppKit glue)
has no unit tests and no coverage figure; it is checked by building it
under the strict settings, by running the app on a fixture
(`scripts/run-app.sh`) and by its capture mode (`--capture <dir>`),
whose images are looked at by a person. There is no mutation testing of
Swift and no duplication or complexity measure for it.
