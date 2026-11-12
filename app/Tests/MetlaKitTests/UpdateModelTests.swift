import Foundation
import Testing
@testable import MetlaKit

/// A feed that gives what the test says and counts the times it was asked.
final class FakeFeed: UpdateFeed, @unchecked Sendable {
    let source = UpdateSource.github
    private let lock = NSLock()
    private var given: Result<Data, CheckFailure>
    private var count = 0

    init(_ given: Result<Data, CheckFailure> = .success(Data(FeedFixture.json.utf8))) {
        self.given = given
    }

    func answer(_ given: Result<Data, CheckFailure>) {
        lock.withLock { self.given = given }
    }

    var asked: Int {
        lock.withLock { count }
    }

    func latest() async throws(CheckFailure) -> Data {
        let given = lock.withLock {
            count += 1
            return self.given
        }
        return try given.get()
    }
}

/// Steps that record what they were asked, fail where the test says, and
/// can hold at a step until the test lets them go on.
final class FakeSteps: UpdateSteps, @unchecked Sendable {
    enum Step: String, Sendable {
        case prepare, download, verify, replace, discard, relaunch
    }

    private let lock = NSLock()
    private var failures: [Step: UpdateFailure]
    private var made: [Step] = []
    private var held: [Step: CheckedContinuation<Void, Never>] = [:]
    private let holds: Set<Step>
    let workspace = URL(fileURLWithPath: "/sandbox/workspace")

    init(failing failures: [Step: UpdateFailure] = [:], holding holds: Set<Step> = []) {
        self.failures = failures
        self.holds = holds
    }

    var calls: [Step] {
        lock.withLock { made }
    }

    func isHeld(at step: Step) -> Bool {
        lock.withLock { held[step] != nil }
    }

    /// Lets a held step go on.
    func release(_ step: Step) {
        lock.withLock { held.removeValue(forKey: step) }?.resume()
    }

    private func enter(_ step: Step) async throws(UpdateFailure) {
        lock.withLock { made.append(step) }
        if holds.contains(step) {
            await withCheckedContinuation { continuation in
                lock.withLock { held[step] = continuation }
            }
        }
        if let failure = lock.withLock({ failures[step] }) {
            throw failure
        }
    }

    func prepare(_ installation: Installation) throws(UpdateFailure) -> URL {
        lock.withLock { made.append(.prepare) }
        if let failure = lock.withLock({ failures[.prepare] }) {
            throw failure
        }
        return workspace
    }

    func download(_ release: Release, into workspace: URL, progress: @escaping @Sendable (Int64) -> Void)
        async throws(UpdateFailure) -> URL
    {
        progress(release.size / 2)
        try await enter(.download)
        return workspace.appendingPathComponent("Metla.zip")
    }

    func verify(_ archive: URL, release: Release, installation: Installation, in workspace: URL)
        async throws(UpdateFailure) -> URL
    {
        try await enter(.verify)
        return workspace.appendingPathComponent("unpacked/Metla.app")
    }

    func replace(_ installation: Installation, with app: URL, in workspace: URL) async throws(UpdateFailure) {
        try await enter(.replace)
    }

    func discard(_ workspace: URL) {
        lock.withLock { made.append(.discard) }
    }

    func relaunch(_ bundle: URL) {
        lock.withLock { made.append(.relaunch) }
    }
}

/// A clock the test sets.
final class TestClock: @unchecked Sendable {
    private let lock = NSLock()
    private var current = Date(timeIntervalSince1970: 1_800_000_000)

    var now: Date {
        lock.withLock { current }
    }

    func advance(_ seconds: TimeInterval) {
        lock.withLock { current += seconds }
    }
}

@MainActor
@Suite struct UpdateModelTests {
    static let running = Installation(
        bundle: URL(fileURLWithPath: "/sandbox/apps/Metla.app"), identifier: "org.metla.Metla",
        version: Version("0.8.0")!, build: 11, block: nil)

    final class Busy: @unchecked Sendable {
        var cleaning = false
    }

    struct Rig {
        let model: UpdateModel
        let feed: FakeFeed
        let steps: FakeSteps
        let preferences: MemoryPreferences
        let clock: TestClock
        let busy: Busy
    }

    func rig(
        feed: FakeFeed = FakeFeed(), steps: FakeSteps = FakeSteps(), installation: Installation = running,
        preferences: MemoryPreferences = MemoryPreferences()
    ) -> Rig {
        let clock = TestClock()
        let busy = Busy()
        let model = UpdateModel(
            installation: installation, feed: feed, steps: steps, preferences: preferences,
            cleanupRunning: { busy.cleaning }, now: { clock.now })
        return Rig(model: model, feed: feed, steps: steps, preferences: preferences, clock: clock, busy: busy)
    }

