import Foundation

/// Why the running copy cannot replace itself: the offer then points to
/// the release page instead of installing. Never worked around (no
/// privilege escalation, no helper).
public enum InstallBlock: Equatable, Sendable {
    /// Signed ad-hoc (a copy built from source): its designated
    /// requirement is its own hash, which no download can satisfy.
    case adHoc
    /// macOS runs it from a randomised read-only place (App Translocation).
    case translocated
    case readOnlyVolume
    /// The bundle or its folder cannot be written by this user.
    case notWritable

    /// The first reason that holds, in this order.
    public static func of(adHoc: Bool, path: String, readOnlyVolume: Bool, writable: Bool) -> InstallBlock? {
        if adHoc {
            return .adHoc
        }
        if path.contains("/AppTranslocation/") {
            return .translocated
        }
        if readOnlyVolume {
            return .readOnlyVolume
        }
        return writable ? nil : .notWritable
    }
}

/// The running copy as the update needs it: where it is, what it is, and
/// whether it can replace itself.
public struct Installation: Equatable, Sendable {
    public let bundle: URL
    public let identifier: String
    public let version: Version
    public let build: Int
    public let block: InstallBlock?

    public init(bundle: URL, identifier: String, version: Version, build: Int, block: InstallBlock?) {
        self.bundle = bundle
        self.identifier = identifier
        self.version = version
        self.build = build
        self.block = block
    }

    /// The copy at `bundle`, signed ad-hoc or not as `adHoc` says; nil
    /// when its Info.plist names no identifier, no version the app can
    /// compare, or no integer build.
    public static func read(_ bundle: URL, adHoc: Bool) -> Installation? {
        let info = BundleInfo.read(bundle)
        guard let identifier = info.identifier, let version = info.version.flatMap({ Version($0) }),
            let build = info.build.flatMap(BundleInfo.number)
        else {
            return nil
        }
        let volume = try? bundle.resourceValues(forKeys: [.volumeIsReadOnlyKey])
        let parent = bundle.deletingLastPathComponent()
        let writable = access(bundle.path, W_OK) == 0 && access(parent.path, W_OK) == 0
        let block = InstallBlock.of(
            adHoc: adHoc, path: bundle.path, readOnlyVolume: volume?.volumeIsReadOnly ?? false, writable: writable)
        return Installation(bundle: bundle, identifier: identifier, version: version, build: build, block: block)
    }
}

/// What a bundle's Info.plist says of it.
public struct BundleInfo: Equatable, Sendable {
    public let identifier: String?
    public let version: String?
    public let build: String?

    public static func read(_ bundle: URL) -> BundleInfo {
        let data = try? Data(contentsOf: bundle.appendingPathComponent("Contents/Info.plist"))
        let plist = data.flatMap { try? PropertyListSerialization.propertyList(from: $0, format: nil) } as? [String: Any]
        return BundleInfo(
            identifier: plist?["CFBundleIdentifier"] as? String,
            version: plist?["CFBundleShortVersionString"] as? String,
            build: plist?["CFBundleVersion"] as? String)
    }

    /// Whether a new bundle that says this may replace `running`: the same
    /// identifier, exactly the version offered and greater than the
    /// running one, and an integer build greater than the running one (no
    /// downgrade, no sidegrade).
    public func accepted(offered: Version, over running: Installation) -> Bool {
        guard identifier == running.identifier, let shown = version.flatMap({ Version($0) }),
            let number = build.flatMap(Self.number)
        else {
            return false
        }
        return shown == offered && shown > running.version && number > running.build
    }

    /// A build number: ASCII digits only.
    static func number(_ text: String) -> Int? {
        guard !text.isEmpty, text.count <= 9, text.allSatisfy({ ("0"..."9").contains($0) }) else {
            return nil
        }
        return Int(text)
    }
}
