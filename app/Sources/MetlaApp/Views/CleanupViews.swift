import AppKit
import MetlaCoreFFI
import MetlaKit
import SwiftUI

/// The cleanup sheet: the preview, then the progress, then the report.
struct CleanupSheet: View {
    let cleanup: CleanupModel
    let strings: Strings
    /// The sheet's window closes when the flow ends (the standalone
    /// window); `nil` for a sheet, which closes with the model's stage.
    var dismiss: (() -> Void)?

    var body: some View {
        Group {
            switch cleanup.stage {
            case .loading, .closed:
                VStack(spacing: 12) {
                    ProgressView()
                    Text(strings.text("preview.loading")).foregroundStyle(.secondary)
                }
                .padding(40)
            case .preview:
                if let preview = cleanup.preview {
                    PreviewView(preview: preview, strings: strings, cancel: close) {
                        Task { await cleanup.clean() }
                    }
                }
            case .running:
                ProgressPane(progress: cleanup.progress, strings: strings) {
                    Task { await cleanup.cancel() }
                }
            case .report:
                if let report = cleanup.report {
                    VStack(alignment: .leading, spacing: 12) {
                        ReportView(report: report, strings: strings, problemsOnly: Bindable(cleanup).problemsOnly)
                        HStack {
                            Spacer()
                            Button(strings.text("report.done"), action: close)
                                .keyboardShortcut(.defaultAction)
                        }
                    }
                    .padding(20)
                }
            }
        }
        .frame(width: 620)
        .frame(minHeight: 200, maxHeight: 640)
    }

    private func close() {
        cleanup.close()
        dismiss?()
    }
}

/// The preview: one group per way of removal, with its badge, rules,
/// count, size and paths; set-aside items with their reasons. Cancel is
/// the default button (Return and Esc); the destructive button is named
/// after the action, never the default, and needs a click or ⌘↩.
struct PreviewView: View {
    @Bindable var preview: PreviewModel
    let strings: Strings
    let cancel: () -> Void
    let clean: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(preview.title).font(.headline)
            ScrollView {
                VStack(alignment: .leading, spacing: 12) {
                    if preview.groups.isEmpty {
                        Text(strings.text("preview.nothing")).foregroundStyle(.secondary)
                    }
                    ForEach(preview.groups) { group in
                        GroupPane(group: group, preview: preview, strings: strings)
                    }
                    if !preview.setAside.isEmpty {
                        SetAsidePane(items: preview.setAside, strings: strings)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            Text(strings.text("preview.estimate"))
                .font(.caption).foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            HStack {
                Spacer()
                Button(strings.text("preview.cancel"), action: cancel)
                    .keyboardShortcut(.defaultAction)
                Button(preview.action, role: .destructive, action: clean)
                    .keyboardShortcut(.return, modifiers: .command)
                    .disabled(!preview.canClean)
            }
        }
        .padding(20)
        .onExitCommand(perform: cancel)
    }
}

/// One group of the preview.
struct GroupPane: View {
    let group: PreviewGroup
    let preview: PreviewModel
    let strings: Strings
    @State private var all = false
    static let shown = 12

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                Text(group.method)
                    .font(.caption.weight(.semibold)).lineLimit(1).fixedSize()
                    .padding(.horizontal, 7).padding(.vertical, 2)
                    .background(Capsule().fill(tint.opacity(0.16)))
                    .foregroundStyle(tint)
                Text(group.rules.joined(separator: ", "))
                    .lineLimit(1).truncationMode(.tail).foregroundStyle(.secondary)
                Spacer(minLength: 8)
                Text(strings.format("preview.count", group.rows.count) + " \u{00B7} "
                    + Format.bytes(group.bytes, strings))
                    .monospacedDigit().lineLimit(1).fixedSize()
            }
            if let separate = group.separate {
                Toggle(isOn: Binding(
                    get: { preview.confirmedGroups.contains(separate) },
                    set: { preview.confirm(separate, $0) })) {
                    Text(confirmText(separate)).fontWeight(.medium)
                }
                .toggleStyle(.symbolCheckbox)
            }
            LazyVStack(alignment: .leading, spacing: 3) {
                ForEach(all ? group.rows : Array(group.rows.prefix(Self.shown))) { row in
                    RowLine(row: row, preview: preview, strings: strings)
                }
            }
            if group.rows.count > Self.shown && !all {
                Button(strings.format("preview.showAll", group.rows.count)) { all = true }
                    .buttonStyle(.link)
            }
        }
        .padding(10)
        .background(RoundedRectangle(cornerRadius: 8).strokeBorder(.quaternary))
    }

    private var tint: Color {
        switch group.id {
        case .quick: .red
        case .separate(.moveToTrash): .blue
        case .separate(.emptyTrash): .red
        case .separate(.removeWorktrees): .green
        }
    }

    private func confirmText(_ separate: Separate) -> String {
        switch separate {
        case .moveToTrash: strings.text("preview.confirm.trash")
        case .emptyTrash: strings.text("preview.confirm.emptyTrash")
        case .removeWorktrees: strings.text("preview.confirm.worktree")
        }
    }
}

