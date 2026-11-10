import AppKit
import MetlaCoreFFI
import MetlaKit
import SwiftUI

/// The rest of `--capture`, after the main screens: the cleanup preview
/// (mixed groups, items set aside), the progress, a real report (with a
/// partly removed and a skipped item), the menu-bar panel, each Settings
/// tab, the first-run sheet and the hotkey's panel, light and dark, each
/// drawn in a window of its own on the fixture.
///
/// The preview and the report are the engine's own: a real preview of the
/// fixture, then a real cleanup of its quick group (the other groups left
/// unchecked, so they are skipped as not confirmed; one cache holds a
/// folder that cannot be emptied, so it is partly removed). The progress
/// view is drawn with numbers taken from that preview at its third item.
@MainActor
enum CaptureExtras {
    static func run(_ model: AppModel, into directory: String) async -> Int {
        guard let cleanup = model.cleanup, let client = model.client else {
            return 0
        }
        await model.attachHotkeys(NoRegistrar())
        let strings = model.strings
        let root = model.settings.root
        _ = try? await client.addExclusion((root as NSString).appendingPathComponent("home/Documents"))
        await model.exclusions?.load()
        await model.general?.load()
        await model.access.check()

        let selection = await previewSelection(model)
        await cleanup.open(selection)
        guard let preview = cleanup.preview else {
            return 0
        }
        var shots: [(String, AnyView)] = []
        shots.append(("preview", AnyView(PreviewView(preview: preview, strings: strings, cancel: {}, clean: {})
            .frame(width: 620, height: 720))))
        let rows = preview.groups.flatMap(\.rows)
        let progress = CleanupProgressData(
            done: 2, items: UInt64(rows.count), removed: rows.prefix(2).reduce(0) { $0 + $1.bytes },
            current: rows.dropFirst(2).first?.shownPath)
        shots.append(("progress", AnyView(ProgressPane(progress: progress, strings: strings) {}.frame(width: 620))))

        let locked = (root as NSString).appendingPathComponent("home/Library/Caches/com.example.editor/locked")
        try? FileManager.default.createDirectory(atPath: locked, withIntermediateDirectories: true)
        FileManager.default.createFile(atPath: (locked as NSString).appendingPathComponent("held"), contents: Data([1]))
        try? FileManager.default.setAttributes([.posixPermissions: 0o555], ofItemAtPath: locked)
        defer { try? FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: locked) }
        // The preview was built before the locked folder: build it again.
        await cleanup.open(selection)
        await cleanup.clean()
        guard let report = cleanup.report else {
            return 0
        }
        shots.append(("report", AnyView(ReportView(report: report, strings: strings, problemsOnly: .constant(false))
            .padding(20).frame(width: 620, height: 560))))
        _ = try? await client.addExclusion((root as NSString).appendingPathComponent("home/.codex"))
        await model.exclusions?.load()
        let hud = AppModel.hud(report, strings)
        shots.append(("hud", AnyView(HUDView(message: hud, strings: strings) {})))
        await model.refresh([.picture, .cleanup])
        shots.append(("menubar", AnyView(MenuBarPanel(model: model, actions: PanelActions()))))
        for (name, tab) in [("general", SettingsTab.general), ("hotkeys", .hotkeys),
                            ("exclusions", .exclusions), ("access", .access)] {
            shots.append(("settings-" + name, AnyView(SettingsView(model: model, tab: tab).frame(height: 520))))
        }
        shots.append(("firstrun", AnyView(FirstRunSheet(access: model.access, strings: strings))))
        return await draw(shots, into: directory)
    }

    /// Everything "Clean…" previews, a blocked worktree, a removable one
    /// and the build output inside it (which the preview sets aside).
    static func previewSelection(_ model: AppModel) async -> [DirId] {
        let root = model.settings.root
        var ids = await model.defaultSelection()
        if let client = model.client {
            let wanted = ["home/code/app-dirty", "home/code/app-clean"].map {
                (root as NSString).appendingPathComponent($0)
            }
            let page = try? await client.candidates(.gitWorktree, order: Order(key: .path, descending: false),
                                                    offset: 0, limit: 100)
            ids += (page?.listed ?? []).filter { wanted.contains($0.candidate.path) }.map(\.candidate.id)
        }
        return ids
    }

    /// Draws each view in a window of its own, light and dark.
    static func draw(_ shots: [(String, AnyView)], into directory: String) async -> Int {
        var written = 0
        for (appearance, suffix) in [(NSAppearance.Name.aqua, "light"), (.darkAqua, "dark")] {
            for (name, view) in shots {
                let window = NSWindow(
                    contentRect: NSRect(x: 80, y: 80, width: 400, height: 300),
                    styleMask: [.titled, .fullSizeContentView], backing: .buffered, defer: false)
                window.titlebarAppearsTransparent = true
                window.titleVisibility = .hidden
                window.appearance = NSAppearance(named: appearance)
                let host = NSHostingView(rootView: view.background(Color(nsColor: .windowBackgroundColor)))
                window.contentView = host
                window.setContentSize(host.fittingSize)
                window.orderFront(nil)
                try? await Task.sleep(for: .milliseconds(800))
                window.setContentSize(host.fittingSize)
                window.displayIfNeeded()
                let file = (directory as NSString).appendingPathComponent("\(name)-\(suffix).png")
                if let png = Capture.png(of: window), (try? png.write(to: URL(fileURLWithPath: file))) != nil {
                    written += 1
                }
                window.orderOut(nil)
            }
        }
        return written
    }
}
