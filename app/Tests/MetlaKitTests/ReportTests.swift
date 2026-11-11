import Foundation
import MetlaCoreFFI
import Testing
@testable import MetlaKit

/// What a report says about the measured free space and about each item,
/// from a report as the engine hands it over.
@Suite struct ReportTests {
    static let english = Names(Strings(language: "en", region: "US"), titles: [])
    static let russian = Names(Strings(language: "ru", region: "RU"), titles: [])

    static func item(_ id: DirId, _ outcome: Outcome) -> ItemReport {
        ItemReport(candidate: id, path: "/fixture/home/item\(id)", rule: "app-cache.app", found: 100, removed: 0,
                   outcome: outcome)
    }

    static func report(_ items: [ItemReport] = [], freed: Int64, names: Names = english) -> ReportData {
        let report = CleanupReport(preview: 1, items: items, freeBefore: 1_000_000, freeAfter: 1_000_000)
        let totals = ReportTotals(found: 100, removed: 50, movedToTrash: 0, freed: freed)
        return ReportData.make(.done(report: report, totals: totals), names: names, home: "/fixture/home")
    }

    @Test func freeSpaceThatGrewIsShownAsTheAmountFreed() {
        let report = Self.report(freed: 2_000_000)
        let strings = Self.english.strings
        #expect(report.freedOnDisk == .grew(2_000_000))
        #expect(report.freedFigure(strings) == Format.bytes(2_000_000, strings))
        #expect(report.freedNote(strings) == strings.text("report.freedNote"))
    }

    @Test func freeSpaceThatDidNotGrowIsSaidInWordsNeverAsANegativeAmount() {
        for freed: Int64 in [0, -459_000] {
            let report = Self.report(freed: freed, names: Self.russian)
            let strings = Self.russian.strings
            #expect(report.freedOnDisk == .didNotGrow)
            #expect(report.freedFigure(strings) == "\u{2014}")
            #expect(report.freedNote(strings).hasPrefix("Свободное место на томе не выросло"))
            #expect(!report.freedFigure(strings).contains("\u{2212}") && !report.freedFigure(strings).contains("-"))
        }
    }

    @MainActor @Test func theHotkeysPanelSaysSoToo() {
        let strings = Self.english.strings
        let grew = AppModel.hud(Self.report(freed: 2_000_000), strings)
        #expect(grew.lines.contains("Freed " + Format.bytes(2_000_000, strings)))
        let none = AppModel.hud(Self.report(freed: -459_000), strings)
        #expect(none.lines.contains("Free space did not grow"))
        #expect(!none.lines.contains { $0.hasPrefix("Freed") })
    }

    @Test func notConfirmingOrCancellingIsTheUsersChoiceNotAProblem() {
        let items = [
            Self.item(1, .removed), Self.item(2, .skipped(.notConfirmed)), Self.item(3, .skipped(.cancelled)),
            Self.item(4, .skipped(.replaced)), Self.item(5, .failed(.unlisted)), Self.item(6, .movedToTrash),
            Self.item(7, .partial(Leftover(failures: 1, first: [], mounts: [], cancelled: false))),
            Self.item(8, .alreadyGone),
        ]
        let report = Self.report(items, freed: 1)
        #expect(report.rows.map(\.tone) == [.done, .untouched, .untouched, .problem, .problem, .done, .problem, .done])
        #expect(report.rows(problemsOnly: true).map(\.id) == [4, 5, 7])
        #expect(report.problems == 3)
    }

    @Test func aFailureIsNamedByItsKindAndWhatTheTrashOrGitSaidIsADetail() {
        let items = [
            Self.item(1, .failed(.trashRefused(refused: 1, of: 2, said: "Permission denied"))),
            Self.item(2, .failed(.gitRefused("fatal: not today"))),
            Self.item(3, .failed(.unlisted)),
        ]
        let russian = Self.report(items, freed: 1, names: Self.russian).rows
        #expect(russian.map(\.outcome) == [
            "Ошибка: 1 из 2 не перемещено в Корзину", "Ошибка: Git не удалил worktree",
            "Ошибка: не удалось прочитать содержимое",
        ])
        #expect(russian.map(\.details) == [["Permission denied"], ["fatal: not today"], []])
        let english = Self.report(items, freed: 1).rows.map(\.outcome)
        #expect(english == [
            "Failed: 1 of 2 not moved to the Trash", "Failed: Git did not remove the worktree",
            "Failed: its contents could not be listed",
        ])
    }

    @Test func whatStayedIsNamedWithItsReasonInTheReportsLanguageAndTheErrnoAsADetail() {
        let left = Leftover(
            failures: 3,
            first: [FailedPath(path: "locked/held", errno: 13), FailedPath(path: "busy/file", errno: 16),
                    FailedPath(path: "odd", errno: 5)],
            mounts: ["/fixture/home/item1/mounted"], cancelled: false)
        let english = Self.report([Self.item(1, .partial(left))], freed: 1).rows[0].details
        #expect(english == [
            "locked/held: no permission (errno 13)", "busy/file: in use (errno 16)", "odd: could not be removed (errno 5)",
            "~/item1/mounted: another volume, left as it is",
        ])
        let russian = Self.report([Self.item(1, .partial(left))], freed: 1, names: Self.russian).rows[0].details
        #expect(russian == [
            "locked/held: нет прав (errno 13)", "busy/file: занят (errno 16)", "odd: не удалось удалить (errno 5)",
            "~/item1/mounted: другой том, оставлен как есть",
        ])
    }
}
