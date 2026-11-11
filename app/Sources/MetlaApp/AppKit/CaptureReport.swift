import AppKit
import MetlaCoreFFI
import MetlaKit

/// The main window on Last report with a report longer than any screen,
/// light and dark. The window must keep its frame whatever the detail
/// shows: a capture that sees it change says so and fails
/// (`Capture.failures`), so the check needs no eyes.
@MainActor
enum CaptureReport {
    /// More report rows than fit on any screen.
    static let items = 3000

    static func run(_ model: AppModel, window: NSWindow, into directory: String) async -> Int {
        guard await longReport(model) else {
            Capture.fail("no report of \(items) items to show")
            return 0
        }
        window.makeKeyAndOrderFront(nil)
        var written = 0
        for (appearance, suffix) in [(NSAppearance.Name.aqua, "light"), (.darkAqua, "dark")] {
            await Capture.appear(appearance, window)
            let file = (directory as NSString).appendingPathComponent("lastreport-\(suffix).png")
            await Capture.kept(window, on: .overview, of: model, name: "overview-before-lastreport-\(suffix)")
            await Capture.kept(window, on: .report, of: model, name: "lastreport-\(suffix)")
            if let png = await Capture.png(of: window, appearance), (try? png.write(to: URL(fileURLWithPath: file))) != nil {
                written += 1
            }
        }
        NSApp.appearance = nil
        return written
    }

    /// `items` old app caches in the fixture, scanned and cleaned at once:
    /// the last report then has at least that many rows.
    private static func longReport(_ model: AppModel) async -> Bool {
        guard let client = model.client, let cleanup = model.cleanup else {
            return false
        }
        let caches = (model.settings.root as NSString).appendingPathComponent("home/Library/Caches")
        let old: [FileAttributeKey: Any] = [.modificationDate: Date(timeIntervalSince1970: 1_600_000_000)]
        for i in 0..<items {
            let dir = (caches as NSString).appendingPathComponent("org.capture.long\(i)")
            let blob = (dir as NSString).appendingPathComponent("blob")
            try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
            FileManager.default.createFile(atPath: blob, contents: Data([1]))
            try? FileManager.default.setAttributes(old, ofItemAtPath: blob)
            try? FileManager.default.setAttributes(old, ofItemAtPath: dir)
        }
        await model.rescan()
        let order = Order(key: .path, descending: false)
        let deadline = Date().addingTimeInterval(120)
        while Date() < deadline {
            try? await Task.sleep(for: .milliseconds(300))
            let total = (try? await client.candidates(.appCache, order: order, offset: 0, limit: 1))?.total ?? 0
            let idle = if case .idle? = (try? await client.freshness())?.state { true } else { false }
            if total >= UInt64(items), idle {
                break
            }
        }
        await cleanup.quickClean([.appCache])
        cleanup.close()
        await model.refresh([.picture, .cleanup])
        return (model.lastReport?.rows(problemsOnly: false).count ?? 0) >= items
    }
}
