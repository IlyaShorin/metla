import Foundation
import MetlaCoreFFI
import Testing
@testable import MetlaKit

/// What the Overview shows, worked out from made-up engine answers.
@Suite struct OverviewDataTests {
    let names = Names(Strings(language: "en", region: "US"), titles: [])
    let volumeRoot = VolumeInfo(name: "Disk", isRoot: true)
    let folder = VolumeInfo(name: "Disk", isRoot: false)

    /// Every category found, and what cleaning the preselected ones does:
    /// some build output is not preselected, the Trash's contents are
    /// deleted, logs go to the Trash, worktrees are never preselected.
    func overview(
        state: EngineState = .idle, picture: Completeness = .complete, volume: VolumeSpace? = VolumeSpace(total: 1000, available: 50),
        gaps: [GapCount] = [GapCount(kind: .denied, count: 2), GapCount(kind: .failed, count: 1)]
    ) -> Overview {
        let found: [(MetlaKit.Category, Disposal, UInt64)] = [
            (.appCache, .deletePermanently, 100), (.devCache, .deletePermanently, 200),
            (.buildArtifact, .deletePermanently, 400), (.log, .moveToTrash, 10),
            (.gitWorktree, .gitWorktreeRemove, 30), (.temp, .moveToTrash, 100)]
        let nothing = Tally(count: 0, bytes: 0)
        let preselected: [MetlaKit.Category: Reclaimable] = [
            .appCache: Reclaimable(freed: Tally(count: 1, bytes: 100), trashed: nothing),
            .devCache: Reclaimable(freed: Tally(count: 1, bytes: 200), trashed: nothing),
            .buildArtifact: Reclaimable(freed: Tally(count: 1, bytes: 300), trashed: nothing),
            .log: Reclaimable(freed: nothing, trashed: Tally(count: 1, bytes: 10)),
            .temp: Reclaimable(freed: Tally(count: 1, bytes: 100), trashed: nothing),
        ]
        // 100 bytes of build output lie inside a worktree: the bar
        // counts them with the worktree.
        let tallies = found.map {
            CategoryTally(
                category: $0.0, disposal: $0.1, tally: Tally(count: 1, bytes: $0.2),
                reclaimable: preselected[$0.0] ?? Reclaimable(freed: nothing, trashed: nothing),
                owned: $0.0 == .buildArtifact ? $0.2 - 100 : $0.2)
        }
        let kinds = [KindBytes(kind: .media, bytes: 300), KindBytes(kind: .other, bytes: 100)]
        let coverage = CoverageSummary(gaps: gaps, skips: [], shallowest: [])
        let freshness = Freshness(fullScan: 1, updated: 2, pending: 0, state: state, picture: picture, watching: true, needsFullScan: false, fullScansEnded: 1, scanReason: nil)
        return Overview(
            root: "/", volume: volume, totals: nil, categories: tallies,
            reclaimable: Reclaimable(freed: Tally(count: 4, bytes: 700), trashed: Tally(count: 1, bytes: 10)),
            kinds: kinds, coverage: coverage, freshness: freshness, profile: .background)
    }

    func make(
        _ overview: Overview, volume: VolumeInfo? = nil, largest: [TopFolder] = [], home: String = "/",
        denied: DeniedFolders = .kept
    ) -> OverviewData {
        OverviewData.make(overview, largest: largest, volume: volume ?? volumeRoot, home: home, denied: denied, names)
    }

