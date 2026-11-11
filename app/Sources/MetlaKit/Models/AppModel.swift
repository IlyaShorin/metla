import Foundation
import MetlaCoreFFI
import Observation

/// The main window's screens.
public enum Screen: Hashable, Sendable {
    case overview
    case folders
    case category(Category)
    /// The last cleanup's report.
    case report
}

/// What a hotkey press asks the app to do.
public enum HotkeyResponse: Equatable, Sendable {
    /// Bring the app forward and show the cleanup preview (its own window
    /// when the main window is closed).
    case showPreview
    /// Bring the main window forward.
    case showApp
    /// Show the small panel with this message; the app stays behind.
    case hud(HUDMessage)
}

/// What the hotkey's panel says.
public struct HUDMessage: Equatable, Sendable {
    public let title: String
    public let lines: [String]
    /// "Show report" belongs to it.
    public let report: Bool
}

/// A category in the sidebar: its estimated size, dimmed when empty.
public struct SidebarRow: Identifiable, Equatable, Sendable {
    public var id: Category { category }
    public let category: Category
    public let name: String
    public let bytes: UInt64
    public let empty: Bool
}

/// The main window's state: the engine client, the latest answers, and
/// the screen models. It refreshes on the engine's notifications, at most
/// a few times a second, and polls while a scan runs (scans report their
/// progress only when asked).
@MainActor
@Observable
public final class AppModel {
    public let settings: LaunchSettings
    public let strings: Strings
    public private(set) var names: Names
    public private(set) var client: EngineClient?
    public private(set) var failure: String?
    public private(set) var overview: Overview?
    public private(set) var largest: [TopFolder] = []
    public private(set) var freshness: Freshness?
    public private(set) var volume = VolumeInfo(name: "", isRoot: true)
    public private(set) var now = Date()
    public private(set) var refreshes = 0
    public var screen: Screen = .overview
    public private(set) var categories: [Category: CategoryModel] = [:]
    public private(set) var folders: FoldersModel?
    public private(set) var cleanup: CleanupModel?
    public private(set) var general: GeneralModel?
    public private(set) var exclusions: ExclusionsModel?
    public let access: AccessModel
    public private(set) var hotkeys: HotkeysModel?
    /// Candidates added from Folders (⌘⌫) to the cleanup selection.
    public private(set) var added: Set<DirId> = []
    /// The last cleanup's report, kept by the engine across launches:
    /// "Last report" in the sidebar.
    public private(set) var lastReport: ReportData?
    /// The power state last told, and whether a scan the user asked for
    /// runs (it runs as Interactive).
    public private(set) var power = PowerState()
    public private(set) var userScan = false
    /// Held while a scan the user asked for runs (App Nap is otherwise
    /// left alone).
    private var scanActivity: (any NSObjectProtocol)?
    private var userScanSeen = false
    private var userScanFrom: Int64?
    public let preferences: any Preferences
    /// The cleanup flow shows in its own window (the main window was
    /// closed when the hotkey asked for it), not as a sheet.
    public var cleanupInWindow = false

    private var pacer = RefreshPacer()
    private var listener: Task<Void, Never>?
    private var poller: Task<Void, Never>?
    /// While a scan or an update runs.
    public static let pollInterval: Duration = .milliseconds(500)
    public static let shownFolders: UInt32 = 6

    public init(
        settings: LaunchSettings, strings: Strings = .current,
        preferences: any Preferences = StandardPreferences(), probe: any AccessProbe = LibraryProbe()
    ) {
        self.settings = settings
        self.strings = strings
        self.preferences = preferences
        names = Names(strings, titles: ruleTitles())
        access = AccessModel(probe: probe, preferences: preferences)
    }

