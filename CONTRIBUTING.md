# Contributing

Read [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and
[docs/GLOSSARY.md](docs/GLOSSARY.md) first. Use the glossary's words in code.

## Ground rules

* **Never delete real data while developing.** Removal code runs only
  against fixtures: synthetic trees under `$TMPDIR` that tests make, or
  ones `metla-cli fixture create <dir>` makes, each marked by a
  `.metla-fixture` file. The debug app, the CLI and every test build
  refuse to clean a root without that marker (the `Production`
  capability exists only in the release app's build); start a release
  build only with `METLA_ROOT` on a fixture. Scanning the real disk is
  read-only. Real paths, names and listings never enter the repository —
  not in tests, fixtures, docs, screenshots or commit messages.
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

## Running the gates

```
export TMPDIR=<a directory on a volume with a few GB free>
scripts/quality.sh                      # differential: changed functions and survivors
MUTATE_ALL=1 scripts/quality.sh         # every mutant again: the gate a milestone ends with
QUALITY_STEPS=tests scripts/quality.sh  # fmt, clippy, tests, slow group, stability only
scripts/quality-swift.sh                # the Swift gates
scripts/fresh-clone-check.sh <new dir>  # every script from a fresh clone, as a newcomer runs them
scripts/architecture.sh                 # uml-viewer's IR of the module tree (needs Clojure)
```

`TMPDIR` must name a directory outside the system's own areas
(`/private`, `/var`, `/tmp`, `/System`, `/Library`, `/usr`), at most 64
characters long once symbolic links are resolved: the tests' fixtures
under macOS's default `/var/folders/...` are classified as system data,
not the user's, and a long path leaves no room for a fixture's socket
(104 bytes) or the deepest test trees. Both gate scripts stop at once,
with exit 2, when it is not so.

The full mutation run takes hours (two workers by default,
`MUTATOR_WORKERS`). It counts only when it ends with a verdict for every
file: a run stopped by a full disk (exit 3, `DISK FULL`) or a file whose
unmutated baseline failed is a lost verdict, to be run again, never a
pass. Keep `TMPDIR` and the build directories (`target/`, `core/target/`)
off a nearly full disk; the tests write synthetic trees and indexes
there.

## The app

```
scripts/build-core.sh      # metla-ffi's static library (fixtures only) and the Swift bindings
scripts/build-core.sh production    # the release app's library: can clean real data
scripts/build-app.sh --release   # Metla.app and Metla.zip (release, ad-hoc signed, can clean
                                 # real data) under target/app/release
scripts/build-app.sh --release --plain   # the same, linked with the plain library: cleans
                                 # fixtures only; for measuring on the real disk
scripts/build-app.sh --debug     # the debug app: cleans fixtures only
scripts/run-app.sh         # starts the debug app on a fixture metla-cli makes
scripts/run-app.sh --capture <dir>   # PNGs of the screens, light and dark (started with
                                     # `open`, so the app is active while it draws)
scripts/run-scenarios.sh   # the cleanup flows and the update end to end on fresh fixtures
```

### The update's test feed

A build whose core is not the production one (the debug app, `--release
--plain`) reads `METLA_UPDATE_FEED`: a `file://` or `http(s)://` URL of a
JSON file in the shape of GitHub's `releases/latest`. Its `Metla.zip` and
release page must be files beside it (in the same folder of the URL); a
production build ignores the variable and says so on stderr (`update:
feed … (fixed in this build; METLA_UPDATE_FEED ignored)`). The app always
verifies against its own designated requirement, so only a copy signed
with a certificate can install from it; an ad-hoc copy only links to the
release page. A launch checks by itself only without `METLA_ROOT` (and
never in a capture, a scenario or `--idle`); "Check Now" asks in any
case. The relaunch after an install passes the app's own `METLA_`
variables on (`open --env`), so a run on a fixture and a test feed goes
on with them. `scripts/run-scenarios.sh update` runs the whole install on fixture
bundles in `<fixture>-update`, with the relaunch recorded, not made.
Never point a test feed at the copy in `/Applications`: use copies in a
folder of your own.

Only the release build of the app can clean a root that is not a
fixture. Develop, test and capture with the debug build; never start a
release build without `METLA_ROOT` pointing at a fixture. To measure the
app on the real disk, use `build-app.sh --release --plain`: the release
app without the capability to clean anything but a fixture. A release
build fails when a binary holds a local path (`scripts/no-local-paths.sh`).

`swift build` and `swift test` in `app/` need the generated bindings
(`build-core.sh`) and the library's directory on the linker's path
(`-Xlinker -L<dir>`, the path `build-core.sh` prints); the scripts pass
it. Start the app only on a fixture (`METLA_ROOT`, `METLA_HOME`,
`METLA_DATA_DIR`) while developing.
