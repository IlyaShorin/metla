import Foundation

/// A release's version, `<major>.<minor>.<patch>`: what a tag names after
/// its `v`, and what a bundle's `CFBundleShortVersionString` says.
public struct Version: Comparable, Hashable, Sendable, CustomStringConvertible {
    public let major: Int
    public let minor: Int
    public let patch: Int

    /// "1.2.3": three numbers of ASCII digits without leading zeros; nil
    /// for anything else (a sign, a fourth part, a suffix, letters).
    public init?(_ text: String) {
        let parts = text.split(separator: ".", omittingEmptySubsequences: false)
        let numbers = parts.compactMap(Self.number)
        guard parts.count == 3, numbers.count == 3 else {
            return nil
        }
        (major, minor, patch) = (numbers[0], numbers[1], numbers[2])
    }

    /// A release's tag: "v1.2.3".
    public init?(tag: String) {
        guard tag.hasPrefix("v") else {
            return nil
        }
        self.init(String(tag.dropFirst()))
    }

    private static func number(_ part: Substring) -> Int? {
        let digits = part.allSatisfy { ("0"..."9").contains($0) }
        guard digits, (1...9).contains(part.count), part == "0" || !part.hasPrefix("0") else {
            return nil
        }
        return Int(part)
    }

    public static func < (a: Version, b: Version) -> Bool {
        (a.major, a.minor, a.patch) < (b.major, b.minor, b.patch)
    }

    public var description: String {
        "\(major).\(minor).\(patch)"
    }
}

/// A release the app may offer: everything it acts on, checked where the
/// feed comes in (`UpdateSource.read`).
public struct Release: Equatable, Sendable {
    public let version: Version
    /// `Metla.zip`, within the source's downloads.
    public let download: URL
    /// The size the feed declares: more than 0, at most 100 MB.
    public let size: Int64
    /// The SHA-256 the feed lists for the download, when it lists one.
    public let digest: Data?
    /// "What's new": the release's page, when it is one of the source's.
    public let notes: URL?
    /// The release page to send the user to: its own, or the list of
    /// releases.
    public let page: URL
}

/// What a check learned from the feed.
public enum FeedReading: Equatable, Sendable {
    case offer(Release)
    /// A release that is not newer, not final or not complete.
    case noUpdate
    /// An answer that is not a release at all.
    case unreadable
}

/// Where the app learns of releases and where it may download them from.
/// A production build always uses `github`; other builds may be given a
/// test feed (`METLA_UPDATE_FEED`).
public struct UpdateSource: Equatable, Sendable {
    /// The repository releases are published in: a fork changes this line.
    public static let repository = "IlyaShorin/metla"
    /// GitHub's latest release of the repository.
    public static let github = UpdateSource(
        feed: URL(string: "https://api.github.com/repos/\(repository)/releases/latest")!,
        downloads: "https://github.com/\(repository)/releases/download/",
        pages: "https://github.com/\(repository)/releases/")
    /// The environment variable that names a test feed: a file or http(s)
    /// URL. Read only by builds other than production.
    public static let variable = "METLA_UPDATE_FEED"
    /// The only asset the app downloads.
    public static let assetName = "Metla.zip"
    /// The largest download the app accepts.
    public static let largest: Int64 = 100_000_000

    public let feed: URL
    /// Every download's URL lies within this.
    public let downloads: String
    /// "What's new" and the release page lie within this.
    public let pages: String

    /// A test feed: its downloads and pages are files beside it (a folder
    /// of a file URL, or of an http(s) server). Nil for anything else.
    public static func test(_ feed: URL) -> UpdateSource? {
        let scheme = feed.scheme?.lowercased()
        let usable = scheme == "file" || ((scheme == "http" || scheme == "https") && !(feed.host ?? "").isEmpty)
        guard usable, !feed.path.isEmpty, !feed.hasDirectoryPath else {
            return nil
        }
        let folder = feed.deletingLastPathComponent().absoluteString
        return UpdateSource(feed: feed, downloads: folder, pages: folder)
    }

    /// The source in force and the line that says which: in a production
    /// build (its core's stamp ends in `-production`) always `github`,
    /// whatever the environment holds; in others the test feed
    /// `METLA_UPDATE_FEED` names, if it is usable.
    public static func chosen(stamp: String, environment: [String: String]) -> (source: UpdateSource, said: String) {
        let given = environment[variable]
        if stamp.hasSuffix("-production") {
            let ignored = given == nil ? "" : "; \(variable) ignored"
            return (github, "update: feed \(github.feed.absoluteString) (fixed in this build\(ignored))")
        }
        if let given, let url = URL(string: given), let test = test(url) {
            return (test, "update: feed \(url.absoluteString) (\(variable))")
        }
        return (github, "update: feed \(github.feed.absoluteString)")
    }

