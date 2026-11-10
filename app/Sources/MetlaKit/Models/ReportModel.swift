import Foundation
import MetlaCoreFFI

/// What happened to an item, without its details.
public enum OutcomeKind: Equatable, Sendable {
    case removed, gone, trash, partial, skipped, failed

    init(_ outcome: Outcome) {
        switch outcome {
        case .removed: self = .removed
        case .alreadyGone: self = .gone
        case .movedToTrash: self = .trash
        case .partial: self = .partial
        case .skipped: self = .skipped
        case .failed: self = .failed
        }
    }
}

/// One item of a cleanup report as the report lists it.
public struct ReportRow: Identifiable, Equatable, Sendable {
    public let id: DirId
    public let path: String
    public let shownPath: String
    public let rule: String
    public let found: UInt64
    public let removed: UInt64
    /// "Removed", "Skipped: not confirmed", "Failed: …"
    public let outcome: String
    public let kind: OutcomeKind
    /// Something stayed or went wrong: shown by the "problems only"
    /// filter, with Reveal in Finder for what remains.
    public let problem: Bool
    /// Leftover paths and their errors, for a partial removal.
    public let details: [String]
}

/// A cleanup's report as the report sheet and "Last report" show it: the
/// engine's three numbers, what went to the Trash, and every item's
/// outcome, or why the cleanup did not run.
public struct ReportData: Equatable, Sendable {
    public let preview: PreviewId?
    public let found: UInt64
    public let removed: UInt64
    /// The measured change in free space; may differ either way.
    public let freed: Int64
    public let movedToTrash: UInt64
    public let rows: [ReportRow]
    /// Why nothing ran, when nothing did.
    public let notRun: String?

    public static func make(_ result: CleanupResult, names: Names, home: String) -> ReportData {
        switch result {
        case .notRun(let reason):
            return ReportData(
                preview: nil, found: 0, removed: 0, freed: 0, movedToTrash: 0, rows: [], notRun: reason)
        case .done(let report, let totals):
            let rows = report.items.map { row($0, names: names, home: home) }
            return ReportData(
                preview: report.preview, found: totals.found, removed: totals.removed, freed: totals.freed,
                movedToTrash: totals.movedToTrash, rows: rows, notRun: nil)
        }
    }

    static func row(_ item: ItemReport, names: Names, home: String) -> ReportRow {
        ReportRow(
            id: item.candidate, path: item.path, shownPath: Format.path(item.path, home: home),
            rule: names.rule(item.rule), found: item.found, removed: item.removed,
            outcome: names.outcome(item.outcome), kind: OutcomeKind(item.outcome), problem: problem(item.outcome),
            details: details(item.outcome, home: home))
    }

    static func problem(_ outcome: Outcome) -> Bool {
        switch outcome {
        case .removed, .alreadyGone, .movedToTrash: false
        case .partial, .skipped, .failed: true
        }
    }

    static func details(_ outcome: Outcome, home: String) -> [String] {
        guard case .partial(let left) = outcome else {
            return []
        }
        let failed = left.first.map { "\(Format.path($0.path, home: home)) (errno \($0.errno))" }
        return failed + left.mounts.map { Format.path($0, home: home) }
    }

    /// The rows the "problems only" filter shows, or all of them.
    public func rows(problemsOnly: Bool) -> [ReportRow] {
        problemsOnly ? rows.filter(\.problem) : rows
    }

    /// How many items did not go as asked.
    public var problems: Int {
        rows.filter(\.problem).count
    }
}
