import Foundation
import Testing
@testable import MetlaKit

/// A folder of a test's own below `$TMPDIR`, removed with everything in it
/// by `remove()` (write permission given back first).
struct Sandbox {
    let url: URL

    init() {
        let dir = Fixture.temporary.appendingPathComponent("metla-update-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        url = dir.resolvingSymlinksInPath()
    }

    func callAsFunction(_ relative: String) -> URL {
        url.appendingPathComponent(relative)
    }

    func folder(_ relative: String) throws -> URL {
        let dir = url.appendingPathComponent(relative)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }

    func remove() {
        let files = FileManager.default
        if let all = files.enumerator(atPath: url.path) {
            for case let relative as String in all {
                try? files.setAttributes([.posixPermissions: 0o755], ofItemAtPath: url.appendingPathComponent(relative).path)
            }
        }
        try? files.setAttributes([.posixPermissions: 0o755], ofItemAtPath: url.path)
        try? files.removeItem(at: url)
    }

    /// Every path below the sandbox, relative to it, sorted.
    func listing() -> [String] {
        (FileManager.default.subpaths(atPath: url.path) ?? []).sorted()
    }
}

/// Real signatures on bundles made here and signed ad-hoc (`codesign -s -`,
/// no keychain identity).
@Suite struct SignatureTests {
    @Test func aBundlePassesItsOwnDesignatedRequirementAndATamperedCopyFails() throws {
        let box = Sandbox()
        defer { box.remove() }
        let app = try BundleFixture.make(in: box.folder("a"), version: "0.8.0", build: "11")
        let own = Signature.of(app)
        let requirement = try #require(own.requirement)
        #expect(own.adHoc)
        #expect(requirement.hasPrefix("cdhash H\""), "an ad-hoc signature's requirement is its own hash")
        #expect(Signature.satisfies(app, requirement))

        let plist = try BundleFixture.copy(app, to: box.folder("plist"))
        try Data("<!-- changed -->".utf8).append(to: plist.appendingPathComponent("Contents/Info.plist"))
        let executable = try BundleFixture.copy(app, to: box.folder("executable"))
        try Data([0]).append(to: executable.appendingPathComponent("Contents/MacOS/Metla"))
        let added = try BundleFixture.copy(app, to: box.folder("added"))
        try Data("x".utf8).write(to: added.appendingPathComponent("Contents/Resources-extra"))
        for tampered in [plist, executable, added] {
            #expect(!Signature.satisfies(tampered, requirement), "\(tampered.path)")
            #expect(!Signature.satisfies(tampered, "identifier \"org.metla.Metla\""), "\(tampered.path)")
        }
    }

    @Test func anotherBundleFailsTheFirstOnesRequirement() throws {
        let box = Sandbox()
        defer { box.remove() }
        let first = try BundleFixture.make(in: box.folder("a"), version: "0.8.0", build: "11")
        let second = try BundleFixture.make(in: box.folder("b"), version: "0.8.1", build: "12")
        let requirement = try #require(Signature.of(first).requirement)
        #expect(!Signature.satisfies(second, requirement))
        #expect(Signature.satisfies(second, try #require(Signature.of(second).requirement)))
    }

    @Test func identifierRequirementsPassAndFailAsTheyShould() throws {
        let box = Sandbox()
        defer { box.remove() }
        let app = try BundleFixture.make(in: box.folder("a"), version: "0.8.0", build: "11")
        #expect(Signature.satisfies(app, "identifier \"org.metla.Metla\""))
        #expect(!Signature.satisfies(app, "identifier \"org.metla.Other\""))
        #expect(!Signature.satisfies(app, "identifier \"org.metla.Metla\" and anchor apple generic"),
                "an ad-hoc signature has no certificate")
        #expect(!Signature.satisfies(app, "this is not a requirement"))
        #expect(!Signature.satisfies(box("missing.app"), "identifier \"org.metla.Metla\""))
    }

    @Test func anUnsealedBundleFails() throws {
        let box = Sandbox()
        defer { box.remove() }
        let app = try BundleFixture.make(in: box.folder("a"), version: "0.8.0", build: "11", signed: false)
        #expect(!Signature.satisfies(app, "identifier \"org.metla.Metla\""))
        #expect(!Signature.satisfies(app, "anchor apple"), "the executable's own signature does not seal the bundle")
    }

    /// A certificate's signature (Apple's, on a system tool, only read) is
    /// not ad-hoc, and its requirement names the certificate's anchor.
    @Test func aCertificateSignatureIsNotAdHoc() throws {
        let tool = Signature.of(URL(fileURLWithPath: "/usr/bin/true"))
        #expect(!tool.adHoc)
        #expect(try #require(tool.requirement).contains("anchor apple"))
        #expect(Signature.of(URL(fileURLWithPath: "/nonexistent/Metla.app")) == Signature.Own(requirement: nil, adHoc: true))
        #expect(Signature.running().requirement != nil, "the test runner's own")
    }
}

