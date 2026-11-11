import Foundation
import MetlaCoreFFI
import Testing
@testable import MetlaKit

@Suite struct LaunchTests {
    @Test func theEnvironmentPointsTheEngineAtAFixture() {
        let environment = ["METLA_ROOT": "/fx", "METLA_HOME": "/fx/home", "METLA_DATA_DIR": "/data",
                           "METLA_PRINT_LAUNCH_TIMING": "1", "METLA_LANGUAGE": "ru"]
        let settings = LaunchSettings.read(
            environment: environment, arguments: ["Metla"], home: "/Users/u", user: "u",
            exists: { $0 == "/fx/.metla-fixture" })
        #expect(settings.root == "/fx" && settings.home == "/fx/home" && settings.dataDir == "/data")
        #expect(settings.isFixture && settings.printLaunchTiming && settings.follow)
        #expect(settings.capture == nil && settings.language == "ru")
        #expect(settings.options == EngineOptions(root: "/fx", home: "/fx/home", user: "u", dataDir: "/data", profile: .background, follow: true))
    }

    @Test func withoutTheEnvironmentTheEngineReadsTheWholeDisk() {
        let settings = LaunchSettings.read(environment: [:], arguments: ["Metla"], home: "/Users/u", user: "u", exists: { _ in false })
        #expect(settings.root == "/" && settings.home == "/Users/u")
        #expect(settings.dataDir == "/Users/u/Library/Application Support/Metla")
        #expect(!settings.isFixture && !settings.printLaunchTiming && settings.language == nil)
    }

    /// The engine reads the root's canonical path; the app names it and
    /// the home the same way, or "~" and the scenarios' paths would not
    /// match what the engine lists.
    @Test func aRootAndHomeNamedThroughALinkAreTheirCanonicalPaths() throws {
        let real = Fixture.scratch()
        let home = real.appendingPathComponent("home")
        try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
        let link = Fixture.scratch().appendingPathComponent("link").path
        try FileManager.default.createSymbolicLink(atPath: link, withDestinationPath: real.path)
        let settings = LaunchSettings.read(
            environment: ["METLA_ROOT": link, "METLA_HOME": link + "/home"], arguments: ["Metla"],
            home: "/Users/u", user: "u", exists: { FileManager.default.fileExists(atPath: $0) })
        let canonical = try #require(realpath(real.path, nil).map { String(cString: $0) })
        #expect(settings.root == canonical && settings.home == canonical + "/home")
    }

    @Test func measuringCanStartARescanAndLeaveOnlyTheMenuBar() {
        let plain = LaunchSettings.read(environment: [:], arguments: ["Metla"], home: "/h", user: "u", exists: { _ in false })
        #expect(!plain.rescanAtLaunch && !plain.menuBarOnly)
        let measured = LaunchSettings.read(
            environment: [:], arguments: ["Metla", "--rescan", "--idle"], home: "/h", user: "u", exists: { _ in false })
        #expect(measured.rescanAtLaunch && measured.menuBarOnly && measured.follow)
    }

    @Test func aCaptureNamesItsDirectoryAndFollowsNothing() {
        let settings = LaunchSettings.read(environment: [:], arguments: ["Metla", "--capture", "/shots"], home: "/h", user: "u", exists: { _ in true })
        #expect(settings.capture == "/shots")
        #expect(!settings.follow && !settings.options.follow)
        #expect(LaunchSettings.value(after: "--capture", in: ["Metla", "--capture"]) == nil)
    }
}

@Suite struct PacerTests {
    @Test func refreshesAreAnIntervalApart() {
        var pacer = RefreshPacer(interval: .milliseconds(250))
        let start = ContinuousClock.now
        #expect(pacer.wait(at: start) == .zero, "the first refresh runs at once")
        pacer.ran(at: start)
        #expect(pacer.wait(at: start + .milliseconds(100)) == .milliseconds(150))
        #expect(pacer.wait(at: start + .milliseconds(250)) == .zero)
        #expect(pacer.wait(at: start + .seconds(5)) == .zero)
    }
}

/// Records what a guard reports.
final class Recorder: Sendable {
    private let calls = Mutex()
    func record(_ call: String) { calls.append(call) }
    var all: [String] { calls.values }

    final class Mutex: @unchecked Sendable {
        private let lock = NSLock()
        private var items: [String] = []
        func append(_ item: String) { lock.withLock { items.append(item) } }
        var values: [String] { lock.withLock { items } }
    }
}

@Suite struct GuardTests {
    @Test @MainActor func aBlockingCallOnTheMainThreadIsReported() {
        let recorder = Recorder()
        let guarded = MainThreadGuard { recorder.record($0) }
        let answer = EngineClient.perform("snapshot", guarded) { 42 }
        #expect(answer == 42)
        #expect(recorder.all == ["snapshot"])
    }

    @Test func aBlockingCallOffTheMainThreadIsNot() async {
        let recorder = Recorder()
        let guarded = MainThreadGuard { recorder.record($0) }
        await Task.detached {
            _ = EngineClient.perform("snapshot", guarded) { 1 }
        }.value
        #expect(recorder.all.isEmpty)
    }
}
