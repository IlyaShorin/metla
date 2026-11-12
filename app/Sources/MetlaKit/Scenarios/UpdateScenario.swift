import Foundation

extension Scenarios {
    /// The update end to end on fixtures in `<root>-update`: an installed
    /// 0.8.0, a feed file beside a `Metla.zip` of 0.8.1, both signed
    /// ad-hoc. What runs for real: the feed read from a file URL by the
    /// app's transport, the download, its size and SHA-256, ditto, the
    /// signature check, the Info.plist check and the swap by rename, in the
    /// system's replacement directory as the app makes it. What is only
    /// observed: the relaunch, recorded as a request (this process does not
    /// end). Two stand-ins, said in the output: the installed copy is
    /// treated as one that may replace itself (the real eligibility of an
    /// ad-hoc copy is checked first, and it says no), and the requirement
    /// is the new build's own, as a certificate's would be both builds'.
    /// Then a tampered 0.8.2 is refused and the copy stays as it was.
    static func update(_ model: AppModel, _ check: inout Check) async {
        let place = URL(fileURLWithPath: model.settings.root + "-update")
        try? FileManager.default.removeItem(at: place)
        do {
            let installed = try BundleFixture.make(in: folder(place, "apps"), version: "0.8.0", build: "11")
            let real = Installation.read(installed, adHoc: Signature.of(installed).adHoc)
            check.expect(real?.block == .adHoc, "an ad-hoc copy is offered the release page only")
            let copy = Installation(
                bundle: installed, identifier: "org.metla.Metla", version: Version("0.8.0")!, build: 11, block: nil)
            let good = try release(in: folder(place, "feed"), version: "0.8.1", build: "12", tampered: false)
            let relaunched = Relaunches()
            let steps = Steps(Installer(
                requirement: good.requirement, transport: SessionTransport(), version: "0.8.0",
                relaunch: { relaunched.add($0) }))
            let feed = FeedClient(source: good.source, transport: SessionTransport(), version: "0.8.0")
            let update = UpdateModel(
                installation: copy, feed: feed, steps: steps, preferences: MemoryPreferences(),
                cleanupRunning: { model.cleanup?.running ?? false })
            await update.check(manual: true)
            check.expect(update.offer?.release.version == Version("0.8.1"), "the feed offers 0.8.1")
            check.expect(update.offer?.release.notes != nil, "with a page of its own")
            await update.install()
            check.expect(update.offer?.phase == .installing, "installed: \(String(describing: update.offer?.phase))")
            check.expect(relaunched.bundles == [installed], "a relaunch of the installed path is requested once")
            check.expect(BundleInfo.read(installed) == BundleInfo(identifier: "org.metla.Metla", version: "0.8.1", build: "12"),
                         "the installed path holds 0.8.1 (12)")
            check.expect(Signature.satisfies(installed, good.requirement), "and its signature holds")
            check.expect(steps.workspaces.count == 1 && steps.workspaces.allSatisfy(gone), "the workspace is removed")
            check.expect(!left(place), "no old copy is left beside it")
            Swift.print("scenario update: ran: feed and download from file URLs (URLSession), size and SHA-256, ditto, "
                + "signature, Info.plist, swap by rename in \(steps.workspaces.first?.path ?? "-") (removed)")
            Swift.print("scenario update: observed: relaunch requested for \(relaunched.bundles.map(\.path))")

            let bad = try release(in: folder(place, "feed-tampered"), version: "0.8.2", build: "13", tampered: true)
            let newer = Installation(
                bundle: installed, identifier: "org.metla.Metla", version: Version("0.8.1")!, build: 12, block: nil)
            let refused = UpdateModel(
                installation: newer, feed: FeedClient(source: bad.source, transport: SessionTransport(), version: "0.8.1"),
                steps: steps, preferences: MemoryPreferences(), cleanupRunning: { false })
            await refused.check(manual: true)
            await refused.install()
            check.expect(refused.offer?.phase == .failed(.notSigned), "a tampered build is refused as not signed")
            check.expect(BundleInfo.read(installed).version == "0.8.1" && Signature.satisfies(installed, good.requirement),
                         "the copy stays as it was")
            check.expect(steps.workspaces.count == 2 && steps.workspaces.allSatisfy(gone), "that workspace is removed too")
            check.expect(relaunched.bundles.count == 1, "no relaunch after a refusal")
            if check.failures.isEmpty {
                try FileManager.default.removeItem(at: place)
            }
        } catch {
            check.fail("the fixtures could not be made: \(error)")
        }
    }