    /// The list of releases, for a release whose own page is not one.
    public var releases: URL {
        URL(string: pages)!
    }

    /// Reads the feed's answer against the running version: a release is
    /// offered when its tag is `v<a>.<b>.<c>` and greater than `running`,
    /// it is neither a draft nor a prerelease, and its asset `Metla.zip`
    /// lies within the source's downloads with a size of at most 100 MB
    /// and, if any, a SHA-256 digest. The release notes are never read.
    public func read(_ answer: Data, running: Version) -> FeedReading {
        guard let wire = try? JSONDecoder().decode(Wire.self, from: answer) else {
            return .unreadable
        }
        guard wire.draft == false, wire.prerelease == false,
            let version = wire.tag_name.flatMap(Version.init(tag:)), version > running,
            let asset = wire.assets?.first(where: { $0.name == Self.assetName }),
            let download = asset.browser_download_url.flatMap(URL.init(string:)), Self.within(download, downloads),
            let size = asset.size, size > 0, size <= Self.largest
        else {
            return .noUpdate
        }
        // None listed, or one that must be a SHA-256.
        let digest = asset.digest.map(Self.sha256)
        guard digest != .some(nil) else {
            return .noUpdate
        }
        let notes = wire.html_url.flatMap(URL.init(string:)).flatMap { Self.within($0, pages) ? $0 : nil }
        return .offer(Release(
            version: version, download: download, size: size, digest: digest ?? nil, notes: notes,
            page: notes ?? releases))
    }

    /// The bytes of a digest as GitHub lists it, `sha256:` and 64 hex
    /// digits; nil for anything else.
    static func sha256(_ listed: String) -> Data? {
        guard listed.hasPrefix("sha256:"), let bytes = Data(hex: String(listed.dropFirst(7))), bytes.count == 32 else {
            return nil
        }
        return bytes
    }

    /// Whether `url` lies within `prefix`: the same scheme, host (in any
    /// case) and port, no user, below the prefix's path, and no `.` or `..`
    /// segment, plain or percent-encoded, that could lead out of it.
    static func within(_ url: URL, _ prefix: String) -> Bool {
        guard let given = URLComponents(url: url, resolvingAgainstBaseURL: false),
            let base = URLComponents(string: prefix),
            given.scheme?.lowercased() == base.scheme?.lowercased(),
            (given.host ?? "").lowercased() == (base.host ?? "").lowercased(),
            given.port == base.port, given.user == nil, given.password == nil
        else {
            return false
        }
        let path = given.percentEncodedPath
        let escapes = path.split(separator: "/").contains { segment in
            let lower = segment.lowercased()
            return segment == "." || segment == ".." || lower.contains("%2e") || lower.contains("%2f")
        }
        return !escapes && path.hasPrefix(base.percentEncodedPath) && path.count > base.percentEncodedPath.count
    }

    /// The parts of GitHub's release record the app reads.
    private struct Wire: Decodable {
        let tag_name: String?
        let draft: Bool?
        let prerelease: Bool?
        let html_url: String?
        let assets: [Asset]?

        struct Asset: Decodable {
            let name: String?
            let size: Int64?
            let digest: String?
            let browser_download_url: String?
        }
    }
}

extension Data {
    /// Bytes from hex digits, two per byte; nil for anything else.
    init?(hex: String) {
        let digits = Array(hex.utf8)
        guard digits.count % 2 == 0 else {
            return nil
        }
        var bytes: [UInt8] = []
        for at in stride(from: 0, to: digits.count, by: 2) {
            guard let high = Self.nibble(digits[at]), let low = Self.nibble(digits[at + 1]) else {
                return nil
            }
            bytes.append(high << 4 | low)
        }
        self.init(bytes)
    }

    private static func nibble(_ digit: UInt8) -> UInt8? {
        switch digit {
        case UInt8(ascii: "0")...UInt8(ascii: "9"): digit - UInt8(ascii: "0")
        case UInt8(ascii: "a")...UInt8(ascii: "f"): digit - UInt8(ascii: "a") + 10
        case UInt8(ascii: "A")...UInt8(ascii: "F"): digit - UInt8(ascii: "A") + 10
        default: nil
        }
    }
}
