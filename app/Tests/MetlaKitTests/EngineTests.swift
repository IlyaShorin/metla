import Foundation
import MetlaCoreFFI
import Testing
@testable import MetlaKit

/// The view models against a real engine on a synthetic fixture.
@MainActor
@Suite struct EngineTests {
    @Test func theOverviewShowsTheFixtureAfterItsFirstScan() async throws {
        let model = try await started()
        defer { model.stop() }
        let sidebar = model.sidebar
        #expect(sidebar.map(\.name) == ["App caches", "Developer caches", "Build artifacts", "Logs", "Git worktrees", "Temporary data"])
        #expect(sidebar.allSatisfy { !$0.empty && $0.bytes > 0 })
        let data = try #require(model.overviewData)
        #expect(data.banner == nil, "a complete picture has no banner")
        #expect(data.disk != nil && data.freed > 0)
        #expect(data.categories.count == 6 && data.kinds.count == 7)
        #expect(!data.largest.isEmpty && data.largest.count <= 6)
        #expect(data.largest.allSatisfy { $0.name.hasPrefix("~") || $0.name.hasPrefix("/") })
        let status = try #require(model.status)
        #expect(status.tone == .current && status.text.hasPrefix("Updated"))
        #expect(model.failure == nil && !model.scanning)
        let disk = try #require(data.disk)
        #expect(disk.folder && model.volume.isRoot == false, "a fixture is a folder inside a volume")
        #expect(disk.title.hasSuffix("fixture") && disk.analysed == model.overview?.totals?.bytes)
        #expect(disk.segments.map(\.name).contains("Outside this folder"))
        #expect(!disk.volume.isEmpty)
        let rows = data.categories.reduce(0) { $0 + $1.bytes }
        #expect(rows == data.freed + data.trashed, "the rows add up to the header")
        let inFolder = disk.segments.prefix(5).reduce(0) { $0 + $1.bytes }
        #expect(inFolder == disk.analysed, "categories and other data in this folder add up to the folder")
        #expect(disk.segments[4].bytes > 0, "worktrees' build output is not counted twice")
    }

    @Test func aSavedPictureIsShownAtOnceWithItsAge() async throws {
        let settings = Fixture.settings()
        let first = try await started(settings)
        let scanned = first.overview?.freshness.fullScan
        first.stop()
        let again = AppModel(settings: settings, strings: Strings(language: "en", region: "US"), probe: FixedProbe([]))
        await again.start()
        defer { again.stop() }
        #expect(again.overview?.freshness.fullScan == scanned, "from the saved picture, no new scan")
        #expect(again.overview?.totals != nil)
    }

    @Test func categoryRowsComeAPageAtATimeInTheOrderAskedFor() async throws {
        let model = try await started()
        defer { model.stop() }
        let client = try #require(model.client)
        let table = CategoryModel(.buildArtifact, client: client, names: model.names, home: model.settings.home, pageSize: 2)
        await table.reload()
        #expect(table.rows.count == 2 && table.total == 9)
        await table.loadMore()
        #expect(table.rows.count == 4)
        await table.shown(table.rows[0].id)
        #expect(table.rows.count == 6, "a row near the end of those loaded loads the next page")
        await table.sort(by: .path, descending: false)
        let paths = table.rows.map(\.path)
        #expect(paths == paths.sorted() && table.rows.count == 2)
        await table.sort(by: .size, descending: true)
        #expect(table.rows[0].bytes >= table.rows[1].bytes)
        #expect(table.order == Order(key: .size, descending: true))
        await table.reload()
        #expect(table.rows.count == 2, "reloads as many as were shown")
    }

    @Test func checkboxesFollowThePreselectionUntilTheUserDecides() async throws {
        let model = try await started()
        defer { model.stop() }
        let table = try #require(model.category(.buildArtifact))
        await table.reload()
        #expect(table.total == 9)
        let managed = table.rows.filter(\.managed)
        #expect(managed.count == 2 && managed.allSatisfy { !table.isChecked($0.id) && $0.recent == false })
        #expect(table.selection.count == 7)
        let first = table.rows[0]
        table.toggle(first.id)
        #expect(!table.isChecked(first.id) && table.selection.count == 6)
        await table.reload()
        #expect(!table.isChecked(first.id), "the user's choice survives a reload")
        table.selectAll()
        #expect(table.selection.count == 9)
        #expect(table.selection.bytes == table.rows.reduce(0) { $0 + $1.bytes })
        #expect(table.rows.allSatisfy { $0.rule == "Cargo build output" || $0.rule == "Node packages" })
        #expect(table.rows.filter(\.recent).isEmpty, "the build output of the fixture is two months old")
        #expect(model.category(.buildArtifact) === table, "one table per category")
    }

