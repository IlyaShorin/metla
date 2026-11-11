import Foundation
import MetlaCoreFFI
import Testing
@testable import MetlaKit

/// The toolbar's status for every state the engine can be in.
@Suite struct StatusLineTests {
    let english = Strings(language: "en", region: "US")
    let now = Date(timeIntervalSince1970: 1_000_000)

    func freshness(
        _ state: EngineState, picture: Completeness = .complete, needsFullScan: Bool = false,
        watching: Bool = true
    ) -> Freshness {
        Freshness(
            fullScan: 999_000, updated: 999_880, pending: 0, state: state, picture: picture,
            watching: watching, needsFullScan: needsFullScan, fullScansEnded: 1, scanReason: nil)
    }

    func line(_ freshness: Freshness, expected: UInt64? = nil) -> StatusLine {
        StatusLine.of(freshness, expected: expected, now: now, english)
    }

    @Test func aScanSaysHowFarItGotAndCanBeStopped() {
        let progress = ProgressSnapshot(dirs: 10, files: 1_900_000, bytes: 610, denied: 0)
        let scanning = freshness(.scanning(progress: progress, interactive: true), picture: .missing)
        let known = line(scanning, expected: 1000)
        #expect(known == StatusLine(text: "Scanning 61% · 1.9M files", tone: .working, canStop: true))
        #expect(line(scanning).text == "Scanning · 1.9M files", "no share without a size to head for")
        #expect(line(scanning, expected: 0).text == "Scanning · 1.9M files")
        let past = ProgressSnapshot(dirs: 10, files: 1, bytes: 5000, denied: 0)
        #expect(line(freshness(.scanning(progress: past, interactive: false), picture: .missing), expected: 1000).text
            == "Scanning 99% · 1 file", "never 100% while it runs")
    }

    @Test func aScanWithAPictureToShowSaysItRefreshes() {
        let progress = ProgressSnapshot(dirs: 10, files: 1_900_000, bytes: 610, denied: 0)
        for picture in [Completeness.complete, .partial] {
            let refreshing = freshness(.scanning(progress: progress, interactive: false), picture: picture)
            #expect(line(refreshing, expected: 1000)
                == StatusLine(text: "Refreshing 61% · 1.9M files", tone: .working, canStop: true))
            #expect(line(refreshing).text == "Refreshing · 1.9M files")
        }
    }

    @Test func workBetweenScansReads() {
        #expect(line(freshness(.updating(pending: 3))) == StatusLine(text: "Updating…", tone: .working, canStop: false))
        #expect(line(freshness(.cleaning)).text == "Cleaning…")
        #expect(line(freshness(.paused(.saver))) == StatusLine(text: "Full scan waits for power", tone: .stale, canStop: false))
        #expect(line(freshness(.off)) == StatusLine(text: "Background analysis off", tone: .off, canStop: false))
    }

    @Test func failuresSayWhy() {
        #expect(line(freshness(.failed(.noSpace))) == StatusLine(text: "Not enough disk space to refresh", tone: .problem, canStop: false))
        #expect(line(freshness(.failed(.index))).text == "The index cannot be written")
        #expect(line(freshness(.failed(.scan))).text == "The disk could not be scanned")
    }

    @Test func anIdlePictureSaysHowOldAndWhetherItIsWhole() {
        #expect(line(freshness(.idle)) == StatusLine(text: "Updated 2 min. ago", tone: .current, canStop: false))
        #expect(line(freshness(.idle, picture: .partial)) == StatusLine(text: "Updated 2 min. ago · partial", tone: .stale, canStop: false))
        #expect(line(freshness(.idle, needsFullScan: true)).text == "Updated 2 min. ago · needs a full scan")
        #expect(line(freshness(.idle, needsFullScan: true)).tone == .stale)
        #expect(line(freshness(.idle, watching: false)).tone == .stale, "not followed: not current")
        #expect(line(freshness(.idle, picture: .missing)) == StatusLine(text: "Never scanned", tone: .stale, canStop: false))
        let never = Freshness(fullScan: nil, updated: nil, pending: 0, state: .idle, picture: .complete, watching: true, needsFullScan: false, fullScansEnded: 1, scanReason: nil)
        #expect(line(never).text == "Never scanned")
    }

    @Test func russianReadsAsWell() {
        let russian = Strings(language: "ru", region: "RU")
        let progress = ProgressSnapshot(dirs: 10, files: 1_900_000, bytes: 610, denied: 0)
        let text = StatusLine.of(
            freshness(.scanning(progress: progress, interactive: true), picture: .missing),
            expected: 1000, now: now, russian).text
        #expect(text == "Сканирование: 61% · 1,9\u{00A0}млн файлов")
        let again = StatusLine.of(freshness(.scanning(progress: progress, interactive: false)), expected: nil, now: now, russian).text
        #expect(again == "Обновление · 1,9\u{00A0}млн файлов")
        let partial = StatusLine.of(freshness(.idle, picture: .partial), expected: nil, now: now, russian).text
        #expect(partial == "Обновлено 2 мин. назад · анализ неполный")
    }
}
