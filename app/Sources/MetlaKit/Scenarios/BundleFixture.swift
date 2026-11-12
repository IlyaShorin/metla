import Foundation

/// Tiny application bundles for the update's tests and its scenario, made
/// in a folder the caller owns: a copy of `/usr/bin/true` as the
/// executable and an Info.plist, signed ad-hoc by `codesign --sign -` (no
/// keychain identity is ever used), zipped by `ditto` as releases are.
public enum BundleFixture {
    public struct ToolFailed: Error, CustomStringConvertible {
        public let description: String
    }

    /// Makes `<folder>/Metla.app` with this identifier, version and build;
    /// signed ad-hoc unless `signed` is false. The bundle.
    @discardableResult
    public static func make(
        in folder: URL, identifier: String = "org.metla.Metla", version: String, build: String, signed: Bool = true
    ) throws -> URL {
        let app = folder.appendingPathComponent("Metla.app")
        let contents = app.appendingPathComponent("Contents")
        try FileManager.default.createDirectory(
            at: contents.appendingPathComponent("MacOS"), withIntermediateDirectories: true)
        try FileManager.default.copyItem(atPath: "/usr/bin/true", toPath: contents.appendingPathComponent("MacOS/Metla").path)
        let info: [String: Any] = [
            "CFBundleExecutable": "Metla", "CFBundleIdentifier": identifier, "CFBundleName": "Metla",
            "CFBundlePackageType": "APPL", "CFBundleShortVersionString": version, "CFBundleVersion": build,
        ]
        let plist = try PropertyListSerialization.data(fromPropertyList: info, format: .xml, options: 0)
        try plist.write(to: contents.appendingPathComponent("Info.plist"))
        if signed {
            try sign(app)
        }
        return app
    }

    /// Signs `app` ad-hoc, replacing any signature it has.
    public static func sign(_ app: URL) throws {
        try run("/usr/bin/codesign", ["--force", "--sign", "-", app.path])
    }

    /// `ditto -c -k --keepParent`: the archive a release build makes.
    public static func zip(_ app: URL, to archive: URL) throws {
        try run("/usr/bin/ditto", ["-c", "-k", "--keepParent", app.path, archive.path])
    }

    /// A copy of `app` in `folder`.
    public static func copy(_ app: URL, to folder: URL) throws -> URL {
        let copy = folder.appendingPathComponent(app.lastPathComponent)
        try run("/usr/bin/ditto", [app.path, copy.path])
        return copy
    }

    static func run(_ tool: String, _ arguments: [String]) throws {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: tool)
        process.arguments = arguments
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        try process.run()
        process.waitUntilExit()
        guard process.terminationStatus == 0 else {
            throw ToolFailed(description: "\(tool) exited with \(process.terminationStatus)")
        }
    }
}