/// The running copy: what the update compares with, and whether it can
/// replace itself.
@Suite struct InstallationTests {
    @Test func theCopyIsReadFromItsInfoPlist() throws {
        let box = Sandbox()
        defer { box.remove() }
        let app = try BundleFixture.make(in: box.folder("a"), version: "0.8.0", build: "11")
        let copy = try #require(Installation.read(app, adHoc: Signature.of(app).adHoc))
        #expect(copy.bundle == app && copy.identifier == "org.metla.Metla")
        #expect(copy.version == Version("0.8.0") && copy.build == 11)
        #expect(copy.block == .adHoc, "a copy signed ad-hoc can only point to the release page")
        #expect(Installation.read(app, adHoc: false)?.block == nil, "writable, in place, not ad-hoc")
        let development = try BundleFixture.make(in: box.folder("d"), version: "development", build: "11", signed: false)
        #expect(Installation.read(development, adHoc: false) == nil, "no version to compare")
        let fraction = try BundleFixture.make(in: box.folder("f"), version: "0.8.0", build: "11.1", signed: false)
        #expect(Installation.read(fraction, adHoc: false) == nil, "a build that is not an integer")
        #expect(Installation.read(box("missing.app"), adHoc: false) == nil)
    }

    @Test func aCopyThatCannotBeWrittenOnlyLinksToTheReleasePage() throws {
        let box = Sandbox()
        defer { box.remove() }
        let parent = try box.folder("locked-parent")
        let app = try BundleFixture.make(in: parent, version: "0.8.0", build: "11")
        try FileManager.default.setAttributes([.posixPermissions: 0o555], ofItemAtPath: parent.path)
        #expect(Installation.read(app, adHoc: false)?.block == .notWritable, "its folder")
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: parent.path)
        try FileManager.default.setAttributes([.posixPermissions: 0o555], ofItemAtPath: app.path)
        #expect(Installation.read(app, adHoc: false)?.block == .notWritable, "the bundle itself")
    }

    @Test func theReasonsComeInOneOrder() {
        let here = "/Applications/Metla.app"
        let moved = "/private/var/folders/xy/abc/T/AppTranslocation/1234-5678/d/Metla.app"
        #expect(InstallBlock.of(adHoc: false, path: here, readOnlyVolume: false, writable: true) == nil)
        #expect(InstallBlock.of(adHoc: true, path: moved, readOnlyVolume: true, writable: false) == .adHoc)
        #expect(InstallBlock.of(adHoc: false, path: moved, readOnlyVolume: true, writable: false) == .translocated)
        #expect(InstallBlock.of(adHoc: false, path: here, readOnlyVolume: true, writable: false) == .readOnlyVolume)
        #expect(InstallBlock.of(adHoc: false, path: here, readOnlyVolume: false, writable: false) == .notWritable)
    }

    /// The new bundle's Info.plist: the same app, the version offered, and
    /// a higher version and build than the running copy's.
    @Test func theNewBundleIsTheSameAppAndNewer() {
        let running = Installation(
            bundle: URL(fileURLWithPath: "/x/Metla.app"), identifier: "org.metla.Metla", version: Version("0.8.0")!,
            build: 11, block: nil)
        let offered = Version("0.8.1")!
        let good = BundleInfo(identifier: "org.metla.Metla", version: "0.8.1", build: "12")
        #expect(good.accepted(offered: offered, over: running))
        let refused: [(String, BundleInfo)] = [
            ("another app", BundleInfo(identifier: "org.metla.Other", version: "0.8.1", build: "12")),
            ("no identifier", BundleInfo(identifier: nil, version: "0.8.1", build: "12")),
            ("not the version offered", BundleInfo(identifier: "org.metla.Metla", version: "0.8.2", build: "12")),
            ("no version", BundleInfo(identifier: "org.metla.Metla", version: nil, build: "12")),
            ("the same build", BundleInfo(identifier: "org.metla.Metla", version: "0.8.1", build: "11")),
            ("a lower build", BundleInfo(identifier: "org.metla.Metla", version: "0.8.1", build: "10")),
            ("a build with letters", BundleInfo(identifier: "org.metla.Metla", version: "0.8.1", build: "12a")),
            ("a build with a fraction", BundleInfo(identifier: "org.metla.Metla", version: "0.8.1", build: "12.0")),
            ("a build with a sign", BundleInfo(identifier: "org.metla.Metla", version: "0.8.1", build: "+12")),
            ("no build", BundleInfo(identifier: "org.metla.Metla", version: "0.8.1", build: nil)),
        ]
        for (name, info) in refused {
            #expect(!info.accepted(offered: offered, over: running), "\(name)")
        }
        let sidegrade = BundleInfo(identifier: "org.metla.Metla", version: "0.8.0", build: "12")
        #expect(!sidegrade.accepted(offered: Version("0.8.0")!, over: running), "not greater than the running version")
    }
}

extension Data {
    /// Appends these bytes to the file at `url`.
    func append(to url: URL) throws {
        let handle = try FileHandle(forWritingTo: url)
        defer { try? handle.close() }
        try handle.seekToEnd()
        try handle.write(contentsOf: self)
    }
}
