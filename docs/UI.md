# Interface specification

Standard macOS look: system controls, system colours and materials, SF
Symbols, light and dark appearance, Dynamic Type-friendly layouts, full
keyboard navigation. No custom chrome. English is the base language; Russian
is shipped as a localisation.

## Main window

`NavigationSplitView`, minimum 900 × 560, remembers its frame.

**Sidebar**

* Section "Disk": **Overview**, **Folders**.
* Section "Can be cleaned": one row per category with its estimated size —
  App caches, Developer caches, Build artifacts, Logs, Git worktrees,
  Temporary data. Rows with nothing to clean are dimmed, not hidden.
* Bottom: **Last report** (only after a cleanup has run).

**Toolbar** (all screens)

* Scan status: a coloured dot plus text. States:
  `Scanning 61% · 1.9M files` (with a Stop button),
  `Updated 2 min ago`, `Updated 2 min ago · partial`,
  `Never scanned`, `Background analysis off`.
* **Refresh** (⌘R) and **Clean…** (⌘⌫, opens the preview for the current
  default selection).

**Overview**

1. Partial-analysis banner when `Coverage` has gaps: what is missing, why,
   buttons "Show folders" (opens the list of inaccessible areas) and
   "Open access settings". Never shown as an error when the scan is merely
   in progress — that state says "Scanning…".
2. Disk card: volume name, used / total, free (red when under 10 %), a
   stacked bar (one segment per cleanable category, "Other data", free) and
   a legend.
3. Two columns:
   * **Can be freed ≈ N** with the word "estimate" and one line about APFS
     clones/snapshots; per-category rows with a bar, size and disposal
     ("permanently", "to Trash", "git worktree remove"). Clicking a row
     opens the category.
   * **Largest folders**: top directories by tree size with a note that
     size is not a sign of junk; "All folders" link.
4. **By data type**: bytes per file kind (Applications, Developer, Media,
   Documents, Archives and disk images, System, Other).

**Category**

A table of candidates: checkbox, path (monospaced, middle-truncated), rule
title, activity (relative time; recent build output is highlighted and not
preselected), size. Sortable by size, activity and path; size descending by
default. Header shows "selected N of M · size" and **Clean selected…**.

* Row context menu and shortcuts: Reveal in Finder (⌘↩), Show in Folders,
  Exclude — keep this (⌘E), Quick Look is not offered.
* Space toggles the checkbox; ⌘A selects all; arrow keys move.
* Git worktrees add status columns: branch, state (Clean, Uncommitted,
  Untracked, Unpushed, Locked, In use, Managed by Codex/Claude). Only
  removable worktrees have an enabled checkbox; the others show why not.
* Build output inside a worktree is listed under Build artifacts like any
  other. Inside a worktree Codex or Claude manages it is never checked by
  default, carries a "managed worktree" note, and the preview asks for
  each such item on its own.

**Folders**

An outline of the whole disk, lazily loaded, sorted by size. Columns: name
with disclosure triangle, share of parent (bar), item count, size.
Directories expand down to files. Badges: rule title on candidates,
"no access" on denied directories, "skipped" on excluded mounts and cloud
placeholders. Footer shows the selection and key hints.

* → expands, ← collapses, ⌘↩ Reveal in Finder, ⌘⌫ add candidate to the
  cleanup selection, ⌘E exclude.
* A breadcrumb in the toolbar shows the selected path.

**Cleanup preview (sheet)**

Title "Remove N items — about X?". One group per disposal, each with a
badge saying exactly how removal happens ("Permanently, bypassing Trash",
"Move to Trash", "git worktree remove"), the rule titles, item count, the
paths (expandable to the full list) and size. A note that sizes are
estimates and that the report will separate found, removed and freed.
Buttons: Cancel (Esc, default focus) and a destructive button named after
the action. The destructive button is never the default button.

When the selection mixes quick-eligible categories with others, the sheet
shows them as separate groups and the other groups need their own
confirmation checkbox before the destructive button enables. An item inside
a managed worktree has a checkbox of its own; an item inside another
selected item is shown once (the outer one), and blocked worktrees are
listed apart with their reasons.

**Progress and report**

During cleanup: progress bar, current item, Cancel. Afterwards a report:
three numbers side by side — **Found**, **Removed**, **Freed on disk**
(measured) — then per-item outcomes (Removed, Partial, Skipped + reason,
Failed + error, Already gone) and "moved to Trash, not freed until the
Trash is emptied" where applicable.

## Menu bar panel

`MenuBarExtra` in window style, ~360 pt wide. Icon: a broom SF-Symbol-style
template image; shows activity while scanning.

* Volume name, free space, a compact usage bar, "used X of Y" and
  "can be cleaned ≈ Z".
* Quick-eligible categories with sizes; one line for the rest ("only with
  confirmation").
* **Clean…** (primary, shows the hotkey), **Open Metla**, **Refresh now**.
* Status line: freshness, partial flag, current power profile.
* Background analysis toggle, Settings…, Quit.

## Settings

Tabs: **General**, **Hotkeys**, **Exclusions**, **Access**.

* General: background analysis on/off (default on); "Reduce activity on
  battery" (default on); launch at login; worktree staleness age; recent
  build-output age.
* Hotkeys: recorders for "Clean" and "Show Metla"; a radio pair
  "Show what was found and ask to confirm" (default) / "Clean the checked
  categories immediately"; three checkboxes for the quick categories,
  enabled only in the second mode; a note that logs, worktrees and temporary
  data can never be added.
* Exclusions: a list of kept paths with + / − and drag-and-drop from Finder.
* Access: Full Disk Access state, what is skipped without it, a button that
  opens the right System Settings pane, and a re-check button.

## First run

A single sheet: what Metla does, that it only reads metadata, why Full Disk
Access matters, buttons "Open System Settings" and "Scan without it". The
first scan starts immediately and the Overview fills in progressively.
