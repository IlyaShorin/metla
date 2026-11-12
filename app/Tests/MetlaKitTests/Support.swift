import Foundation
import MetlaCoreFFI
import Testing
@testable import MetlaKit

/// A synthetic root made once per run by `metla-cli fixture create` (the
/// CLI the build made: `METLA_CLI`, or the repository's debug build), and
/// a data directory of its own for every engine.
enum Fixture {
    static let repository = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        .deletingLastPathComponent()

    /// The shared fixture's root; tests only read it.
    static let root: String = make()

    static func make() -> String {
        let environment = ProcessInfo.processInfo.environment
        let cli = environment["METLA_CLI"] ?? repository.appendingPathComponent("core/target/debug/metla-cli").path
        let dir = scratch().appendingPathComponent("fixture")
        let process = Process()
        process.executableURL = URL(fileURLWithPath: cli)
        process.arguments = ["fixture", "create", dir.path]
        process.standardOutput = FileHandle.nullDevice
        try? process.run()
        process.waitUntilExit()
        precondition(process.terminationStatus == 0, "metla-cli fixture create failed: \(cli)")
        return dir.resolvingSymlinksInPath().path
    }

    /// `$TMPDIR` as the test was started with (scripts/quality-swift.sh
    /// points it at the build volume), else the system's.
    static let temporary: URL = {
        let given = ProcessInfo.processInfo.environment["TMPDIR"]
        return given.map { URL(fileURLWithPath: $0) } ?? FileManager.default.temporaryDirectory
    }()

    /// A new empty directory below the temporary directory.
    static func scratch() -> URL {
        let dir = temporary.appendingPathComponent("metla-kit-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir.resolvingSymlinksInPath()
    }

    static func settings(root: String = Fixture.root, data: String = scratch().path) -> LaunchSettings {
        LaunchSettings(
            root: root, home: (root as NSString).appendingPathComponent("home"), user: "tester",
            dataDir: data, isFixture: true, printLaunchTiming: false, capture: nil, follow: false)
    }
}

/// Waits until `done` holds, at most `seconds`.
@MainActor
func until(_ seconds: Double = 30, _ done: @MainActor () async -> Bool) async throws {
    let deadline = Date().addingTimeInterval(seconds)
    while await !done() {
        guard Date() < deadline else {
            throw Timeout()
        }
        try await Task.sleep(for: .milliseconds(50))
    }
}

struct Timeout: Error {}

/// An app model on `settings`, its first scan done; it moves to the
/// Trash with `trash` (by default one that refuses: the shared fixture is
/// only read).
@MainActor
func started(
    _ settings: LaunchSettings = Fixture.settings(), trash: any TrashHost = RefusingTrash(),
    preferences: any Preferences = MemoryPreferences(), probe: any AccessProbe = FixedProbe([])
) async throws -> AppModel {
    let model = AppModel(
        settings: settings, strings: Strings(language: "en", region: "US"), preferences: preferences, probe: probe)
    await model.start(
        mainThread: MainThreadGuard { call in Issue.record("main thread: \(call)") }, trash: trash,
        login: FakeLogin())
    try await until {
        await model.refresh([.picture])
        guard case .idle? = model.freshness?.state else { return false }
        return model.overview?.freshness.fullScan != nil
    }
    return model
}

/// A Trash that moves nothing.
final class RefusingTrash: TrashHost, Sendable {
    func trash(path: String) -> TrashOutcome {
        .refused(reason: "no Trash in this test")
    }
}

/// A probe with fixed answers, for `places` in order.
struct FixedProbe: AccessProbe {
    let given: [ProbeAnswer]
    let places: [String]

    init(_ given: [ProbeAnswer], places: [String] = []) {
        self.given = given
        self.places = places
    }

    func answers() async -> [ProbeAnswer] {
        given
    }
}

/// A probe whose answers the test changes; it counts the times it was
/// asked.
actor ChangingProbe: AccessProbe {
    private var given: [ProbeAnswer]
    private(set) var asked = 0

    init(_ given: [ProbeAnswer]) {
        self.given = given
    }

    func answer(_ given: [ProbeAnswer]) {
        self.given = given
    }

    func answers() async -> [ProbeAnswer] {
        asked += 1
        return given
    }
}

/// A login item that remembers, or refuses.
@MainActor
final class FakeLogin: LoginItem {
    var enabled = false
    var refuses = false

    func set(_ enabled: Bool) throws {
        if refuses {
            throw CocoaError(.featureUnsupported)
        }
        self.enabled = enabled
    }
}
