# Glossary

| Term | Meaning |
|------|---------|
| **Root** | The directory an engine instance analyses. `/` in production; a synthetic tree in tests. Nothing outside the root is ever read or removed. |
| **Index** | The persistent SQLite store of directory totals, candidates and scan metadata. |
| **Allocated size** | Bytes a file occupies on disk (`st_blocks × 512`). All sizes in Metla are allocated sizes and are estimates of what removal would free. |
| **Own / tree totals** | *Own*: files directly in a directory. *Tree*: the directory and everything below it. |
| **Rule** | A named, verifiable description of one kind of removable directory (for example "Cargo build output"). Has an id, a category, a matcher and a human explanation. |
| **Marker** | A file or directory whose presence a rule requires, next to the directory (*sibling marker*) or inside it (*inner marker*). |
| **Candidate** | A directory matched by a rule and not excluded. The only thing that can be cleaned. |
| **Category** | The kind of a candidate: app cache, developer cache, build artifact, log, Git worktree, temporary data. Decides the disposal and whether quick clean is possible. |
| **Disposal** | How a candidate is removed: permanent delete, move to Trash, or `git worktree remove`. |
| **Exclusion** | A path the user wants kept. It and everything beneath it never become candidates. |
| **Activity** | The newest modification time seen anywhere in a candidate's tree. Recently active build output is not preselected. |
| **Preview** | The exact list shown before a cleanup: items, estimated sizes, disposals, warnings. Confirmation applies to one preview. |
| **Revalidation** | Re-checking a preview item immediately before acting on it. |
| **Quick clean** | Hotkey-triggered cleanup without a confirmation step. Opt-in, and limited by type to caches and build artifacts. |
| **Found / removed / freed** | *Found*: estimated size when scanned. *Removed*: allocated size of what was actually unlinked. *Freed*: measured change in volume free space. |
| **Coverage** | What the analysis could not see: denied or failed directories, skipped mounts, missing Full Disk Access. A scan with gaps is *partial*. |
| **Gap** | A directory the scan wanted to read but could not: *denied* (`EACCES`/`EPERM`), *failed* (any other error, with its errno), *vanished* (gone between being listed and being read) or *cancelled* (not reached before the scan was stopped). A gap has no own totals. Any gap makes the scan partial. |
| **Skip** | A directory the scan deliberately does not descend: on *another volume*, a second *mount point* of a volume already walked, a *dataless* cloud placeholder, or an *excluded* system path. Skips are counted in coverage but do not make a scan partial. |
| **Scan scope** | The policy, as data, that decides skips: the devices a scan stays on and the paths it does not descend. |
| **Dataless** | A cloud placeholder whose contents are not on this disk (`SF_DATALESS`). Reading it would trigger a download, so it is never descended. |
| **Picture** | The directory rows of the last finished scan held by the index, with that scan's root, times and coverage. A new scan replaces it in one transaction. |
| **Live tree** | The directories at most three levels below the root, with counters that grow while a scan runs, for showing progress. Bounded in size. |
| **Directory facts** | What the walker knows about a directory it has just read, at no extra I/O: its path, its entries, the parent's marker entries, and whether it is inside a Git repository. The input of the classifier. |
| **Unaccounted** | Used space on the scanned volume that the scan did not find: used (total − available) minus the scanned tree size. Includes other volumes of the container, snapshots, purgeable space and gaps; negative when clones are counted twice. |
| **Freshness** | When the index was last fully scanned and last updated, and how many changes are pending. |
| **Coalescer** | The pure state machine that turns file-system events into a bounded set of due rescan tasks. |
| **Settle time** | How long a directory must be quiet before it is rescanned. Grows while the directory keeps changing. |
| **Rescan task** | A request to re-read one directory (*shallow*) or a whole subtree (*recursive*). |
| **Power profile** | `Interactive`, `Background`, `Saver` or `Off`: how much CPU and I/O the engine may use. |
| **Worktree status** | The result of inspecting a linked Git worktree: clean or not, pushed or not, locked, in use, managed by another tool. |
| **Managed worktree** | A worktree owned by another tool (Codex Desktop, Claude Code). Shown, never removed. |
| **Fixture** | A synthetic directory tree marked with `.metla-fixture`, the only place development builds may delete. |
