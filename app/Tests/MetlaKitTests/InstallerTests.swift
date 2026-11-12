import CryptoKit
import Foundation
import Testing
@testable import MetlaKit

/// The installer's real steps on bundles, archives and folders made in a
/// sandbox of the test's own: nothing outside it is read for writing.
@Suite struct InstallerTests {
    static let running = Installation(
        bundle: URL(fileURLWithPath: "/sandbox/apps/Metla.app"), identifier: "org.metla.Metla",
        version: Version("0.8.0")!, build: 11, block: nil)

    /// An installer whose workspace is a folder of the sandbox.
    static func installer(_ box: Sandbox, requirement: String, rename: @escaping @Sendable (String, String) -> Int32 = {
        Darwin.rename($0, $1)
    }) -> Installer {
        Installer(
            requirement: requirement, transport: SessionTransport(), version: "0.8.0",
            workspace: { _ in try box.folder("workspace") }, rename: rename, relaunch: { _ in })
    }

    /// A release of `archive` as a test feed beside it lists it.
    static func release(_ archive: URL, size: Int64? = nil, digest: Data?? = .none, version: String = "0.8.1") throws
        -> Release
    {
        let bytes = try Data(contentsOf: archive)
        let listed = digest ?? Data(SHA256.hash(data: bytes))
        return Release(
            version: Version(version)!, download: archive, size: size ?? Int64(bytes.count), digest: listed,
            notes: nil, page: archive.deletingLastPathComponent())
    }