/// One item: its path, size and warnings; an item in a managed worktree
/// with a checkbox of its own.
struct RowLine: View {
    let row: PreviewRow
    let preview: PreviewModel
    let strings: Strings

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            if row.ownConfirmation {
                Toggle(isOn: Binding(
                    get: { preview.confirmedItems.contains(row.id) },
                    set: { preview.confirmItem(row.id, $0) })) { EmptyView() }
                    .toggleStyle(.symbolCheckbox)
                    .accessibilityLabel(strings.text("preview.confirmItem"))
            }
            VStack(alignment: .leading, spacing: 0) {
                Text(row.shownPath)
                    .font(.system(.callout, design: .monospaced))
                    .lineLimit(1).truncationMode(.middle)
                    .help(row.path)
                    .foregroundStyle(preview.covered.contains(row.id) ? Color.primary : Color.secondary)
                if !row.warnings.isEmpty {
                    Text(row.warnings.joined(separator: " \u{00B7} "))
                        .font(.caption).foregroundStyle(.orange)
                }
            }
            Spacer(minLength: 8)
            Text(Format.bytes(row.bytes, strings)).monospacedDigit().foregroundStyle(.secondary)
        }
    }
}

/// Selected items the preview set aside, with why.
struct SetAsidePane: View {
    let items: [SetAside]
    let strings: Strings

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(strings.text("preview.setAside")).font(.subheadline.weight(.semibold))
            ForEach(items) { item in
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(item.shownPath)
                        .font(.system(.callout, design: .monospaced))
                        .lineLimit(1).truncationMode(.middle)
                    Spacer(minLength: 8)
                    Text(item.reason).font(.caption).foregroundStyle(.secondary)
                        .lineLimit(2).multilineTextAlignment(.trailing)
                }
            }
        }
        .padding(10)
        .background(RoundedRectangle(cornerRadius: 8).fill(.quaternary.opacity(0.5)))
    }
}

/// A cleanup running: the bar, the current item, what is removed so far,
/// and Cancel.
struct ProgressPane: View {
    let progress: CleanupProgressData?
    let strings: Strings
    let cancel: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(strings.text("progress.title")).font(.headline)
            ProgressView(value: CleanupProgressData.bar(progress))
            HStack {
                Text(progress.map { strings.format("progress.items", Int($0.done), Int($0.items)) } ?? "")
                    .monospacedDigit()
                Spacer()
                Text(strings.format("progress.removed", Format.bytes(progress?.removed ?? 0, strings)))
                    .monospacedDigit()
            }
            .font(.callout).foregroundStyle(.secondary)
            Text(progress?.current ?? " ")
                .font(.system(.callout, design: .monospaced))
                .lineLimit(1).truncationMode(.middle)
            HStack {
                Spacer()
                Button(strings.text("preview.cancel"), action: cancel)
                    .keyboardShortcut(.cancelAction)
            }
        }
        .padding(20)
    }
}

/// A report: Found, Removed and Freed on disk side by side, what went to
/// the Trash, and every item's outcome, with the problems alone if asked.
struct ReportView: View {
    let report: ReportData
    let strings: Strings
    @Binding var problemsOnly: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(strings.text("report.title")).font(.headline)
            if let reason = report.notRun {
                Text(strings.format("report.notRun", reason)).foregroundStyle(.secondary)
            } else {
                numbers
                Text(report.freedNote(strings))
                    .font(.caption).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                if report.movedToTrash > 0 {
                    Text(strings.format("report.trash", Format.bytes(report.movedToTrash, strings)))
                        .font(.caption).foregroundStyle(.secondary)
                }
                Toggle(strings.text("report.problemsOnly"), isOn: $problemsOnly)
                    .toggleStyle(.symbolCheckbox)
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 4) {
                        ForEach(report.rows(problemsOnly: problemsOnly)) { row in
                            ReportLine(row: row, strings: strings)
                        }
                    }
                }
                .frame(minHeight: 80)
            }
        }
    }

    private var numbers: some View {
        HStack(spacing: 24) {
            number(strings.text("report.found"), Format.bytes(report.found, strings))
            number(strings.text("report.removed"), Format.bytes(report.removed, strings))
            number(strings.text("report.freed"), report.freedFigure(strings))
        }
    }

    private func number(_ title: String, _ value: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title).font(.caption).foregroundStyle(.secondary)
            Text(value).font(.title2.weight(.semibold)).monospacedDigit()
        }
    }
}

/// One item of a report.
struct ReportLine: View {
    let row: ReportRow
    let strings: Strings

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text(mark)
                .fontWeight(.bold).frame(width: 14)
                .foregroundStyle(tint)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 0) {
                Text(row.shownPath)
                    .font(.system(.callout, design: .monospaced))
                    .lineLimit(1).truncationMode(.middle)
                Text(row.outcome).font(.caption).foregroundStyle(.secondary)
                ForEach(row.details, id: \.self) { detail in
                    Text(detail).font(.caption2).foregroundStyle(.secondary).lineLimit(1)
                }
            }
            Spacer(minLength: 8)
            Text(Format.bytes(row.found, strings)).monospacedDigit().foregroundStyle(.secondary)
            if row.tone != .done {
                Button(strings.text("report.reveal")) { Finder.reveal([row.path]) }
                    .buttonStyle(.link)
                    .disabled(!FileManager.default.fileExists(atPath: row.path))
            }
        }
        .accessibilityElement(children: .combine)
    }

    /// Done, left alone by the user's choice, or a problem.
    private var mark: String {
        switch row.tone {
        case .done: "\u{2713}"
        case .untouched: "\u{2013}"
        case .problem: "!"
        }
    }

    private var tint: Color {
        switch row.tone {
        case .done: .green
        case .untouched: .secondary
        case .problem: .orange
        }
    }
}

/// "Last report" in the sidebar: the report the engine keeps.
struct LastReportView: View {
    let model: AppModel
    @State private var problemsOnly = false

    var body: some View {
        Group {
            if let report = model.lastReport {
                ReportView(report: report, strings: model.strings, problemsOnly: $problemsOnly)
                    .padding(20)
            } else {
                ContentUnavailableView(model.strings.text("report.none"), systemImage: "doc.text")
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }
}
