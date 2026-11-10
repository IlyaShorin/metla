# Contributing

Read [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and
[docs/GLOSSARY.md](docs/GLOSSARY.md) first. Use the glossary's words in code.

## Ground rules

* **Never delete real data while developing.** Deletion code runs only
  against synthetic trees under a temporary directory. Scanning the real
  disk is read-only. Real paths, names and listings never enter the
  repository — not in tests, fixtures, docs, screenshots or commit messages.
* **Test first.** Write the failing test, watch it fail for the expected
  reason, make it pass with the smallest change. Tests exercise behaviour
  through the public interface of a module.
* **Respect the dependency rule** in ARCHITECTURE.md §2. `domain` and
  `rules` stay pure.
* **Make illegal states unrepresentable** rather than checking for them.
* No `unsafe` outside `fs` and `watch::sys` (the FSEvents adapter, which
  holds every foreign declaration of the watcher); every `unsafe` block
  states its invariant. No `#[allow]` to silence a lint without a
  reason next to it.

## Commits

Small and ordered. Subject in the imperative, at most 72 characters. The
body says why, what changed and how it was proved (test names, tool output).

## Quality gates (Rust)

```
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
cargo test -- --ignored   # slow: they wait for real FSEvents deliveries
scripts/quality.sh        # all of the above once, then crapper, dryer, mutator
```

Tests that wait for fseventsd to deliver an event are `#[ignore]`d:
under load it delivers seconds late, and the suite the mutator runs for
every mutant must pass every time, or its failures "kill" mutants they
have nothing to do with. `scripts/quality.sh` runs them once, and runs
the fast suite five times in a row before the mutator; one failure
fails the gate. The fast suite feeds the engine through
`Engine::changed` with millisecond settle times, decodes FSEvents
batches with `watch::events_of`, and starts and stops real streams only
where nothing is written. Threads the tests wait for run at the
priority of the profile in force (the test profiles do not throttle),
not at background priority.

Every function has CRAP below 10, no duplication scores 0.8 or higher, no
mutation site is uncovered and no mutant survives. The tools live at
<https://github.com/unclebob/crapper>, `/dryer`, `/mutator`, `/uml-viewer`.

A mutant's timeout is about max(90 s, 3 × a warm run of its crate's
tests) (`MUTATOR_TIMEOUT_SECONDS`, `MUTATOR_TIMEOUT_FACTOR`). The mutator
counts a timed-out mutant as killed; `scripts/quality.sh` runs the tests
through `scripts/mutant-test.sh`, which traces every run, and prints how
many mutants of each file were killed by the timeout. A test that waits
gives up on its own deadline, well below that timeout, rather than
waiting for the mutator to stop it.

These tools do not analyse Swift. Swift is held to: Swift 6 language mode
with strict concurrency, warnings as errors, `swift test`, and line coverage
of `MetlaKit` reported by `llvm-cov` — `scripts/quality-swift.sh`. See
[docs/QUALITY.md](docs/QUALITY.md) for what each gate covers and what it
does not.

## The app

```
scripts/build-core.sh      # metla-ffi's static library (fixtures only) and the Swift bindings
scripts/build-core.sh production    # the release app's library: can clean real data
scripts/build-app.sh       # Metla.app (release, ad-hoc signed) under target/app/release
METLA_CONFIGURATION=debug scripts/build-app.sh   # the debug app: cleans fixtures only
scripts/run-app.sh         # starts the debug app on a fixture metla-cli makes
scripts/run-app.sh --capture <dir>   # PNGs of the screens, light and dark
scripts/run-scenarios.sh   # the cleanup flows end to end on fresh fixtures
```

Only the release build of the app can clean a root that is not a
fixture. Develop, test and capture with the debug build; never start a
release build without `METLA_ROOT` pointing at a fixture.

`swift build` and `swift test` in `app/` need the generated bindings
(`build-core.sh`) and the library's directory on the linker's path
(`-Xlinker -L<dir>`, the path `build-core.sh` prints); the scripts pass
it. Start the app only on a fixture (`METLA_ROOT`, `METLA_HOME`,
`METLA_DATA_DIR`) while developing.
