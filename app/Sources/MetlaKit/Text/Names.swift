import Foundation
import MetlaCoreFFI

/// The names the app gives the engine's values, in one language.
public struct Names: Sendable {
    public let strings: Strings
    let titles: [String: String]

    /// `titles`: every rule's English title by id (`ruleTitles()`).
    public init(_ strings: Strings, titles: [RuleTitle]) {
        self.strings = strings
        self.titles = Dictionary(titles.map { ($0.rule, $0.title) }, uniquingKeysWith: { a, _ in a })
    }

    public func category(_ category: Category) -> String {
        strings.text("category." + Names.key(category))
    }

    /// How a category's candidates are removed, in a word or three.
    public func disposal(_ disposal: Disposal) -> String {
        switch disposal {
        case .deletePermanently: strings.text("disposal.permanently")
        case .moveToTrash: strings.text("disposal.trash")
        case .gitWorktreeRemove: strings.text("disposal.worktree")
        }
    }

    public func kind(_ kind: FileKind) -> String {
        switch kind {
        case .applications: strings.text("kind.applications")
        case .developer: strings.text("kind.developer")
        case .media: strings.text("kind.media")
        case .documents: strings.text("kind.documents")
        case .archives: strings.text("kind.archives")
        case .system: strings.text("kind.system")
        case .other: strings.text("kind.other")
        }
    }

    /// A rule's title: the translation by id, else the catalogue's English
    /// title, else the id.
    public func rule(_ id: String) -> String {
        let english = titles[id] ?? id
        return strings.bundle.localizedString(forKey: "rule." + id, value: english, table: nil)
    }

    /// Why a worktree cannot be removed.
    public func reason(_ reason: Reason) -> String {
        switch reason {
        case .uncommitted: strings.text("reason.uncommitted")
        case .untracked: strings.text("reason.untracked")
        case .unpushed: strings.text("reason.unpushed")
        case .locked: strings.text("reason.locked")
        case .prunable: strings.text("reason.prunable")
        case .operationInProgress: strings.text("reason.operation")
        case .inUse: strings.text("reason.inUse")
        case .managed(let manager): manager == .codex
            ? strings.text("reason.managedCodex") : strings.text("reason.managedClaude")
        case .notListed: strings.text("reason.notListed")
        case .inspectionFailed: strings.text("reason.failed")
        }
    }

    /// A worktree's state in a list: clean, the reasons it is blocked, or
    /// that it is still being checked.
    public func removability(_ removability: Removability) -> String {
        switch removability {
        case .unknown: strings.text("worktree.checking")
        case .removable: strings.text("worktree.clean")
        case .blocked(_, let reasons): reasons.map(reason).joined(separator: ", ")
        }
    }

    static func key(_ category: Category) -> String {
        switch category {
        case .appCache: "appCache"
        case .devCache: "devCache"
        case .buildArtifact: "buildArtifact"
        case .log: "log"
        case .gitWorktree: "gitWorktree"
        case .temp: "temp"
        }
    }
}
