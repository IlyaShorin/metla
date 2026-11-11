# Limitations

What Metla does not do, does not know, or has not been shown to do, each
with what follows from it. Numbers are in [PERFORMANCE.md](PERFORMANCE.md),
the gates in [QUALITY.md](QUALITY.md).

## Distribution

1. **Ad-hoc signed, not notarised, no hardened runtime.** There is no
   Developer ID behind the app. A copy you build yourself opens normally;
   a downloaded copy is quarantined and macOS asks you to allow it in
   System Settings → Privacy & Security. There are no automatic updates.
2. **One architecture per build.** `build-app.sh` builds the Swift side
   for the machine it runs on, and the Rust library for the Rust targets
   installed (here only `aarch64-apple-darwin`). The release app made here
   runs on Apple Silicon only; an Intel Mac needs its own build.
3. **No link-time optimisation.** The release profile does not enable LTO;
   the measured numbers are of that build.

## What the numbers mean

4. **Freed space is measured, not promised.** "Freed on disk" is the change
   in the volume's free space during the cleanup. Removing an APFS clone
   frees nothing while the original exists; a local snapshot keeps removed
   blocks until it expires; other programs write meanwhile. The report
   says when free space did not grow instead of showing a negative amount,
   and keeps found, removed and freed apart.
5. **Sizes are allocated-size estimates.** A file with several hard links
   counts its share per link (rounded down); clones and snapshots cannot be
   seen from metadata, so a folder's size can exceed what removing it frees
   and the sum of folders can exceed the used space ("Unaccounted" is then
   negative).
6. **Purgeable space is not shown as free.** Metla reports the volume's
   available space as `statfs` gives it; space macOS would purge on demand
   is part of "used".

## What it can see

7. **Without Full Disk Access the picture is partial.** macOS keeps Mail,
   Messages, Safari and other apps' data from every app; those folders are
   skipped, counted and named in the partial banner. Metla cannot grant the
   access itself and never works around it. Whether access is granted is
   probed by listing a few protected folders; when that is inconclusive the
   app says it cannot tell.
8. **One root, one volume.** The engine analyses `/` (the system and data
   volumes of the boot container). Other volumes, disk images, network
   shares and cloud placeholders are skipped, not analysed; iCloud Drive
   files that are only in the cloud are not descended into, so nothing is
   downloaded and their local bytes, if any, are not counted.
9. **The picture can lag behind the disk.** While nobody looks, a change
   waits for the stream's latency (15 s) and the pause between batches
   (60 s Background, 180 s on battery or in Low Power Mode), by design. When
   `fseventsd` is late or drops events (seen at a full core after the build
   volume's FSEvents journal grew to 2.8 GB), the engine falls back to full
   rescans and the picture is right only after them.
10. **Changes while the app is not running** are replayed from the volume's
    FSEvents history when it is still the same; otherwise the next start
    makes a full scan.

## What it relies on

11. **Git.** Build output is offered only when Git says it is ignored and
    holds nothing tracked, and worktrees only when Git's status allows it.
    Without a working `git` (Xcode Command Line Tools), or in a repository
    Git refuses (owned by another user), nothing there is offered. Git's own
    messages in a report are shown in English (`LC_ALL=C`).
12. **The Trash.** Logs go to the Trash through `FileManager.trashItem`
    (so "Put Back" works); when the Trash refuses, the item fails and stays,
    with the system's message as a detail.
13. **Region formats.** Numbers and dates follow the user's region, also
    when the app's language is not the region's (English text with a
    decimal comma on a Russian-region Mac).

## Not verified by an automated run

14. **A real key press of a global hotkey.** Registration through Carbon is
    verified (the app prints what the system answered); pressing the keys
    and the app reacting end to end needs a person or a tool with
    Accessibility access, which was not granted or asked for.
15. **The login item.** `SMAppService.mainApp` registration is called and its
    error shown; starting at login after a restart was not observed.
16. **The Full Disk Access deep link.** The button opens the System Settings
    URL; that the right pane comes up was not checked, and granting access
    was never done on the development machine.
17. **A person's click on the menu bar.** The capture opens the real
    menu-bar panel by clicking the app's own status item in-process and
    takes the window server's image of it; a click from outside the app
    (the system's event path) was not simulated.
18. **Removal on the internal SSD.** Removal throughput was measured on a RAM
    disk (fixtures only ever live there); an SSD and APFS will differ.
19. **Cold-cache timings.** Purging the file-system cache needs root, so
    every scan and launch number is with a warm cache.

## Quality tools

20. **The Swift half is outside the ub-mode tools.** crapper, dryer and
    mutator analyse Rust only. Swift is held to the Swift 6 compiler in
    strict concurrency mode with warnings as errors, Swift Testing and
    llvm-cov line coverage of `MetlaKit`; the SwiftUI views and the AppKit
    glue (`MetlaApp`: hotkey adapter, HUD, power monitor, login item,
    capture) have no unit tests and are checked by building them, the
    scenarios and the captures.
21. **Two code paths are tested only through seams.** The classic listing
    (`readdir`) that `getattrlistbulk`'s `ENOTSUP` falls back to is driven
    by a fake call: no file system here refuses the bulk call. A kept
    directory whose entries cannot be listed for the Trash
    (`ActionFailure::Unlisted`) is not reached in tests: the check right
    before acting lists the same directory first.
