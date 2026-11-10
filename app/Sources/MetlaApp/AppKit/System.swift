import AppKit

/// System Settings panes the app points to.
enum SystemSettings {
    /// Privacy & Security › Full Disk Access.
    @MainActor
    static func openFullDiskAccess() {
        let pane = "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles"
        if let url = URL(string: pane) {
            NSWorkspace.shared.open(url)
        }
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
