import Foundation
import MetlaCoreFFI

/// End-to-end runs of the cleanup flows through the real view models on a
/// fresh fixture (`--scenario <name>`): the UI-level proof that a flow
/// removes exactly what was confirmed. Each run checks the fixture on
/// disk afterwards and answers 0 when it is as expected.
@MainActor
public enum Scenarios {
    public nonisolated static let names = ["default-clean", "selected-worktree", "cancel", "quick-clean"]

    /// Runs `name` on the fixture `settings` names; 0 when the disk ends
    /// as expected, 1 when not, 2 when it cannot run.
    public static func run(_ name: String, settings: LaunchSettings, strings: Strings) async -> Int32 {
        guard settings.isFixture else {
            print("scenario: \(settings.root) is not a fixture")
            return 2
        }
        let trash = FixtureTrash(root: settings.root)
        let model = AppModel(settings: settings, strings: strings, preferences: MemoryPreferences(), probe: NoProbe())
        await model.start(trash: trash)
        defer { model.stop() }
        var check = Check(root: settings.root)
        guard await settled(model) else {
            print("scenario: the first scan did not end")
            return 2
        }
        switch name {
        case "default-clean": await defaultClean(model, &check)
        case "selected-worktree": await selectedWorktree(model, &check)
        case "cancel": await cancel(model, trash: trash, &check)
        case "quick-clean": await quickClean(model, &check)
        default:
            print("scenario: no scenario \(name); there are \(names.joined(separator: ", "))")
            return 2
        }
        if let report = model.cleanup?.report {
            check.agrees(with: report)
        }
        check.print(name)
        return check.failures.isEmpty ? 0 : 1
    }

    /// Waits for the first scan, at most a minute.
    static func settled(_ model: AppModel) async -> Bool {
        for _ in 0..<1200 {
            await model.refresh([.picture])
            if case .idle? = model.freshness?.state, model.overview?.freshness.fullScan != nil {
                return true
            }
            try? await Task.sleep(for: .milliseconds(50))
        }
        return false
    }

    /// "Clean…": the default selection, both separate groups checked.
    static func defaultClean(_ model: AppModel, _ check: inout Check) async {
        guard let cleanup = model.cleanup else { return }
        await model.cleanDefault()
        guard let preview = cleanup.preview else {
            check.fail("no preview")
            return
        }
        preview.confirm(.moveToTrash, true)
        preview.confirm(.emptyTrash, true)
        check.expect(preview.groups.count == 3, "three groups: quick, to the Trash, empty the Trash")
        await cleanup.clean()
        check.gone([
            "home/Library/Logs/com.example.editor", "home/code/app/target", "home/code/app/node_modules",
            "home/code/app-clean/target", "home/code/app-dirty/target",
        ])
        check.empty([
            "home/Library/Caches/com.example.editor", "home/Library/Caches/Homebrew", "home/.npm/_cacache",
            "home/.Trash", "home/Library/Logs/DiagnosticReports",
        ])
        check.kept([
            "home/Documents/letter.txt", "home/code/app/src/main.rs", "home/code/app-clean/README.md",
            "home/code/app-dirty/README.md", "home/code/app/.claude/worktrees/feature/target/debug/app",
            "home/.codex/worktrees/a1b2/app/target/debug/app",
        ])
    }

    /// "Clean selected…" with a removable worktree checked in its table.
    static func selectedWorktree(_ model: AppModel, _ check: inout Check) async {
        guard let cleanup = model.cleanup, let table = model.category(.gitWorktree) else { return }
        let path = (model.settings.root as NSString).appendingPathComponent("home/code/app-clean")
        for _ in 0..<600 {
            await table.reload()
            if table.rows.contains(where: { $0.path == path && $0.selectable }) { break }
            try? await Task.sleep(for: .milliseconds(50))
        }
        guard let row = table.rows.first(where: { $0.path == path && $0.selectable }) else {
            check.fail("the clean worktree never became selectable")
            return
        }
        table.toggle(row.id)
        await model.cleanSelected()
        guard let preview = cleanup.preview else {
            check.fail("no preview")
            return
        }
        check.expect(preview.groups.map(\.id) == [.separate(.removeWorktrees)], "one group: git worktree remove")
        check.expect(!preview.canClean, "the worktree waits for its own confirmation")
        preview.confirm(.removeWorktrees, true)
        await cleanup.clean()
        check.gone(["home/code/app-clean"])
        check.kept([
            "home/code/app-dirty/README.md", "home/code/app-untracked/notes.txt", "home/code/app-locked",
            "home/Library/Caches/com.example.editor/cache.db", "home/code/app/target/debug/app",
        ])
    }

    /// "Clean…" with the Trash group checked, cancelled while the first
    /// item goes to the Trash.
    static func cancel(_ model: AppModel, trash: FixtureTrash, _ check: inout Check) async {
        guard let cleanup = model.cleanup, let client = model.client else { return }
        trash.onMove = { client.cancelNow() }
        await model.cleanDefault()
        cleanup.preview?.confirm(.moveToTrash, true)
        await cleanup.clean()
        let rows = cleanup.report?.rows ?? []
        let cancelled = model.strings.format("outcome.skipped", model.strings.text("skip.cancelled"))
        let skipped = rows.filter { $0.outcome == cancelled }
        check.expect(!skipped.isEmpty, "items after the cancel are skipped")
        check.expect(trash.moved.count == 1, "one item went to the Trash before the cancel")
        check.kept(skipped.map { String($0.path.dropFirst(model.settings.root.count + 1)) })
        check.kept(["home/.Trash/old-installer.dmg", "home/Documents/letter.txt"])
    }

