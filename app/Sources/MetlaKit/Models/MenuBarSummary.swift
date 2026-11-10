import Foundation
import MetlaCoreFFI

/// What the menu-bar panel shows, from the cached snapshot only: it never
/// waits for the engine.
public struct MenuBarSummary: Equatable, Sendable {
    public struct Row: Equatable, Sendable, Identifiable {
        public var id: String { name }
        public let name: String
        public let size: String
    }

    public let volume: String
    public let free: String
    /// Used share of the volume, 0 to 1.
    public let used: Double
    public let usedLine: String
    /// "Can be cleaned ≈ 105 GB": deleted for good.
    public let canBeFreed: String
    /// What would only move to the Trash, when there is any.
    public let toTrash: String?
    /// The quick-eligible categories with what cleaning them would free.
    public let quick: [Row]
    /// Logs, worktrees, temporary data: only with a confirmation.
    public let rest: Row
    /// "Clean…" or "Clean now", by the hotkey mode.
    public let cleanTitle: String
    /// The Clean hotkey ("⌃⌥⌘⌫"), when one is bound.
    public let hotkey: String?
    /// Freshness, partial, power profile.
    public let status: String

    public static func make(
        _ overview: Overview, volume: String, mode: HotkeyMode, hotkey: KeyCombo?, status: StatusLine?,
        _ names: Names
    ) -> MenuBarSummary {
        let strings = names.strings
        let space = overview.volume ?? VolumeSpace(total: 0, available: 0)
        let usedBytes = space.total - min(space.available, space.total)
        let byCategory = Dictionary(uniqueKeysWithValues: overview.categories.map { ($0.category, $0) })
        let quick = QuickCategory.all.map { quick in
            let freed = byCategory[quick.category]?.reclaimable.freed.bytes ?? 0
            return Row(name: names.category(quick.category), size: Format.bytes(freed, strings))
        }
        let others: [Category] = [.log, .gitWorktree, .temp]
        let rest = others.reduce(0) { $0 + (byCategory[$1]?.tally.bytes ?? 0) }
        let trashed = overview.reclaimable.trashed.bytes
        return MenuBarSummary(
            volume: volume,
            free: strings.format("menubar.free", Format.bytes(space.available, strings)),
            used: Format.share(usedBytes, of: space.total),
            usedLine: strings.format(
                "overview.used", Format.bytes(usedBytes, strings), Format.bytes(space.total, strings)),
            canBeFreed: strings.format("menubar.canBeFreed", Format.bytes(overview.reclaimable.freed.bytes, strings)),
            toTrash: trashed == 0 ? nil : strings.format("menubar.toTrash", Format.bytes(trashed, strings)),
            quick: quick,
            rest: Row(name: strings.text("menubar.rest"), size: Format.bytes(rest, strings)),
            cleanTitle: mode == .immediate ? strings.text("menubar.cleanNow") : strings.text("toolbar.clean"),
            hotkey: hotkey?.display,
            status: statusLine(overview, status: status, names))
    }

    static func statusLine(_ overview: Overview, status: StatusLine?, _ names: Names) -> String {
        let strings = names.strings
        let parts = [status?.text, profile(overview.profile, strings)].compactMap { $0 }
        return parts.joined(separator: " · ")
    }

    /// The power profile in force, in words.
    public static func profile(_ profile: PowerProfile, _ strings: Strings) -> String {
        switch profile {
        case .interactive: strings.text("power.interactive")
        case .background: strings.text("power.background")
        case .saver: strings.text("power.saver")
        case .off: strings.text("power.off")
        }
    }
}
