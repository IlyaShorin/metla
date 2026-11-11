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

    /// The user's scan is over once the engine has counted the full scan
    /// it asked for as ended and no full scan runs, on any timing: a scan
    /// on a fixture may be over before rescan() returns.
    @Test func aScanTheUserAskedForRunsAsInteractiveUntilItEnds() async throws {
        let model = try await started(Fixture.settings())
        defer { model.stop() }
        let before = try #require(model.freshness).fullScansEnded
        await model.rescan()
        try await until {
            await model.refresh([.activity])
            return !model.userScan
        }
        let after = try #require(model.freshness)
        #expect(after.fullScansEnded >= before + 1, "the scan asked for ended")
        #expect(model.overview?.profile == .background, "the profile given back")
        await model.rescan()
        await model.stopScan()
        #expect(!model.userScan, "a stopped scan is over")
    }

    /// When the user's scan is over, as one table: the engine's count of
    /// ended full scans has reached the one it waits for, and no full scan
    /// runs (an update in place is not a scan).
    @Test func theUsersScanIsOverWhenItsFullScanHasEnded() {
        func freshness(_ ended: UInt64, _ state: EngineState) -> Freshness {
            Freshness(fullScan: 1, updated: 1, pending: 0, state: state, picture: .complete, watching: true,
                      needsFullScan: false, fullScansEnded: ended)
        }
        let scanning = EngineState.scanning(progress: ProgressSnapshot(dirs: 1, files: 1, bytes: 1, denied: 0),
                                            interactive: true)
        let rows: [(UInt64, Freshness, Bool)] = [
            (3, freshness(2, .idle), false),
            (3, freshness(3, .idle), true),
            (3, freshness(4, .idle), true),
            (3, freshness(3, scanning), false),
            (3, freshness(3, .updating(pending: 2)), true),
            (3, freshness(3, .failed(.noSpace)), true),
        ]
        for (target, now, over) in rows {
            #expect(AppModel.userScanOver(endsAt: target, now) == over, "\(target) \(now.fullScansEnded) \(now.state)")
        }
    }

    /// The refresh that sees the user's scan over leaves the model
    /// consistent: the snapshot it keeps already has the profile given
    /// back, so the status line and the menu-bar panel say "in the
    /// background" at once, not after some later event.
    @Test func theRefreshThatEndsTheUsersScanShowsTheProfileGivenBack() async throws {
        let model = try await started(Fixture.settings())
        defer { model.stop() }
        await model.rescan()
        try await until {
            await model.refresh([.activity])
            return !model.userScan
        }
        #expect(model.overview?.profile == .background, "read again after the profile was given back")
        #expect(model.freshness == model.overview?.freshness)
    }

    /// The first scan of a disk without a picture is the one the user
    /// waits for (the first run, a lost index): it runs as the user's
    /// scan, Interactive and holding an activity so that App Nap does not
    /// slow it while the app is behind other windows. A scan of a disk
    /// with a picture, and a scan the user's own Refresh started, are not
    /// taken over.
    @Test func theFirstScanOfADiskWithoutAPictureIsTheUsers() {
        func freshness(_ state: EngineState, _ picture: Completeness) -> Freshness {
            Freshness(fullScan: nil, updated: nil, pending: 0, state: state, picture: picture, watching: true,
                      needsFullScan: true, fullScansEnded: 0)
        }
        let progress = ProgressSnapshot(dirs: 1, files: 1, bytes: 1, denied: 0)
        #expect(AppModel.takesOver(freshness(.scanning(progress: progress, interactive: false), .missing)))
        #expect(!AppModel.takesOver(freshness(.scanning(progress: progress, interactive: true), .missing)))
        #expect(!AppModel.takesOver(freshness(.scanning(progress: progress, interactive: false), .partial)))
        #expect(!AppModel.takesOver(freshness(.scanning(progress: progress, interactive: false), .complete)))
        #expect(!AppModel.takesOver(freshness(.idle, .missing)), "no scan runs: nothing to hold")
        #expect(!AppModel.takesOver(freshness(.off, .missing)))
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
