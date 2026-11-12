import CryptoKit
import Foundation

/// The install's real steps: download into a workspace on the
/// installation's volume, check size and digest, unpack with ditto, verify
/// the signature against `requirement` and the Info.plist against the
/// running copy, swap by rename, relaunch. Nothing is replaced before all
/// of it has passed; nothing on disk is touched but the workspace and the
/// installation's own place.
public struct Installer: UpdateSteps {
    /// The requirement the new bundle must satisfy: the app passes its own
    /// designated requirement (`Signature.running()`), always.
    let requirement: String
    let transport: SessionTransport
    let headers: [String: String]
    /// The workspace for a bundle: by default a new item replacement
    /// directory for it, on its volume, so the swap is a rename.
    let workspace: @Sendable (URL) throws -> URL
    /// `rename(2)`; a test can make it fail.
    let rename: @Sendable (String, String) -> Int32
    let relaunching: @MainActor @Sendable (URL) -> Void

    public init(
        requirement: String, transport: SessionTransport, version: String,
        relaunch: @escaping @MainActor @Sendable (URL) -> Void
    ) {
        self.init(
            requirement: requirement, transport: transport, version: version, workspace: Self.replacementDirectory,
            rename: { Darwin.rename($0, $1) }, relaunch: relaunch)
    }

    init(
        requirement: String, transport: SessionTransport, version: String,
        workspace: @escaping @Sendable (URL) throws -> URL, rename: @escaping @Sendable (String, String) -> Int32,
        relaunch: @escaping @MainActor @Sendable (URL) -> Void
    ) {
        self.requirement = requirement
        self.transport = transport
        headers = ["User-Agent": "Metla/\(version)"]
        self.workspace = workspace
        self.rename = rename
        relaunching = relaunch
    }

    /// `FileManager`'s item replacement directory for `bundle`: a new
    /// folder on the bundle's volume, made by the system.
    public static func replacementDirectory(for bundle: URL) throws -> URL {
        try FileManager.default.url(for: .itemReplacementDirectory, in: .userDomainMask, appropriateFor: bundle, create: true)
    }

    public func prepare(_ installation: Installation) throws(UpdateFailure) -> URL {
        do {
            return try workspace(installation.bundle)
        } catch {
            throw .cannotWrite
        }
    }

    public func download(_ release: Release, into workspace: URL, progress: @escaping @Sendable (Int64) -> Void)
        async throws(UpdateFailure) -> URL
    {
        let archive = workspace.appendingPathComponent(UpdateSource.assetName)
        let received: Int64
        do {
            received = try await transport.download(
                release.download, to: archive, limit: release.size, headers: headers, progress: progress)
        } catch {
            switch error {
            case .offline: throw .noConnection
            case .tooLarge: throw .mismatch
            case .status, .failed: throw .downloadFailed
            }
        }
        try Self.check(archive, received: received, against: release)
        return archive
    }

    /// The download is what the feed lists: all of its declared size, not
    /// a byte more, and its SHA-256 when one is listed.
    static func check(_ archive: URL, received: Int64, against release: Release) throws(UpdateFailure) {
        guard received >= release.size else {
            throw .downloadFailed
        }
        guard received == release.size, release.digest.map({ sha256(archive) == $0 }) ?? true else {
            throw .mismatch
        }
    }

    static func sha256(_ file: URL) -> Data? {
        guard let handle = try? FileHandle(forReadingFrom: file) else {
            return nil
        }
        defer { try? handle.close() }
        var hash = SHA256()
        while let chunk = try? handle.read(upToCount: 1 << 20), !chunk.isEmpty {
            hash.update(data: chunk)
        }
        return Data(hash.finalize())
    }

    public func verify(_ archive: URL, release: Release, installation: Installation, in workspace: URL)
        async throws(UpdateFailure) -> URL
    {
        let app = try Self.unpack(archive, into: workspace.appendingPathComponent("unpacked"))
        guard Signature.satisfies(app, requirement) else {
            throw .notSigned
        }
        guard BundleInfo.read(app).accepted(offered: release.version, over: installation) else {
            throw .mismatch
        }
        return app
    }