    /// Opens the engine off the main thread, shows the saved picture, and
    /// follows the engine from then on. `trash` moves to the Trash (the
    /// system's), `login` starts the app at login.
    public func start(
        mainThread: MainThreadGuard = .standard, trash: any TrashHost = SystemTrash(),
        login: (any LoginItem)? = nil
    ) async {
        do {
            let client = try await EngineClient.open(settings.options, trash: trash, mainThread: mainThread)
            self.client = client
            folders = FoldersModel(root: settings.root, client: client, names: names)
            cleanup = CleanupModel(client: client, names: names, home: settings.home)
            exclusions = ExclusionsModel(client: client, strings: strings, home: settings.home)
            if let login {
                general = GeneralModel(client: client, login: login, preferences: preferences)
            }
            await readLastReport(client)
            volume = await Self.volume(of: settings.root)
            await refresh(Self.everything)
            listen(to: client)
        } catch {
            failure = String(describing: error)
        }
    }

    /// Binds the global hotkeys through `registrar` and registers them.
    public func attachHotkeys(_ registrar: any HotkeyRegistrar) async {
        let model = HotkeysModel(preferences: preferences, registrar: registrar, client: client, strings: strings)
        hotkeys = model
        await model.load()
        model.registerAll()
    }

    /// Stops following the engine; the client goes with the model.
    public func stop() {
        listener?.cancel()
        poller?.cancel()
    }

    static let everything: Set<Changed> = [.picture, .activity, .exclusions, .worktrees]

    private func listen(to client: EngineClient) {
        listener = Task { [weak self] in
            for await _ in client.changes {
                let topics = client.drain()
                guard let self, !topics.isEmpty else {
                    continue
                }
                await self.paced(topics)
            }
        }
    }

    /// Refreshes after the pacer's wait, with whatever came meanwhile.
    func paced(_ topics: Set<Changed>) async {
        let wait = pacer.wait(at: .now)
        if wait > .zero {
            try? await Task.sleep(for: wait)
        }
        let more = client?.drain() ?? []
        await refresh(topics.union(more))
    }

    /// Reads what `topics` say changed: the freshness always; the
    /// picture, the largest folders and the screen shown when the picture
    /// changed or a scan runs.
    public func refresh(_ topics: Set<Changed>) async {
        guard let client else {
            return
        }
        pacer.ran(at: .now)
        refreshes += 1
        now = Date()
        do {
            freshness = try await client.freshness()
            if !topics.isDisjoint(with: [.picture, .exclusions, .worktrees]) || scanning {
                try await readPicture(client)
            }
            if !topics.isDisjoint(with: [.picture, .exclusions, .worktrees]) {
                await refreshScreen(picture: topics.contains(.picture))
            }
            if topics.contains(.cleanup) {
                await readLastReport(client)
                await pruneAdded(client)
            }
            if topics.contains(.settings) {
                await hotkeys?.load()
            }
            await endUserScanIfOver(topics)
            failure = nil
        } catch {
            failure = String(describing: error)
        }
        pollWhileBusy()
    }

    private func readPicture(_ client: EngineClient) async throws {
        let overview = try await client.snapshot()
        self.overview = overview
        freshness = overview.freshness
        largest = try await client.largestFolders(limit: Self.shownFolders)
    }

    private func refreshScreen(picture: Bool) async {
        switch screen {
        case .category(let category):
            await categories[category]?.reload()
        case .folders where picture:
            await folders?.reload()
        default:
            break
        }
    }

    /// A scan or an update runs.
    public var scanning: Bool {
        switch freshness?.state {
        case .scanning?, .updating?: true
        default: false
        }
    }

    private func pollWhileBusy() {
        guard scanning, poller == nil else {
            return
        }
        poller = Task { [weak self] in
            while let self, self.scanning, !Task.isCancelled {
                try? await Task.sleep(for: Self.pollInterval)
                await self.refresh([.activity])
            }
            self?.poller = nil
        }
    }

    /// The toolbar's status.
    public var status: StatusLine? {
        freshness.map { StatusLine.of($0, expected: overview?.totals?.bytes, now: now, strings) }
    }

