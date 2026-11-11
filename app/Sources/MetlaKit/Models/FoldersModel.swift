import Foundation
import MetlaCoreFFI
import Observation

/// One row of the Folders outline: a directory of the picture, a file
/// listed now, or the files a listing did not keep.
@MainActor
@Observable
public final class FolderNode: Identifiable {
    public enum Kind: Equatable, Sendable {
        case directory(DirId)
        case file
        /// "N more files", or entries past the page.
        case more(UInt64)
    }

    /// What a directory carries next to its name.
    public enum Badge: Equatable, Sendable {
        /// The rule it matched.
        case rule(String)
        case noAccess
        case skipped
    }

    public nonisolated let id: String
    public let name: String
    public let path: String
    public let kind: Kind
    public let bytes: UInt64
    /// Files and directories below a directory.
    public let items: UInt64?
    /// Its share of its parent's size.
    public let share: Double
    public let badge: Badge?
    /// `nil` until the directory is expanded the first time.
    public internal(set) var children: [FolderNode]?
    public internal(set) var isExpanded = false

    init(id: String, name: String, path: String, kind: Kind, bytes: UInt64, items: UInt64?, share: Double, badge: Badge?) {
        self.id = id
        self.name = name
        self.path = path
        self.kind = kind
        self.bytes = bytes
        self.items = items
        self.share = share
        self.badge = badge
    }

    public var isDirectory: Bool {
        if case .directory = kind { true } else { false }
    }

    /// Rows below it as shown: its children when expanded.
    public var shownChildren: [FolderNode]? {
        isExpanded ? children : (isDirectory ? [] : nil)
    }
}

/// The Folders outline: the picture's directories, loaded a level at a
/// time as they are expanded, down to files listed live by the engine.
@MainActor
@Observable
public final class FoldersModel {
    /// Entries asked for per directory; the rest are one "more" row.
    public static let pageSize: UInt32 = 500

    public private(set) var top: [FolderNode] = []
    public private(set) var rootPath: String
    public var selection: FolderNode.ID?
    public private(set) var failure: String?

    private let client: EngineClient
    private let names: Names
    private var nodes: [String: FolderNode] = [:]
    private var reloading: Task<Void, Never>?

    public init(root: String, client: EngineClient, names: Names) {
        self.rootPath = root
        self.client = client
        self.names = names
    }

    /// The root's entries, again; directories that were expanded are
    /// expanded again (a rescan may have given them new ids). Reloads run
    /// one after another: the app reloads when the picture changes while
    /// the user may reload too, and two interleaved reloads would each
    /// see the other's half-built tree and lose what was expanded.
    public func reload() async {
        let previous = reloading
        let next = Task { [weak self] in
            await previous?.value
            await self?.reloadNow()
        }
        reloading = next
        await next.value
    }

    /// Builds the new tree aside while the shown one stays whole, loads
    /// in it whatever is expanded in the shown tree (an expansion made
    /// meanwhile included), and swaps both in one step, with no wait
    /// between the last look at what is expanded and the swap: the new
    /// tree is expanded exactly where the shown one is at that moment.
    private func reloadNow() async {
        var fresh: [String: FolderNode] = [:]
        let made = await entries(of: .path(rootPath), parent: rootPath) ?? []
        made.forEach { fresh[$0.id] = $0 }
        var loaded: Set<String> = []
        while let path = expandedPaths().first(where: { !loaded.contains($0) }) {
            loaded.insert(path)
            guard let node = fresh[path], case .directory(let id) = node.kind, node.children == nil else {
                continue
            }
            let children = await entries(of: .id(id), parent: node.path) ?? []
            node.children = children
            children.forEach { fresh[$0.id] = $0 }
        }
        let expanded = Set(expandedPaths())
        for node in fresh.values {
            node.isExpanded = node.children != nil && expanded.contains(node.path)
        }
        nodes = fresh
        top = made
    }

    /// The paths expanded in the tree shown, shallowest first.
    private func expandedPaths() -> [String] {
        nodes.values.filter(\.isExpanded).map(\.path).sorted()
    }

