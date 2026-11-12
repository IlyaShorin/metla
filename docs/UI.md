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
* Bottom: **Last report** (only after a cleanup has run; the engine keeps
  it across launches).
* Foot of the sidebar, on every screen, below Last report, while there is
  an update to offer: a compact strip.
  * "Metla 0.8.1 is available" with **Install** (prominent), **What's
    New** (a link, only when the release has a page of the repository's
    releases) and **Later** (a link: hidden until a check finds the
    release again; no "skip this version"). While a cleanup runs, Install
    is disabled with a note ("install once it has ended"); it does not
    queue.
  * A copy that cannot replace itself: the reason in one line (built from
    source, run from a temporary place, a read-only volume, a folder it
    cannot write) and **Open the Release Page** instead of Install, with
    Later.
  * Under way: "Downloading 1.2 MB of 2.7 MB" with a bar, "Checking the
    signature", "Installing" (an indeterminate bar); no buttons.
  * Failed: one sentence with the reason, from a closed list (no
    connection; the download failed or was cut short; it does not match
    what GitHub lists; "the download is not signed by this app's signer —
    nothing was installed"; Metla cannot write to its folder; a cleanup is
    running) with Open the Release Page and Later.
  * The buttons stand in one row; where they do not fit (Russian), Install
    or the release page above the links.

**App menu**

"Check for Updates…" under "About Metla": checks at once and opens
Settings → General, where the answer shows. No system notification, no
badge, no sound, anywhere.

**Toolbar** (all screens)

* Scan status: a coloured dot plus text. States:
  `Scanning 61% · 1.9M files` (with a Stop button) for a first scan,
  `Refreshing 61% · 1.9M files` for a full scan while a picture exists
  (the last picture stays shown and usable, Clean included: a cleanup
  starts at once and the scan is done again after it),
  `Updated 2 min ago`, `Updated 2 min ago · partial`,
  `Never scanned`, `Background analysis off`. A picture whose only gaps
  are folders closed to every app (Full Disk Access granted) is not
  marked partial.
* **Refresh** (⌘R, a full scan run as `Interactive`) and **Clean…** (⌘⌫,
  opens the preview of the default selection: every preselected
  candidate). **Clean selected…** previews the cleanup selection: what the
  category tables check and what Folders added (⌘⌫ there).
* The engine follows changes at once while the window is visible (its
  attention); otherwise the power profile decides.

**Overview**

1. Partial-analysis banner when `Coverage` has gaps, by Full Disk Access
   as probed (one pure decision, `OverviewData.notice`):
   * not granted or cannot tell: what is missing, why, buttons "Show
     folders" (opens the list of inaccessible areas) and "Open access
     settings"; when no folder is denied, the banner of unread folders
     below, without the access button;
   * granted, and the only gaps are denied folders: no banner. They are
     *closed folders*, closed to every app whatever the setting; a quiet
     line under the disk card's legend counts them ("279 system folders
     are closed to every app and are not counted.") with a "Show folders"
     link;
   * granted, with other gaps (failed, vanished, cancelled): a banner that
     counts those and the closed folders apart, with "Show folders" and
     no access button;
   * granted, but the picture holds as denied a folder the probe has just
     listed (it was made before access was granted): the denied folders
     count as unread in that banner, never as closed, and one full scan
     per launch, started as Refresh starts it, reads the picture again
     (the Overview says "Refreshing…" meanwhile).

   Never shown as an error when the scan is merely
   in progress — that state says "Scanning…" (the numbers grow) for a
   first scan and "Refreshing in the background…" when the last picture
   stays shown.
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
* Git worktrees replace the Rule column with branch (blocked ones keep
  theirs) and state (Clean, Uncommitted, Untracked, Unpushed, Locked, In
  use, Managed by Codex/Claude), the state taking the flexible width,
  every reason in its tooltip and the selected row's reasons in the
  footer. Only removable worktrees have an enabled checkbox.
* Checkboxes are drawn with SwiftUI shapes in the accent colour (they
  look like the system's and the capture mode can draw their marks).
* Build output inside a worktree is listed under Build artifacts like any
  other. Inside a worktree Codex or Claude manages it is never checked by
  default, carries a "managed worktree" note, and the preview asks for
  each such item on its own.

**Folders**

An outline of the whole disk, lazily loaded, sorted by size. Columns: name
with disclosure triangle, share of parent (bar), item count, size.
Directories expand down to files; a directory lists at most its 10,000
largest files and one "N more" row for the rest. Badges: rule title on
candidates, "no access" on denied directories, "skipped" on excluded
mounts and cloud placeholders. Footer shows the selection and key hints.

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

Quick-eligible items form one group the main confirmation covers. Every
other group (Move to Trash, Empty the Trash, git worktree remove) has a
confirmation checkbox of its own, unchecked by default; an item inside a
managed worktree has a checkbox of its own. The destructive button
("Remove N Items", "Move N Items to Trash" when only the Trash group is
covered) enables once the confirmation covers at least one item — what
it covers is the core's decision (`covered`); groups left unchecked are
sent as not confirmed and come back "Skipped: not confirmed". An item
inside another selected item is set aside ("Inside ~/…"), and blocked
worktrees are set aside with their reasons. Paths show the first 12 of a
group, "Show all N" for the rest (a lazy list). Return and Esc cancel;
⌘↩ or a click on the destructive button acts.

**Progress and report**

During cleanup: progress bar, "N of M", removed so far, the current
item, Cancel (the cleanup stops before the next entry; the report says
what was done). Until the engine has started it the bar is
indeterminate. The bytes removed grow while an item is being removed,
and the bar shows the larger of the share of items done and the share
of the preview's estimate removed, so one big cache does not look
frozen. Afterwards a report: three numbers side by side —
**Found**, **Removed**, **Freed on disk** (measured; one sentence why it
may differ) — then "moved to the Trash, not freed until the Trash is
emptied" where applicable, a "Problems only" filter and per-item
outcomes (Removed, Partly removed with what stayed, Skipped + reason,
Failed + error, Already gone, Moved to the Trash) with Reveal in Finder
for what remains. Freed space that did not grow (zero or less) is said
in words, never shown as a negative amount. Reasons are named by kind
in the app's language (what stayed: no permission, in use, read-only
volume; a failure: not moved to the Trash, Git did not remove the
worktree), with the errno or the Trash's or Git's own words as a
detail. Items left unconfirmed or cancelled carry a neutral mark and
are not problems; skipped by a check, partly removed and failed items
are.

