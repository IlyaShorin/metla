import Foundation
import MetlaCoreFFI
import Testing
@testable import MetlaKit

@MainActor
@Suite struct SettingsTests {
    @Test func powerStatesMapToProfilesByOneTable() {
        let rows: [(PowerState, Bool, Bool, Bool, PowerProfile)] = [
            (PowerState(), true, true, false, .background),
            (PowerState(onBattery: true), true, true, false, .saver),
            (PowerState(onBattery: true), true, false, false, .background),
            (PowerState(lowPowerMode: true), true, false, false, .saver),
            (PowerState(thermal: .fair), true, true, false, .background),
            (PowerState(thermal: .serious), true, true, false, .saver),
            (PowerState(thermal: .critical), true, false, false, .saver),
            (PowerState(onBattery: true), true, true, true, .interactive),
            (PowerState(lowPowerMode: true), true, true, true, .interactive),
            (PowerState(), false, true, true, .off),
            (PowerState(onBattery: true), false, true, false, .off),
        ]
        for (state, background, reduce, userScan, expected) in rows {
            let profile = state.profile(background: background, reduceOnBattery: reduce, userScan: userScan)
            #expect(profile == expected, "\(state) background \(background) reduce \(reduce) scan \(userScan)")
        }
    }

    @Test func fullDiskAccessIsWhatTheFirstProtectedPlaceAnswers() async {
        #expect(FullDiskAccess.from([.listed]) == .granted)
        #expect(FullDiskAccess.from([.denied, .listed]) == .notGranted)
        #expect(FullDiskAccess.from([.missing, .listed]) == .granted)
        #expect(FullDiskAccess.from([.missing, .missing]) == .unknown)
        #expect(FullDiskAccess.from([]) == .unknown)
        let access = AccessModel(probe: FixedProbe([.missing, .denied]), preferences: MemoryPreferences())
        #expect(access.state == .unknown, "nothing assumed before a probe")
        await access.check()
        #expect(access.state == .notGranted)
        #expect(AccessModel.settingsURL.absoluteString.hasSuffix("Privacy_AllFiles"))
    }

