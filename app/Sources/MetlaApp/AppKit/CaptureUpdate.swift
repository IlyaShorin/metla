import AppKit
import MetlaKit
import SwiftUI

/// The update's pictures, light and dark, in the run's language: its strip
/// in each state (in a window of the sidebar's width), the main window with
/// an offer in its sidebar, the real menu-bar panel with its line, and the
/// real Settings → General with the offer. The model is the real one; its
/// feed is a canned answer in GitHub's shape and its steps wait for the
/// capture: nothing is downloaded, nothing installed, no network used.
@MainActor
enum CaptureUpdate {
    static func run(
        _ model: AppModel, window: NSWindow, into directory: String,
        settings: @MainActor (SettingsTab) async -> NSWindow?
    ) async -> Int {
        let strings = model.strings
        let steps = HeldSteps()
        let cleaning = Flag()
        let copy = Installation(
            bundle: URL(fileURLWithPath: model.settings.root).appendingPathComponent("Metla.app"),
            identifier: "org.metla.Metla", version: Version("0.8.0")!, build: 11, block: nil)
        let update = UpdateModel(
            installation: copy, feed: CannedFeed(), steps: steps, preferences: MemoryPreferences(),
            cleanupRunning: { cleaning.on })
        await update.check(manual: true)
        var written = await strip("update-offer", update, strings, into: directory)
        cleaning.on = true
        written += await strip("update-cleanup", update, strings, into: directory)
        cleaning.on = false
        let adHoc = Installation(
            bundle: copy.bundle, identifier: copy.identifier, version: copy.version, build: copy.build, block: .adHoc)
        let linkOnly = UpdateModel(
            installation: adHoc, feed: CannedFeed(), steps: steps, preferences: MemoryPreferences(),
            cleanupRunning: { false })
        await linkOnly.check(manual: true)
        written += await strip("update-linkonly", linkOnly, strings, into: directory)

        steps.hold([.download, .verify, .replace], failing: [.replace: .cannotWrite])
        let install = Task { await update.install() }
        await steps.held(.download)
        try? await Task.sleep(for: .milliseconds(300))
        written += await strip("update-downloading", update, strings, into: directory)
        steps.release(.download)
        await steps.held(.verify)
        written += await strip("update-verifying", update, strings, into: directory)
        steps.release(.verify)
        await steps.held(.replace)
        written += await strip("update-installing", update, strings, into: directory)
        steps.release(.replace)
        await install.value
        await update.check(manual: true)
        steps.hold([], failing: [.verify: .notSigned])
        await update.install()
        written += await strip("update-failed", update, strings, into: directory)

        await update.check(manual: true)
        model.attachUpdates(update)
        written += await main(window, model, into: directory)
        let panel = await CaptureExtras.drawMenuBar(into: directory, name: "menubar-update")
        // As for the panel without an offer: where the window server gives
        // no picture of it, its view is drawn in a window of its own.
        written += panel > 0 ? panel : await CaptureExtras.draw(
            [("menubar-update", AnyView(MenuBarPanel(model: model, actions: PanelActions())))], into: directory)
        written += await CaptureExtras.drawSettings(settings, into: directory, tabs: [("general-update", .general)])
        return written
    }

    /// The strip as it is now, at the sidebar's width.
    static func strip(_ name: String, _ update: UpdateModel, _ strings: Strings, into directory: String) async -> Int {
        let view = UpdateStrip(update: update, strings: strings).frame(width: 212).padding(10)
        return await CaptureExtras.draw([(name, AnyView(view))], into: directory)
    }

    /// The main window on the Overview, the offer at the sidebar's foot.
    static func main(_ window: NSWindow, _ model: AppModel, into directory: String) async -> Int {
        window.makeKeyAndOrderFront(nil)
        var written = 0
        for (appearance, suffix) in [(NSAppearance.Name.aqua, "light"), (.darkAqua, "dark")] {
            await Capture.appear(appearance, window)
            await Capture.kept(window, on: .overview, of: model, name: "update-window-\(suffix)")
            let file = (directory as NSString).appendingPathComponent("update-window-\(suffix).png")
            if let png = await Capture.png(of: window, appearance), (try? png.write(to: URL(fileURLWithPath: file))) != nil {
                written += 1
            }
        }
        NSApp.appearance = nil
        return written
    }
}

/// GitHub's answer for a 0.8.1, without asking GitHub.
private struct CannedFeed: UpdateFeed {
    let source = UpdateSource.github

    func latest() async throws(CheckFailure) -> Data {
        let repository = "https://github.com/\(UpdateSource.repository)/releases"
        let record: [String: Any] = [
            "tag_name": "v0.8.1", "draft": false, "prerelease": false, "html_url": "\(repository)/tag/v0.8.1",
            "assets": [[
                "name": "Metla.zip", "size": 2_730_275,
                "browser_download_url": "\(repository)/download/v0.8.1/Metla.zip",
            ]],
        ]
        return (try? JSONSerialization.data(withJSONObject: record)) ?? Data()
    }
}

/// A switch the capture turns.
private final class Flag: @unchecked Sendable {
    var on = false
}

/// Steps that touch nothing: each waits where the capture says, and fails
/// where it says.
private final class HeldSteps: UpdateSteps, @unchecked Sendable {
    enum Step: Hashable, Sendable {
        case download, verify, replace
    }

    private let lock = NSLock()
    private var holds: Set<Step> = []
    private var failures: [Step: UpdateFailure] = [:]
    private var waiting: [Step: CheckedContinuation<Void, Never>] = [:]

    func hold(_ steps: Set<Step>, failing: [Step: UpdateFailure]) {
        lock.withLock {
            holds = steps
            failures = failing
        }
    }

    /// Returns once `step` waits.
    func held(_ step: Step) async {
        while lock.withLock({ waiting[step] == nil }) {
            try? await Task.sleep(for: .milliseconds(50))
        }
    }

    func release(_ step: Step) {
        lock.withLock { waiting.removeValue(forKey: step) }?.resume()
    }

    private func enter(_ step: Step) async throws(UpdateFailure) {
        if lock.withLock({ holds.contains(step) }) {
            await withCheckedContinuation { continuation in
                lock.withLock { waiting[step] = continuation }
            }
        }
        if let failure = lock.withLock({ failures[step] }) {
            throw failure
        }
    }

    func prepare(_ installation: Installation) throws(UpdateFailure) -> URL {
        installation.bundle.deletingLastPathComponent()
    }

    func download(_ release: Release, into workspace: URL, progress: @escaping @Sendable (Int64) -> Void)
        async throws(UpdateFailure) -> URL
    {
        progress(1_200_000)
        try await enter(.download)
        return workspace
    }

    func verify(_ archive: URL, release: Release, installation: Installation, in workspace: URL)
        async throws(UpdateFailure) -> URL
    {
        try await enter(.verify)
        return workspace
    }

    func replace(_ installation: Installation, with app: URL, in workspace: URL) async throws(UpdateFailure) {
        try await enter(.replace)
    }

    func discard(_ workspace: URL) {}

    @MainActor
    func relaunch(_ bundle: URL) {}
}
