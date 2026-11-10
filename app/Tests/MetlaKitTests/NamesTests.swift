import Foundation
import MetlaCoreFFI
import Testing
@testable import MetlaKit

@Suite struct NamesTests {
    let titles = [RuleTitle(rule: "build.cargo-target", title: "Cargo build output"),
                  RuleTitle(rule: "app-cache.new-one", title: "A new cache")]

    @Test func categoriesDisposalsAndKindsHaveNames() {
        let names = Names(Strings(language: "en", region: "US"), titles: titles)
        let categories: [MetlaKit.Category] = [.appCache, .devCache, .buildArtifact, .log, .gitWorktree, .temp]
        #expect(categories.map(names.category) == ["App caches", "Developer caches", "Build artifacts", "Logs", "Git worktrees", "Temporary data"])
        #expect([Disposal.deletePermanently, .moveToTrash, .gitWorktreeRemove].map(names.disposal) == ["permanently", "to Trash", "git worktree remove"])
        let kinds: [FileKind] = [.applications, .developer, .media, .documents, .archives, .system, .other]
        #expect(kinds.map(names.kind) == ["Applications", "Developer", "Media", "Documents", "Archives and disk images", "System", "Other"])
    }

    @Test func ruleTitlesAreLocalisedByIdWithTheEnglishTitleAsFallback() {
        let russian = Names(Strings(language: "ru", region: "RU"), titles: titles)
        #expect(russian.rule("build.cargo-target") == "Сборка Cargo")
        #expect(russian.rule("app-cache.new-one") == "A new cache", "no translation: the catalogue's title")
        #expect(russian.rule("log.unknown") == "log.unknown", "no title at all: the id")
        let english = Names(Strings(language: "en", region: "US"), titles: titles)
        #expect(english.rule("build.cargo-target") == "Cargo build output")
    }

    @Test func worktreesSayWhyTheyCannotBeRemoved() {
        let names = Names(Strings(language: "en", region: "US"), titles: [])
        let reasons: [Reason] = [.uncommitted, .untracked, .unpushed, .locked, .prunable, .operationInProgress,
                                 .inUse, .managed(.codex), .managed(.claude), .notListed, .inspectionFailed("x")]
        #expect(reasons.map(names.reason) == ["Uncommitted", "Untracked", "Unpushed", "Locked", "Prunable",
                                              "Operation in progress", "In use", "Managed by Codex",
                                              "Managed by Claude", "Not listed by Git", "Git could not tell"])
        #expect(names.removability(.unknown) == "Checking…")
        #expect(names.removability(.removable(branch: "main")) == "Clean")
        #expect(names.removability(.blocked([.uncommitted, .locked])) == "Uncommitted, Locked")
    }
}