    @Test func worktreesThatCannotBeRemovedCannotBeSelected() async throws {
        let model = try await started()
        defer { model.stop() }
        let table = try #require(model.category(.gitWorktree))
        try await until {
            await table.reload()
            return !table.rows.isEmpty && table.rows.allSatisfy { $0.state != "Checking…" }
        }
        #expect(table.total == 7)
        let blocked = table.rows.filter { !$0.selectable }
        #expect(blocked.count >= 5)
        #expect(blocked.allSatisfy { $0.state?.isEmpty == false && !$0.reasons.isEmpty })
        let dirty = try #require(blocked.first { $0.path.hasSuffix("/app-dirty") })
        #expect(dirty.branch == "app-dirty" && dirty.reasons == ["Uncommitted"], "a blocked worktree keeps its branch")
        let locked = try #require(blocked.first { $0.state?.contains("Locked") == true })
        table.toggle(locked.id)
        #expect(!table.isChecked(locked.id), "a blocked worktree stays unchecked")
        #expect(table.rows.allSatisfy { !table.isChecked($0.id) }, "worktrees are never preselected")
    }

    @Test func anExclusionHidesARowAndRemovingItBringsItBack() async throws {
        let model = try await started()
        defer { model.stop() }
        let client = try #require(model.client)
        let table = try #require(model.category(.appCache))
        await table.reload()
        let before = table.total
        let row = try #require(table.rows.first)
        let refreshes = model.refreshes
        await table.exclude(row.id)
        #expect(table.total == before - 1)
        #expect(try await client.exclusions() == [row.path])
        try await until { model.refreshes > refreshes }
        #expect(try await client.removeExclusion(row.path))
        await table.reload()
        #expect(table.total == before)
        await model.exclude(row.path)
        #expect(try await client.exclusions() == [row.path])
    }

    /// The app reloads Folders when the picture changes, perhaps while
    /// the user's own reload runs: what was expanded stays expanded.
    @Test func reloadsAtOnceKeepWhatWasExpanded() async throws {
        let model = try await started()
        defer { model.stop() }
        await model.show(.folders)
        let folders = try #require(model.folders)
        let home = try #require(folders.top.first { $0.name == "home" })
        await folders.expand(home)
        let code = try #require(home.children?.first { $0.name == "code" })
        await folders.expand(code)
        async let first: Void = folders.reload()
        async let second: Void = folders.reload()
        async let third: Void = folders.reload()
        _ = await (first, second, third)
        #expect(folders.node(home.path)?.isExpanded == true)
        #expect(folders.node(code.path)?.isExpanded == true)
    }

    /// A row taken before a reload stays usable: expanding it expands the
    /// node of the same path in the tree shown now, and the caller's own
    /// node gets its children too.
    @Test func expandingANodeFromBeforeAReloadExpandsTheOneShownNow() async throws {
        let model = try await started()
        defer { model.stop() }
        await model.show(.folders)
        let folders = try #require(model.folders)
        let stale = try #require(folders.top.first { $0.name == "home" })
        await folders.reload()
        #expect(folders.node(stale.path) !== stale, "a reload makes new nodes")
        await folders.expand(stale)
        #expect(folders.node(stale.path)?.isExpanded == true)
        #expect(stale.isExpanded && stale.children?.isEmpty == false, "the caller's node can go on")
        let code = try #require(stale.children?.first { $0.name == "code" })
        #expect(folders.node(code.path) != nil)
        folders.collapse(stale)
        #expect(folders.node(stale.path)?.isExpanded == false, "a fold reaches the node shown too")
    }

