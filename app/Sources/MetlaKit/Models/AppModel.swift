import Foundation
import MetlaCoreFFI
import Observation

/// The main window's screens.
public enum Screen: Hashable, Sendable {
    case overview
    case folders
    case category(Category)
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
    public var showsCleanSheet = false
    public private(set) var categories: [Category: CategoryModel] = [:]
    public private(set) var folders: FoldersModel?

    private var pacer = RefreshPacer()
    private var listener: Task<Void, Never>?
    private var poller: Task<Void, Never>?
    /// While a scan or an update runs.
    public static let pollInterval: Duration = .milliseconds(500)
    public static let shownFolders: UInt32 = 6

    public init(settings: LaunchSettings, strings: Strings = .current) {
        self.settings = settings
        self.strings = strings
        names = Names(strings, titles: ruleTitles())
    }

    /// Opens the engine off the main thread, shows the saved picture, and
    /// follows the engine from then on.
    public func start(mainThread: MainThreadGuard = .standard) async {
        do {
            let client = try await EngineClient.open(settings.options, mainThread: mainThread)
            self.client = client
            folders = FoldersModel(root: settings.root, client: client, names: names)
            volume = await Self.volume(of: settings.root)
            await refresh(Self.everything)
            listen(to: client)
        } catch {
            failure = String(describing: error)
        }
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
        case .overview:
            break
        }
    }

    /// "Show in Folders": the Folders screen, opened down to `path`.
    public func showInFolders(_ path: String) async {
        screen = .folders
        await folders?.reveal(path)
    }

    /// Refresh (⌘R): a full scan the user asked for.
    public func rescan() async {
        try? await client?.startScan()
        await refresh([.activity])
    }

    /// Stop: cancels the running scan.
    public func stopScan() async {
        await client?.cancelScan()
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
