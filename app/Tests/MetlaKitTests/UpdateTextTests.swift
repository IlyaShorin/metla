import Foundation
import Testing
@testable import MetlaKit

/// What the strip, the panel's line and Settings say in each state.
@Suite struct UpdateTextTests {
    let english = Strings(language: "en", region: "US")
    let russian = Strings(language: "ru", region: "RU")
    let copy = InstallerTests.running
    let release = FeedFixture.release

    func offer(_ phase: UpdateModel.Phase) -> UpdateModel.Offer {
        UpdateModel.Offer(release: release, phase: phase)
    }

    func blocked(_ block: InstallBlock) -> Installation {
        Installation(bundle: copy.bundle, identifier: copy.identifier, version: copy.version, build: copy.build, block: block)
    }

    @Test func anOfferHasInstallWhatsNewAndLater() {
        let shown = UpdateShown.of(offer(.offered), installation: copy, cleanupRunning: false, english)
        #expect(shown == UpdateShown(
            title: "Metla 0.8.1 is available", note: nil, progress: nil,
            actions: [.install(enabled: true), .whatsNew(release.notes!), .later]))
        #expect(UpdateShown.of(offer(.offered), installation: copy, cleanupRunning: false, russian)?.title
                == "Доступна Metla 0.8.1")
        #expect(UpdateShown.of(nil, installation: copy, cleanupRunning: false, english) == nil)
        let bare = Release(version: release.version, download: release.download, size: release.size, digest: nil,
                           notes: nil, page: UpdateSource.github.releases)
        let withoutNotes = UpdateShown.of(.init(release: bare, phase: .offered), installation: copy, cleanupRunning: false, english)
        #expect(withoutNotes?.actions == [.install(enabled: true), .later], "no What's New without a page of the release")
    }

    @Test func duringACleanupInstallIsDisabledWithANote() {
        let shown = UpdateShown.of(offer(.offered), installation: copy, cleanupRunning: true, english)
        #expect(shown?.actions.first == .install(enabled: false))
        #expect(shown?.note == "A cleanup is running: install once it has ended.")
    }

    @Test func aCopyThatCannotReplaceItselfLinksToTheReleasePageWithItsReason() {
        let reasons: [(InstallBlock, String)] = [
            (.adHoc, "This copy was built from source and cannot install it itself."),
            (.translocated, "macOS runs this copy from a temporary place: move Metla to Applications first."),
            (.readOnlyVolume, "This copy is on a read-only volume and cannot replace itself."),
            (.notWritable, "Metla cannot write to its folder and cannot replace itself."),
        ]
        for (block, reason) in reasons {
            let shown = UpdateShown.of(offer(.offered), installation: blocked(block), cleanupRunning: false, english)
            #expect(shown?.note == reason)
            #expect(shown?.actions == [.releasePage(release.page), .later])
            #expect(UpdateShown.text(block, russian) != reason, "translated")
        }
    }

    @Test func progressReadsAsBytesAndAShare() {
        let downloading = UpdateShown.of(
            offer(.downloading(received: 1_200_000, total: 2_700_000)), installation: copy, cleanupRunning: false, english)
        #expect(downloading?.title == "Downloading 1.2 MB of 2.7 MB")
        #expect(downloading?.progress == .share(1_200_000 / 2_700_000) && downloading?.actions == [])
        let russianTitle = UpdateShown.of(
            offer(.downloading(received: 1_200_000, total: 2_700_000)), installation: copy, cleanupRunning: false, russian)?.title
        #expect(russianTitle == "Загрузка: 1,2 МБ из 2,7 МБ")
        #expect(UpdateShown.of(offer(.verifying), installation: copy, cleanupRunning: false, english)
                == UpdateShown(title: "Checking the signature", note: nil, progress: .working, actions: []))
        #expect(UpdateShown.of(offer(.installing), installation: copy, cleanupRunning: false, english)?.title == "Installing")
    }

    @Test func aFailureIsOneSentenceWithTheReleasePageAndLater() {
        let sentences: [(UpdateFailure, String)] = [
            (.noConnection, "No connection: nothing was installed."),
            (.downloadFailed, "The download failed or was cut short: nothing was installed."),
            (.mismatch, "The download does not match what GitHub lists: nothing was installed."),
            (.notSigned, "The download is not signed by this app's signer — nothing was installed."),
            (.cannotWrite, "Metla cannot write to its folder: nothing was installed."),
            (.cleanupRunning, "A cleanup is running: nothing was installed."),
        ]
        for (failure, sentence) in sentences {
            let shown = UpdateShown.of(offer(.failed(failure)), installation: copy, cleanupRunning: false, english)
            #expect(shown == UpdateShown(title: sentence, note: nil, progress: nil, actions: [.releasePage(release.page), .later]))
            #expect(UpdateShown.text(failure, russian).hasSuffix("ничего не установлено."), "\(failure)")
        }
    }

    @Test func settingsSayTheVersionTheLastCheckAndItsOutcome() {
        #expect(UpdateShown.version(copy) == "0.8.0 (11)")
        let now = Date(timeIntervalSince1970: 1_800_000_000)
        #expect(UpdateShown.lastCheck(nil, now: now, english) == "Not checked yet")
        #expect(UpdateShown.lastCheck(now.addingTimeInterval(-300), now: now, english) == "Last checked 5 min. ago")
        #expect(UpdateShown.outcome(nil, checking: false, english) == nil)
        #expect(UpdateShown.outcome(.upToDate, checking: true, english) == "Checking…")
        #expect(UpdateShown.outcome(.upToDate, checking: false, english) == "Metla is up to date.")
        #expect(UpdateShown.outcome(.failed(.noConnection), checking: false, english) == "Could not reach GitHub.")
        #expect(UpdateShown.outcome(.failed(.answered(403)), checking: false, english) == "GitHub answered with error 403.")
        #expect(UpdateShown.outcome(.failed(.unreadable), checking: false, english) == "GitHub's answer could not be read.")
        #expect(UpdateShown.outcome(.failed(.answered(403)), checking: false, russian) == "GitHub ответил ошибкой 403.")
    }
}
