import Foundation
import MetlaCoreFFI
import Observation

/// A running cleanup as the progress view shows it.
public struct CleanupProgressData: Equatable, Sendable {
    public init(done: UInt64, items: UInt64, removed: UInt64, current: String?) {
        self.done = done
        self.items = items
        self.removed = removed
        self.current = current
    }

    public let done: UInt64
    public let items: UInt64
    public let removed: UInt64
    /// The item being cleaned now (`~/…`).
    public let current: String?

    public var share: Double {
        items == 0 ? 0 : min(1, Double(done) / Double(items))
    }
}

/// The cleanup flow: the preview of a selection, its execution with
/// exactly what the user confirmed, the progress, and the report; and the
/// hotkey's quick clean, which has no preview. Nothing is removed except
/// by `clean()` on a preview, or `quickClean` of quick categories.
@MainActor
@Observable
public final class CleanupModel {
    public enum Stage: Equatable, Sendable {
        case closed
        /// The engine builds the preview (worktrees are asked with Git).
        case loading
        case preview
        case running
        case report
    }

    public private(set) var stage: Stage = .closed
    public private(set) var preview: PreviewModel?
    public private(set) var progress: CleanupProgressData?
    public private(set) var report: ReportData?
    public private(set) var failure: String?
    /// The report shows only what did not go as asked.
    public var problemsOnly = false

    private let client: EngineClient
    private let names: Names
    private let home: String
    /// How often a running cleanup's progress is read.
    public static let pollInterval: Duration = .milliseconds(100)

    public init(client: EngineClient, names: Names, home: String) {
        self.client = client
        self.names = names
        self.home = home
    }

    /// A cleanup runs.
    public var running: Bool {
        stage == .running
    }

    /// Shows the preview of `selection`.
    public func open(_ selection: [DirId]) async {
        stage = .loading
        failure = nil
        do {
            let built = try await client.preview(selection)
            preview = PreviewModel(built, names: names, home: home)
            stage = .preview
        } catch {
            failure = String(describing: error)
            stage = .closed
        }
    }

    /// Executes the preview with exactly what is checked, follows its
    /// progress and shows its report. Does nothing unless the
    /// confirmation covers at least one item.
    public func clean() async {
        guard let preview, preview.canClean, stage == .preview else {
            return
        }
        let confirmed = preview.confirmations
        let id = preview.preview.id
        await run { try await self.client.executeConfirmed(id, confirmed) }
    }

    /// Cleans the preselected candidates of exactly `categories`, with no
    /// preview; the report says what happened.
    public func quickClean(_ categories: [QuickCategory]) async {
        await run { try await self.client.quickClean(categories) }
    }

    /// Cancel: the cleanup stops before the next entry; the report says
    /// truthfully what was done.
    public func cancel() async {
        await client.cancelCleanup()
    }

    /// Closes the sheet; a running cleanup goes on.
    public func close() {
        guard stage != .running else {
            return
        }
        stage = .closed
        preview = nil
        progress = nil
        problemsOnly = false
    }

    /// Shows the report the engine keeps for the cleanup that just ended.
    private func showReport() async {
        guard let last = await client.lastReport() else {
            stage = .closed
            return
        }
        report = ReportData.make(last, names: names, home: home)
        stage = .report
    }

    private func run(_ start: @escaping () async throws -> Void) async {
        let ended = client.cleanupsEnded
        stage = .running
        progress = nil
        failure = nil
        let activity = ProcessInfo.processInfo.beginActivity(
            options: .userInitiated, reason: "Cleaning what the user confirmed")
        defer { ProcessInfo.processInfo.endActivity(activity) }
        do {
            try await start()
        } catch {
            failure = String(describing: error)
            stage = .closed
            return
        }
        while client.cleanupsEnded == ended {
            await readProgress()
            try? await Task.sleep(for: Self.pollInterval)
        }
        await showReport()
        progress = nil
    }

    private func readProgress() async {
        guard let now = await client.cleanupProgress() else {
            return
        }
        progress = CleanupProgressData(
            done: now.done, items: now.items, removed: now.removed,
            current: now.current.map { Format.path($0, home: home) })
    }
}