    @Test func aCheckThatFindsANewerReleaseOffersIt() async {
        let it = rig()
        #expect(it.model.offer == nil && it.model.lastCheck == nil && it.model.automatic)
        await it.model.check(manual: true)
        #expect(it.model.offer == UpdateModel.Offer(release: FeedFixture.release, phase: .offered))
        #expect(it.model.outcome == nil && it.model.lastCheck == it.clock.now && !it.model.checking)
    }

    @Test func aCheckThatFindsNothingNewSaysUpToDate() async {
        let it = rig(feed: FakeFeed(.success(FeedFixture.with { $0["tag_name"] = "v0.8.0" })))
        await it.model.check(manual: false)
        #expect(it.model.offer == nil && it.model.outcome == .upToDate && it.model.lastCheck == it.clock.now)
    }

    /// A failed automatic check is silent; a failed manual check says why.
    @Test func onlyAManualCheckSaysWhyItFailed() async {
        for failure in [CheckFailure.noConnection, .answered(403)] {
            let it = rig(feed: FakeFeed(.failure(failure)))
            await it.model.check(manual: false)
            #expect(it.model.outcome == nil && it.model.lastCheck == nil, "\(failure)")
            await it.model.check(manual: true)
            #expect(it.model.outcome == .failed(failure) && it.model.lastCheck == nil, "\(failure)")
        }
        let garbage = rig(feed: FakeFeed(.success(Data("<html>".utf8))))
        await garbage.model.check(manual: false)
        #expect(garbage.model.outcome == nil)
        await garbage.model.check(manual: true)
        #expect(garbage.model.outcome == .failed(.unreadable) && garbage.model.lastCheck == nil)
    }

    @Test func laterHidesTheOfferUntilACheckFindsItAgain() async {
        let it = rig()
        await it.model.check(manual: false)
        it.model.later()
        #expect(it.model.offer == nil)
        await it.model.check(manual: false)
        #expect(it.model.offer?.phase == .offered, "the next check finds the release again")
        it.feed.answer(.success(FeedFixture.with { $0["draft"] = true }))
        await it.model.check(manual: false)
        #expect(it.model.offer == nil, "a release no longer offered goes")
    }

    @Test func theSettingTurnsOffOnlyTheAutomaticChecks() async {
        let preferences = MemoryPreferences()
        let it = rig(preferences: preferences)
        it.model.setAutomatic(false)
        #expect(preferences.string(forKey: UpdateModel.automaticKey) == "0")
        await it.model.check(manual: false)
        #expect(it.feed.asked == 0 && it.model.offer == nil)
        await it.model.check(manual: true)
        #expect(it.feed.asked == 1 && it.model.offer != nil)
        #expect(!rig(preferences: preferences).model.automatic, "kept")
        it.model.setAutomatic(true)
        #expect(rig(preferences: preferences).model.automatic)
    }

    @Test func theLastCheckIsKept() async {
        let preferences = MemoryPreferences()
        let it = rig(preferences: preferences)
        await it.model.check(manual: true)
        let again = rig(preferences: preferences)
        #expect(again.model.lastCheck == it.clock.now)
    }

    @Test func installRunsEveryStepInOrderAndRelaunches() async throws {
        let steps = FakeSteps(holding: [.download, .verify, .replace])
        let it = rig(steps: steps)
        await it.model.check(manual: true)
        let install = Task { await it.model.install() }
        try await until { steps.isHeld(at: .download) && it.model.offer?.phase != .downloading(received: 0, total: 2_730_275) }
        #expect(it.model.offer?.phase == .downloading(received: 1_365_137, total: 2_730_275))
        steps.release(.download)
        try await until { steps.isHeld(at: .verify) }
        #expect(it.model.offer?.phase == .verifying)
        steps.release(.verify)
        try await until { steps.isHeld(at: .replace) }
        #expect(it.model.offer?.phase == .installing)
        steps.release(.replace)
        await install.value
        #expect(steps.calls == [.prepare, .download, .verify, .replace, .relaunch])
        #expect(it.model.offer?.phase == .installing, "until the app quits")
    }

