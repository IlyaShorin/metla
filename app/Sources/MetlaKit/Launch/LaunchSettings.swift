import Foundation
import MetlaCoreFFI

/// Where the engine runs and how the app was started, read once from the
/// environment and the arguments.
///
/// * `METLA_ROOT`: the directory analysed (default `/`);
/// * `METLA_HOME`: the home directory the rules use (default the user's);
/// * `METLA_DATA_DIR`: where the index lives (default
///   `~/Library/Application Support/Metla`);
/// * `METLA_PRINT_LAUNCH_TIMING=1`: print the time to the first window;
/// * `METLA_LANGUAGE`: `en` or `ru` instead of the system's choice;
/// * `--capture <dir>`: render the screens to PNG files and quit;
/// * `--scenario <name>`: drive a cleanup flow end to end on the fixture
///   and quit, non-zero when the disk is not as expected;
/// * `--rescan`, `--idle`: for measuring (docs/PERFORMANCE.md), start a
///   full scan as Refresh does once the saved picture is shown, and close
///   the main window then, leaving the app in the menu bar.
public struct LaunchSettings: Equatable, Sendable {
    public var root: String
    public var home: String
    public var user: String
    public var dataDir: String
    /// The root holds the fixture marker: the engine may clean it.
    public var isFixture: Bool
    public var printLaunchTiming: Bool
    public var capture: String?
    /// The scenario to run, if any.
    public var scenario: String? = nil
    /// Follow changes on disk (an FSEvents stream); a capture does not.
    public var follow: Bool
    /// The app's language, when not the system's choice.
    public var language: String? = nil
    /// Start a full scan once the saved picture is shown (`--rescan`).
    public var rescanAtLaunch = false
    /// Close the main window once the saved picture is shown (`--idle`).
    public var menuBarOnly = false

    /// The marker of a synthetic tree (metla-core's `FIXTURE_MARKER`).
    public static let fixtureMarker = ".metla-fixture"

    /// Reads the settings; `exists` says whether a file is there.
    public static func read(
        environment: [String: String],
        arguments: [String],
        home: String,
        user: String,
        exists: (String) -> Bool
    ) -> LaunchSettings {
        let root = canonical(environment["METLA_ROOT"] ?? "/")
        let support = (home as NSString).appendingPathComponent("Library/Application Support/Metla")
        let marker = (root as NSString).appendingPathComponent(fixtureMarker)
        let capture = value(after: "--capture", in: arguments)
        let scenario = value(after: "--scenario", in: arguments)
        return LaunchSettings(
            root: root,
            home: canonical(environment["METLA_HOME"] ?? home),
            user: user,
            dataDir: environment["METLA_DATA_DIR"] ?? support,
            isFixture: exists(marker),
            printLaunchTiming: environment["METLA_PRINT_LAUNCH_TIMING"] == "1",
            capture: capture,
            scenario: scenario,
            follow: capture == nil && scenario == nil,
            language: environment["METLA_LANGUAGE"],
            rescanAtLaunch: arguments.contains("--rescan"),
            menuBarOnly: arguments.contains("--idle")
        )
    }

    /// The settings of this process.
    public static func current() -> LaunchSettings {
        let process = ProcessInfo.processInfo
        return read(
            environment: process.environment,
            arguments: process.arguments,
            home: NSHomeDirectory(),
            user: NSUserName(),
            exists: { FileManager.default.fileExists(atPath: $0) }
        )
    }

    /// The `UserDefaults` suite the app keeps its own preferences in:
    /// none (the standard domain) on the real disk, a suite of its own on
    /// a fixture, so a development run never writes the user's settings.
    public var preferencesSuite: String? {
        isFixture ? "org.metla.Metla.dev" : nil
    }

    /// The preferences of a run with these settings.
    public var preferences: StandardPreferences {
        StandardPreferences(preferencesSuite.flatMap(UserDefaults.init(suiteName:)) ?? .standard)
    }

    /// The engine's options.
    public var options: EngineOptions {
        EngineOptions(
            root: root, home: home, user: user, dataDir: dataDir,
            profile: .background, follow: follow)
    }

    /// The path with every link resolved, as the engine names the root
    /// (`realpath`); a path that does not resolve stays as given.
    static func canonical(_ path: String) -> String {
        guard let resolved = realpath(path, nil) else {
            return path
        }
        defer { free(resolved) }
        return String(cString: resolved)
    }

    static func value(after flag: String, in arguments: [String]) -> String? {
        guard let at = arguments.firstIndex(of: flag), at + 1 < arguments.count else {
            return nil
        }
        return arguments[at + 1]
    }
}
