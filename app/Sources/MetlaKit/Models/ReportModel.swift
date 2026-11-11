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
    /// Its mark, and whether the "problems only" filter shows it.
    public let tone: ReportTone
    /// Leftover paths and their errors, for a partial removal.
    public let details: [String]

    /// Something stayed or went wrong: shown by the "problems only"
    /// filter, with Reveal in Finder for what remains.
    public var problem: Bool {
        tone == .problem
    }
}

/// How a report marks an item.
public enum ReportTone: Equatable, Sendable {
    /// Done as asked: removed, moved to the Trash, or already gone.
    case done
    /// Left alone because the user chose so: not confirmed, or cancelled.
    case untouched
    /// Something stayed or went wrong: skipped by a check, partly
    /// removed, failed.
    case problem

    init(_ outcome: Outcome) {
        switch outcome {
        case .removed, .alreadyGone, .movedToTrash: self = .done
        case .skipped(.notConfirmed), .skipped(.cancelled): self = .untouched
        case .partial, .skipped, .failed: self = .problem
        }
    }
}

/// "Freed on disk" as a report shows it: a measured change of the
/// volume's free space that is zero or negative is not a freed amount.
public enum FreedOnDisk: Equatable, Sendable {
    /// The volume's free space grew by this much.
    case grew(UInt64)
    /// It stayed or shrank: other programs wrote meanwhile, or clones and
    /// snapshots still hold the blocks.
    case didNotGrow

    init(_ measured: Int64) {
        self = measured > 0 ? .grew(UInt64(measured)) : .didNotGrow
    }
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
            outcome: names.outcome(item.outcome), kind: OutcomeKind(item.outcome), tone: ReportTone(item.outcome),
            details: details(item.outcome, names: names, home: home))
    }

    /// What a partial removal left: each path with why, the errno as a
    /// detail, and the other volumes left as they are; for a failure, what
    /// the Trash or Git said.
    static func details(_ outcome: Outcome, names: Names, home: String) -> [String] {
        switch outcome {
        case .failed(.trashRefused(_, _, let said)), .failed(.gitRefused(let said)):
            return [said]
        case .partial(let left):
            return leftovers(left, names: names, home: home)
        default:
            return []
        }
    }

    static func leftovers(_ left: Leftover, names: Names, home: String) -> [String] {
        let strings = names.strings
        let failed = left.first.map { failed in
            strings.format(
                "report.leftover", Format.path(failed.path, home: home), names.errno(failed.errno), Int(failed.errno))
        }
        return failed + left.mounts.map { strings.format("report.leftoverMount", Format.path($0, home: home)) }
    }

    /// "Freed on disk": the growth of the volume's free space, if it grew.
    public var freedOnDisk: FreedOnDisk {
        FreedOnDisk(freed)
    }

    /// The figure under "Freed on disk": an amount, or a dash when the
    /// free space did not grow (never a negative amount).
    public func freedFigure(_ strings: Strings) -> String {
        switch freedOnDisk {
        case .grew(let bytes): Format.bytes(bytes, strings)
        case .didNotGrow: "\u{2014}"
        }
    }

    /// The sentence under the numbers: why the freed amount may differ
    /// from what was removed, or why the free space did not grow.
    public func freedNote(_ strings: Strings) -> String {
        switch freedOnDisk {
        case .grew: strings.text("report.freedNote")
        case .didNotGrow: strings.text("report.notGrown")
        }
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
