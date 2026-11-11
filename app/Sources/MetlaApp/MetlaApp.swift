import AppKit
import MetlaKit
import SwiftUI

@main
struct MetlaApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @State private var model: AppModel
    @State private var bridge: Bridge
    private let settings: LaunchSettings

    init() {
        let settings = LaunchSettings.current()
        self.settings = settings
        let model = AppModel(
            settings: settings, strings: Strings(language: settings.language), preferences: settings.preferences)
        _model = State(initialValue: model)
        let bridge = Bridge(model: model)
        _bridge = State(initialValue: bridge)
        delegate.bridge = bridge
    }

    var body: some Scene {
        Window("Metla", id: "main") {
            MainWindow(model: model)
                .background(WindowOpener(bridge: bridge))
        }
        .defaultSize(width: 1120, height: 720)
        .commands {
            CommandGroup(after: .toolbar) {
                Button(model.strings.text("toolbar.refresh")) {
                    Task { await model.rescan() }
                }
                .keyboardShortcut("r", modifiers: .command)
            }
        }
        Window(model.strings.text("report.title"), id: "cleanup") {
            if let cleanup = model.cleanup {
                CleanupSheet(cleanup: cleanup, strings: model.strings) { bridge.closeCleanupWindow() }
                    .background(WindowOpener(bridge: bridge))
            }
        }
        .windowResizability(.contentSize)
        MenuBarExtra(isInserted: $bridge.showsInMenuBar) {
            MenuBarPanel(model: model, actions: bridge.panelActions)
                .onAppear { Task { await model.attention(true) } }
                .onDisappear { Task { await model.attention(false) } }
        } label: {
            MenuBarIcon(scanning: model.scanning)
                .background(WindowOpener(bridge: bridge))
        }
        .menuBarExtraStyle(.window)
        Settings {
            SettingsView(model: model, chosen: $bridge.settingsTab)
        }
    }
}

/// Starts the app once AppKit has finished launching, not when a scene's
/// view appears: the window state macOS restores can bring up no window
/// at all. Keeps the app in the menu bar when its last window closes,
/// unless "Show in menu bar" is off.
final class AppDelegate: NSObject, NSApplicationDelegate {
    var bridge: Bridge?

    func applicationDidFinishLaunching(_ notification: Notification) {
        Task { await bridge?.launch() }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        !GeneralModel.showsInMenuBar(LaunchSettings.current().preferences)
    }
}

/// Hands SwiftUI's window actions to the bridge, from whatever view is up.
struct WindowOpener: View {
    let bridge: Bridge
    @Environment(\.openWindow) private var openWindow
    @Environment(\.openSettings) private var openSettings

    var body: some View {
        Color.clear
            .onAppear { bridge.appeared(openWindow: openWindow, openSettings: openSettings) }
    }
}

/// The broom in the menu bar: a template image, with motion marks while a
/// scan runs.
struct MenuBarIcon: View {
    let scanning: Bool

    var body: some View {
        Image(nsImage: Broom.image(moving: scanning))
            .accessibilityLabel("Metla")
    }
}