    @Test func aVolumeRootStacksCategoriesOtherDataAndFree() throws {
        let disk = try #require(make(overview()).disk)
        #expect(disk.title == "Disk" && !disk.folder)
        #expect(disk.used == 950 && disk.total == 1000 && disk.free == 50)
        #expect(disk.low, "under a tenth free")
        let segments = disk.segments.map { ($0.name, $0.bytes) }
        #expect(segments.map(\.0) == ["App caches", "Developer caches", "Build artifacts", "Logs, worktrees, temporary", "Other data", "Free"])
        #expect(segments.map(\.1) == [100, 200, 300, 140, 210, 50], "the bar's own sizes: nothing twice")
        #expect(segments.dropLast().map(\.1).reduce(0, +) == disk.used)
        #expect(disk.segments.last?.share == 0.05)
        #expect(make(overview(volume: VolumeSpace(total: 1000, available: 100))).disk?.low == false, "exactly a tenth is not low")
        #expect(make(overview(volume: nil)).disk == nil)
    }

    @Test func aFolderIsShownAsTheFolderOnItsVolume() throws {
        var analysed = overview()
        analysed.root = "/Users/u/project"
        analysed.totals = TreeTotals(bytes: 900, files: 3, dirs: 2, newest: nil)
        let disk = try #require(make(analysed, volume: folder, home: "/Users/u").disk)
        #expect(disk.folder && disk.title == "~/project" && disk.analysed == 900)
        #expect(disk.volume == "Disk" && disk.free == 50)
        let segments = disk.segments.map { ($0.name, $0.bytes) }
        #expect(segments.map(\.0) == ["App caches", "Developer caches", "Build artifacts", "Logs, worktrees, temporary",
                                      "Other data in this folder", "Outside this folder", "Free"])
        #expect(segments.map(\.1) == [100, 200, 300, 140, 160, 50, 50], "outside: used 950 minus the folder's 900")
        #expect(segments.prefix(5).map(\.1).reduce(0, +) == 900, "the folder's segments add up to its size")
        let russian = OverviewData.make(analysed, largest: [], volume: folder, home: "/Users/u", denied: .kept, Names(Strings(language: "ru", region: "RU"), titles: []))
        #expect(russian.disk?.segments[5].name == "Вне этой папки")
        let unscanned = try #require(make(overview(), volume: folder).disk)
        #expect(unscanned.analysed == 0 && unscanned.segments[5].bytes == 950, "nothing analysed yet: all used space is outside")
    }

    @Test func canBeFreedRowsShowThePreselectedAmountsThatMakeTheTotal() {
        let data = make(overview())
        #expect(data.freed == 700 && data.trashed == 10)
        let rows = data.categories
        #expect(rows.map(\.bytes) == [100, 200, 300, 10, 0, 100], "what cleaning the preselected ones does")
        #expect(rows.reduce(0) { $0 + $1.bytes } == data.freed + data.trashed, "the rows add up to the header")
        #expect(rows.map(\.found) == [nil, nil, 400, nil, 30, nil], "found shown only where it differs")
        #expect(rows.map(\.note) == ["permanently", "permanently", "permanently", "to Trash", "git worktree remove", "permanently"],
                "by what cleaning does: the Trash's contents are deleted")
        let shares: [Double] = [1.0 / 3, 2.0 / 3, 1, 10.0 / 300, 0, 1.0 / 3]
        #expect(rows.map(\.share) == shares)
        #expect(rows[2].category == .buildArtifact)
        #expect(data.kinds.first?.share == 0.75)
    }

    @Test func theLargestFoldersAreShownBelowHome() {
        let largest = [TopFolder(path: "/Users/u/code", bytes: 80, files: 2, counting: false),
                       TopFolder(path: "/Applications", bytes: 40, files: 1, counting: false)]
        let data = make(overview(), largest: largest, home: "/Users/u")
        #expect(data.largest.map(\.name) == ["~/code", "/Applications"])
        #expect(data.largest.map(\.share) == [1, 0.5])
    }

    static let everyKind: [DeniedFolders] = [.kept, .closed, .outdated]

    @Test func aRunningScanSaysScanningWhateverTheAccess() {
        let progress = ProgressSnapshot(dirs: 1, files: 1, bytes: 1, denied: 0)
        for denied in Self.everyKind {
            let first = make(overview(state: .scanning(progress: progress, interactive: false), picture: .missing), denied: denied)
            #expect(first.notice == .banner(.scanning))
            #expect(first.growing, "totals still growing")
            for picture in [Completeness.partial, .complete] {
                let again = make(overview(state: .scanning(progress: progress, interactive: false), picture: picture), denied: denied)
                #expect(again.notice == .banner(.refreshing), "a scan that runs is not shown as partial")
                #expect(!again.growing, "the last picture's totals stay as they are")
            }
            #expect(make(overview(), denied: denied).notice == nil, "a complete picture has nothing to say")
        }
    }

    /// Without Full Disk Access, or when it cannot be told, the banner
    /// warns and offers the access settings.
    @Test func withoutFullDiskAccessAPartialPictureHasTheBannerThatAsksForIt() {
        let partial = make(overview(picture: .partial), denied: .kept)
        #expect(partial.notice == .banner(.partial(denied: 2, missing: 1)))
        #expect(partial.banner == .partial(denied: 2, missing: 1) && partial.closed == nil)
        #expect(!partial.growing)
        let deniedOnly = make(overview(picture: .partial, gaps: [GapCount(kind: .denied, count: 279)]), denied: .kept)
        #expect(deniedOnly.banner == .partial(denied: 279, missing: 0))
    }

    /// Without access, but with no folder denied: access would not help,
    /// so the banner neither says "no access to 0 folders" nor offers it.
    @Test func withoutFullDiskAccessButNothingDeniedTheBannerDoesNotAskForIt() {
        for kind in [GapKind.failed, .vanished, .cancelled] {
            let gaps = [GapCount(kind: .denied, count: 0), GapCount(kind: kind, count: 3)]
            #expect(make(overview(picture: .partial, gaps: gaps), denied: .kept).notice == .banner(.unread(missing: 3, closed: 0)))
        }
    }

    /// With access granted, a picture made before it holds as denied
    /// places access opens: its denied folders are only unread, never
    /// "closed to every app", until a full scan reads them again.
    @Test func aPictureMadeBeforeAccessWasGrantedDoesNotCallItsFoldersClosed() {
        let deniedOnly = make(overview(picture: .partial, gaps: [GapCount(kind: .denied, count: 279)]), denied: .outdated)
        #expect(deniedOnly.notice == .banner(.unread(missing: 279, closed: 0)))
        #expect(deniedOnly.closed == nil)
        #expect(make(overview(picture: .partial), denied: .outdated).notice == .banner(.unread(missing: 3, closed: 0)))
    }

    /// With Full Disk Access the folders still denied are closed to every
    /// app: a quiet line of the disk card says how many, no banner.
    @Test func withFullDiskAccessFoldersStillDeniedAreClosedToEveryAppAndRaiseNoBanner() {
        let gaps = [GapCount(kind: .denied, count: 279), GapCount(kind: .failed, count: 0),
                    GapCount(kind: .vanished, count: 0), GapCount(kind: .cancelled, count: 0)]
        let data = make(overview(picture: .partial, gaps: gaps), denied: .closed)
        #expect(data.notice == .closed(279))
        #expect(data.banner == nil, "nothing to warn of: access cannot open them")
        #expect(data.closed == 279)
    }

    /// With Full Disk Access, folders not read for another reason keep a
    /// banner, which counts the closed ones apart and does not ask for
    /// access.
    @Test func withFullDiskAccessOtherGapsKeepABannerThatDoesNotAskForAccess() {
        let data = make(overview(picture: .partial), denied: .closed)
        #expect(data.notice == .banner(.unread(missing: 1, closed: 2)))
        #expect(data.closed == nil, "the closed ones are said once, in the banner")
        for kind in [GapKind.failed, .vanished, .cancelled] {
            let gaps = [GapCount(kind: .denied, count: 0), GapCount(kind: kind, count: 3)]
            #expect(make(overview(picture: .partial, gaps: gaps), denied: .closed).banner == .unread(missing: 3, closed: 0))
        }
    }
}
