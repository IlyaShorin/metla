import Foundation
import MetlaCoreFFI
import Observation

/// One candidate as the category table shows it.
public struct CandidateRow: Identifiable, Equatable, Sendable {
    public let id: DirId
    public let path: String
    /// `~/…` below the home directory.
    public let shownPath: String
    public let rule: String
    public let activity: Date?
    /// Build output active within the recent-activity age (not preselected
    /// for that reason, not for being in a managed worktree): highlighted,
    /// and not checked by default.
    public let recent: Bool
    public let bytes: UInt64
    public let preselected: Bool
    /// Inside a worktree another tool manages.
    public let managed: Bool
    /// A worktree's state ("Clean", the reasons it is blocked, "Checking…");
    /// `nil` for everything else.
    public let state: String?
    /// Every reason a worktree is blocked for, one per entry.
    public let reasons: [String]
    public let branch: String?
    /// Only removable worktrees can be selected.
    public let selectable: Bool

    init(_ listed: Listed, home: String, _ names: Names) {
        let candidate = listed.candidate
        id = candidate.id
        path = candidate.path
        shownPath = Format.path(candidate.path, home: home)
        rule = names.rule(candidate.rule)
        activity = candidate.activity.map(Format.date)
        recent = !candidate.preselected && candidate.managed == nil && candidate.rule.hasPrefix("build.")
        bytes = candidate.bytes
        preselected = candidate.preselected
        managed = candidate.managed != nil
        state = listed.removability.map(names.removability)
        reasons = if case .blocked(_, let reasons)? = listed.removability { reasons.map(names.reason) } else { [] }
        switch listed.removability {
        case .removable(let branch)?:
            self.branch = branch
            selectable = true
        case .blocked(let branch, _)?:
            self.branch = branch
            selectable = false
        case .unknown?:
            branch = nil
            selectable = false
        case nil:
            branch = nil
            selectable = true
        }
    }
}

/// A category's table: a page of candidates loaded at a time, in the
/// order asked for, and which are checked. Rows not loaded yet are not
/// counted in the selection.
@MainActor
@Observable
public final class CategoryModel {
    public let category: Category
    /// Rows asked for at a time.
    public let pageSize: UInt32

    public private(set) var rows: [CandidateRow] = []
    public private(set) var total = 0
    public private(set) var order = Order(key: .size, descending: true)
    public private(set) var checked: Set<DirId> = []
    /// Rows whose checkbox the user set; the rest follow `preselected`.
    private var decided: Set<DirId> = []
    public private(set) var failure: String?

    private let client: EngineClient
    private let names: Names
    private let home: String

    public init(_ category: Category, client: EngineClient, names: Names, home: String, pageSize: UInt32 = 200) {
        self.category = category
        self.pageSize = pageSize
        self.client = client
        self.names = names
        self.home = home
    }

    /// Loads as many rows as are shown again, from the start.
    public func reload() async {
        let wanted = max(pageSize, UInt32(clamping: rows.count))
        await load(offset: 0, limit: wanted, replacing: true)
    }

    /// Loads the next page when the table needs rows past those loaded.
    public func loadMore() async {
        guard rows.count < total else {
            return
        }
        await load(offset: UInt64(rows.count), limit: pageSize, replacing: false)
    }

    /// Loads more when the row `id` is one of the last loaded.
    public func shown(_ id: DirId) async {
        guard let at = rows.lastIndex(where: { $0.id == id }), at >= rows.count - 20 else {
            return
        }
        await loadMore()
    }

    public func sort(by key: SortKey, descending: Bool) async {
        order = Order(key: key, descending: descending)
        rows = []
        await reload()
    }

    private func load(offset: UInt64, limit: UInt32, replacing: Bool) async {
        do {
            let page = try await client.candidates(category, order: order, offset: offset, limit: limit)
            let loaded = page.listed.map { CandidateRow($0, home: home, names) }
            rows = replacing ? loaded : rows + loaded
            total = Int(page.total)
            failure = nil
            settle()
        } catch {
            failure = String(describing: error)
        }
    }

    /// Checks what the user has not decided as the engine preselects it.
    private func settle() {
        for row in rows where !decided.contains(row.id) {
            if row.preselected && row.selectable {
                checked.insert(row.id)
            } else {
                checked.remove(row.id)
            }
        }
    }

    public func isChecked(_ id: DirId) -> Bool {
        checked.contains(id)
    }

    /// Space toggles a row's checkbox; a row that cannot be selected stays
    /// unchecked.
    public func toggle(_ id: DirId) {
        guard let row = rows.first(where: { $0.id == id }), row.selectable else {
            return
        }
        decided.insert(id)
        if checked.contains(id) {
            checked.remove(id)
        } else {
            checked.insert(id)
        }
    }

    /// ⌘A: every loaded row that can be selected.
    public func selectAll() {
        for row in rows where row.selectable {
            decided.insert(row.id)
            checked.insert(row.id)
        }
    }

    /// "selected N of M" and their size.
    public var selection: (count: Int, bytes: UInt64) {
        let chosen = rows.filter { checked.contains($0.id) }
        return (chosen.count, chosen.reduce(0) { $0 + $1.bytes })
    }

    /// Excludes the row's path (keep this); the table loads again.
    public func exclude(_ id: DirId) async {
        guard let row = rows.first(where: { $0.id == id }) else {
            return
        }
        do {
            _ = try await client.addExclusion(row.path)
            await reload()
        } catch {
            failure = String(describing: error)
        }
    }
}
