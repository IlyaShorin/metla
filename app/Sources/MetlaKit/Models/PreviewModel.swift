import Foundation
import MetlaCoreFFI
import Observation

/// The candidates chosen for "Clean selected…": the category tables'
/// checkboxes and the Folders view's "add to cleanup" feed this one set.
public struct CleanupSelection: Equatable, Sendable {
    public private(set) var ids: Set<DirId> = []

    public init(_ ids: Set<DirId> = []) {
        self.ids = ids
    }

    public mutating func set(_ id: DirId, _ chosen: Bool) {
        if chosen {
            ids.insert(id)
        } else {
            ids.remove(id)
        }
    }

    public func contains(_ id: DirId) -> Bool {
        ids.contains(id)
    }

    /// Keeps only `offered`: after a cleanup or a rescan nothing the engine
    /// no longer offers stays chosen.
    public mutating func keep(only offered: Set<DirId>) {
        ids.formIntersection(offered)
    }

    /// In a fixed order, for the engine.
    public var sorted: [DirId] {
        ids.sorted()
    }
}

/// One item of a preview as the sheet lists it.
public struct PreviewRow: Identifiable, Equatable, Sendable {
    public let id: DirId
    public let path: String
    public let shownPath: String
    public let rule: String
    public let bytes: UInt64
    public let warnings: [String]
    /// Inside a worktree another tool manages: it needs a checkbox of its
    /// own, unchecked until the user checks it.
    public let ownConfirmation: Bool
}

/// One group of a preview: how its items are removed, and whether it
/// needs a confirmation of its own.
public struct PreviewGroup: Identifiable, Equatable, Sendable {
    public let id: Group
    /// The badge: "Permanently, bypassing Trash", "Move to Trash", …
    public let method: String
    /// The rules' titles, once each, largest first.
    public let rules: [String]
    public let rows: [PreviewRow]
    public let bytes: UInt64

    /// The separate group to confirm, or `nil` for the quick group, which
    /// the main confirmation covers.
    public var separate: Separate? {
        if case .separate(let separate) = id { separate } else { nil }
    }
}

/// A selected candidate the preview set aside, with why.
public struct SetAside: Identifiable, Equatable, Sendable {
    public let id: DirId
    public let shownPath: String
    public let rule: String
    public let reason: String
}

/// The cleanup preview sheet: the engine's preview grouped as it is
/// confirmed, and what the user checked. The confirmation sent is built
/// from exactly what is checked; the engine skips everything else as not
/// confirmed. The destructive action is possible only while it covers at
/// least one item.
@MainActor
@Observable
public final class PreviewModel {
    public let preview: CleanupPreview
    public let groups: [PreviewGroup]
    public let setAside: [SetAside]
    public private(set) var confirmedGroups: Set<Separate> = []
    public private(set) var confirmedItems: Set<DirId> = []
    /// What the current confirmation covers, as the core decides it.
    public private(set) var covered: Set<DirId> = []
    private let strings: Strings

    public init(_ preview: CleanupPreview, names: Names, home: String) {
        self.preview = preview
        strings = names.strings
        groups = Self.grouped(preview.items, names: names, home: home)
        setAside = preview.rejected.map { rejected in
            SetAside(
                id: rejected.candidate.id, shownPath: Format.path(rejected.candidate.path, home: home),
                rule: names.rule(rejected.candidate.rule),
                reason: Self.why(rejected.why, preview: preview, names: names, home: home))
        }
        recount()
    }

    static func grouped(_ items: [PreviewItem], names: Names, home: String) -> [PreviewGroup] {
        let order: [Group] = [.quick, .separate(.moveToTrash), .separate(.emptyTrash), .separate(.removeWorktrees)]
        let byGroup = Dictionary(grouping: items) { groupOf(rule: $0.candidate.rule) }
        return order.compactMap { group in
            guard let items = byGroup[group], !items.isEmpty else { return nil }
            let rows = items.map { item in
                PreviewRow(
                    id: item.candidate.id, path: item.candidate.path,
                    shownPath: Format.path(item.candidate.path, home: home),
                    rule: names.rule(item.candidate.rule), bytes: item.candidate.bytes,
                    warnings: item.warnings.map(names.warning), ownConfirmation: item.candidate.managed != nil)
            }
            var rules: [String] = []
            for row in rows where !rules.contains(row.rule) {
                rules.append(row.rule)
            }
            return PreviewGroup(
                id: group, method: names.method(group), rules: rules, rows: rows,
                bytes: rows.reduce(0) { $0 + $1.bytes })
        }
    }

    static func why(_ rejection: Rejection, preview: CleanupPreview, names: Names, home: String) -> String {
        switch rejection {
        case .blocked(let reasons):
            return reasons.map(names.reason).joined(separator: ", ")
        case .nested(let inside):
            let outer = preview.items.first { $0.candidate.id == inside }?.candidate.path ?? ""
            return names.strings.format("preview.nested", Format.path(outer, home: home))
        }
    }

    /// Checks or unchecks a separate group.
    public func confirm(_ separate: Separate, _ on: Bool) {
        if on {
            confirmedGroups.insert(separate)
        } else {
            confirmedGroups.remove(separate)
        }
        recount()
    }

    /// Checks or unchecks an item inside a managed worktree.
    public func confirmItem(_ id: DirId, _ on: Bool) {
        if on {
            confirmedItems.insert(id)
        } else {
            confirmedItems.remove(id)
        }
        recount()
    }

    /// The typed confirmation: exactly what is checked.
    public var confirmations: Confirmations {
        Confirmations(groups: confirmedGroups.sorted { "\($0)" < "\($1)" }, items: confirmedItems.sorted())
    }

    private func recount() {
        covered = Set(MetlaCoreFFI.covered(preview: preview, confirmed: confirmations))
    }

    /// The items the destructive action would act on, and their estimate.
    public var coveredTotals: (count: Int, bytes: UInt64) {
        let rows = groups.flatMap(\.rows).filter { covered.contains($0.id) }
        return (rows.count, rows.reduce(0) { $0 + $1.bytes })
    }

    /// The destructive button is enabled.
    public var canClean: Bool {
        !covered.isEmpty
    }

    /// What the destructive button will do with the boxes as they are
    /// now: "Remove 7 items — about 1.2 GB?", "Move 2 items to the Trash
    /// — about 4 MB?", or that nothing is checked. What is offered but
    /// unchecked shows in its group.
    public var title: String {
        let (count, bytes) = coveredTotals
        guard count > 0 else {
            return strings.text("preview.titleNone")
        }
        return strings.format(onlyTrash ? "preview.titleTrash" : "preview.title", count, Format.bytes(bytes, strings))
    }

    /// Named after what it does: "Move 2 items to Trash" when everything
    /// covered goes there, else "Remove N items".
    public var action: String {
        strings.format(onlyTrash ? "preview.actionTrash" : "preview.action", coveredTotals.count)
    }

    /// Everything covered goes to the Trash.
    private var onlyTrash: Bool {
        let trash = groups.filter { $0.id == .separate(.moveToTrash) }.flatMap(\.rows).map(\.id)
        return !covered.isEmpty && covered.isSubset(of: Set(trash))
    }
}