    public func node(_ id: FolderNode.ID?) -> FolderNode? {
        id.flatMap { nodes[$0] }
    }

    /// → on a directory: its entries, loaded the first time. A node from
    /// before a reload stands for the one of the same path shown now:
    /// that one is expanded, and the caller's node gets the same children.
    public func expand(_ node: FolderNode) async {
        var shown = nodes[node.id] ?? node
        while shown.children == nil {
            guard case .directory(let id) = shown.kind else {
                return
            }
            let children = await entries(of: .id(id), parent: shown.path) ?? []
            // A reload may have swapped the tree meanwhile: load again for
            // the node shown now, whose directory id may be new.
            guard shown === (nodes[node.id] ?? node) else {
                shown = nodes[node.id] ?? node
                continue
            }
            shown.children = children
            children.forEach { nodes[$0.id] = $0 }
        }
        guard shown.isDirectory else {
            return
        }
        shown.isExpanded = true
        node.children = shown.children
        node.isExpanded = true
    }

    /// ←: folds the directory (the one shown now of the same path too).
    public func collapse(_ node: FolderNode) {
        nodes[node.id]?.isExpanded = false
        node.isExpanded = false
    }

    /// Opens the outline down to `path` and selects it ("Show in
    /// Folders"); false when the picture has no such directory.
    @discardableResult
    public func reveal(_ path: String) async -> Bool {
        if top.isEmpty {
            await reload()
        }
        var level = top
        var found: FolderNode?
        for name in path.dropFirst(rootPath.count).split(separator: "/").map(String.init) {
            guard let node = level.first(where: { $0.name == name && $0.isDirectory }) else {
                return false
            }
            await expand(node)
            found = node
            level = node.children ?? []
        }
        selection = found?.id
        return found != nil
    }

    /// The selected path from the root down, for the breadcrumb.
    public var breadcrumb: [String] {
        guard let node = node(selection) else {
            return []
        }
        let below = node.path.dropFirst(rootPath.count).split(separator: "/").map(String.init)
        return [rootPath] + below
    }

    private func entries(of dir: DirRef, parent: String) async -> [FolderNode]? {
        do {
            let page = try await client.children(dir, offset: 0, limit: Self.pageSize)
            failure = nil
            var made = page.items.map { make($0, in: page) }
            let rest = page.total - UInt64(page.items.count)
            if rest > 0 {
                made.append(more(rest, files: 0, bytes: 0, in: page.path))
            }
            return made
        } catch {
            failure = String(describing: error)
            return nil
        }
    }

    private func make(_ item: FolderItem, in page: FolderPage) -> FolderNode {
        let total = page.dir.totals.bytes
        switch item {
        case .dir(let dir):
            let path = (page.path as NSString).appendingPathComponent(dir.name)
            return FolderNode(
                id: path, name: dir.name, path: path, kind: .directory(dir.id),
                bytes: dir.totals.bytes, items: dir.totals.files + max(dir.totals.dirs, 1) - 1,
                share: Format.share(dir.totals.bytes, of: total), badge: badge(dir))
        case .file(let name, let bytes):
            let path = (page.path as NSString).appendingPathComponent(name)
            return FolderNode(
                id: path, name: name, path: path, kind: .file, bytes: bytes, items: nil,
                share: Format.share(bytes, of: total), badge: nil)
        case .moreFiles(let files, let bytes):
            return more(files, files: files, bytes: bytes, in: page.path)
        }
    }

    private func more(_ count: UInt64, files: UInt64, bytes: UInt64, in path: String) -> FolderNode {
        let text = names.strings.format("folders.more", Format.count(count, names.strings))
        return FolderNode(
            id: path + "/\u{2026}" + String(files), name: text, path: path, kind: .more(count),
            bytes: bytes, items: nil, share: 0, badge: nil)
    }

    private func badge(_ dir: FolderDir) -> FolderNode.Badge? {
        if let rule = dir.rule {
            return .rule(names.rule(rule))
        }
        if dir.gap != nil {
            return .noAccess
        }
        return dir.skip.map { _ in .skipped }
    }
}
