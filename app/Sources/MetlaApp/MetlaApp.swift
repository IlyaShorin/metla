import AppKit
import MetlaKit
import SwiftUI

@main
struct MetlaApp: App {
    @State private var model: AppModel
    private let settings: LaunchSettings

    init() {
        let settings = LaunchSettings.current()
        self.settings = settings
        _model = State(initialValue: AppModel(settings: settings, strings: Strings(language: settings.language)))
    }

    var body: some Scene {
        Window("Metla", id: "main") {
            MainWindow(model: model)
                .task { await launch() }
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
    }

    /// Opens the engine; in capture mode renders the screens and quits.
    private func launch() async {
        LaunchTiming.firstWindow(print: settings.printLaunchTiming)
        await model.start()
        LaunchTiming.firstPicture(print: settings.printLaunchTiming)
        if let directory = settings.capture {
            let written = await Capture.run(model, into: directory)
            print("capture: \(written) images in \(directory)")
            NSApp.terminate(nil)
        }
    }
}
