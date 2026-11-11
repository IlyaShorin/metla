import Foundation
import MetlaCoreFFI
import Testing
@testable import MetlaKit

/// The cleanup flow through the real view models and a real engine, each
/// test on a fixture of its own: what is checked is removed, nothing else.
@MainActor
@Suite struct CleanupTests {
    /// A started model on a fresh fixture, its worktree statuses known,
    /// moving to a Trash inside the fixture.
    static func fresh() async throws -> (AppModel, FixtureTrash) {
        let root = Fixture.make()
        let trash = FixtureTrash(root: root)
        let model = try await started(Fixture.settings(root: root), trash: trash)
        return (model, trash)
    }

    static func exists(_ model: AppModel, _ relative: String) -> Bool {
        FileManager.default.fileExists(atPath: (model.settings.root as NSString).appendingPathComponent(relative))
    }

    /// The directory is there and holds nothing: a rule that keeps its
    /// directory empties it.
    static func emptied(_ model: AppModel, _ relative: String) -> Bool {
        let path = (model.settings.root as NSString).appendingPathComponent(relative)
        return (try? FileManager.default.contentsOfDirectory(atPath: path))?.isEmpty == true
    }

    static func ids(_ model: AppModel, _ relatives: [String]) async throws -> [DirId] {
        let client = try #require(model.client)
        let all = await model.offered(client)
        return try relatives.map { relative in
            let path = (model.settings.root as NSString).appendingPathComponent(relative)
            return try #require(all.first { $0.path == path }, "\(relative) is offered").id
        }
    }

    static func cleaned(_ cleanup: CleanupModel) async throws {
        await cleanup.clean()
        #expect(cleanup.stage == .report)
    }

    @Test func onlyTheQuickGroupGoesUntilAnotherGroupIsChecked() async throws {
        let (model, _) = try await Self.fresh()
        defer { model.stop() }
        let cleanup = try #require(model.cleanup)
        await model.cleanDefault()
        let preview = try #require(cleanup.preview)
        #expect(cleanup.stage == .preview)
        let kinds = preview.groups.map(\.id)
        #expect(kinds == [.quick, .separate(.moveToTrash), .separate(.emptyTrash)])
        #expect(preview.groups[0].method == "Permanently, bypassing Trash")
        #expect(preview.groups[1].method == "Move to Trash")
        #expect(preview.canClean, "the quick group is covered by the main confirmation")
        let quick = preview.groups[0].rows.count
        #expect(preview.coveredTotals.count == quick)
        let strings = model.strings
        let quickBytes = Format.bytes(preview.groups[0].bytes, strings)
        #expect(preview.title == "Remove \(quick) items \u{2014} about \(quickBytes)?", "what the button does")
        #expect(preview.action == "Remove \(quick) Items")
        preview.confirm(.moveToTrash, true)
        let both = quick + preview.groups[1].rows.count
        let bothBytes = Format.bytes(preview.groups[0].bytes + preview.groups[1].bytes, strings)
        #expect(preview.title == "Remove \(both) items \u{2014} about \(bothBytes)?", "the title follows the boxes")
        preview.confirm(.moveToTrash, false)
        try await Self.cleaned(cleanup)
        #expect(Self.emptied(model, "home/Library/Caches/com.example.editor"))
        #expect(Self.exists(model, "home/Library/Logs/com.example.editor/today.log"), "logs not confirmed")
        #expect(Self.exists(model, "home/.Trash/old-installer.dmg"), "the Trash not confirmed")
        let report = try #require(cleanup.report)
        let skipped = report.rows.filter { $0.outcome == "Skipped: not confirmed" }
        #expect(skipped.count == 3, "two log folders and the Trash")
        #expect(skipped.allSatisfy { $0.tone == .untouched }, "not confirming is the user's choice")
        #expect(report.problems == 0 && report.rows(problemsOnly: true).isEmpty)
    }

    @Test func theButtonWaitsForASeparateGroupToBeChecked() async throws {
        let (model, trash) = try await Self.fresh()
        defer { model.stop() }
        let cleanup = try #require(model.cleanup)
        let logs = try await Self.ids(model, ["home/Library/Logs/com.example.editor"])
        await cleanup.open(logs)
        let preview = try #require(cleanup.preview)
        #expect(!preview.canClean, "nothing is covered yet")
        #expect(preview.title == "Nothing is checked to remove")
        await cleanup.clean()
        #expect(cleanup.stage == .preview, "a disabled button does nothing")
        preview.confirm(.moveToTrash, true)
        #expect(preview.canClean && preview.action == "Move 1 Item to Trash")
        let bytes = Format.bytes(preview.groups[0].bytes, model.strings)
        #expect(preview.title == "Move 1 item to the Trash \u{2014} about \(bytes)?")
        try await Self.cleaned(cleanup)
        #expect(!Self.exists(model, "home/Library/Logs/com.example.editor"))
        #expect(trash.moved.count == 1, "through the app's Trash")
        #expect(cleanup.report?.rows.first?.outcome == "Moved to the Trash")
    }

