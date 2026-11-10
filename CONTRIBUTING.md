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
cargo test -p metla-core --test core -- --ignored   # slow: real FSEvents latency
scripts/quality.sh        # all of the above once, then crapper, dryer, mutator
```

Tests that wait for real FSEvents latency are `#[ignore]`d so the suite
the mutator runs for every mutant stays fast; `scripts/quality.sh` runs
them once. The fast suite drives the stream with a flush and the engine
with millisecond settle times instead.

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
scripts/build-core.sh      # metla-ffi's static library and the Swift bindings
scripts/build-app.sh       # Metla.app (ad-hoc signed) under target/app
scripts/run-app.sh         # starts it on a fixture metla-cli makes
scripts/run-app.sh --capture <dir>   # PNGs of the main screens, light and dark
```

`swift build` and `swift test` in `app/` need the generated bindings
(`build-core.sh`) and the library's directory on the linker's path
(`-Xlinker -L<dir>`, the path `build-core.sh` prints); the scripts pass
it. Start the app only on a fixture (`METLA_ROOT`, `METLA_HOME`,
`METLA_DATA_DIR`) while developing.