    @Test func theDownloadMustBeAllOfTheDeclaredSizeAndItsDigest() throws {
        let box = Sandbox()
        defer { box.remove() }
        let file = box("Metla.zip")
        try Data(count: 100).write(to: file)
        let release = try Self.release(file)
        try Installer.check(file, received: 100, against: release)
        try Installer.check(file, received: 100, against: Self.release(file, digest: .some(nil)))
        #expect(throws: UpdateFailure.downloadFailed, "cut short") {
            try Installer.check(file, received: 99, against: release)
        }
        #expect(throws: UpdateFailure.mismatch, "longer than declared") {
            try Installer.check(file, received: 101, against: release)
        }
        #expect(throws: UpdateFailure.mismatch, "another digest") {
            try Installer.check(file, received: 100, against: Self.release(file, digest: .some(Data(count: 32))))
        }
    }

    @Test func aDownloadLandsInTheWorkspaceOrSaysWhyNot() async throws {
        let box = Sandbox()
        defer { box.remove() }
        let feed = try box.folder("feed")
        let app = try BundleFixture.make(in: box.folder("new"), version: "0.8.1", build: "12")
        try BundleFixture.zip(app, to: feed.appendingPathComponent("Metla.zip"))
        let archive = feed.appendingPathComponent("Metla.zip")
        let installer = Self.installer(box, requirement: "identifier \"org.metla.Metla\"")
        let workspace = try installer.prepare(Self.running)
        let seen = Seen()
        let got = try await installer.download(Self.release(archive), into: workspace, progress: { seen.add($0) })
        #expect(got == workspace.appendingPathComponent("Metla.zip"))
        let size = Int64(try Data(contentsOf: archive).count)
        #expect(try Data(contentsOf: got) == Data(contentsOf: archive))
        #expect(seen.last == size)
        let cases: [(Release, UpdateFailure)] = [
            (try Self.release(archive, digest: .some(Data(count: 32))), .mismatch),
            (try Self.release(archive, size: size + 1), .downloadFailed),
            (try Self.release(archive, size: size - 1), .mismatch),
            (try Self.release(archive).moved(to: feed.appendingPathComponent("missing.zip")), .downloadFailed),
        ]
        for (release, failure) in cases {
            try? FileManager.default.removeItem(at: workspace.appendingPathComponent("Metla.zip"))
            await #expect(throws: failure, "\(release)") {
                try await installer.download(release, into: workspace, progress: { _ in })
            }
        }
        #expect(throws: UpdateFailure.cannotWrite) {
            try Installer(
                requirement: "", transport: SessionTransport(), version: "0.8.0",
                workspace: { _ in throw CocoaError(.fileWriteNoPermission) }, rename: { _, _ in 0 }, relaunch: { _ in }
            ).prepare(Self.running)
        }
    }

    @Test func theArchiveMustHoldExactlyOneMetlaApp() throws {
        let box = Sandbox()
        defer { box.remove() }
        let app = try BundleFixture.make(in: box.folder("one"), version: "0.8.1", build: "12")
        try BundleFixture.zip(app, to: box("one.zip"))
        let unpacked = try Installer.unpack(box("one.zip"), into: box("out-one"))
        #expect(unpacked == box("out-one/Metla.app") && Signature.satisfies(unpacked, "identifier \"org.metla.Metla\""))

        let two = try box.folder("two")
        try BundleFixture.make(in: two.appendingPathComponent("Metla.app/.."), version: "0.8.1", build: "12")
        try Data("x".utf8).write(to: two.appendingPathComponent("README"))
        try BundleFixture.run("/usr/bin/ditto", ["-c", "-k", two.path, box("two.zip").path])
        let renamed = try box.folder("renamed")
        try BundleFixture.make(in: renamed, version: "0.8.1", build: "12")
        try FileManager.default.moveItem(at: renamed.appendingPathComponent("Metla.app"), to: renamed.appendingPathComponent("Other.app"))
        try BundleFixture.zip(renamed.appendingPathComponent("Other.app"), to: box("renamed.zip"))
        let link = try box.folder("link")
        try FileManager.default.createSymbolicLink(atPath: link.appendingPathComponent("Metla.app").path, withDestinationPath: app.path)
        try BundleFixture.run("/usr/bin/ditto", ["-c", "-k", link.path, box("link.zip").path])
        try Data("not a zip".utf8).write(to: box("text.zip"))
        for name in ["two", "renamed", "link", "text", "missing"] {
            #expect(throws: UpdateFailure.mismatch, "\(name)") {
                try Installer.unpack(box("\(name).zip"), into: box("out-\(name)"))
            }
        }
    }

    /// The signature against the requirement first, then the Info.plist
    /// against the running copy.
    @Test func onlyABundleOfTheSameSignerAndNewerIsAccepted() async throws {
        let box = Sandbox()
        defer { box.remove() }
        func archive(_ name: String, version: String = "0.8.1", build: String = "12", identifier: String = "org.metla.Metla",
                     tamper: Bool = false) throws -> URL {
            // A file of its own in each, so that no two are the same code.
            let app = try BundleFixture.make(
                in: box.folder(name), identifier: identifier, version: version, build: build, signed: false)
            let resources = app.appendingPathComponent("Contents/Resources")
            try FileManager.default.createDirectory(at: resources, withIntermediateDirectories: true)
            try Data(name.utf8).write(to: resources.appendingPathComponent("name"))
            try BundleFixture.sign(app)
            if tamper {
                try Data("x".utf8).append(to: app.appendingPathComponent("Contents/MacOS/Metla"))
            }
            try BundleFixture.zip(app, to: box("\(name).zip"))
            return box("\(name).zip")
        }
        let good = try archive("good")
        let signer = "identifier \"org.metla.Metla\""
        let installer = Self.installer(box, requirement: signer)
        let workspace = try box.folder("verify-good")
        let app = try await installer.verify(good, release: Self.release(good), installation: Self.running, in: workspace)
        #expect(app == workspace.appendingPathComponent("unpacked/Metla.app"))

        let own = try #require(Signature.of(app).requirement)
        let cases: [(String, URL, String, UpdateFailure)] = [
            ("another signer", try archive("other"), own, .notSigned),
            ("tampered", try archive("tampered", tamper: true), signer, .notSigned),
            ("another app", try archive("id", identifier: "org.metla.Other"), "identifier \"org.metla.Other\"", .mismatch),
            ("not the version offered", try archive("version", version: "0.8.2"), signer, .mismatch),
            ("the same build", try archive("build", build: "11"), signer, .mismatch),
        ]
        for (name, zip, requirement, failure) in cases {
            let installer = Self.installer(box, requirement: requirement)
            await #expect(throws: failure, "\(name)") {
                try await installer.verify(zip, release: Self.release(zip), installation: Self.running,
                                           in: box.folder("verify-\(name)"))
            }
        }
    }

    /// The swap: the old bundle aside, the new one in its place, the
    /// workspace gone, and nothing else in the sandbox changed.
    @Test func theSwapPutsTheNewBundleInPlaceAndRemovesTheWorkspace() async throws {
        let box = Sandbox()
        defer { box.remove() }
        try Data("keep".utf8).write(to: box.folder("outside").appendingPathComponent("file"))
        let old = try BundleFixture.make(in: box.folder("apps"), version: "0.8.0", build: "11")
        let installation = try #require(Installation.read(old, adHoc: false))
        let installer = Self.installer(box, requirement: "")
        let workspace = try installer.prepare(installation)
        let new = try BundleFixture.make(in: workspace.appendingPathComponent("unpacked"), version: "0.8.1", build: "12")
        let before = box.listing().filter { !$0.hasPrefix("workspace") }
        try await installer.replace(installation, with: new, in: workspace)
        #expect(BundleInfo.read(old).version == "0.8.1", "the new bundle at the old path")
        #expect(!FileManager.default.fileExists(atPath: workspace.path), "the workspace is gone")
        #expect(box.listing() == before, "nothing else changed: \(box.listing())")
        #expect(try String(contentsOf: box("outside/file"), encoding: .utf8) == "keep")
    }

    @Test func aFailedSecondRenamePutsTheOldBundleBack() async throws {
        let box = Sandbox()
        defer { box.remove() }
        let old = try BundleFixture.make(in: box.folder("apps"), version: "0.8.0", build: "11")
        let installation = try #require(Installation.read(old, adHoc: false))
        let renames = Counter()
        let installer = Self.installer(box, requirement: "") { from, to in
            renames.next() == 2 ? -1 : Darwin.rename(from, to)
        }
        let workspace = try installer.prepare(installation)
        let new = try BundleFixture.make(in: workspace.appendingPathComponent("unpacked"), version: "0.8.1", build: "12")
        await #expect(throws: UpdateFailure.cannotWrite) {
            try await installer.replace(installation, with: new, in: workspace)
        }
        #expect(renames.count == 3, "aside, in place (failed), back")
        let requirement = try #require(Signature.of(old).requirement)
        #expect(BundleInfo.read(old).version == "0.8.0" && Signature.satisfies(old, requirement))
        #expect(!FileManager.default.fileExists(atPath: workspace.path), "the workspace is gone")
        #expect(box.listing().allSatisfy { !$0.contains("Metla-old") })
    }

    @Test func anOldBundleThatCannotGoBackStaysInTheWorkspace() async throws {
        let box = Sandbox()
        defer { box.remove() }
        let old = try BundleFixture.make(in: box.folder("apps"), version: "0.8.0", build: "11")
        let installation = try #require(Installation.read(old, adHoc: false))
        let renames = Counter()
        let installer = Self.installer(box, requirement: "") { from, to in
            renames.next() >= 2 ? -1 : Darwin.rename(from, to)
        }
        let workspace = try installer.prepare(installation)
        let new = try BundleFixture.make(in: workspace.appendingPathComponent("unpacked"), version: "0.8.1", build: "12")
        await #expect(throws: UpdateFailure.cannotWrite) {
            try await installer.replace(installation, with: new, in: workspace)
        }
        #expect(BundleInfo.read(workspace.appendingPathComponent("Metla-old.app")).version == "0.8.0", "kept, not removed")
    }

    /// A folder the user cannot write: the first rename is refused, and
    /// nothing moves.
    @Test func aFolderThatCannotBeWrittenRefusesTheSwap() async throws {
        let box = Sandbox()
        defer { box.remove() }
        let apps = try box.folder("apps")
        let old = try BundleFixture.make(in: apps, version: "0.8.0", build: "11")
        let installation = try #require(Installation.read(old, adHoc: false))
        let installer = Self.installer(box, requirement: "")
        let workspace = try installer.prepare(installation)
        let new = try BundleFixture.make(in: workspace.appendingPathComponent("unpacked"), version: "0.8.1", build: "12")
        try FileManager.default.setAttributes([.posixPermissions: 0o555], ofItemAtPath: apps.path)
        await #expect(throws: UpdateFailure.cannotWrite) {
            try await installer.replace(installation, with: new, in: workspace)
        }
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: apps.path)
        #expect(BundleInfo.read(old).version == "0.8.0" && !FileManager.default.fileExists(atPath: workspace.path))
    }

    /// The system's replacement directory is on the bundle's volume (so
    /// the swap is a rename), and discarding it removes it.
    @Test func theWorkspaceIsOnTheBundlesVolume() throws {
        let box = Sandbox()
        defer { box.remove() }
        let old = try BundleFixture.make(in: box.folder("apps"), version: "0.8.0", build: "11", signed: false)
        let workspace = try Installer.replacementDirectory(for: old)
        let installer = Installer(requirement: "", transport: SessionTransport(), version: "0.8.0", relaunch: { _ in })
        defer { installer.discard(workspace) }
        let volume = { (url: URL) in try url.resourceValues(forKeys: [.volumeIdentifierKey]).volumeIdentifier as? NSObject }
        #expect(try volume(workspace) == volume(old))
        #expect(!workspace.path.hasPrefix(box.url.path), "a folder of the system's, not of the bundle's")
        installer.discard(workspace)
        #expect(!FileManager.default.fileExists(atPath: workspace.path))
    }

    /// The relaunch waits for the old process to end, then opens the
    /// bundle (here `touch` stands in for `open`).
    @Test func theRelaunchWaitsForTheProcessToEnd() async throws {
        let box = Sandbox()
        defer { box.remove() }
        let sleeper = Process()
        sleeper.executableURL = URL(fileURLWithPath: "/bin/sleep")
        sleeper.arguments = ["1"]
        try sleeper.run()
        let marker = box("opened with spaces")
        let helper = try Relaunch.after(pid: sleeper.processIdentifier, open: marker, opener: "/usr/bin/touch")
        try await Task.sleep(for: .milliseconds(400))
        #expect(!FileManager.default.fileExists(atPath: marker.path), "not while the process runs")
        sleeper.waitUntilExit()
        helper.waitUntilExit()
        #expect(FileManager.default.fileExists(atPath: marker.path) && helper.terminationStatus == 0)
        let gone = try Relaunch.after(pid: sleeper.processIdentifier, open: box("second"), opener: "/usr/bin/touch")
        gone.waitUntilExit()
        #expect(FileManager.default.fileExists(atPath: box("second").path), "at once for a process that has ended")
    }
}

extension Release {
    /// The same release, downloaded from `url`.
    func moved(to url: URL) -> Release {
        Release(version: version, download: url, size: size, digest: digest, notes: notes, page: page)
    }
}

/// Counts calls from any thread.
final class Counter: @unchecked Sendable {
    private let lock = NSLock()
    private var value = 0

    func next() -> Int {
        lock.withLock {
            value += 1
            return value
        }
    }

    var count: Int {
        lock.withLock { value }
    }
}