    /// While a reload runs the tree shown stays whole (every row is found),
    /// and a fold made meanwhile is still a fold afterwards.
    @Test func whileAReloadRunsTheTreeStaysWholeAndAFoldIsKept() async throws {
        let model = try await started()
        defer { model.stop() }
        await model.show(.folders)
        let folders = try #require(model.folders)
        let home = try #require(folders.top.first { $0.name == "home" })
        await folders.expand(home)
        let code = try #require(home.children?.first { $0.name == "code" })
        await folders.expand(code)
        let done = Flag()
        let reload = Task { @MainActor in
            await folders.reload()
            done.set = true
        }
        var looks = 0
        while !done.set, looks < 10_000 {
            #expect(folders.node(home.path) != nil && folders.node(code.path) != nil, "look \(looks)")
            if looks == 1 {
                folders.collapse(code)
            }
            looks += 1
            await Task.yield()
        }
        await reload.value
        #expect(folders.node(home.path)?.isExpanded == true)
        #expect(folders.node(code.path)?.isExpanded == false, "folded during the reload")
    }

    @Test func foldersOpenALevelAtATimeDownToFiles() async throws {
        let model = try await started()
        defer { model.stop() }
        await model.show(.folders)
        let folders = try #require(model.folders)
        let home = try #require(folders.top.first { $0.name == "home" })
        #expect(folders.top.contains { $0.name == ".metla-fixture" && $0.kind == .file })
        #expect(home.children == nil && home.shownChildren?.isEmpty == true)
        await folders.expand(home)
        #expect(home.isExpanded && home.children?.isEmpty == false)
        let trash = try #require(home.children?.first { $0.name == ".Trash" })
        #expect(trash.badge == .rule("Trash"))
        let code = try #require(home.children?.first { $0.name == "code" })
        await folders.expand(code)
        let app = try #require(code.children?.first { $0.name == "app" })
        await folders.expand(app)
        let target = try #require(app.children?.first { $0.name == "target" })
        #expect(target.badge == .rule("Cargo build output"))
        #expect(target.items == 4 && target.share > 0 && target.share <= 1)
        #expect(app.children?.contains { $0.kind == .file && $0.name == "Cargo.toml" } == true)
        folders.selection = target.id
        #expect(folders.breadcrumb.suffix(4) == ["home", "code", "app", "target"])
        #expect(folders.node(target.id) === target && folders.node(nil) == nil)
        folders.collapse(app)
        #expect(!app.isExpanded && app.shownChildren?.isEmpty == true)
        await folders.reload()
        #expect(folders.node(code.path)?.isExpanded == true, "expanded again after a reload")
        let file = try #require(app.children?.first { $0.kind == .file })
        await folders.expand(file)
        #expect(file.shownChildren == nil && !file.isDirectory)
    }

    @Test func showInFoldersOpensTheOutlineDownToTheCandidate() async throws {
        let model = try await started()
        defer { model.stop() }
        let table = try #require(model.category(.buildArtifact))
        await table.reload()
        let row = try #require(table.rows.first { $0.shownPath == "~/code/app/target" })
        await model.showInFolders(row.path)
        #expect(model.screen == .folders)
        let folders = try #require(model.folders)
        let selected = try #require(folders.node(folders.selection))
        #expect(selected.path == row.path && selected.badge == .rule("Cargo build output"))
        #expect(folders.node((row.path as NSString).deletingLastPathComponent)?.isExpanded == true)
        #expect(await folders.reveal(model.settings.root + "/nowhere") == false)
    }

    @Test func refreshAndStopReachTheEngine() async throws {
        let model = try await started()
        defer { model.stop() }
        let scans = model.overview?.freshness.fullScan
        await model.rescan()
        await model.stopScan()
        try await until {
            await model.refresh([.picture])
            guard case .idle? = model.freshness?.state else { return false }
            return true
        }
        #expect(model.overview?.freshness.fullScan != nil && scans != nil)
        await model.attention(true)
        await model.attention(false)
        await model.show(.overview)
        #expect(model.screen == .overview)
    }

    /// A root with a folder nobody may list: the picture is partial.
    func closedRoot() throws -> URL {
        let root = Fixture.scratch().appendingPathComponent("root")
        let closed = root.appendingPathComponent("closed")
        try FileManager.default.createDirectory(at: closed.appendingPathComponent("inside"), withIntermediateDirectories: true)
        try Data().write(to: root.appendingPathComponent(LaunchSettings.fixtureMarker))
        try FileManager.default.setAttributes([.posixPermissions: 0], ofItemAtPath: closed.path)
        return root
    }