The preview's title says what its destructive button will do with the
boxes as they are: the count and size it covers ("Move N items to the
Trash" when only that group is covered, "Nothing is checked to remove"
when nothing is).

## Global hotkeys

Carbon `RegisterEventHotKey` (no Accessibility permission). **Clean**
(default ⌃⌥⌘⌫) and **Show Metla** (unassigned). In "Show and confirm"
mode the Clean hotkey brings Metla forward with the preview of the
default selection (in a window of its own when the main window is
closed). In "Clean immediately" mode it quick-cleans the checked quick
categories and shows a small non-activating panel: found, removed,
freed, problems, "Show report"; with no picture, nothing to clean or a
cleanup running it only says so.

## Menu bar panel

`MenuBarExtra` in window style, ~360 pt wide. Icon: a broom SF-Symbol-style
template image; shows activity while scanning.

* Volume name, free space, a compact usage bar, "used X of Y" and
  "can be cleaned ≈ Z" (deleted for good), what would go to the Trash
  apart.
* Quick-eligible categories with what cleaning them frees; one line for
  the rest ("only with confirmation").
* **Clean…** or **Clean now** by the hotkey mode (primary, shows the
  hotkey), **Open Metla**, **Refresh now**.
* While there is an update to offer, one line above the status line: the
  offer and Install (the release page for a copy that cannot replace
  itself), or the progress, or the failure, as the sidebar's strip says
  it.
* Status line: freshness, partial flag, current power profile.
* Background analysis checkbox, Settings…, Quit.
* It reads the cached snapshot only. While it is open the engine has
  the app's attention. The app stays in the menu bar when its window
  closes, unless "Show in menu bar" is off.

## Settings

Tabs: **General**, **Hotkeys**, **Exclusions**, **Access**.

* General: background analysis on/off (default on); "Reduce activity on
  battery" (default on); launch at login (`SMAppService.mainApp`, its
  error shown when the system refuses); worktree staleness age and
  recent build-output age in days (− / +); show in menu bar. Then
  "Updates": "Check for updates automatically" (default on) with what it
  sends; one row with the running version and build, when it last checked
  (and, when it found nothing, "Metla is up to date." or why the check
  failed) and **Check Now**; while there is an offer, the sidebar's strip
  under it, with the same buttons. The Settings window is 620 points tall,
  for this tab with an offer in Russian.
* Hotkeys: recorders for "Clean" and "Show Metla" (a combination without
  ⌘, ⌥ or ⌃ is refused; one another app holds is shown next to the
  recorder); a radio pair "Show what was found and ask to confirm"
  (default) / "Clean the checked categories immediately" — choosing the
  second asks first: "the checked categories are removed permanently,
  without a list, every time the hotkey is pressed", confirmed with a
  destructive button; three checkboxes for the quick categories, enabled
  only in the second mode; a note that logs, worktrees and temporary data
  can never be added.
* Exclusions: kept paths with what each hides ("hides 2 items · 1 MB"),
  Add…, Remove and drag-and-drop from Finder.
* Access: Full Disk Access as probed (granted, not granted, cannot
  tell), what is skipped without it, a button that opens the right
  System Settings pane, Check Again, and a note that Metla may need a
  restart after access is granted. With access granted it says instead
  that some system folders stay closed to every app, and drops the
  restart note. The probe runs at launch, whenever the app becomes
  active and after each full scan.
* Toggles are checkboxes (`SymbolCheckbox`) rather than switches.

## First run

A single sheet, once: what Metla does, that it only reads metadata, why
Full Disk Access matters, buttons "Open System Settings" and "Scan without
it"; when access is granted already, that it is, and one button,
"Continue". The first scan starts immediately either way and the Overview fills
in progressively; without Full Disk Access its partial banner says what
could not be read.

## Power

The app maps the Mac's power state to the engine's profile with one
table (`PowerState.profile`): background analysis off → Off; a scan the
user asked for → Interactive for that scan; Low Power Mode, a serious or
critical thermal state, or battery with "Reduce activity on battery" →
Saver; otherwise Background. It learns of changes from IOKit's power
source notifications, Low Power Mode and thermal state notifications; it
never polls. App Nap is left alone; only a scan the user asked for and a
cleanup hold an activity. The first scan of a disk the engine has no
picture of (the first run, a lost index) is the one the user waits for:
the app takes it over as the user's scan, so it runs as Interactive and
App Nap cannot slow it while the window is behind others. The user's
scan is over when the engine counts the full scan it asked for as ended
(completed, stopped or failed) and none runs.
