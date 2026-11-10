import Foundation
import MetlaCoreFFI
import Testing
@testable import MetlaKit

/// What the Overview shows, worked out from made-up engine answers.
@Suite struct OverviewDataTests {
    let names = Names(Strings(language: "en", region: "US"), titles: [])

    func overview(state: EngineState = .idle, picture: Completeness = .complete, volume: VolumeSpace? = VolumeSpace(total: 1000, available: 50)) -> Overview {
        let sizes: [(MetlaKit.Category, Disposal, UInt64)] = [
            (.appCache, .deletePermanently, 100), (.devCache, .deletePermanently, 200),
            (.buildArtifact, .deletePermanently, 400), (.log, .moveToTrash, 10),
            (.gitWorktree, .gitWorktreeRemove, 30), (.temp, .moveToTrash, 0)]
        let tallies = sizes.map { CategoryTally(category: $0.0, disposal: $0.1, tally: Tally(count: $0.2 == 0 ? 0 : 1, bytes: $0.2)) }
        let kinds = [KindBytes(kind: .media, bytes: 300), KindBytes(kind: .other, bytes: 100)]
        let coverage = CoverageSummary(
            gaps: [GapCount(kind: .denied, count: 2), GapCount(kind: .failed, count: 1)],
            skips: [], shallowest: [])
        let freshness = Freshness(fullScan: 1, updated: 2, pending: 0, state: state, picture: picture, watching: true, needsFullScan: false)
        return Overview(
            root: "/", volume: volume, totals: nil, categories: tallies,
            reclaimable: Reclaimable(freed: Tally(count: 3, bytes: 700), trashed: Tally(count: 1, bytes: 10)),
            kinds: kinds, coverage: coverage, freshness: freshness, profile: .background)
    }

    @Test func theDiskCardStacksCategoriesOtherDataAndFree() throws {
        let data = OverviewData.make(overview(), largest: [], volume: "Disk", home: "/Users/u", names)
        let disk = try #require(data.disk)
        #expect(disk.name == "Disk" && disk.used == 950 && disk.total == 1000 && disk.free == 50)
        #expect(disk.low, "under a tenth free")
        let segments = disk.segments.map { ($0.name, $0.bytes) }
        #expect(segments.map(\.0) == ["App caches", "Developer caches", "Build artifacts", "Logs, worktrees, temporary", "Other data", "Free"])
        #expect(segments.map(\.1) == [100, 200, 400, 40, 210, 50])
        #expect(disk.segments.last?.share == 0.05)
        let roomy = OverviewData.make(overview(volume: VolumeSpace(total: 1000, available: 100)), largest: [], volume: "D", home: "/", names)
        #expect(roomy.disk?.low == false, "exactly a tenth is not low")
        #expect(OverviewData.make(overview(volume: nil), largest: [], volume: "D", home: "/", names).disk == nil)
    }

    @Test func canBeFreedListsEveryCategoryWithHowItGoes() {
        let data = OverviewData.make(overview(), largest: [], volume: "D", home: "/", names)
        #expect(data.freed == 700 && data.trashed == 10)
        #expect(data.categories.map(\.note) == ["permanently", "permanently", "permanently", "to Trash", "git worktree remove", "to Trash"])
        #expect(data.categories.map(\.share) == [0.25, 0.5, 1, 0.025, 0.075, 0])
        #expect(data.categories[2].category == .buildArtifact)
        #expect(data.kinds.first?.share == 0.75)
    }

    @Test func theLargestFoldersAreShownBelowHome() {
        let largest = [TopFolder(path: "/Users/u/code", bytes: 80, files: 2, counting: false),
                       TopFolder(path: "/Applications", bytes: 40, files: 1, counting: false)]
        let data = OverviewData.make(overview(), largest: largest, volume: "D", home: "/Users/u", names)
        #expect(data.largest.map(\.name) == ["~/code", "/Applications"])
        #expect(data.largest.map(\.share) == [1, 0.5])
    }

    @Test func aPartialPictureHasABannerAndARunningScanSaysScanning() {
        let partial = OverviewData.make(overview(picture: .partial), largest: [], volume: "D", home: "/", names)
        #expect(partial.banner == .partial(denied: 2, missing: 1))
        #expect(!partial.growing)
        let progress = ProgressSnapshot(dirs: 1, files: 1, bytes: 1, denied: 0)
        let scanning = OverviewData.make(overview(state: .scanning(progress: progress, interactive: false), picture: .partial), largest: [], volume: "D", home: "/", names)
        #expect(scanning.banner == .scanning, "a scan that runs is not shown as partial")
        #expect(scanning.growing, "totals still growing")
        let complete = OverviewData.make(overview(), largest: [], volume: "D", home: "/", names)
        #expect(complete.banner == nil)
    }
}