    func reopen(_ root: URL) {
        try? FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: root.appendingPathComponent("closed").path)
    }

    @Test func aPartialPictureIsShownAsPartial() async throws {
        let root = try closedRoot()
        defer { reopen(root) }
        let model = try await started(Fixture.settings(root: root.path), probe: FixedProbe([.denied]))
        defer { model.stop() }
        #expect(model.access.state == .notGranted, "probed when the model starts")
        #expect(model.overviewData?.banner == .partial(denied: 1, missing: 0))
        #expect(model.status?.text.hasSuffix("· partial") == true)
        #expect(model.status?.tone == .stale)
    }

    /// With Full Disk Access a folder still denied is closed to every app:
    /// no banner, no "partial" in the status, a quiet count instead.
    @Test func withFullDiskAccessAFolderStillDeniedIsOnlyCounted() async throws {
        let root = try closedRoot()
        defer { reopen(root) }
        let model = try await started(Fixture.settings(root: root.path), probe: FixedProbe([.listed]))
        defer { model.stop() }
        #expect(model.access.state == .granted)
        let data = try #require(model.overviewData)
        #expect(data.banner == nil && data.closed == 1)
        #expect(model.status?.text.hasPrefix("Updated") == true && model.status?.text.contains("partial") == false)
        #expect(model.status?.tone == .current)
        #expect(model.freshness?.fullScansEnded == 1, "no place the probe listed is denied: no scan again")
    }

    /// With access granted, a picture that holds as denied a place the
    /// probe has just listed was made before access: one full scan, as
    /// Refresh starts it, reads it again, and only one per launch, even
    /// when the folder stays denied. Its folders are unread, never
    /// "closed to every app".
    @Test func aPictureMadeBeforeAccessIsScannedAgainOnce() async throws {
        let root = try closedRoot()
        defer { reopen(root) }
        let closed = root.appendingPathComponent("closed").path
        let model = try await started(Fixture.settings(root: root.path), probe: FixedProbe([.listed], places: [closed]))
        defer { model.stop() }
        try await until {
            await model.refresh([.activity, .picture])
            guard case .idle? = model.freshness?.state else { return false }
            return (model.freshness?.fullScansEnded ?? 0) >= 2
        }
        for _ in 0..<5 {
            try await Task.sleep(for: .milliseconds(100))
            await model.refresh([.activity, .picture])
        }
        #expect(model.freshness?.fullScansEnded == 2, "the first scan and one more, not a loop")
        #expect(model.denied == .outdated)
        let data = try #require(model.overviewData)
        #expect(data.banner == .unread(missing: 1, closed: 0) && data.closed == nil)
        #expect(model.status?.text.hasSuffix("· partial") == true)
    }

    /// Full Disk Access is probed when the model starts, before the first
    /// picture is shown: it tells that picture's gaps.
    @Test func fullDiskAccessIsProbedBeforeTheFirstPicture() async throws {
        let model = AppModel(
            settings: Fixture.settings(), strings: Strings(language: "en", region: "US"),
            preferences: MemoryPreferences(), probe: FixedProbe([.listed]))
        await model.start(
            mainThread: MainThreadGuard { call in Issue.record("main thread: \(call)") }, trash: RefusingTrash(),
            login: FakeLogin())
        defer { model.stop() }
        #expect(model.overview != nil, "the first picture is shown")
        #expect(model.access.state == .granted)
    }

    /// Full Disk Access is probed again when a full scan ends, and not at
    /// every refresh.
    @Test func fullDiskAccessIsProbedAgainWhenAFullScanEnds() async throws {
        let probe = ChangingProbe([.denied])
        let model = try await started(probe: probe)
        defer { model.stop() }
        #expect(model.access.state == .notGranted)
        let asked = await probe.asked
        for _ in 0..<3 {
            await model.refresh([.activity, .picture])
        }
        #expect(await probe.asked == asked, "no scan ended: not probed")
        await probe.answer([.listed])
        await model.rescan()
        try await until {
            await model.refresh([.activity])
            return model.access.state == .granted
        }
        #expect(await probe.asked == asked + 1)
    }

    @Test func anEngineThatCannotOpenSaysWhy() async {
        var settings = Fixture.settings(root: "/nonexistent/metla/root")
        settings.isFixture = false
        let model = AppModel(settings: settings, strings: Strings(language: "en", region: "US"), probe: FixedProbe([]))
        await model.start()
        #expect(model.failure != nil && model.client == nil)
        await model.refresh([.picture])
        #expect(model.overview == nil && model.status == nil && model.overviewData == nil)
        #expect(model.category(.log) == nil && model.sidebar.isEmpty)
    }
}

/// Set by a task when it is done.
@MainActor
final class Flag {
    var set = false
}
