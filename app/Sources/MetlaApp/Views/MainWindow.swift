import MetlaCoreFFI
import MetlaKit
import SwiftUI

/// The main window: the sidebar, the screen chosen in it, and the scan
/// status in the toolbar.
struct MainWindow: View {
    @Bindable var model: AppModel
    @Environment(\.controlActiveState) private var active

    var body: some View {
        NavigationSplitView {
            SidebarView(model: model)
                .navigationSplitViewColumnWidth(min: 200, ideal: 232, max: 300)
        } detail: {
            detail
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .navigationTitle(title)
        .navigationSubtitle(subtitle)
        .toolbar { toolbar }
        .frame(minWidth: 900, minHeight: 560)
        .sheet(isPresented: sheetShown) {
            if let cleanup = model.cleanup {
                CleanupSheet(cleanup: cleanup, strings: model.strings)
            }
        }
        .sheet(isPresented: firstRun) {
            FirstRunSheet(access: model.access, strings: model.strings)
        }
        .onChange(of: active) { _, state in
            Task { await model.attention(state != .inactive) }
        }
    }

    @ViewBuilder private var detail: some View {
        if let failure = model.failure, model.overview == nil {
            ContentUnavailableView(model.strings.text("app.failed"), systemImage: "exclamationmark.triangle", description: Text(failure))
        } else {
            switch model.screen {
            case .overview:
                OverviewView(model: model)
            case .folders:
                if let folders = model.folders {
                    FoldersView(model: model, folders: folders)
                }
            case .category(let category):
                if let table = model.category(category) {
                    CategoryView(model: model, table: table)
                }
            case .report:
                LastReportView(model: model)
            }
        }
    }

    /// The cleanup sheet is up while the flow is not closed, unless the
    /// standalone window shows it.
    private var sheetShown: Binding<Bool> {
        Binding(
            get: { model.cleanup.map { $0.stage != .closed } ?? false && !model.cleanupInWindow },
            set: { shown in if !shown { model.cleanup?.close() } })
    }

    /// The first-run sheet, once.
    private var firstRun: Binding<Bool> {
        Binding(
            get: { model.access.showsFirstRun && model.settings.capture == nil && model.settings.scenario == nil },
            set: { shown in if !shown { model.access.finishFirstRun() } })
    }

    private var title: String {
        switch model.screen {
        case .overview: model.strings.text("sidebar.overview")
        case .folders: model.strings.text("sidebar.folders")
        case .category(let category): model.names.category(category)
        case .report: model.strings.text("sidebar.lastReport")
        }
    }

    /// The breadcrumb on Folders.
    private var subtitle: String {
        guard case .folders = model.screen, let node = model.folders?.node(model.folders?.selection) else {
            return ""
        }
        let path = Format.path(node.path, home: model.settings.home)
        return path.split(separator: "/").joined(separator: " \u{203A} ")
    }

    @ToolbarContentBuilder private var toolbar: some ToolbarContent {
        ToolbarItemGroup(placement: .primaryAction) {
            if let status = model.status {
                StatusView(status: status)
                if status.canStop {
                    Button(model.strings.text("toolbar.stop")) {
                        Task { await model.stopScan() }
                    }
                    .accessibilityLabel(model.strings.text("toolbar.stop"))
                }
            }
            Button {
                Task { await model.rescan() }
            } label: {
                Label(model.strings.text("toolbar.refresh"), systemImage: "arrow.clockwise")
            }
            .help(model.strings.text("toolbar.refresh"))
            Button(model.strings.text("toolbar.clean")) {
                Task { await model.cleanDefault() }
            }
            .keyboardShortcut(.delete, modifiers: .command)
            .buttonStyle(.borderedProminent)
        }
    }
}

/// The scan status: a coloured dot and its words.
struct StatusView: View {
    let status: StatusLine

    var body: some View {
        HStack(spacing: 6) {
            Circle()
                .fill(color)
                .frame(width: 8, height: 8)
                .accessibilityHidden(true)
            Text(status.text)
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
        .padding(.horizontal, 6)
        .accessibilityElement(children: .combine)
    }

    private var color: Color {
        switch status.tone {
        case .working: .green
        case .current: .green
        case .stale: .orange
        case .problem: .red
        case .off: .gray
        }
    }
}
