import Foundation
import MetlaCoreFFI
import Observation

/// Starting the app at login (the app: `SMAppService.mainApp`).
@MainActor
public protocol LoginItem: AnyObject {
    var enabled: Bool { get }
    /// Registers or unregisters; the error says why it did not.
    func set(_ enabled: Bool) throws
}

/// Settings → General: what the engine keeps (background analysis,
/// battery behaviour, the two ages) and what the app keeps (launch at
/// login, the menu bar).
@MainActor
@Observable
public final class GeneralModel {
    public private(set) var settings: EngineSettings?
    public private(set) var launchAtLogin = false
    public private(set) var loginFailure: String?
    public private(set) var showInMenuBar = true

    private let client: EngineClient
    private let login: any LoginItem
    private let preferences: any Preferences
    static let menuBarKey = "showInMenuBar"
    static let day: TimeInterval = 24 * 3600

    public init(client: EngineClient, login: any LoginItem, preferences: any Preferences) {
        self.client = client
        self.login = login
        self.preferences = preferences
        showInMenuBar = Self.showsInMenuBar(preferences)
        launchAtLogin = login.enabled
    }

    /// The menu-bar switch as kept (on unless turned off).
    public static func showsInMenuBar(_ preferences: any Preferences) -> Bool {
        preferences.string(forKey: menuBarKey) != "0"
    }

    public func load() async {
        settings = await client.settings()
    }

    public func setBackground(_ on: Bool) async {
        await change { $0.background = on }
    }

    public func setReduceOnBattery(_ on: Bool) async {
        await change { $0.reduceOnBattery = on }
    }

    /// Worktrees changed within this many days are in use.
    public func setStaleDays(_ days: Int) async {
        await change { $0.staleAfter = TimeInterval(max(1, days)) * Self.day }
    }

    /// Build output active within this many days is not preselected.
    public func setRecentDays(_ days: Int) async {
        await change { $0.recentAge = TimeInterval(max(1, days)) * Self.day }
    }

    public var staleDays: Int {
        Int(((settings?.staleAfter ?? 0) / Self.day).rounded())
    }

    public var recentDays: Int {
        Int(((settings?.recentAge ?? 0) / Self.day).rounded())
    }

    private func change(_ edit: (inout EngineSettings) -> Void) async {
        var next = await client.settings()
        edit(&next)
        try? await client.setSettings(next)
        settings = next
    }

    public func setLaunchAtLogin(_ on: Bool) {
        do {
            try login.set(on)
            loginFailure = nil
        } catch {
            loginFailure = error.localizedDescription
        }
        launchAtLogin = login.enabled
    }

    public func setShowInMenuBar(_ on: Bool) {
        preferences.set(on ? "1" : "0", forKey: Self.menuBarKey)
        showInMenuBar = on
    }
}

/// One kept path and what it hides.
public struct ExclusionRow: Identifiable, Equatable, Sendable {
    public var id: String { path }
    public let path: String
    public let shownPath: String
    /// "hides 2 items · 1.2 GB" or "hides nothing that can be cleaned".
    public let hides: String
}

/// Settings → Exclusions: the kept paths, with what each hides.
@MainActor
@Observable
public final class ExclusionsModel {
    public private(set) var rows: [ExclusionRow] = []
    public private(set) var failure: String?
    private let client: EngineClient
    private let strings: Strings
    private let home: String

    public init(client: EngineClient, strings: Strings, home: String) {
        self.client = client
        self.strings = strings
        self.home = home
    }

    public func load() async {
        do {
            var rows: [ExclusionRow] = []
            for path in try await client.exclusions() {
                let hidden = try await client.hiddenBy(path)
                rows.append(ExclusionRow(path: path, shownPath: Format.path(path, home: home), hides: hides(hidden)))
            }
            self.rows = rows
            failure = nil
        } catch {
            failure = String(describing: error)
        }
    }

    func hides(_ tally: Tally) -> String {
        tally.count == 0
            ? strings.text("exclusions.hidesNothing")
            : strings.format("exclusions.hides", Int(tally.count), Format.bytes(tally.bytes, strings))
    }

    /// "+" and folders dropped from Finder.
    public func add(_ paths: [String]) async {
        var refused: String?
        for path in paths {
            do {
                _ = try await client.addExclusion(path)
            } catch {
                refused = String(describing: error)
            }
        }
        await load()
        failure = refused ?? failure
    }

    /// "−".
    public func remove(_ path: String) async {
        _ = try? await client.removeExclusion(path)
        await load()
    }
}

/// Settings → Access, and the first-run sheet: Full Disk Access as probed,
/// and whether the first-run sheet was seen.
@MainActor
@Observable
public final class AccessModel {
    public private(set) var state: FullDiskAccess = .unknown
    public private(set) var firstRunDone: Bool
    private let probe: any AccessProbe
    private let preferences: any Preferences
    static let firstRunKey = "firstRunDone"
    /// System Settings → Privacy & Security → Full Disk Access.
    public static let settingsURL = URL(
        string: "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")!

    public init(probe: any AccessProbe, preferences: any Preferences) {
        self.probe = probe
        self.preferences = preferences
        firstRunDone = preferences.string(forKey: Self.firstRunKey) == "1"
    }

    /// Probes again ("Re-check").
    public func check() async {
        state = FullDiskAccess.from(await probe.answers())
    }

    /// The first-run sheet shows once.
    public var showsFirstRun: Bool {
        !firstRunDone
    }

    /// The first-run sheet was answered either way.
    public func finishFirstRun() {
        preferences.set("1", forKey: Self.firstRunKey)
        firstRunDone = true
    }
}
