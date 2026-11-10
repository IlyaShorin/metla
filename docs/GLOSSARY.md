# Glossary

| Term | Meaning |
|------|---------|
| **Root** | The directory an engine instance analyses. `/` in production; a synthetic tree in tests. Nothing outside the root is ever read or removed. |
| **Index** | The persistent SQLite store of directory totals, candidates and scan metadata. |
| **Allocated size** | Bytes a file occupies on disk (`st_blocks × 512`). All sizes in Metla are allocated sizes and are estimates of what removal would free. |
| **Own / tree totals** | *Own*: files directly in a directory. *Tree*: the directory and everything below it. |
| **Rule** | A named, verifiable description of one kind of removable directory (for example "Cargo build output"). Has an id, a category, a matcher and a human explanation. |
| **Rule id** | A rule's stable name: its category's name, a dot and its own name (`build.cargo-target`). The category is read from the id. |
| **Rule set** | The catalogue compiled for one home directory and user; the pure classifier. |
| **Location rule** | A rule matched by an exact path pattern under the home directory (or an absolute one). |
| **Project-output rule** | A rule matched by a directory name, markers and a surrounding Git repository. |
| **Deny list** | Names a rule's `*` never matches (for example Apple's own caches). |
| **Marker** | A file or directory whose presence a rule requires, next to the directory (*sibling marker*) or inside it (*inner marker*). |
| **Probe** | The first bytes (at most 2048) of a small file a rule inspects — a `.git` pointer or a `CACHEDIR.TAG` — read by the walker. |
| **Candidate** | A directory matched by a rule, not excluded, not cleaned since the scan, and — for project output — verified by Git. The only thing that can be cleaned. A linked worktree's build output is a candidate of its own. |
| **Verdict** | Git's word on a directory of project output, asked after every scan and again before removal: `Verified` (ignored, nothing tracked inside), `NotIgnored`, `HasTrackedFiles` or `Unverifiable`. Only verified output is offered. |
| **Keeps the directory** | Said of a rule whose directory its owner expects to exist (an app's cache folder, the Trash): a cleanup removes what it holds and leaves the empty directory. |
| **Category** | The kind of a candidate: app cache, developer cache, build artifact, log, Git worktree, temporary data. Decides the disposal and whether quick clean is possible. |
| **Disposal** | How a candidate is removed: permanent delete, move to Trash, or `git worktree remove`. |
| **Exclusion** | A path the user wants kept. It, everything beneath it, and any candidate that holds it never appear as candidates. Applied when the index is read. |
| **Activity** | The newest modification time seen anywhere in a candidate's tree. Recently active build output is not preselected. |
| **Recent-activity age** | How old activity may be and still count as recent (default 14 days). |
| **Preselected** | Checked by default in a cleanup selection: every candidate except recently active build output, worktrees, and anything inside a managed worktree. Derived when read, never stored. "Can be freed" is the total of the preselected candidates that are deleted permanently; what would move to the Trash is shown apart, because it is not freed until the Trash is emptied. |
| **Tally** | A count of candidates and their estimated bytes. |
| **File kind** | What a file is for "By data type": Applications, Developer, Media, Documents, Archives and disk images, System, Other. |
| **Area** | Where a directory is for file kinds (user, system, application bundle, developer); a directory takes the stronger of its parent's area and its own. |
| **Scan session** | One scan being written through the index's single writer (`ScanWriter`); finishing it swaps the picture in. |
| **Preview** | The exact list shown before a cleanup: items, estimated sizes, groups, warnings, and the selected candidates set aside with the reason (a blocked worktree, an item inside another item). It has an id and is executed at most once. |
| **Group** | How preview items are removed and confirmed: *quick* (caches and build output, confirmed with the preview) or a *separate* group — move to Trash, empty the Trash, remove worktrees — that needs its own confirmation. |
| **Confirmation** | What the user confirmed on top of a preview: separate groups, and items confirmed one by one (build output inside a managed worktree). Anything not covered is skipped as not confirmed. |
| **Revalidation** | Re-checking a preview item immediately before acting on it: still inside the root, not excluded, reached without a symlink, the same directory on the same device, its rule still matching, Git still verifying project output. Any failure skips the item with the reason and touches nothing. |
| **Outcome** | What happened to one item: removed, partly removed (with what stayed), skipped (with the reason), failed, already gone, or moved to the Trash. |
| **Trash host** | What moves an item to the Trash: the app's `FileManager.trashItem`, or a fixture's trash directory in tests and the CLI. |
| **Quick clean** | Hotkey-triggered cleanup without a confirmation step. Opt-in, and limited by type to caches and build artifacts. |
| **Found / removed / freed** | *Found*: estimated size when scanned. *Removed*: allocated size of what was actually unlinked. *Freed*: measured change in volume free space. |
| **Coverage** | What the analysis could not see: denied or failed directories, skipped mounts, missing Full Disk Access. A scan with gaps is *partial*. |
| **Gap** | A directory the scan wanted to read but could not: *denied* (`EACCES`/`EPERM`), *failed* (any other error, with its errno), *vanished* (gone between being listed and being read) or *cancelled* (not reached before the scan was stopped). A gap has no own totals. Any gap makes the scan partial. |
| **Skip** | A directory the scan deliberately does not descend: on *another volume*, a second *mount point* of a volume already walked, a *dataless* cloud placeholder, or an *excluded* system path. Skips are counted in coverage but do not make a scan partial. |
| **Scan scope** | The policy, as data, that decides skips: the devices a scan stays on and the paths it does not descend. |
| **Dataless** | A cloud placeholder whose contents are not on this disk (`SF_DATALESS`). Reading it would trigger a download, so it is never descended. |
| **Picture** | The directory rows of the last finished scan held by the index, with that scan's root, times and coverage. A new scan replaces it in one transaction. |
| **Live tree** | The directories at most three levels below the root, with counters that grow while a scan runs, for showing progress. Bounded in size. |
| **Directory facts** | What the walker knows about a directory it has just read: its path, its entries, the parent's and grandparent's marker entries, whether it is inside a Git repository, and its probes. The input of the classifier. |
| **Unaccounted** | Used space on the scanned volume that the scan did not find: used (total − available) minus the scanned tree size. Includes other volumes of the container, snapshots, purgeable space and gaps; negative when clones are counted twice. |
| **Freshness** | When the picture was last fully scanned and last updated, how many changes are pending, what the engine is doing, whether the picture is complete, whether changes are followed and whether a full scan is needed. The UI reads it; it never guesses. |
| **Engine** | The one long-lived object that owns the index, answers from the saved picture at once, and keeps the picture fresh: full scans, rescans and cleanups run on its scheduler, one at a time. |
| **Scheduler** | The engine's one thread that changes the picture. It sleeps until a command comes or the coalescer's next task is due; it never polls. |
| **Wake-up** | One turn of the scheduler: a command, a batch of events or a deadline. An idle engine has none. |
| **Coalescer** | The pure state machine that turns file-system events into a bounded set of due rescan tasks. |
| **Settle time** | How long a directory must be quiet before it is rescanned. Grows while the directory keeps changing, up to the *cap*; nothing waits longer than one cap. |
| **Batch** | The due tasks the coalescer hands out at once (at most the profile's batch size), applied one after another, deepest first. |
| **Rescan task** | A request to re-read one directory (*shallow*), a whole subtree (*recursive*), or the whole root (*full*). |
| **Shallow rescan** | Re-reading one directory and comparing it with what the picture holds: unchanged subdirectories keep their stored subtrees, new or changed ones are walked. |
| **Origin** | Where a walk starts: the scan root, or one directory of a picture, which inherits from the stored rows above it what an unbroken walk would have handed down. |
| **Passed down** | What a read directory hands to the classification of the directories below it: its marker entries, whether it is in a repository or in a candidate, and its area. Stored in its row. |
| **Change stamp** | A directory's ctime in nanoseconds. It moves with every change of the directory's entries, permissions or owner; an unchanged stamp means an unchanged directory. |
| **In-place update** | Applying one rescan to the picture in one transaction; the totals above, the bytes by kind and the coverage are recomputed from the rows. |
| **Event history** | The FSEvents record of changes on a volume, with ids and a UUID. The engine stores the id after which nothing is pending; a restart resumes there when the UUID is the same and the id is not ahead. Otherwise the history is *lost* and a full scan is needed. |
| **Hold** | After the user cancels a scan, the engine starts no full scan on its own until the user starts one. |
| **Removability** | What a list shows about a worktree: unknown (not computed yet), removable, or blocked with the reasons. For display only. |
| **Power profile** | `Interactive`, `Background`, `Saver` or `Off`: how many threads, how long changes settle, how big a batch, the pause between batches, whether the engine may start a full scan on its own and at what priority. One table. |
| **Worktree status** | The result of inspecting a linked Git worktree with stock Git commands: removable, or blocked for at least one reason (uncommitted, untracked, unpushed, locked, prunable, an operation in progress, in use, managed by another tool, not listed, inspection failed). |
| **Managed worktree** | A worktree owned by another tool: Codex Desktop (`~/.codex/worktrees/…`) or Claude Code (`…/.claude/worktrees/…`). Shown, never removed; its build output is never preselected and is cleaned only when confirmed by itself. |
| **Cleaned directory** | A directory a cleanup acted on since the last scan; the index offers nothing at or below it until the next scan. |
| **Fixture** | A synthetic directory tree marked with a `.metla-fixture` file, the only place development builds and the CLI may delete. `metla-cli fixture create` makes one. |
| **Production capability** | The value that lets a cleaner act on a root that is not a fixture. Only a release build with the `production` feature can make it; the CLI never does. |
