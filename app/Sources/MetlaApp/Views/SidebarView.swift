import MetlaCoreFFI
import MetlaKit
import SwiftUI

/// Disk (Overview, Folders) and the categories with their sizes, dimmed
/// when there is nothing to clean.
struct SidebarView: View {
    @Bindable var model: AppModel

    var body: some View {
        List(selection: selection) {
            Section(model.strings.text("sidebar.disk")) {
                Label(model.strings.text("sidebar.overview"), systemImage: "internaldrive")
                    .tag(Screen.overview)
                Label(model.strings.text("sidebar.folders"), systemImage: "folder")
                    .tag(Screen.folders)
            }
            Section(model.strings.text("sidebar.cleanable")) {
                ForEach(model.sidebar) { row in
                    HStack {
                        Text(row.name)
                        Spacer()
                        Text(Format.bytes(row.bytes, model.strings))
                            .foregroundStyle(.secondary)
                            .monospacedDigit()
                    }
                    .opacity(row.empty ? 0.45 : 1)
                    .tag(Screen.category(row.category))
                    .accessibilityLabel("\(row.name), \(Format.bytes(row.bytes, model.strings))")
                }
            }
            if model.lastReport != nil {
                Section {
                    Label(model.strings.text("sidebar.lastReport"), systemImage: "doc.text")
                        .tag(Screen.report)
                }
            }
        }
        .listStyle(.sidebar)
    }

    private var selection: Binding<Screen?> {
        Binding(
            get: { model.screen },
            set: { screen in
                guard let screen else { return }
                Task { await model.show(screen) }
            })
    }
}
