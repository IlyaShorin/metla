import MetlaCoreFFI
import MetlaKit
import SwiftUI

/// The disk as an outline sorted by size, loaded a level at a time down
/// to files: name with badges, share of the parent, items, size.
struct FoldersView: View {
    let model: AppModel
    @Bindable var folders: FoldersModel

    private var strings: Strings { model.strings }

    var body: some View {
        VStack(spacing: 0) {
            Table(of: FolderNode.self, selection: $folders.selection) {
                TableColumn(strings.text("folders.name")) { node in
                    NameCell(node: node, strings: strings)
                }
                .width(min: 220, ideal: 340)
                TableColumn(strings.text("folders.share")) { node in
                    ShareBar(share: node.share, tint: node.badge.isRule ? .orange : .secondary)
                }
                .width(min: 100, ideal: 180)
                TableColumn(strings.text("folders.items")) { node in
                    Text(node.items.map { Format.count($0, strings) } ?? "")
                        .foregroundStyle(.secondary).monospacedDigit()
                        .frame(maxWidth: .infinity, alignment: .trailing)
                }
                .width(min: 60, ideal: 90)
                TableColumn(strings.text("folders.size")) { node in
                    Text(node.bytes > 0 || node.isDirectory ? Format.bytes(node.bytes, strings) : "")
                        .monospacedDigit()
                        .frame(maxWidth: .infinity, alignment: .trailing)
                }
                .width(min: 70, ideal: 90)
            } rows: {
                FolderRows(nodes: folders.top, folders: folders)
            }
            .contextMenu(forSelectionType: FolderNode.ID.self) { ids in
                Button(strings.text("menu.reveal")) { reveal(ids) }
                    .keyboardShortcut(.return, modifiers: .command)
                Button(strings.text("menu.exclude")) { exclude(ids) }
                    .keyboardShortcut("e", modifiers: .command)
            }
            .onKeyPress(.return) {
                reveal(Set([folders.selection].compactMap { $0 }))
                return .handled
            }
            .onKeyPress(keys: ["e"]) { press in
                guard press.modifiers.contains(.command) else { return .ignored }
                exclude(Set([folders.selection].compactMap { $0 }))
                return .handled
            }
            Divider()
            footer
        }
    }

    private var footer: some View {
        HStack(spacing: 16) {
            Text(selectionText).lineLimit(1).truncationMode(.middle)
            Spacer()
            Text(strings.text("folders.hint"))
        }
        .font(.caption).foregroundStyle(.secondary)
        .padding(.horizontal, 16).padding(.vertical, 8)
    }

    private var selectionText: String {
        guard let node = folders.node(folders.selection) else {
            return strings.text("folders.nothingSelected")
        }
        let path = Format.path(node.path, home: model.settings.home)
        return strings.format("folders.selected", path, Format.bytes(node.bytes, strings))
    }

    private func reveal(_ ids: Set<FolderNode.ID>) {
        Finder.reveal(ids.compactMap { folders.node($0)?.path })
    }

    private func exclude(_ ids: Set<FolderNode.ID>) {
        let paths = ids.compactMap { folders.node($0) }.filter(\.isDirectory).map(\.path)
        Task {
            for path in paths {
                await model.exclude(path)
            }
        }
    }
}

/// The rows of `nodes`, a directory's own rows below it when expanded.
struct FolderRows: TableRowContent {
    let nodes: [FolderNode]
    let folders: FoldersModel

    var tableRowBody: some TableRowContent<FolderNode> {
        ForEach(nodes) { node in
            if node.isDirectory {
                DisclosureTableRow(node, isExpanded: expansion(node)) {
                    FolderRows(nodes: node.children ?? [], folders: folders)
                }
            } else {
                TableRow(node)
            }
        }
    }

    private func expansion(_ node: FolderNode) -> Binding<Bool> {
        Binding(
            get: { node.isExpanded },
            set: { open in
                if open {
                    Task { await folders.expand(node) }
                } else {
                    folders.collapse(node)
                }
            })
    }
}

struct NameCell: View {
    let node: FolderNode
    let strings: Strings

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: icon).foregroundStyle(.secondary).accessibilityHidden(true)
            Text(node.name).lineLimit(1).truncationMode(.middle)
                .foregroundStyle(node.isDirectory ? Color.primary : Color.secondary)
            if let badge = node.badge {
                // A badge keeps its width: the name shrinks first.
                Text(text(badge))
                    .font(.caption.weight(.semibold))
                    .lineLimit(1).fixedSize()
                    .padding(.horizontal, 6).padding(.vertical, 1)
                    .background(Capsule().fill(color(badge).opacity(0.18)))
                    .foregroundStyle(color(badge))
                    .layoutPriority(1)
            }
        }
        .accessibilityElement(children: .combine)
    }

    private var icon: String {
        switch node.kind {
        case .directory: "folder"
        case .file: "doc"
        case .more: "ellipsis"
        }
    }

    private func text(_ badge: FolderNode.Badge) -> String {
        switch badge {
        case .rule(let title): title
        case .noAccess: strings.text("folders.noAccess")
        case .skipped: strings.text("folders.skipped")
        }
    }

    private func color(_ badge: FolderNode.Badge) -> Color {
        switch badge {
        case .rule: .orange
        case .noAccess: .red
        case .skipped: .gray
        }
    }
}

extension Optional where Wrapped == FolderNode.Badge {
    var isRule: Bool {
        if case .rule? = self { true } else { false }
    }
}