    /// Every failure ends in the failed phase with its reason; what a step
    /// before the swap left is discarded, and nothing is relaunched.
    @Test func everyFailureIsShownAndNothingIsRelaunched() async {
        let cases: [(FakeSteps.Step, UpdateFailure, [FakeSteps.Step])] = [
            (.prepare, .cannotWrite, [.prepare]),
            (.download, .noConnection, [.prepare, .download, .discard]),
            (.download, .downloadFailed, [.prepare, .download, .discard]),
            (.download, .mismatch, [.prepare, .download, .discard]),
            (.verify, .notSigned, [.prepare, .download, .verify, .discard]),
            (.verify, .mismatch, [.prepare, .download, .verify, .discard]),
            (.replace, .cannotWrite, [.prepare, .download, .verify, .replace]),
        ]
        for (step, failure, calls) in cases {
            let steps = FakeSteps(failing: [step: failure])
            let it = rig(steps: steps)
            await it.model.check(manual: true)
            await it.model.install()
            #expect(it.model.offer?.phase == .failed(failure), "\(step) \(failure)")
            #expect(steps.calls == calls, "\(step) \(failure)")
            await it.model.install()
            #expect(steps.calls == calls, "a failed offer does not install again")
            await it.model.check(manual: false)
            #expect(it.model.offer?.phase == .offered, "the next check offers it again")
        }
    }

    /// Never during a cleanup: Install does nothing but say so, and does
    /// not wait for the cleanup to end.
    @Test func aCleanupKeepsTheInstallFromStarting() async throws {
        let steps = FakeSteps()
        let it = rig(steps: steps)
        await it.model.check(manual: true)
        it.busy.cleaning = true
        #expect(it.model.cleanupRunning)
        await it.model.install()
        #expect(it.model.offer?.phase == .failed(.cleanupRunning) && steps.calls.isEmpty)
        it.busy.cleaning = false
        try await Task.sleep(for: .milliseconds(50))
        #expect(steps.calls.isEmpty, "it does not queue")

        let late = FakeSteps(holding: [.verify])
        let other = rig(steps: late)
        await other.model.check(manual: true)
        let install = Task { await other.model.install() }
        try await until { late.isHeld(at: .verify) }
        other.busy.cleaning = true
        late.release(.verify)
        await install.value
        #expect(other.model.offer?.phase == .failed(.cleanupRunning), "a cleanup that started meanwhile")
        #expect(late.calls == [.prepare, .download, .verify, .discard], "nothing replaced")
    }

    @Test func aCopyThatCannotReplaceItselfNeverInstalls() async {
        for block in [InstallBlock.adHoc, .translocated, .readOnlyVolume, .notWritable] {
            let steps = FakeSteps()
            let blocked = Installation(
                bundle: Self.running.bundle, identifier: "org.metla.Metla", version: Version("0.8.0")!, build: 11,
                block: block)
            let it = rig(steps: steps, installation: blocked)
            await it.model.check(manual: true)
            #expect(it.model.offer?.phase == .offered)
            await it.model.install()
            #expect(steps.calls.isEmpty && it.model.offer?.phase == .offered, "\(block)")
        }
    }

    /// A check while an install runs leaves the install alone; Later too.
    @Test func aRunningInstallIsLeftAlone() async throws {
        let steps = FakeSteps(holding: [.download])
        let it = rig(steps: steps)
        await it.model.check(manual: true)
        let install = Task { await it.model.install() }
        try await until { steps.isHeld(at: .download) }
        it.feed.answer(.success(FeedFixture.with { $0["tag_name"] = "v0.8.0" }))
        await it.model.check(manual: true)
        it.model.later()
        guard case .downloading? = it.model.offer?.phase else {
            Issue.record("still downloading: \(String(describing: it.model.offer))")
            steps.release(.download)
            await install.value
            return
        }
        await it.model.install()
        steps.release(.download)
        await install.value
        #expect(steps.calls == [.prepare, .download, .verify, .replace, .relaunch], "one install")
    }

    @Test func checksDoNotOverlap() async throws {
        let it = rig()
        async let first: Void = it.model.check(manual: true)
        async let second: Void = it.model.check(manual: true)
        _ = await (first, second)
        #expect(it.feed.asked == 1)
    }

    /// At launch, then every 24 hours; nothing while the setting is off.
    @Test func theScheduleChecksAtOnceAndThenDaily() async {
        let it = rig()
        let slept = Slept()
        await it.model.follow { duration in
            try await slept.sleep(duration, stopAfter: 3)
        }
        #expect(it.feed.asked == 3)
        #expect(await slept.durations == [.seconds(86_400), .seconds(86_400), .seconds(86_400)])
        it.model.setAutomatic(false)
        let off = Slept()
        await it.model.follow { duration in
            try await off.sleep(duration, stopAfter: 3)
        }
        #expect(it.feed.asked == 3, "no check while the setting is off")
    }

