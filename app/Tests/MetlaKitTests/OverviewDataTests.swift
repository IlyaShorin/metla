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
    func overview(state: EngineState = .idle, picture: Completeness = .complete, volume: VolumeSpace? = VolumeSpace(total: 1000, available: 50)) -> Overview {
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
        let coverage = CoverageSummary(
            gaps: [GapCount(kind: .denied, count: 2), GapCount(kind: .failed, count: 1)],
            skips: [], shallowest: [])
        let freshness = Freshness(fullScan: 1, updated: 2, pending: 0, state: state, picture: picture, watching: true, needsFullScan: false, fullScansEnded: 1)
        return Overview(
            root: "/", volume: volume, totals: nil, categories: tallies,
            reclaimable: Reclaimable(freed: Tally(count: 4, bytes: 700), trashed: Tally(count: 1, bytes: 10)),
            kinds: kinds, coverage: coverage, freshness: freshness, profile: .background)
    }

    func make(_ overview: Overview, volume: VolumeInfo? = nil, largest: [TopFolder] = [], home: String = "/") -> OverviewData {
        OverviewData.make(overview, largest: largest, volume: volume ?? volumeRoot, home: home, names)
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
        let russian = OverviewData.make(analysed, largest: [], volume: folder, home: "/Users/u", Names(Strings(language: "ru", region: "RU"), titles: []))
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

    @Test func aPartialPictureHasABannerAndARunningScanSaysScanning() {
        let partial = make(overview(picture: .partial))
        #expect(partial.banner == .partial(denied: 2, missing: 1))
        #expect(!partial.growing)
        let progress = ProgressSnapshot(dirs: 1, files: 1, bytes: 1, denied: 0)
        let scanning = make(overview(state: .scanning(progress: progress, interactive: false), picture: .partial))
        #expect(scanning.banner == .scanning, "a scan that runs is not shown as partial")
        #expect(scanning.growing, "totals still growing")
        #expect(make(overview()).banner == nil)
    }
}