    @Test func anItemInAManagedWorktreeIsConfirmedByItself() async throws {
        let (model, _) = try await Self.fresh()
        defer { model.stop() }
        let cleanup = try #require(model.cleanup)
        let managed = "home/code/app/.claude/worktrees/feature/target"
        let chosen = try await Self.ids(model, [managed, "home/code/app/target"])
        await cleanup.open(chosen)
        let preview = try #require(cleanup.preview)
        let row = try #require(preview.groups.flatMap(\.rows).first { $0.ownConfirmation })
        #expect(row.warnings == ["Inside a worktree Claude Code manages"])
        #expect(preview.coveredTotals.count == 1, "the managed item waits for its own checkbox")
        try await Self.cleaned(cleanup)
        #expect(!Self.exists(model, "home/code/app/target"))
        #expect(Self.exists(model, managed + "/debug/app"), "not confirmed by itself")
        cleanup.close()
        let again = try await Self.ids(model, [managed])
        await cleanup.open(again)
        let second = try #require(cleanup.preview)
        second.confirmItem(row.id, true)
        #expect(second.canClean)
        try await Self.cleaned(cleanup)
        #expect(!Self.exists(model, managed))
    }

    @Test func aBlockedWorktreeAndAnItemInsideAnotherAreSetAside() async throws {
        let (model, _) = try await Self.fresh()
        defer { model.stop() }
        let cleanup = try #require(model.cleanup)
        let chosen = try await Self.ids(
            model, ["home/code/app-clean", "home/code/app-clean/target", "home/code/app-dirty"])
        await cleanup.open(chosen)
        let preview = try #require(cleanup.preview)
        #expect(preview.groups.map(\.id) == [.separate(.removeWorktrees)])
        #expect(preview.groups[0].method == "git worktree remove")
        let reasons = preview.setAside.map(\.reason).sorted()
        #expect(reasons == ["Inside ~/code/app-clean", "Uncommitted"])
        preview.confirm(.removeWorktrees, true)
        try await Self.cleaned(cleanup)
        #expect(!Self.exists(model, "home/code/app-clean"))
        #expect(Self.exists(model, "home/code/app-dirty/README.md"))
    }

    @Test func theReportShowsTheEnginesNumbersAndIsKeptAcrossLaunches() async throws {
        let (model, _) = try await Self.fresh()
        let cleanup = try #require(model.cleanup)
        await model.cleanDefault()
        let estimate = try #require(cleanup.preview).groups.reduce(0) { $0 + $1.bytes }
        try await Self.cleaned(cleanup)
        let report = try #require(cleanup.report)
        #expect(report.found == estimate, "found: the preview's estimate of every item, done or not")
        #expect(report.removed > 0 && report.removed <= report.found)
        let client = try #require(model.client)
        guard case .done(_, let totals)? = await client.lastReport() else {
            Issue.record("no report")
            return
        }
        #expect((report.found, report.removed, report.freed) == (totals.found, totals.removed, totals.freed))
        try await until { await model.refresh([.cleanup]); return model.lastReport == report }
        let settings = model.settings
        model.stop()
        let again = AppModel(settings: settings, strings: Strings(language: "en", region: "US"))
        await again.start(trash: FixtureTrash(root: settings.root))
        defer { again.stop() }
        #expect(again.lastReport == report, "kept by the engine")
    }

    @Test func runningTheSameCleanupAgainConvergesAndRemovesNothingMore() async throws {
        let (model, _) = try await Self.fresh()
        defer { model.stop() }
        let cleanup = try #require(model.cleanup)
        let cache = "home/Library/Caches/com.example.editor"
        let first = try await Self.ids(model, [cache])
        await cleanup.open(first)
        try await Self.cleaned(cleanup)
        #expect(cleanup.report?.rows.first?.kind == .removed && Self.emptied(model, cache))
        cleanup.close()
        try await until {
            await model.refresh([.picture])
            return (try? await Self.ids(model, [cache])) != nil
        }
        let again = try await Self.ids(model, [cache])
        await cleanup.open(again)
        try await Self.cleaned(cleanup)
        let report = try #require(cleanup.report)
        #expect(report.rows.map(\.kind) == [.removed], "the emptied directory again: removed")
        #expect(report.found == 0 && report.removed == 0, "nothing more to remove")
        #expect(Self.emptied(model, cache))
    }

