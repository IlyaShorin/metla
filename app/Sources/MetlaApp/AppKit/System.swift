import AppKit
import MetlaKit

/// System Settings panes the app points to.
enum SystemSettings {
    /// Privacy & Security › Full Disk Access.
    @MainActor
    static func openFullDiskAccess() {
        NSWorkspace.shared.open(AccessModel.settingsURL)
    }
}

/// Finder, for "Reveal in Finder".
enum Finder {
    @MainActor
    static func reveal(_ paths: [String]) {
        let urls = paths.map { URL(fileURLWithPath: $0) }
        NSWorkspace.shared.activateFileViewerSelecting(urls)
    }
}
