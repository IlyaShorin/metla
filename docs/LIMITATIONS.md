# Limitations

What Metla does not do, does not know, or has not been shown to do, each
with what follows from it. Numbers are in [PERFORMANCE.md](PERFORMANCE.md),
the gates in [QUALITY.md](QUALITY.md).

## Distribution

1. **No Developer ID, not notarised, no hardened runtime.** Releases up
   to 0.7.2 are ad-hoc signed; later releases are signed with the
   maintainer's Apple Development certificate and a secure timestamp. A
   copy you build yourself is ad-hoc signed unless `METLA_SIGN_IDENTITY`
   names a certificate of your keychain, and opens normally; a downloaded
   copy is quarantined either way, and macOS asks you to allow it once in
   System Settings → Privacy & Security. macOS ties Full Disk Access to
   the signature: the grant survives an update from one certificate-signed
   release to the next, and has to be given again after an update from
   0.7.2 or earlier, after every ad-hoc build, and when switching between
   a release and a copy built from source. The certificate is a
   development one; it expires and is replaced about once a year. Not
   checked: a certificate-signed build on a second Mac, and what becomes
   of the grant when the certificate is replaced by a renewed one. Since
   0.8.0 the app offers updates itself and installs one only into a copy
   signed by the same certificate (item 33); an ad-hoc copy only links to
   the release page.
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
   probed by listing a few protected folders (at launch, when the app
   becomes active, after a full scan ends); when that is inconclusive the
   app says it cannot tell, and treats it as not granted.
   **Even with Full Disk Access some system folders are never read:**
   root's own with mode 700 (`EACCES`) and those the system protects
   (`EPERM`: Spotlight's index, data vaults). They are not counted, so
   every size is a lower bound by them. With access granted the Overview
   counts them in a quiet line of the disk card, not in a banner, and the
   status does not mark the picture partial for them. A picture taken
   before access was granted is told by the probe's own folders: when the
   picture holds one of them as denied although the probe has just
   listed it, its denied folders are counted only as unread, and one full
   scan per launch (started as Refresh starts it) reads it again. Looking
   a folder up lists its own entries once more through the engine. The
   engine counts every denied folder alike (`EACCES` and `EPERM`, a system
   folder and one only this user cannot read): with access granted and a
   picture that holds none of the probe's folders as denied, *every*
   denied folder is called closed to every app, also another user's home
   folder. When the probe's folders are not in the picture (another root,
   or a picture whose scan did not reach them), an outdated picture is not
   recognised.
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
    was never done on the development machine. What the Overview and the
    status say with access granted, and the one full scan for a picture
    taken before access, are checked by tests with a probe that answers
    "granted" (on a fixture whose folder of mode 000 the probe claims to
    have listed), not by the capture: the development build is not
    granted access, and the capture's fixture has no denied folder. The
    new banner, the disk card's line, the first-run sheet and Settings →
    Access with access granted were drawn once (0.7.2, English and
    Russian, light and dark) by a capture changed for the purpose and not
    kept.
