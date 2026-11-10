import Foundation
import MetlaCoreFFI

/// What the Overview shows, worked out from the engine's answers: plain
/// values the view lays out as they are.
public struct OverviewData: Equatable, Sendable {
    /// A segment of the disk bar, a row of a list: a name, bytes, and a
    /// share from 0 to 1.
    public struct Part: Equatable, Sendable, Identifiable {
        public let id: String
        public let name: String
        public let bytes: UInt64
        public let share: Double
        /// A category's disposal ("permanently"), when it is one.
        public var note: String?
        /// The category it opens, when it is one.
        public var category: Category?
        /// Everything found, when it differs from `bytes` (what cleaning
        /// the preselected candidates would do).
        public var found: UInt64?
    }

    /// The disk card: the volume when the analysed root is its mount
    /// point, else the analysed folder on its volume.
    public struct Disk: Equatable, Sendable {
        /// The volume's name, or the analysed folder (`~/…`).
        public let title: String
        /// The root is a folder inside a volume, not its mount point.
        public let folder: Bool
        /// The volume's name.
        public let volume: String
        /// The analysed folder's size; 0 before its first scan.
        public let analysed: UInt64
        public let used: UInt64
        public let total: UInt64
        public let free: UInt64
        /// Under a tenth of the volume is free.
        public let low: Bool
        public let segments: [Part]
    }

    /// The partial-analysis banner, or "Scanning…" while a scan runs.
    public enum Banner: Equatable, Sendable {
        case scanning
        case partial(denied: UInt64, missing: UInt64)
    }

    public let banner: Banner?
    public let disk: Disk?
    /// "Can be freed ≈": deleted for good; what would only move to the
    /// Trash is apart.
    public let freed: UInt64
    public let trashed: UInt64
    public let categories: [Part]
    public let largest: [Part]
    public let kinds: [Part]
    /// Totals still growing: a scan runs.
    public let growing: Bool

    /// Worked out from one snapshot, the largest folders and the volume;
    /// `home` shortens paths to `~/…`.
    public static func make(
        _ overview: Overview, largest: [TopFolder], volume: VolumeInfo, home: String, _ names: Names
    ) -> OverviewData {
        let scanning = if case .scanning = overview.freshness.state { true } else { false }
        let categories = categoryParts(overview, names)
        let owned = overview.categories.map(\.owned)
        return OverviewData(
            banner: banner(overview, scanning: scanning),
            disk: disk(overview, volume: volume, home: home, owned: owned, names.strings),
            freed: overview.reclaimable.freed.bytes,
            trashed: overview.reclaimable.trashed.bytes,
            categories: categories,
            largest: folderParts(largest, home: home),
            kinds: kindParts(overview, names),
            growing: scanning)
    }

    static func banner(_ overview: Overview, scanning: Bool) -> Banner? {
        if scanning {
            return .scanning
        }
        guard overview.freshness.picture == .partial, let coverage = overview.coverage else {
            return nil
        }
        let denied = coverage.gaps.first { $0.kind == .denied }?.count ?? 0
        let all = coverage.gaps.reduce(0) { $0 + $1.count }
        return .partial(denied: denied, missing: all - denied)
    }

    /// One row per category: what cleaning its preselected candidates
    /// would do (the rows add up to "can be freed" and "to the Trash"),
    /// how, and everything found where that differs.
    static func categoryParts(_ overview: Overview, _ names: Names) -> [Part] {
        let amounts = overview.categories.map { $0.reclaimable.freed.bytes + $0.reclaimable.trashed.bytes }
        let largest = amounts.max() ?? 0
        return zip(overview.categories, amounts).map { tally, bytes in
            Part(
                id: Names.key(tally.category), name: names.category(tally.category),
                bytes: bytes, share: Format.share(bytes, of: largest),
                note: how(tally, names), category: tally.category,
                found: tally.tally.bytes == bytes ? nil : tally.tally.bytes)
        }
    }

    /// How a category's preselected candidates go: by what they are
    /// (the Trash's contents are deleted for good), else as the category
    /// goes.
    static func how(_ tally: CategoryTally, _ names: Names) -> String {
        let freed = tally.reclaimable.freed.bytes > 0
        let trashed = tally.reclaimable.trashed.bytes > 0
        let permanently = names.disposal(.deletePermanently)
        let trash = names.disposal(.moveToTrash)
        switch (freed, trashed) {
        case (true, true): return permanently + " + " + trash
        case (true, false): return permanently
        case (false, true): return trash
        case (false, false): return names.disposal(tally.disposal)
        }
    }

    /// The disk card; `owned` is each category's share of the bar, in
    /// which a candidate inside another counts with the outer one, so the
    /// segments add up to the analysed size.
    static func disk(
        _ overview: Overview, volume: VolumeInfo, home: String, owned: [UInt64], _ strings: Strings
    ) -> Disk? {
        guard let space = overview.volume, space.total > 0 else {
            return nil
        }
        let used = space.total - min(space.available, space.total)
        let cleanable = owned.reduce(0, +)
        let analysed = overview.totals?.bytes ?? 0
        var parts = zip(["category.appCache", "category.devCache", "category.buildArtifact"], owned)
            .map { (strings.text($0.0), $0.1) }
        parts.append((strings.text("overview.restCategories"), owned.dropFirst(3).reduce(0, +)))
        if volume.isRoot {
            parts.append((strings.text("overview.other"), used - min(cleanable, used)))
        } else {
            parts.append((strings.text("overview.otherInFolder"), analysed - min(cleanable, analysed)))
            parts.append((strings.text("overview.outside"), used - min(analysed, used)))
        }
        parts.append((strings.text("overview.free"), space.available))
        let segments = parts.enumerated().map { index, part in
            Part(id: String(index), name: part.0, bytes: part.1, share: Format.share(part.1, of: space.total))
        }
        return Disk(
            title: volume.isRoot ? volume.name : Format.path(overview.root, home: home),
            folder: !volume.isRoot, volume: volume.name, analysed: analysed, used: used,
            total: space.total, free: space.available, low: space.available * 10 < space.total,
            segments: segments)
    }

    static func folderParts(_ folders: [TopFolder], home: String) -> [Part] {
        let largest = folders.map(\.bytes).max() ?? 0
        return folders.map { folder in
            Part(
                id: folder.path, name: Format.path(folder.path, home: home), bytes: folder.bytes,
                share: Format.share(folder.bytes, of: largest))
        }
    }

    static func kindParts(_ overview: Overview, _ names: Names) -> [Part] {
        let total = overview.kinds.reduce(0) { $0 + $1.bytes }
        return overview.kinds.map { kind in
            Part(
                id: names.kind(kind.kind), name: names.kind(kind.kind), bytes: kind.bytes,
                share: Format.share(kind.bytes, of: total))
        }
    }
}

/// The volume the analysed root is on: its name, and whether the root is
/// its mount point (a whole volume) or a folder inside it.
public struct VolumeInfo: Equatable, Sendable {
    public let name: String
    public let isRoot: Bool

    public init(name: String, isRoot: Bool) {
        self.name = name
        self.isRoot = isRoot
    }
}