    /// The Clean hotkey in "clean immediately" mode with two quick
    /// categories checked: only those go.
    static func quickClean(_ model: AppModel, _ check: inout Check) async {
        let registrar = NoRegistrar()
        await model.attachHotkeys(registrar)
        guard let hotkeys = model.hotkeys else { return }
        await hotkeys.choose(.immediate)
        await hotkeys.confirmImmediate()
        await hotkeys.setQuick(.appCache, true)
        await hotkeys.setQuick(.devCache, true)
        guard case .hud(let message) = await model.cleanHotkey() else {
            check.fail("the hotkey did not clean")
            return
        }
        check.expect(message.report, "the panel offers the report")
        check.empty(["home/Library/Caches/com.example.editor", "home/Library/Caches/Homebrew", "home/.npm/_cacache"])
        check.kept([
            "home/code/app/target/debug/app", "home/code/app/node_modules/left-pad/index.js",
            "home/Library/Logs/com.example.editor/today.log", "home/.Trash/old-installer.dmg",
            "home/code/app-clean/README.md",
        ])
    }
}

/// What a scenario found wrong.
struct Check {
    let root: String
    var failures: [String] = []
    var passed = 0

    init(root: String) {
        self.root = root
    }

    func full(_ relative: String) -> String {
        (root as NSString).appendingPathComponent(relative)
    }

    mutating func expect(_ condition: Bool, _ what: String) {
        if condition {
            passed += 1
        } else {
            failures.append(what)
        }
    }

    mutating func fail(_ what: String) {
        failures.append(what)
    }

    mutating func gone(_ relatives: [String]) {
        for relative in relatives {
            expect(!FileManager.default.fileExists(atPath: full(relative)), "\(relative) is gone")
        }
    }

    mutating func kept(_ relatives: [String]) {
        for relative in relatives {
            expect(FileManager.default.fileExists(atPath: full(relative)), "\(relative) is kept")
        }
    }

    /// The directories are there and hold nothing (a kept directory).
    mutating func empty(_ relatives: [String]) {
        for relative in relatives {
            let entries = try? FileManager.default.contentsOfDirectory(atPath: full(relative))
            expect(entries?.isEmpty == true, "\(relative) is there and empty")
        }
    }

    /// The disk says what the report says: an item reported removed,
    /// moved or gone is gone (or, where its rule keeps the directory,
    /// empty); one reported skipped is still there.
    mutating func agrees(with report: ReportData) {
        let files = FileManager.default
        for row in report.rows {
            let there = files.fileExists(atPath: row.path)
            let empty = (try? files.contentsOfDirectory(atPath: row.path))?.isEmpty == true
            switch row.kind {
            case .removed, .trash, .gone:
                expect(!there || empty, "\(row.shownPath): \(row.outcome), and so it is")
            case .skipped:
                expect(there, "\(row.shownPath): \(row.outcome), and still there")
            case .partial, .failed:
                break
            }
        }
    }

    func print(_ name: String) {
        Swift.print("scenario \(name): \(passed) checks passed, \(failures.count) failed")
        for failure in failures {
            Swift.print("  FAILED: \(failure)")
        }
    }
}

/// A Trash beside the fixture: moves into `<root>-trash` on the same
/// volume, never into the system's; can do something while it moves (a
/// scenario's Cancel).
public final class FixtureTrash: TrashHost, @unchecked Sendable {
    public let dir: String
    private let lock = NSLock()
    private var movedPaths: [String] = []
    /// Called as each move starts, on the engine's thread.
    public var onMove: @Sendable () -> Void = {}

    public init(root: String) {
        dir = root + "-trash"
        try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
    }

    public var moved: [String] {
        lock.withLock { movedPaths }
    }

    public func trash(path: String) -> TrashOutcome {
        onMove()
        let target = (dir as NSString).appendingPathComponent(UUID().uuidString)
        do {
            try FileManager.default.moveItem(atPath: path, toPath: target)
            lock.withLock { movedPaths.append(path) }
            return .moved
        } catch {
            return .refused(reason: error.localizedDescription)
        }
    }
}

/// Preferences that live in memory only.
public final class MemoryPreferences: Preferences, @unchecked Sendable {
    private let lock = NSLock()
    private var values: [String: String] = [:]

    public init() {}

    public func string(forKey key: String) -> String? {
        lock.withLock { values[key] }
    }

    public func set(_ value: String?, forKey key: String) {
        lock.withLock { values[key] = value }
    }
}

/// A probe that tries nothing.
struct NoProbe: AccessProbe {
    func answers() async -> [ProbeAnswer] {
        []
    }
}

/// A registrar that registers nothing with the system (scenarios and
/// captures).
@MainActor
public final class NoRegistrar: HotkeyRegistrar {
    public init() {}

    public func register(_ combo: KeyCombo?, for action: HotkeyAction) -> Registration {
        combo == nil ? .unassigned : .registered
    }
}