17. **A person's click on the menu bar.** The capture opens the real
    menu-bar panel by clicking the app's own status item in-process and
    takes the window server's image of it; a click from outside the app
    (the system's event path) was not simulated.
18. **Removal on the internal SSD was measured on a synthetic tree
    only** (300,000 files of 4 KB, the removal function on its own, warm
    cache, two runs each on a loaded machine; see
    [PERFORMANCE.md](PERFORMANCE.md)). A cleanup of real caches through
    the app was not timed, and the live progress was seen only in tests,
    not on a real cleanup.
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
22. **No complete mutation run on the final commit.** Long runs were
    stopped by the user's decision at the end of M7: the last complete
    `MUTATE_ALL` run is at 88cd5b2 (850 of 852 mutants killed, the two
    survivors fixed afterwards). On the final Rust sources only the 53
    mutants in functions changed since ran again (two differential
    runs, 53 and 26 mutants), all killed; the other verdicts are that
    run's, 18 of them kills by timeout on a loaded machine, not checked
    again unless they are among those ([QUALITY.md](QUALITY.md)).
23. **The first scan of the real disk through the app was measured once
    each way** (199 s taken over as the user's scan; not ended after
    27 minutes before), with the app behind other windows; never with
    the app active. Since 0.7.1 the engine starts that first scan on
    `Interactive`'s threads (8 on the machine measured) instead of
    `Background`'s two; its time through the app on the real disk has
    not been measured again.
24. **Most app measurements on the real disk are single runs or were not
    taken** (idle in the menu bar, churn with the app running, a full
    rescan, repeated launches): see [PERFORMANCE.md](PERFORMANCE.md).
25. **A Refresh pressed while the engine starts a scan of its own.**
    The app decides which full scan the user waits for from a reading
    taken just before it asks for the scan. If the engine starts a full
    scan of its own between the two, the app gives the interactive
    profile and its activity back one scan early; the scan asked for
    still runs. No test pins this. The Swift gate failed in some runs
    before the last three timing defects were fixed and passed eight
    times in a row afterwards, which does not prove that none is left.
26. **`scripts/quality-swift.sh` has no full-disk guard**, unlike
    `scripts/quality.sh`: on a volume that runs full its tests fail for
    reasons that are not in the code.
27. **Events below what a cleanup removed are dropped for a while.**
    During a cleanup and for the stream's latency plus 5 s after it,
    events below the items it began are not applied: the cleanup walks
    those directories again itself. Something written there in that
    window by another program is seen only at its next change there, or
    at the next full scan. Lost-history events (wrapped ids, a changed
    root) are never dropped.
28. **Other volumes mounted below the root are ignored from the moment
    they are seen mounted;** the list is read again when a volume is
    mounted, but a mount point that later becomes an ordinary directory
    of the root's volume stays ignored until the engine is opened again.
29. **Preemption and the event flood were measured on a synthetic tree
    only** (PERFORMANCE.md, 0.7.1): a cleanup through the app on the
    real disk, and the full scan the user saw after one, were not
    reproduced on the real disk; which reason started that scan is not
    known (the engine now writes it to stderr when a full scan starts).
30. **Lost events are replayed from fseventsd's history, measured once**
    (a replay after a provoked `UserDropped` named every change made
    meanwhile); the replay through the engine was tested with an
    injected loss, not with a real one, and the fallback after three
    replays in a row that did not catch up is tested only by its pure
    table (`on_loss`): no test made three losses arrive before a replay's
    end. When that fallback happens, or the kernel loses events, a full
    scan follows, no sooner than 30 minutes (10 while the window is
    open) after the last one; the gap and the quiet were chosen, not
    measured.
31. **The stream leaves out at most eight directories** (fseventsd's
    limit): the data directory and the other volumes below the root,
    those outside `/System` first; any more are filtered only after
    delivery, so their churn still fills the stream's queue. The stream
    is one on the root's path, not one per device: the system and data
    volumes report one device, and the data volume's events come under
    their firmlinked paths, which a stream relative to a device would
    name otherwise.
32. **A development build shares the installed app's bundle id**
    (`org.metla.Metla`). Its own settings go to a suite of their own on
    a fixture (`org.metla.Metla.dev`), and `run-app.sh` and
    `run-scenarios.sh` start it with `-ApplePersistenceIgnoreState YES`,
    so it neither reads nor writes the window state macOS restores for
    that id. But what AppKit and SwiftUI keep in the id's own preferences
    (window frames, the sidebar's width) is written there by every
    development run, and a debug app started by hand without that
    argument still reads and writes the installed app's window state: a
    crash of such a run can leave the installed app's next launch at
    macOS's "reopen windows?" question.

## Updates

33. **What the in-app update can and cannot do.**
    * It installs only what satisfies the running copy's own designated
      requirement. A copy signed ad-hoc (built from source) cannot be
      updated in place: its requirement is its own hash. Nor can a copy
      macOS runs translocated, one on a read-only volume, or one whose
      bundle or folder this user cannot write; each only links to the
      release page. Nothing asks for an administrator's password.
    * The designated requirement codesign writes for an Apple
      Development certificate names the app's identifier, the leaf
      certificate's name and Apple's intermediate, not the certificate
      itself: a renewed certificate with the same name is expected to
      satisfy it, which was not checked. A build signed with another
      certificate is refused as "not signed by this app's signer".
    * The signature is checked strictly, every architecture and nested
      code included, but without revocation: a revoked certificate's
      build would still be accepted. The download is not quarantined
      (the app sets no quarantine on what it downloads), so Gatekeeper
      does not assess the new copy at its first launch; the requirement
      check stands in for it.
    * The swap is two renames: the old bundle aside into a folder the
      system makes for replacements on the same volume, the new one into
      its place. A crash between the two leaves no app at its place and
      the old one in that folder; when putting the old one back fails,
      the app says on stderr where it is and keeps that folder.
    * After Install the app quits; a small shell started by it waits for
      the old process to end (at most 60 s) and opens the bundle at the
      same path, with the app's own `METLA_` variables and nothing else
      of its environment.
    * GitHub's API answers at most 60 unauthenticated requests an hour
      per address; past that a check fails with "GitHub answered with
      error 403" (silently, when it was automatic).
    * Not checked by an automated run: an update between two
      certificate-signed builds (the maintainer's test, by a test feed:
      CONTRIBUTING), a real download from GitHub (possible only once a
      release after 0.8.0 exists), the relaunch itself (the scenario
      records it as a request), and the Full Disk Access grant seen to
      survive an update.
