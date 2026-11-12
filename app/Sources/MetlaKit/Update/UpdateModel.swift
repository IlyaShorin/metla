import Foundation
import Observation

/// Why an install did not happen; a closed list, each said in one sentence.
public enum UpdateFailure: Error, Equatable, Sendable {
    case noConnection
    /// The download failed or was cut short.
    case downloadFailed
    /// The download is not what the feed lists: its size, its digest, the
    /// archive's contents or the bundle's identifier, version or build.
    case mismatch
    /// The download is not signed by this app's signer.
    case notSigned
    case cannotWrite
    case cleanupRunning
}

/// Why a check got no answer it could use.
public enum CheckFailure: Error, Equatable, Sendable {
    case noConnection
    /// The feed answered with another HTTP status than 200.
    case answered(Int)
    /// The answer is not a release.
    case unreadable
}

/// Where a check asks: the latest release, as the source's feed gives it.
public protocol UpdateFeed: Sendable {
    var source: UpdateSource { get }
    func latest() async throws(CheckFailure) -> Data
}

/// The install, step by step: each throws the reason it failed.
public protocol UpdateSteps: Sendable {
    /// A new folder on the installation's volume for what follows.
    func prepare(_ installation: Installation) throws(UpdateFailure) -> URL
    /// The release's archive in `workspace`, its size and digest checked.
    func download(_ release: Release, into workspace: URL, progress: @escaping @Sendable (Int64) -> Void)
        async throws(UpdateFailure) -> URL
    /// The archive unpacked to one `Metla.app`, its signature and Info.plist
    /// verified against the running copy.
    func verify(_ archive: URL, release: Release, installation: Installation, in workspace: URL)
        async throws(UpdateFailure) -> URL
    /// Puts `app` in the installation's place and removes `workspace`
    /// (the old bundle with it).
    func replace(_ installation: Installation, with app: URL, in workspace: URL) async throws(UpdateFailure)
    /// Removes `workspace` after a step before the replacement failed.
    func discard(_ workspace: URL)
    /// Starts the bundle at `bundle` once this process has ended, and ends it.
    @MainActor func relaunch(_ bundle: URL)
}

/// When the app checks by itself: at launch and every 24 hours while it
/// runs, when the setting is on (`UpdateModel.follow`), and only in an
/// ordinary launch (`LaunchSettings.checksForUpdates`).
public enum UpdateSchedule {
    public static let interval: Duration = .seconds(24 * 3600)
}

/// The in-app update: the offer, a check's outcome, and the install from
/// download to relaunch. One install at a time; never during a cleanup.
@MainActor
@Observable
public final class UpdateModel {
    /// Where an offer stands.
    public enum Phase: Equatable, Sendable {
        case offered
        case downloading(received: Int64, total: Int64)
        case verifying
        case installing
        case failed(UpdateFailure)

        /// An install is under way.
        public var busy: Bool {
            switch self {
            case .downloading, .verifying, .installing: true
            case .offered, .failed: false
            }
        }
    }

    public struct Offer: Equatable, Sendable {
        public let release: Release
        public internal(set) var phase: Phase
    }

    /// What the last check found when it offered nothing.
    public enum Outcome: Equatable, Sendable {
        case upToDate
        case failed(CheckFailure)
    }

    public let installation: Installation
    public private(set) var offer: Offer?
    public private(set) var checking = false
    public private(set) var outcome: Outcome?
    /// When a check last got an answer.
    public private(set) var lastCheck: Date?
    /// Checks at launch and daily (on unless turned off).
    public private(set) var automatic: Bool

    private let feed: any UpdateFeed
    private let steps: any UpdateSteps
    private let preferences: any Preferences
    private let cleaning: @MainActor () -> Bool
    private let now: @Sendable () -> Date
    static let automaticKey = "updateAutomatic"
    static let lastCheckKey = "updateLastCheck"

