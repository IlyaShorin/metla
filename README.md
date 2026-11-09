# Metla

A free, open-source SSD cleaner for macOS. *Metla* is Russian for "broom".

Metla shows what occupies your internal disk — by category and by folder,
down to individual files — and removes what is verifiably disposable:
application and developer-tool caches, build output, logs, stale Git
worktrees and temporary data.

* **Rust core** scans, classifies and cleans. Metadata only, incremental,
  bounded memory.
* **SwiftUI/AppKit app** with a main window and a menu bar panel.
* **Nothing is junk because it is big.** Every candidate comes from a named
  rule with a verifiable marker; see [docs/RULES.md](docs/RULES.md).
* **Nothing is removed blind.** You see what, how much and how (permanently,
  to Trash, or `git worktree remove`) before confirming, and each item is
  re-checked right before removal.

Status: under construction. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## License

MIT
