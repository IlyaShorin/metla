import AppKit
import MetlaCoreFFI
import MetlaKit
import SwiftUI

/// What ties the app's model to the system: launching once, the global
/// hotkeys and their panel, the power monitor, the menu bar and the
/// windows SwiftUI opens.
@MainActor
@Observable
final class Bridge {
    let model: AppModel
    var showsInMenuBar: Bool {
        didSet { model.general?.setShowInMenuBar(showsInMenuBar) }
    }
    @ObservationIgnored var openWindow: OpenWindowAction?
    @ObservationIgnored var openSettings: OpenSettingsAction?
    /// The Settings window's tab.
    var settingsTab: SettingsTab = .general
    @ObservationIgnored private var launched = false
    @ObservationIgnored private var hotkeys: HotkeyCenter?
    @ObservationIgnored private var power: PowerMonitor?
    @ObservationIgnored private let hud = HUD()

    init(model: AppModel) {
        self.model = model
        showsInMenuBar = GeneralModel.showsInMenuBar(model.preferences)
    }

    /// Opens the engine once (from whichever scene comes up first); in
    /// capture or scenario mode renders or runs and quits.
    func launch() async {
        guard !launched else { return }
        launched = true
        let settings = model.settings
        LaunchTiming.firstWindow(print: settings.printLaunchTiming)
        if let scenario = settings.scenario {
            let code = await Scenarios.run(scenario, settings: settings, strings: model.strings)
            fflush(stdout)
            exit(code)
        }
        // A capture cleans its fixture into the fixture's own Trash.
        let trash: any TrashHost = settings.capture == nil ? SystemTrash() : FixtureTrash(root: settings.root)
        await model.start(trash: trash, login: SystemLoginItem())
        LaunchTiming.firstPicture(print: settings.printLaunchTiming)
        if let directory = settings.capture {
            let written = await Capture.run(model, into: directory) { [weak self] tab in
                await self?.showSettings(tab)
            }
            print("capture: \(written) images in \(directory)")
            fflush(stdout)
            // Not NSApp.terminate: with the capture's report sheet up,
            // SwiftUI leaves the app running.
            exit(0)
        }
        if settings.menuBarOnly {
            NSApp.windows.first { $0.identifier?.rawValue == "main" }?.close()
        }
        if settings.printLaunchTiming {
            Task { await Measure.follow(model) }
        }
        hotkeys = HotkeyCenter { [weak self] action in self?.pressed(action) }
        if let hotkeys {
            await model.attachHotkeys(hotkeys)
            // What the system answered, for whoever starts the app from a
            // terminal: a combination another app holds says so here too.
            for action in HotkeyAction.allCases {
                let combo = model.hotkeys?.bindings[action]?.display ?? "-"
                print("hotkey \(action.rawValue) \(combo): \(model.hotkeys?.registrations[action] ?? .unassigned)")
            }
            fflush(stdout)
        }
        power = PowerMonitor { [weak self] state in
            Task { await self?.model.powerChanged(state) }
        }
        power?.tell()
        await model.access.check()
    }

    /// A global hotkey was pressed.
    func pressed(_ action: HotkeyAction) {
        Task {
            switch action {
            case .show:
                showMain()
            case .clean:
                switch await model.cleanHotkey() {
                case .showPreview: await showPreview()
                case .showApp: showMain()
                case .hud(let message):
                    hud.show(message, strings: model.strings) { [weak self] in self?.showReport() }
                }
            }
        }
    }

    func showMain() {
        NSApp.activate()
        openWindow?(id: "main")
    }

    /// The preview of the default selection, as a sheet of the main
    /// window when it is open, else in a window of its own.
    func showPreview() async {
        NSApp.activate()
        let mainOpen = NSApp.windows.contains { $0.identifier?.rawValue == "main" && $0.isVisible }
        model.cleanupInWindow = !mainOpen
        if !mainOpen {
            openWindow?(id: "cleanup")
        }
        await model.cleanDefault()
    }

    /// Opens the Settings window at `tab`; the window once it is up.
    func showSettings(_ tab: SettingsTab) async -> NSWindow? {
        settingsTab = tab
        openSettings?()
        let deadline = Date().addingTimeInterval(10)
        while Date() < deadline {
            if let window = NSApp.windows.first(where: { $0.isVisible && $0.identifier?.rawValue.contains("Settings") == true }) {
                return window
            }
            try? await Task.sleep(for: .milliseconds(100))
        }
        return nil
    }

    func closeCleanupWindow() {
        model.cleanupInWindow = false
        NSApp.windows.first { $0.identifier?.rawValue == "cleanup" }?.close()
    }

    func showReport() {
        showMain()
        Task { await model.show(.report) }
    }

    /// The menu-bar panel's buttons.
    var panelActions: PanelActions {
        PanelActions(
            clean: { [weak self] in self?.pressed(.clean) },
            open: { [weak self] in self?.showMain() },
            refresh: { [weak self] in Task { await self?.model.rescan() } },
            background: { [weak self] on in Task { await self?.model.general?.setBackground(on) } },
            settings: { [weak self] in
                NSApp.activate()
                self?.openSettings?()
            },
            quit: { NSApp.terminate(nil) })
    }
}

/// The menu-bar icon: a broom drawn as a template image (no colour), with
/// motion marks while a scan runs.
enum Broom {
    static func image(moving: Bool) -> NSImage {
        let image = NSImage(size: NSSize(width: 18, height: 18), flipped: true) { _ in
            NSColor.black.setStroke()
            NSColor.black.setFill()
            let handle = NSBezierPath()
            handle.move(to: NSPoint(x: 13.5, y: 1.5))
            handle.line(to: NSPoint(x: 8.5, y: 9))
            handle.lineWidth = 1.6
            handle.lineCapStyle = .round
            handle.stroke()
            let head = NSBezierPath()
            head.move(to: NSPoint(x: 7, y: 8))
            head.line(to: NSPoint(x: 11, y: 10.5))
            head.line(to: NSPoint(x: 8.5, y: 16.5))
            head.line(to: NSPoint(x: 2, y: 13))
            head.close()
            head.fill()
            if moving {
                for (x, y) in [(13.0, 13.0), (15.5, 10.5), (16.0, 15.0)] {
                    NSBezierPath(ovalIn: NSRect(x: x, y: y, width: 1.8, height: 1.8)).fill()
                }
            }
            return true
        }
        image.isTemplate = true
        return image
    }
}