    public init(
        installation: Installation, feed: any UpdateFeed, steps: any UpdateSteps, preferences: any Preferences,
        cleanupRunning: @escaping @MainActor () -> Bool, now: @escaping @Sendable () -> Date = { Date() }
    ) {
        self.installation = installation
        self.feed = feed
        self.steps = steps
        self.preferences = preferences
        cleaning = cleanupRunning
        self.now = now
        automatic = preferences.string(forKey: Self.automaticKey) != "0"
        lastCheck = preferences.string(forKey: Self.lastCheckKey).flatMap(TimeInterval.init)
            .map(Date.init(timeIntervalSince1970:))
    }

    /// A cleanup runs: Install is disabled.
    public var cleanupRunning: Bool {
        cleaning()
    }

    public func setAutomatic(_ on: Bool) {
        preferences.set(on ? "1" : "0", forKey: Self.automaticKey)
        automatic = on
    }

    /// Asks the feed once. An automatic check does nothing while the
    /// setting is off and says nothing when it fails; a manual one says
    /// why. A check never disturbs an install under way.
    public func check(manual: Bool) async {
        guard !checking, manual || automatic else {
            return
        }
        checking = true
        defer { checking = false }
        let answer: Data
        do throws(CheckFailure) {
            answer = try await feed.latest()
        } catch {
            if manual {
                outcome = .failed(error)
            }
            return
        }
        let reading = feed.source.read(answer, running: installation.version)
        if reading == .unreadable {
            if manual {
                outcome = .failed(.unreadable)
            }
            return
        }
        checked()
        guard offer?.phase.busy != true else {
            return
        }
        if case .offer(let release) = reading {
            offer = Offer(release: release, phase: .offered)
            outcome = nil
        } else {
            offer = nil
            outcome = .upToDate
        }
    }

    private func checked() {
        let at = now()
        lastCheck = at
        preferences.set(String(at.timeIntervalSince1970), forKey: Self.lastCheckKey)
    }

    /// Hides the offer until a check finds the release again.
    public func later() {
        guard offer?.phase.busy != true else {
            return
        }
        offer = nil
    }

    /// Install: download, check, unpack, verify, replace, relaunch, with
    /// no further question. Only an offered release of a copy that can
    /// replace itself; refused while a cleanup runs.
    public func install() async {
        guard let release = offer?.release, offer?.phase == .offered, installation.block == nil else {
            return
        }
        guard !cleaning() else {
            show(.failed(.cleanupRunning))
            return
        }
        show(.downloading(received: 0, total: release.size))
        let workspace: URL
        do throws(UpdateFailure) {
            workspace = try steps.prepare(installation)
        } catch {
            show(.failed(error))
            return
        }
        let app: URL
        do throws(UpdateFailure) {
            let archive = try await steps.download(release, into: workspace) { received in
                Task { @MainActor [weak self] in self?.received(received, of: release) }
            }
            show(.verifying)
            app = try await steps.verify(archive, release: release, installation: installation, in: workspace)
            guard !cleaning() else {
                throw UpdateFailure.cleanupRunning
            }
            show(.installing)
        } catch {
            steps.discard(workspace)
            show(.failed(error))
            return
        }
        do throws(UpdateFailure) {
            try await steps.replace(installation, with: app, in: workspace)
        } catch {
            show(.failed(error))
            return
        }
        steps.relaunch(installation.bundle)
    }

    private func show(_ phase: Phase) {
        offer?.phase = phase
    }

    private func received(_ bytes: Int64, of release: Release) {
        if case .downloading? = offer?.phase, offer?.release == release {
            show(.downloading(received: bytes, total: release.size))
        }
    }

    /// Checks now and then every `UpdateSchedule.interval` until the task
    /// is cancelled (`sleep` throws); each check honours the setting.
    public func follow(sleep: @Sendable (Duration) async throws -> Void = { try await Task.sleep(for: $0) }) async {
        while true {
            await check(manual: false)
            do {
                try await sleep(UpdateSchedule.interval)
            } catch {
                return
            }
        }
    }
}
