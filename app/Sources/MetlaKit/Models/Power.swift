import Foundation
import MetlaCoreFFI

/// What the Mac's power state says about how hard Metla may work.
public struct PowerState: Equatable, Sendable {
    public enum Thermal: Equatable, Sendable {
        case nominal, fair, serious, critical
    }

    public var onBattery: Bool
    public var lowPowerMode: Bool
    public var thermal: Thermal

    public init(onBattery: Bool = false, lowPowerMode: Bool = false, thermal: Thermal = .nominal) {
        self.onBattery = onBattery
        self.lowPowerMode = lowPowerMode
        self.thermal = thermal
    }

    /// The engine's power profile for this state, as one table: background
    /// analysis off → Off; a scan the user asked for → Interactive for that
    /// scan; Low Power Mode, a serious or critical thermal state, or
    /// battery with "reduce on battery" → Saver; else Background.
    public func profile(background: Bool, reduceOnBattery: Bool, userScan: Bool) -> PowerProfile {
        guard background else {
            return .off
        }
        if userScan {
            return .interactive
        }
        let hot = thermal == .serious || thermal == .critical
        if lowPowerMode || hot || (onBattery && reduceOnBattery) {
            return .saver
        }
        return .background
    }
}

/// Full Disk Access as the app can tell it: only by trying to read what
/// only Full Disk Access lets read. Nothing is assumed and nothing works
/// around the system's privacy rules.
public enum FullDiskAccess: Equatable, Sendable {
    case granted
    case notGranted
    /// No protected place could be tried.
    case unknown

    /// The state from what listing each protected place answered, in
    /// order: the first that exists decides.
    public static func from(_ answers: [ProbeAnswer]) -> FullDiskAccess {
        for answer in answers {
            switch answer {
            case .listed: return .granted
            case .denied: return .notGranted
            case .missing: continue
            }
        }
        return .unknown
    }
}

/// What listing one protected directory answered.
public enum ProbeAnswer: Equatable, Sendable {
    case listed
    case denied
    case missing
}

/// Tries to list directories only Full Disk Access lets the app read.
public protocol AccessProbe: Sendable {
    /// The directories it tries, in the order of its answers.
    var places: [String] { get }
    func answers() async -> [ProbeAnswer]
}

extension AccessProbe {
    public var places: [String] { [] }
}

/// The probe of the app: lists (and only lists) a few directories of the
/// user's Library that macOS protects, and `/Library/Application
/// Support/com.apple.TCC`.
public struct LibraryProbe: AccessProbe {
    public let places: [String]

    public init(home: String = NSHomeDirectory()) {
        places = [
            (home as NSString).appendingPathComponent("Library/Mail"),
            (home as NSString).appendingPathComponent("Library/Safari"),
            "/Library/Application Support/com.apple.TCC",
        ]
    }

    @concurrent
    public func answers() async -> [ProbeAnswer] {
        places.map { place in
            do {
                _ = try FileManager.default.contentsOfDirectory(atPath: place)
                return .listed
            } catch let error as CocoaError where error.code == .fileReadNoSuchFile {
                return .missing
            } catch {
                return .denied
            }
        }
    }
}