    /// The Overview's values, once there is a snapshot.
    public var overviewData: OverviewData? {
        overview.map {
            OverviewData.make($0, largest: largest, volume: volume, home: settings.home, names)
        }
    }

    /// The sidebar's categories, in the engine's order.
    public var sidebar: [SidebarRow] {
        (overview?.categories ?? []).map { tally in
            SidebarRow(
                category: tally.category, name: names.category(tally.category),
                bytes: tally.tally.bytes, empty: tally.tally.count == 0)
        }
    }

    /// The table of `category`, made the first time it is shown.
    public func category(_ category: Category) -> CategoryModel? {
        if let model = categories[category] {
            return model
        }
        guard let client else {
            return nil
        }
        let model = CategoryModel(category, client: client, names: names, home: settings.home)
        categories[category] = model
        return model
    }

    /// Shows `screen` and loads what it lists.
    public func show(_ screen: Screen) async {
        self.screen = screen
        switch screen {
        case .category(let category):
            await self.category(category)?.reload()
        case .folders:
            await folders?.reload()
        case .report, .overview:
            break
        }
    }

    /// "Show in Folders": the Folders screen, opened down to `path`.
    public func showInFolders(_ path: String) async {
        screen = .folders
        await folders?.reveal(path)
    }

    /// Refresh (⌘R): a full scan the user asked for, run as Interactive.
    public func rescan() async {
        userScan = true
        userScanSeen = false
        userScanFrom = freshness?.fullScan
        if scanActivity == nil {
            scanActivity = ProcessInfo.processInfo.beginActivity(
                options: .userInitiated, reason: "A scan the user asked for")
        }
        await applyPower()
        try? await client?.startScan()
        await refresh([.activity])
    }

    /// The user's scan is over once none runs and it was seen running, a
    /// newer full scan is in the picture, or the picture changed (a scan
    /// too quick to be seen).
    private func endUserScanIfOver(_ topics: Set<Changed>) async {
        guard userScan else { return }
        if scanning {
            userScanSeen = true
            return
        }
        let ended = userScanSeen || freshness?.fullScan != userScanFrom || topics.contains(.picture)
        guard ended else { return }
        await endUserScan()
    }

    private func endUserScan() async {
        userScan = false
        userScanSeen = false
        scanActivity.map(ProcessInfo.processInfo.endActivity)
        scanActivity = nil
        await applyPower()
    }

    /// The Mac's power state changed (or is told at the start).
    public func powerChanged(_ state: PowerState) async {
        power = state
        await applyPower()
    }

    private func applyPower() async {
        guard let client else {
            return
        }
        let kept = await client.settings()
        let profile = power.profile(
            background: kept.background, reduceOnBattery: kept.reduceOnBattery, userScan: userScan)
        try? await client.setPowerProfile(profile)
    }

    /// Every candidate the engine offers now, with whether it is
    /// preselected, read a page at a time.
    func offered(_ client: EngineClient) async -> [Candidate] {
        var all: [Candidate] = []
        let order = Order(key: .size, descending: true)
        while let page = try? await client.candidates(nil, order: order, offset: UInt64(all.count), limit: 1000),
              !page.listed.isEmpty {
            all += page.listed.map(\.candidate)
            if all.count >= page.total {
                break
            }
        }
        return all
    }

    /// The selection "Clean…" previews: every preselected candidate.
    public func defaultSelection() async -> [DirId] {
        guard let client else {
            return []
        }
        return await offered(client).filter(\.preselected).map(\.id)
    }

    /// "Clean selected…": what the tables check and Folders added.
    public var selection: CleanupSelection {
        let checked = categories.values.flatMap(\.checked)
        return CleanupSelection(Set(checked).union(added))
    }

    /// Folders' ⌘⌫: a candidate joins the cleanup selection.
    public func addToCleanup(_ id: DirId) {
        added.insert(id)
    }