    /// Unpacks `archive` with `ditto -x -k` into the new folder `folder`;
    /// it must hold exactly one entry, `Metla.app`, a real directory.
    static func unpack(_ archive: URL, into folder: URL) throws(UpdateFailure) -> URL {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/ditto")
        process.arguments = ["-x", "-k", archive.path, folder.path]
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        do {
            try process.run()
        } catch {
            throw .mismatch
        }
        process.waitUntilExit()
        let app = folder.appendingPathComponent("Metla.app")
        var status = stat()
        guard process.terminationStatus == 0,
            (try? FileManager.default.contentsOfDirectory(atPath: folder.path)) == ["Metla.app"],
            lstat(app.path, &status) == 0, status.st_mode & S_IFMT == S_IFDIR
        else {
            throw .mismatch
        }
        return app
    }

    public func replace(_ installation: Installation, with app: URL, in workspace: URL) async throws(UpdateFailure) {
        do throws(SwapFailure) {
            try Self.swap(app, into: installation.bundle, aside: workspace.appendingPathComponent("Metla-old.app"),
                          rename: rename)
        } catch .stranded {
            // The old bundle could not go back: it stays in the workspace.
            FileHandle.standardError.write(Data("update: the old copy could not be put back; it is at \(workspace.path)\n".utf8))
            throw .cannotWrite
        } catch {
            discard(workspace)
            throw .cannotWrite
        }
        discard(workspace)
    }

    /// Why a swap did not happen.
    enum SwapFailure: Error {
        /// Nothing changed: the old bundle is in its place.
        case refused
        /// The old bundle was set aside and could not be put back.
        case stranded
    }

    /// Renames the old bundle aside, then the new one into its place; when
    /// that fails, renames the old one back.
    static func swap(
        _ new: URL, into bundle: URL, aside: URL, rename: (String, String) -> Int32
    ) throws(SwapFailure) {
        guard rename(bundle.path, aside.path) == 0 else {
            throw .refused
        }
        guard rename(new.path, bundle.path) == 0 else {
            guard rename(aside.path, bundle.path) == 0 else {
                throw .stranded
            }
            throw .refused
        }
    }

    public func discard(_ workspace: URL) {
        try? FileManager.default.removeItem(at: workspace)
    }

    @MainActor
    public func relaunch(_ bundle: URL) {
        relaunching(bundle)
    }
}

/// Starting the app again once this process has ended.
public enum Relaunch {
    /// Starts a shell, detached from this process's output, that waits
    /// until process `pid` has ended (at most `seconds`) and then runs
    /// `opener` with `--env NAME=value` for each of `environment` and then
    /// `bundle`. The arguments are passed as arguments, never put into the
    /// script.
    @discardableResult
    public static func after(
        pid: Int32, open bundle: URL, environment: [String: String] = [:], opener: String = "/usr/bin/open",
        seconds: Int = 60
    ) throws -> Process {
        let script = """
            pid=$1 app=$2 opener=$3 tries=$(( $4 * 5 ))
            shift 4
            while /bin/kill -0 "$pid" 2>/dev/null && [ "$tries" -gt 0 ]; do
                /bin/sleep 0.2
                tries=$(( tries - 1 ))
            done
            exec "$opener" "$@" "$app"
            """
        let carried = environment.sorted { $0.key < $1.key }.flatMap { ["--env", "\($0.key)=\($0.value)"] }
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/sh")
        process.arguments = ["-c", script, "metla-relaunch", String(pid), bundle.path, opener, String(seconds)] + carried
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        try process.run()
        return process
    }

    /// What the new copy is started with: the app's own `METLA_`
    /// variables, so that a run on a fixture or a test feed goes on with
    /// them (`open` starts an app without the caller's environment). A
    /// launch from the Dock or Finder has none.
    public static func carried(_ environment: [String: String]) -> [String: String] {
        environment.filter { $0.key.hasPrefix("METLA_") }
    }
}