    /// Only an ordinary launch on the real disk checks by itself.
    @Test func testRunsAndFixturesNeverCheck() {
        func read(_ environment: [String: String], _ arguments: [String]) -> LaunchSettings {
            LaunchSettings.read(environment: environment, arguments: ["Metla"] + arguments, home: "/h", user: "u",
                                exists: { $0 == "/fx/.metla-fixture" })
        }
        #expect(read([:], []).checksForUpdates)
        #expect(read([:], ["--rescan"]).checksForUpdates)
        #expect(!read([:], ["--scenario", "default-clean"]).checksForUpdates)
        #expect(!read([:], ["--capture", "/shots"]).checksForUpdates)
        #expect(!read([:], ["--idle"]).checksForUpdates)
        #expect(!read(["METLA_ROOT": "/fx"], []).checksForUpdates)
        #expect(!read(["METLA_ROOT": "/"], []).checksForUpdates, "any root given")
        let made = LaunchSettings(
            root: "/", home: "/h", user: "u", dataDir: "/d", isFixture: false, printLaunchTiming: false, capture: nil,
            follow: true)
        #expect(!made.checksForUpdates, "made in code: off")
    }
}

/// Records the sleeps asked for and stops the loop after a number of them.
actor Slept {
    private(set) var durations: [Duration] = []

    func sleep(_ duration: Duration, stopAfter: Int) throws {
        durations.append(duration)
        if durations.count >= stopAfter {
            throw CancellationError()
        }
    }
}

/// What the app makes for its own copy.
@MainActor
@Suite struct UpdateWiringTests {
    /// A production build reads GitHub whatever the environment says, and
    /// its one trust check is the requirement of the running app's own
    /// signature.
    @Test func theAppChecksGitHubAndVerifiesAgainstItsOwnRequirement() throws {
        let box = Sandbox()
        defer { box.remove() }
        let app = try BundleFixture.make(in: box.folder("apps"), version: "0.8.0", build: "11")
        let own = Signature.Own(requirement: "identifier \"org.metla.Metla\" and anchor apple generic", adHoc: false)
        let made = UpdateModel.app(
            bundle: app, stamp: "62f142ca7975e910-production",
            environment: [UpdateSource.variable: "file:///tmp/feed/latest.json"], signature: own,
            preferences: MemoryPreferences(), cleanupRunning: { false }, relaunch: { _ in })
        let model = try #require(made.model)
        #expect(model.parts.feed.source == .github)
        #expect(made.said.hasSuffix("(fixed in this build; METLA_UPDATE_FEED ignored)"))
        #expect((model.parts.steps as? Installer)?.requirement == own.requirement)
        #expect(model.installation.block == nil && model.installation.version == Version("0.8.0"))

        let plain = UpdateModel.app(
            bundle: app, stamp: "62f142ca7975e910-plain",
            environment: [UpdateSource.variable: "file:///tmp/feed/latest.json"], signature: own,
            preferences: MemoryPreferences(), cleanupRunning: { false }, relaunch: { _ in })
        #expect(plain.model?.parts.feed.source.feed.absoluteString == "file:///tmp/feed/latest.json")
        #expect((plain.model?.parts.steps as? Installer)?.requirement == own.requirement, "the same requirement")
    }

    @Test func anAdHocOrUnsignedCopyOnlyLinksAndACopyWithoutAVersionHasNoUpdate() throws {
        let box = Sandbox()
        defer { box.remove() }
        let app = try BundleFixture.make(in: box.folder("apps"), version: "0.8.0", build: "11")
        for own in [Signature.Own(requirement: "cdhash H\"00\"", adHoc: true), Signature.Own(requirement: nil, adHoc: false)] {
            let made = UpdateModel.app(
                bundle: app, stamp: "x-production", environment: [:], signature: own, preferences: MemoryPreferences(),
                cleanupRunning: { false }, relaunch: { _ in })
            #expect(made.model?.installation.block == .adHoc)
        }
        let bare = UpdateModel.app(
            bundle: box("none.app"), stamp: "x-production", environment: [:],
            signature: Signature.Own(requirement: nil, adHoc: true), preferences: MemoryPreferences(),
            cleanupRunning: { false }, relaunch: { _ in })
        #expect(bare.model == nil && bare.said.hasSuffix("no version to compare: no updates"))
    }
}
