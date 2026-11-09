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
* No `unsafe` outside `fs` and `watch`'s system adapters; every `unsafe`
  block states its invariant. No `#[allow]` to silence a lint without a
  reason next to it.

## Commits

Small and ordered. Subject in the imperative, at most 72 characters. The
body says why, what changed and how it was proved (test names, tool output).

## Quality gates (Rust)

```
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
scripts/quality.sh        # crapper, dryer, mutator
```

Every function has CRAP below 10, no duplication scores 0.8 or higher, no
mutation site is uncovered and no mutant survives. The tools live at
<https://github.com/unclebob/crapper>, `/dryer`, `/mutator`, `/uml-viewer`.

These tools do not analyse Swift. Swift is held to: Swift 6 language mode
with strict concurrency, warnings as errors, `swift test`, and line coverage
of `MetlaKit` reported by `llvm-cov`.