    /// A release in `folder`: a bundle with a file of its own, signed
    /// ad-hoc (then changed, if `tampered`), zipped, and a feed beside it
    /// in GitHub's shape. Its source and the bundle's own requirement.
    private static func release(in folder: URL, version: String, build: String, tampered: Bool) throws
        -> (source: UpdateSource, requirement: String)
    {
        let app = try BundleFixture.make(in: folder.appendingPathComponent("build"), version: version, build: build, signed: false)
        let resources = app.appendingPathComponent("Contents/Resources")
        try FileManager.default.createDirectory(at: resources, withIntermediateDirectories: true)
        try Data(version.utf8).write(to: resources.appendingPathComponent("version"))
        try BundleFixture.sign(app)
        let requirement = Signature.of(app).requirement ?? ""
        if tampered {
            try Data("changed after signing".utf8).write(to: resources.appendingPathComponent("version"))
        }
        let archive = folder.appendingPathComponent(UpdateSource.assetName)
        try BundleFixture.zip(app, to: archive)
        try FileManager.default.removeItem(at: folder.appendingPathComponent("build"))
        let bytes = try Data(contentsOf: archive)
        let digest = Installer.sha256(archive).map { $0.map { String(format: "%02x", $0) }.joined() } ?? ""
        let record: [String: Any] = [
            "tag_name": "v\(version)", "draft": false, "prerelease": false, "name": "Metla \(version)",
            "html_url": folder.appendingPathComponent("notes.html").absoluteString,
            "body": "Never shown.",
            "assets": [[
                "name": UpdateSource.assetName, "size": bytes.count, "digest": "sha256:\(digest)",
                "browser_download_url": archive.absoluteString,
            ]],
        ]
        let feed = folder.appendingPathComponent("latest.json")
        try JSONSerialization.data(withJSONObject: record).write(to: feed)
        return (UpdateSource.test(feed)!, requirement)
    }

    private static func folder(_ place: URL, _ name: String) throws -> URL {
        let folder = place.appendingPathComponent(name)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        return folder
    }

    private static func gone(_ url: URL) -> Bool {
        !FileManager.default.fileExists(atPath: url.path)
    }

    /// Whether a set-aside copy is anywhere below `place`.
    private static func left(_ place: URL) -> Bool {
        (FileManager.default.subpaths(atPath: place.path) ?? []).contains { $0.contains("Metla-old") }
    }

    /// The relaunches asked for.
    final class Relaunches: @unchecked Sendable {
        private let lock = NSLock()
        private var asked: [URL] = []

        func add(_ bundle: URL) {
            lock.withLock { asked.append(bundle) }
        }

        var bundles: [URL] {
            lock.withLock { asked }
        }
    }

    /// The installer's steps, with the workspaces it made written down.
    final class Steps: UpdateSteps, @unchecked Sendable {
        private let installer: Installer
        private let lock = NSLock()
        private var made: [URL] = []

        init(_ installer: Installer) {
            self.installer = installer
        }

        var workspaces: [URL] {
            lock.withLock { made }
        }

        func prepare(_ installation: Installation) throws(UpdateFailure) -> URL {
            let workspace = try installer.prepare(installation)
            lock.withLock { made.append(workspace) }
            return workspace
        }

        func download(_ release: Release, into workspace: URL, progress: @escaping @Sendable (Int64) -> Void)
            async throws(UpdateFailure) -> URL
        {
            try await installer.download(release, into: workspace, progress: progress)
        }

        func verify(_ archive: URL, release: Release, installation: Installation, in workspace: URL)
            async throws(UpdateFailure) -> URL
        {
            try await installer.verify(archive, release: release, installation: installation, in: workspace)
        }

        func replace(_ installation: Installation, with app: URL, in workspace: URL) async throws(UpdateFailure) {
            try await installer.replace(installation, with: app, in: workspace)
        }

        func discard(_ workspace: URL) {
            installer.discard(workspace)
        }

        @MainActor
        func relaunch(_ bundle: URL) {
            installer.relaunch(bundle)
        }
    }
}
