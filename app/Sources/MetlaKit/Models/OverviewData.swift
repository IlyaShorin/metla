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
    }

    public struct Disk: Equatable, Sendable {
        public let name: String
        public let used: UInt64
        public let total: UInt64
        public let free: UInt64
        /// Under a tenth of the disk is free.
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

    /// Worked out from one snapshot, the largest folders and the volume's
    /// name; `home` shortens paths to `~/…`.
    public static func make(
        _ overview: Overview, largest: [TopFolder], volume: String, home: String, _ names: Names
    ) -> OverviewData {
        let scanning = if case .scanning = overview.freshness.state { true } else { false }
        let categories = categoryParts(overview, names)
        return OverviewData(
            banner: banner(overview, scanning: scanning),
            disk: disk(overview, volume: volume, categories: categories, names.strings),
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

    static func categoryParts(_ overview: Overview, _ names: Names) -> [Part] {
        let largest = overview.categories.map(\.tally.bytes).max() ?? 0
        return overview.categories.map { tally in
            Part(
                id: Names.key(tally.category), name: names.category(tally.category),
                bytes: tally.tally.bytes, share: Format.share(tally.tally.bytes, of: largest),
                note: names.disposal(tally.disposal), category: tally.category)
        }
    }

    static func disk(_ overview: Overview, volume: String, categories: [Part], _ strings: Strings) -> Disk? {
        guard let space = overview.volume, space.total > 0 else {
            return nil
        }
        let used = space.total - min(space.available, space.total)
        var segments = categories.prefix(3).map { $0 }
        let rest = categories.dropFirst(3).reduce(0) { $0 + $1.bytes }
        segments.append(Part(id: "rest", name: strings.text("overview.restCategories"), bytes: rest, share: 0))
        let cleanable = segments.reduce(0) { $0 + $1.bytes }
        segments.append(Part(id: "other", name: strings.text("overview.other"), bytes: used - min(cleanable, used), share: 0))
        segments.append(Part(id: "free", name: strings.text("overview.free"), bytes: space.available, share: 0))
        let shared = segments.map { part in
            Part(id: part.id, name: part.name, bytes: part.bytes, share: Format.share(part.bytes, of: space.total))
        }
        return Disk(
            name: volume, used: used, total: space.total, free: space.available,
            low: space.available * 10 < space.total, segments: shared)
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
