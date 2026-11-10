import MetlaCoreFFI
import MetlaKit
import SwiftUI

/// A category's candidates: checkbox, path, rule, activity, size (and a
/// worktree's branch and state), sortable, loaded a page at a time.
struct CategoryView: View {
    let model: AppModel
    @Bindable var table: CategoryModel
    @State private var selected = Set<DirId>()
    @State private var sortOrder = [KeyPathComparator(\CandidateRow.bytes, order: .reverse)]

    private var strings: Strings { model.strings }
    private var worktrees: Bool { table.category == .gitWorktree }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            grid
            .contextMenu(forSelectionType: DirId.self) { ids in
                menu(for: ids)
            }
            .onChange(of: sortOrder) { _, order in
                Task { await sort(order) }
            }
            .onKeyPress(.space) {
                selected.forEach(table.toggle)
                return .handled
            }
            .onKeyPress(keys: ["a"]) { press in
                guard press.modifiers.contains(.command) else { return .ignored }
                table.selectAll()
                return .handled
            }
            .onKeyPress(keys: ["e"]) { press in
                guard press.modifiers.contains(.command) else { return .ignored }
                exclude(selected)
                return .handled
            }
            .onKeyPress(.return) {
                reveal(selected)
                return .handled
            }
            Text(strings.text("category.hint"))
                .font(.caption).foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 16).padding(.vertical, 8)
        }
    }

    private var header: some View {
        let selection = table.selection
        return HStack(spacing: 12) {
            Text(strings.format("category.selected", selection.count, table.total, Format.bytes(selection.bytes, strings)))
                .foregroundStyle(.secondary)
            Spacer()
            Button(strings.text("category.clean")) { model.showsCleanSheet = true }
                .buttonStyle(.borderedProminent)
                .disabled(selection.count == 0)
        }
        .padding(.horizontal, 16).padding(.vertical, 10)
    }

    /// Worktrees have two columns more: branch and state. (Columns chosen
    /// inside one table need macOS 14.4, so there are two tables.)
    @ViewBuilder private var grid: some View {
        if worktrees {
            Table(of: CandidateRow.self, selection: $selected, sortOrder: $sortOrder) {
                check
                path
                rule
                branch
                state
                activity
                size
            } rows: {
                ForEach(table.rows) { TableRow($0) }
            }
        } else {
            Table(of: CandidateRow.self, selection: $selected, sortOrder: $sortOrder) {
                check
                path
                rule
                activity
                size
            } rows: {
                ForEach(table.rows) { TableRow($0) }
            }
        }
    }

    private var check: some TableColumnContent<CandidateRow, Never> {
        TableColumn("") { (row: CandidateRow) in
            Toggle(isOn: Binding(get: { table.isChecked(row.id) }, set: { _ in table.toggle(row.id) })) {
                EmptyView()
            }
            .toggleStyle(.checkbox)
            .disabled(!row.selectable)
            .accessibilityLabel(strings.format("category.check", row.shownPath))
            .onAppear { Task { await table.shown(row.id) } }
        }
        .width(24)
    }

    private var path: some TableColumnContent<CandidateRow, KeyPathComparator<CandidateRow>> {
        TableColumn(strings.text("category.path"), value: \.shownPath) { row in
            Text(row.shownPath)
                .font(.system(.callout, design: .monospaced))
                .lineLimit(1).truncationMode(.middle)
                .help(row.path)
        }
        .width(min: 180, ideal: 340)
    }

    private var rule: some TableColumnContent<CandidateRow, Never> {
        TableColumn(strings.text("category.rule")) { (row: CandidateRow) in
            Text(row.managed ? row.rule + " \u{00B7} " + strings.text("category.managed") : row.rule)
                .lineLimit(1)
        }
        .width(min: 120, ideal: 170)
    }

    private var branch: some TableColumnContent<CandidateRow, Never> {
        TableColumn(strings.text("category.branch")) { (row: CandidateRow) in
            Text(row.branch ?? "\u{2014}").lineLimit(1)
        }
        .width(min: 60, ideal: 110)
    }

    private var state: some TableColumnContent<CandidateRow, Never> {
        TableColumn(strings.text("category.state")) { (row: CandidateRow) in
            Text(row.state ?? "").lineLimit(1)
                .foregroundStyle(row.selectable ? Color.primary : Color.secondary)
                .help(row.state ?? "")
        }
        .width(min: 80, ideal: 160)
    }

    private var activity: some TableColumnContent<CandidateRow, KeyPathComparator<CandidateRow>> {
        TableColumn(strings.text("category.activity"), value: \.activityOrder) { row in
            Text(row.activity.map { Format.relative($0, now: model.now, strings) } ?? "\u{2014}")
                .foregroundStyle(row.recent ? Color.orange : Color.secondary)
                .fontWeight(row.recent ? .medium : .regular)
        }
        .width(min: 80, ideal: 110)
    }

    private var size: some TableColumnContent<CandidateRow, KeyPathComparator<CandidateRow>> {
        TableColumn(strings.text("category.size"), value: \.bytes) { row in
            Text(Format.bytes(row.bytes, strings))
                .monospacedDigit()
                .frame(maxWidth: .infinity, alignment: .trailing)
        }
        .width(min: 70, ideal: 84)
    }

    @ViewBuilder
    private func menu(for ids: Set<DirId>) -> some View {
        Button(strings.text("menu.reveal")) { reveal(ids) }
            .keyboardShortcut(.return, modifiers: .command)
        Button(strings.text("menu.showInFolders")) {
            Task { await model.show(.folders) }
        }
        Button(strings.text("menu.exclude")) { exclude(ids) }
            .keyboardShortcut("e", modifiers: .command)
    }

    private func reveal(_ ids: Set<DirId>) {
        Finder.reveal(table.rows.filter { ids.contains($0.id) }.map(\.path))
    }

    private func exclude(_ ids: Set<DirId>) {
        Task {
            for id in ids {
                await table.exclude(id)
            }
        }
    }

    /// The table's sort order, as the engine sorts.
    private func sort(_ order: [KeyPathComparator<CandidateRow>]) async {
        guard let first = order.first else { return }
        let descending = first.order == .reverse
        let key: SortKey = switch first.keyPath {
        case \CandidateRow.shownPath: .path
        case \CandidateRow.activityOrder: .activity
        default: .size
        }
        await table.sort(by: key, descending: descending)
    }
}

extension CandidateRow {
    /// For sorting by activity: unknown activity sorts as oldest.
    var activityOrder: Double { activity?.timeIntervalSince1970 ?? 0 }
}