    private func pruneAdded(_ client: EngineClient) async {
        guard !added.isEmpty else {
            return
        }
        let offered = Set(await offered(client).map(\.id))
        added.formIntersection(offered)
    }

    /// Clean… (⌘⌫): the preview of the default selection.
    public func cleanDefault() async {
        await cleanup?.open(await defaultSelection())
    }

    /// Clean selected…: the preview of the selection.
    public func cleanSelected() async {
        await cleanup?.open(selection.sorted)
    }

    /// The Clean hotkey: by the mode, show the preview, or clean the
    /// chosen quick categories at once and say what happened.
    public func cleanHotkey() async -> HotkeyResponse {
        guard let hotkeys, hotkeys.mode == .immediate else {
            return .showPreview
        }
        return .hud(await quickCleanNow(Array(hotkeys.quick)))
    }

    /// The hotkey's quick clean of `categories`; a press while a cleanup
    /// runs, with no picture, or with nothing to clean only says so.
    public func quickCleanNow(_ categories: [QuickCategory]) async -> HUDMessage {
        guard let cleanup, !cleanup.running else {
            return HUDMessage(title: strings.text("hud.busy"), lines: [], report: false)
        }
        guard overview?.totals != nil else {
            return HUDMessage(title: strings.text("hud.noPicture"), lines: [], report: false)
        }
        let chosen = categories.filter { quick in
            (overview?.categories.first { $0.category == quick.category }?.reclaimable.freed.bytes ?? 0) > 0
        }
        guard !chosen.isEmpty else {
            return HUDMessage(title: strings.text("hud.nothing"), lines: [], report: false)
        }
        await cleanup.quickClean(QuickCategory.all.filter(chosen.contains))
        await refresh([.picture, .cleanup])
        return Self.hud(cleanup.report, strings)
    }

    /// What the hotkey's panel says after a quick clean with `report`.
    public static func hud(_ report: ReportData?, _ strings: Strings) -> HUDMessage {
        guard let report, report.notRun == nil else {
            return HUDMessage(title: strings.text("hud.notRun"), lines: [report?.notRun ?? ""], report: false)
        }
        var lines = [
            strings.format("hud.found", Format.bytes(report.found, strings)),
            strings.format("hud.removed", Format.bytes(report.removed, strings)),
        ]
        switch report.freedOnDisk {
        case .grew(let bytes): lines.append(strings.format("hud.freed", Format.bytes(bytes, strings)))
        case .didNotGrow: lines.append(strings.text("hud.notGrown"))
        }
        if report.problems > 0 {
            lines.append(strings.format("hud.problems", report.problems))
        }
        return HUDMessage(title: strings.text("hud.cleaned"), lines: lines, report: true)
    }

    private func readLastReport(_ client: EngineClient) async {
        lastReport = await client.lastReport().map {
            ReportData.make($0, names: names, home: settings.home)
        }
    }

    /// Stop: cancels the running scan.
    public func stopScan() async {
        await client?.cancelScan()
        if userScan {
            await endUserScan()
        }
        await refresh([.activity])
    }

    /// The window is visible or not: the engine follows changes at once
    /// while it is.
    public func attention(_ looking: Bool) async {
        try? await client?.setAttention(looking)
    }

    /// Keeps `path` (⌘E): no candidate there is offered any more.
    public func exclude(_ path: String) async {
        _ = try? await client?.addExclusion(path)
    }

    /// The volume `root` is on ("Macintosh HD") and whether `root` is its
    /// mount point, read off the main thread.
    @concurrent
    static func volume(of root: String) async -> VolumeInfo {
        let url = URL(fileURLWithPath: root)
        let values = try? url.resourceValues(forKeys: [.volumeNameKey, .volumeURLKey])
        let mount = values?.volume?.standardizedFileURL.path
        return VolumeInfo(name: values?.volumeName ?? root, isRoot: mount == url.standardizedFileURL.path)
    }
}