    @Test func theProbeListsOnlyAndTellsMissingFromDenied() async throws {
        let dir = Fixture.scratch()
        let readable = dir.appendingPathComponent("Library/Mail")
        try FileManager.default.createDirectory(at: readable, withIntermediateDirectories: true)
        let answers = await LibraryProbe(home: dir.path).answers()
        #expect(answers.count == 3 && answers[0] == .listed && answers[1] == .missing)
        let locked = dir.appendingPathComponent("Library/Safari")
        try FileManager.default.createDirectory(at: locked, withIntermediateDirectories: true)
        try FileManager.default.setAttributes([.posixPermissions: 0o000], ofItemAtPath: locked.path)
        defer { try? FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: locked.path) }
        #expect(await LibraryProbe(home: dir.path).answers()[1] == .denied)
    }

    @Test func theFirstRunSheetShowsOnce() {
        let preferences = MemoryPreferences()
        let access = AccessModel(probe: FixedProbe([]), preferences: preferences)
        #expect(access.showsFirstRun)
        access.finishFirstRun()
        #expect(!access.showsFirstRun)
        #expect(!AccessModel(probe: FixedProbe([]), preferences: preferences).showsFirstRun, "kept")
    }

    @Test func exclusionsRoundTripAndSayWhatTheyHide() async throws {
        let model = try await started(Fixture.settings())
        defer { model.stop() }
        let exclusions = try #require(model.exclusions)
        await exclusions.load()
        #expect(exclusions.rows.isEmpty)
        let root = model.settings.root
        let caches = (root as NSString).appendingPathComponent("home/Library/Caches")
        let documents = (root as NSString).appendingPathComponent("home/Documents")
        await exclusions.add([caches, documents])
        #expect(exclusions.rows.map(\.shownPath) == ["~/Documents", "~/Library/Caches"])
        #expect(exclusions.rows[1].hides.hasPrefix("hides 2 items · "))
        #expect(exclusions.rows[0].hides == "hides nothing that can be cleaned")
        await exclusions.add(["relative/path"])
        #expect(exclusions.failure != nil && exclusions.rows.count == 2, "checked where it comes in")
        await exclusions.remove(caches)
        #expect(exclusions.rows.map(\.shownPath) == ["~/Documents"])
    }

    @Test func generalSettingsAreKeptByTheEngineAndTheApp() async throws {
        let preferences = MemoryPreferences()
        let model = try await started(Fixture.settings(), preferences: preferences)
        defer { model.stop() }
        let general = try #require(model.general)
        await general.load()
        #expect(general.staleDays == 30 && general.recentDays == 14)
        await general.setStaleDays(7)
        await general.setRecentDays(0)
        #expect(general.staleDays == 7 && general.recentDays == 1, "at least a day")
        await general.setReduceOnBattery(false)
        let kept = try await #require(model.client).settings()
        #expect(kept.staleAfter == 7 * 86_400 && kept.recentAge == 86_400 && !kept.reduceOnBattery)
        #expect(general.showInMenuBar)
        general.setShowInMenuBar(false)
        #expect(!GeneralModel.showsInMenuBar(preferences))
        general.setLaunchAtLogin(true)
        #expect(general.launchAtLogin && general.loginFailure == nil)
        await general.setBackground(false)
        await model.powerChanged(PowerState(onBattery: true))
        try await until {
            await model.refresh([.picture])
            return model.overview?.profile == .off
        }
        await general.setBackground(true)
        await model.powerChanged(PowerState(onBattery: true))
        try await until {
            await model.refresh([.picture])
            return model.overview?.profile == .background
        }
    }

    @Test func aScanTheUserAskedForRunsAsInteractiveUntilItEnds() async throws {
        let model = try await started(Fixture.settings())
        defer { model.stop() }
        let before = model.overview?.freshness.fullScan
        await model.rescan()
        #expect(model.userScan)
        try await until {
            await model.refresh([.activity])
            return !model.userScan
        }
        #expect(model.overview?.freshness.fullScan != before || before == nil, "a new full scan")
        try await until {
            await model.refresh([.picture])
            return model.overview?.profile == .background
        }
        await model.rescan()
        await model.stopScan()
        #expect(!model.userScan, "a stopped scan is over")
    }

    @Test func aLoginItemTheSystemRefusesSaysWhy() async throws {
        let login = FakeLogin()
        login.refuses = true
        let client = try #require(try await started(Fixture.settings()).client)
        let general = GeneralModel(client: client, login: login, preferences: MemoryPreferences())
        general.setLaunchAtLogin(true)
        #expect(!general.launchAtLogin && general.loginFailure != nil)
    }

    @Test func theMenuBarSummaryReadsTheCachedSnapshot() async throws {
        let model = try await started(Fixture.settings())
        defer { model.stop() }
        let overview = try #require(model.overview)
        let summary = MenuBarSummary.make(
            overview, volume: "Disk", mode: .confirm, hotkey: .clean, status: model.status, model.names)
        #expect(summary.volume == "Disk" && summary.free.hasSuffix("free"))
        #expect(summary.used > 0 && summary.used <= 1)
        #expect(summary.quick.map(\.name) == ["App caches", "Developer caches", "Build artifacts"])
        #expect(summary.canBeFreed.hasPrefix("can be cleaned ≈ "))
        #expect(summary.toTrash?.hasPrefix("and ") == true, "logs go to the Trash")
        #expect(summary.rest.name == "Logs, worktrees, temporary — only with confirmation")
        #expect(summary.cleanTitle == "Clean…" && summary.hotkey == "⌃⌥⌘⌫")
        #expect(summary.status.hasPrefix("Updated") && summary.status.hasSuffix("in the background"))
        let now = MenuBarSummary.make(overview, volume: "Disk", mode: .immediate, hotkey: nil, status: nil, model.names)
        #expect(now.cleanTitle == "Clean now" && now.hotkey == nil && now.status == "in the background")
        let russian = Names(Strings(language: "ru", region: "RU"), titles: [])
        #expect(MenuBarSummary.profile(.saver, russian.strings) == "экономный режим")
        #expect(MenuBarSummary.profile(.interactive, model.strings) == "full speed")
        #expect(MenuBarSummary.profile(.off, model.strings) == "background analysis off")
    }
}
