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
    static func run(
        _ model: AppModel, into directory: String, settings: @MainActor (SettingsTab) async -> NSWindow?
    ) async -> Int {
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
        let realHUD = await drawHUD(hud, strings: strings, into: directory)
        if realHUD == 0 {
            shots.append(("hud", AnyView(HUDView(message: hud, strings: strings) {})))
        }
        await model.refresh([.picture, .cleanup])
        let realPanel = await drawMenuBar(into: directory)
        if realPanel == 0 {
            shots.append(("menubar", AnyView(MenuBarPanel(model: model, actions: PanelActions()))))
        }
        shots.append(("firstrun", AnyView(FirstRunSheet(access: model.access, strings: strings))))
        return realHUD + realPanel + (await draw(shots, into: directory))
            + (await drawSettings(settings, into: directory))
    }

    /// The hotkey's real panel (`HUD`, a HUD-style panel that is dark in
    /// either appearance), shown as the hotkey shows it, in the light and
    /// the dark appearance of the app. 0 when no panel came up.
    static func drawHUD(_ message: HUDMessage, strings: Strings, into directory: String) async -> Int {
        let hud = HUD()
        var written = 0
        for (appearance, suffix) in [(NSAppearance.Name.aqua, "light"), (.darkAqua, "dark")] {
            NSApp.appearance = NSAppearance(named: appearance)
            hud.show(message, strings: strings) {}
            try? await Task.sleep(for: .milliseconds(800))
            guard let panel = NSApp.windows.first(where: { $0.isVisible && $0.styleMask.contains(.hudWindow) }) else {
                break
            }
            let file = (directory as NSString).appendingPathComponent("hud-\(suffix).png")
            // A HUD panel is translucent and dark whatever the appearance:
            // its top band says nothing about the appearance.
            if let png = await Capture.png(of: panel, .darkAqua, checked: false),
                (try? png.write(to: URL(fileURLWithPath: file))) != nil
            {
                written += 1
            }
            hud.close()
        }
        NSApp.appearance = nil
        return written
    }

    /// The real menu-bar panel, light and dark: the app clicks its own
    /// status item's button (no permission: both are the app's) and
    /// paints the window that opens. 0 when there is no status item or no
    /// panel opened; the capture then draws the panel's view in a window
    /// of its own.
    static func drawMenuBar(into directory: String) async -> Int {
        let bar = NSApp.windows.first { NSStringFromClass(type(of: $0)).contains("StatusBarWindow") }
        guard let button = bar.flatMap({ statusButton(in: $0.contentView) }) else {
            FileHandle.standardError.write(Data("capture: no status item button\n".utf8))
            return 0
        }
        let before = Set(NSApp.windows.filter(\.isVisible).map(ObjectIdentifier.init))
        button.performClick(nil)
        var opened: NSWindow?
        for _ in 0..<40 where opened == nil {
            try? await Task.sleep(for: .milliseconds(100))
            opened = NSApp.windows.first { $0.isVisible && !before.contains(ObjectIdentifier($0)) }
        }
        guard let panel = opened else {
            FileHandle.standardError.write(Data("capture: the status item opened no panel\n".utf8))
            return 0
        }
        var written = 0
        for (appearance, suffix) in [(NSAppearance.Name.aqua, "light"), (.darkAqua, "dark")] {
            await Capture.appear(appearance, panel)
            try? await Task.sleep(for: .milliseconds(800))
            let file = (directory as NSString).appendingPathComponent("menubar-\(suffix).png")
            if let png = await Capture.png(of: panel, appearance), (try? png.write(to: URL(fileURLWithPath: file))) != nil {
                written += 1
            }
        }
        // A second click closes it, as on screen.
        button.performClick(nil)
        NSApp.appearance = nil
        return written
    }

    private static func statusButton(in view: NSView?) -> NSButton? {
        guard let view else { return nil }
        if let button = view as? NSButton { return button }
        for sub in view.subviews {
            if let found = statusButton(in: sub) { return found }
        }
        return nil
    }

    /// The real Settings window, each tab, light and dark, title bar and
    /// toolbar included (a copy of its view in a window of its own drew
    /// the first row under the window's top edge).
    static func drawSettings(
        _ open: @MainActor (SettingsTab) async -> NSWindow?, into directory: String
    ) async -> Int {
        var written = 0
        for (appearance, suffix) in [(NSAppearance.Name.aqua, "light"), (.darkAqua, "dark")] {
            for (name, tab) in [("general", SettingsTab.general), ("hotkeys", .hotkeys),
                                ("exclusions", .exclusions), ("access", .access)] {
                guard let window = await open(tab) else {
                    continue
                }
                Capture.sharpest(window)
                await Capture.appear(appearance, window)
                try? await Task.sleep(for: .milliseconds(800))
                window.displayIfNeeded()
                let file = (directory as NSString).appendingPathComponent("settings-\(name)-\(suffix).png")
                if let png = await Capture.png(of: window, appearance),
                    (try? png.write(to: URL(fileURLWithPath: file))) != nil
                {
                    written += 1
                }
            }
        }
        return written
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
                // No title bar band: a sheet, the menu-bar panel and the
                // hotkey's panel have none.
                let host = NSHostingView(rootView: view.background(Color(nsColor: .windowBackgroundColor)))
                host.safeAreaRegions = []
                window.contentView = host
                window.setContentSize(host.fittingSize)
                Capture.sharpest(window)
                window.makeKeyAndOrderFront(nil)
                try? await Task.sleep(for: .milliseconds(800))
                window.setContentSize(host.fittingSize)
                window.displayIfNeeded()
                let file = (directory as NSString).appendingPathComponent("\(name)-\(suffix).png")
                if let png = await Capture.png(of: window, appearance),
                    (try? png.write(to: URL(fileURLWithPath: file))) != nil
                {
                    written += 1
                }
                window.orderOut(nil)
            }
        }
        NSApp.appearance = nil
        return written
    }
}