    @Test func cancelStopsBeforeTheNextItemAndTheReportSaysSo() async throws {
        let root = Fixture.make()
        let gate = FixtureTrash(root: root)
        let model = try await started(Fixture.settings(root: root), trash: gate)
        defer { model.stop() }
        let cleanup = try #require(model.cleanup)
        gate.onMove = { [client = model.client] in client?.cancelNow() }
        await model.cleanDefault()
        let preview = try #require(cleanup.preview)
        preview.confirm(.moveToTrash, true)
        try await Self.cleaned(cleanup)
        let rows = try #require(cleanup.report).rows
        let cancelled = rows.filter { $0.outcome == "Skipped: cancelled" }
        #expect(!cancelled.isEmpty, "items after the cancel are skipped")
        #expect(rows.contains { $0.outcome == "Moved to the Trash" })
        for row in cancelled {
            #expect(FileManager.default.fileExists(atPath: row.path), "\(row.shownPath) untouched")
        }
    }

    @Test func aCleanedRowIsNoLongerCheckedOrSelected() async throws {
        let (model, _) = try await Self.fresh()
        defer { model.stop() }
        let table = try #require(model.category(.buildArtifact))
        await table.reload()
        let target = try #require(table.rows.first { $0.path.hasSuffix("/home/code/app/target") })
        for row in table.rows where row.id != target.id && table.isChecked(row.id) {
            table.toggle(row.id)
        }
        #expect(model.selection.ids == [target.id])
        await model.cleanSelected()
        let cleanup = try #require(model.cleanup)
        try await Self.cleaned(cleanup)
        try await until {
            await model.refresh([.picture, .cleanup])
            await table.reload()
            return !table.rows.contains { $0.id == target.id }
        }
        #expect(!table.isChecked(target.id) && model.selection.ids.isEmpty, "nothing stale stays selected")
    }

    @Test func progressIsAShareOfTheItemsDone() {
        #expect(CleanupProgressData(done: 0, items: 0, removed: 0, current: nil).share == 0)
        #expect(CleanupProgressData(done: 3, items: 4, removed: 10, current: "~/x").share == 0.75)
        #expect(CleanupProgressData(done: 5, items: 4, removed: 10, current: nil).share == 1)
    }

    @Test func theSystemTrashMovesAndSaysWhyNot() throws {
        let missing = Fixture.scratch().appendingPathComponent("not-there").path
        guard case .refused(let reason) = SystemTrash().trash(path: missing) else {
            Issue.record("a missing path cannot be moved")
            return
        }
        #expect(!reason.isEmpty)
    }

    @Test func aQuickCleanTakesOnlyTheChosenQuickCategories() async throws {
        let (model, _) = try await Self.fresh()
        defer { model.stop() }
        #expect(QuickCategory.all == [.appCache, .devCache, .buildArtifact], "nothing else can be asked for")
        let hud = await model.quickCleanNow([.appCache])
        #expect(hud.title == "Cleaned" && hud.report && hud.lines.count == 3)
        #expect(Self.emptied(model, "home/Library/Caches/com.example.editor"))
        #expect(Self.exists(model, "home/Library/Caches/Homebrew/downloads/tool.tar.gz"), "a developer cache")
        #expect(Self.exists(model, "home/code/app/target/debug/app"), "build output")
        #expect(Self.exists(model, "home/Library/Logs/com.example.editor/today.log"))
        let nothing = await model.quickCleanNow([.appCache])
        #expect(nothing.title == "Nothing to clean")
    }

    @Test func withoutTheFixtureMarkerTheDebugEngineCleansNothing() async throws {
        let root = Fixture.make()
        try FileManager.default.removeItem(atPath: (root as NSString).appendingPathComponent(".metla-fixture"))
        var settings = Fixture.settings(root: root)
        settings.isFixture = false
        let model = try await started(settings, trash: FixtureTrash(root: root))
        defer { model.stop() }
        #expect(!cleansBeyondFixtures())
        let client = try #require(model.client)
        let ended = client.cleanupsEnded
        try await client.executeConfirmed(1, Confirmations(groups: [.moveToTrash], items: []))
        try await until { client.cleanupsEnded > ended }
        guard case .notRun? = await client.lastReport() else {
            Issue.record("executeConfirmed ran")
            return
        }
        let hud = await model.quickCleanNow([.appCache, .devCache, .buildArtifact])
        #expect(hud.title == "Nothing was cleaned")
        let cleanup = try #require(model.cleanup)
        await cleanup.open([1])
        #expect(cleanup.stage == .closed && cleanup.failure != nil, "no preview of a root that is not a fixture")
        #expect(Self.exists(model, "home/Library/Caches/com.example.editor/cache.db"))
        #expect(Self.exists(model, "home/code/app/target/debug/app"))
    }
}
