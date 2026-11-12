import AppKit
import MetlaKit
import SwiftUI

/// The update's strip at the bottom of the sidebar (and in Settings →
/// General): the offer with Install, What's New and Later, the progress,
/// or why it failed. Nothing when there is no offer.
struct UpdateStrip: View {
    let update: UpdateModel
    let strings: Strings

    var body: some View {
        if let shown = UpdateShown.of(
            update.offer, installation: update.installation, cleanupRunning: update.cleanupRunning, strings)
        {
            VStack(alignment: .leading, spacing: 6) {
                Text(shown.title)
                    .font(.callout.weight(.medium))
                    .fixedSize(horizontal: false, vertical: true)
                if let note = shown.note {
                    Text(note)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                UpdateProgress(progress: shown.progress)
                if !shown.actions.isEmpty {
                    // One row; where it does not fit (Russian, in the
                    // sidebar), the button above its links.
                    let links = shown.actions.filter { !UpdateLine.primary($0) }
                    ViewThatFits(in: .horizontal) {
                        HStack(spacing: 10) { buttons(shown.actions) }
                        VStack(alignment: .leading, spacing: 6) {
                            buttons(shown.actions.filter(UpdateLine.primary))
                            HStack(spacing: 10) { buttons(links) }
                        }
                    }
                    .controlSize(.small)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(10)
            .background(.quaternary.opacity(0.6), in: RoundedRectangle(cornerRadius: 8))
            .accessibilityElement(children: .contain)
        }
    }

    @ViewBuilder private func buttons(_ actions: [UpdateShown.Action]) -> some View {
        ForEach(Array(actions.enumerated()), id: \.offset) { _, action in
            UpdateButton(action: action, update: update, strings: strings)
        }
    }
}

/// One of the update's buttons: Install and the release page as buttons,
/// What's New and Later as links.
struct UpdateButton: View {
    let action: UpdateShown.Action
    let update: UpdateModel
    let strings: Strings

    var body: some View {
        switch action {
        case .install(let enabled):
            Button(strings.text("update.install")) { Task { await update.install() } }
                .buttonStyle(.borderedProminent)
                .disabled(!enabled)
        case .releasePage(let page):
            Button(strings.text("update.releasePage")) { NSWorkspace.shared.open(page) }
                .buttonStyle(.bordered)
        case .whatsNew(let notes):
            Button(strings.text("update.whatsNew")) { NSWorkspace.shared.open(notes) }
                .buttonStyle(.link)
        case .later:
            Button(strings.text("update.later")) { update.later() }
                .buttonStyle(.link)
        }
    }
}

struct UpdateProgress: View {
    let progress: UpdateShown.Progress?

    var body: some View {
        switch progress {
        case .share(let share)?:
            ProgressView(value: share).progressViewStyle(.linear)
        case .working?:
            ProgressView().progressViewStyle(.linear)
        case nil:
            EmptyView()
        }
    }
}

/// The menu-bar panel's line above the status: the offer and Install (or
/// the release page), the progress, or the failure.
struct UpdateLine: View {
    let update: UpdateModel
    let strings: Strings

    var body: some View {
        if let shown = UpdateShown.of(
            update.offer, installation: update.installation, cleanupRunning: update.cleanupRunning, strings)
        {
            HStack(alignment: .center, spacing: 8) {
                VStack(alignment: .leading, spacing: 3) {
                    Text(shown.title)
                        .font(.callout)
                        .lineLimit(2)
                        .fixedSize(horizontal: false, vertical: true)
                    UpdateProgress(progress: shown.progress)
                }
                Spacer(minLength: 4)
                if let primary = shown.actions.first(where: Self.primary) {
                    UpdateButton(action: primary, update: update, strings: strings)
                        .controlSize(.small)
                }
            }
        }
    }

    /// Install and the release page; the others are links.
    static func primary(_ action: UpdateShown.Action) -> Bool {
        switch action {
        case .install, .releasePage: true
        case .whatsNew, .later: false
        }
    }
}

/// Settings → General's updates: the automatic check, the version, when it
/// last checked, Check Now and what it found.
struct UpdatesSection: View {
    let update: UpdateModel
    let strings: Strings

    var body: some View {
        Section(strings.text("update.section")) {
            Toggle(isOn: Binding(get: { update.automatic }, set: { update.setAutomatic($0) })) {
                Titled(strings.text("update.automatic"), note: strings.text("update.automaticNote"))
            }
            // The version, when it last checked and what it found, and
            // Check Now: one row.
            LabeledContent {
                Button(strings.text("update.checkNow")) { Task { await update.check(manual: true) } }
                    .disabled(update.checking || update.offer?.phase.busy == true)
            } label: {
                VStack(alignment: .leading, spacing: 2) {
                    Text("\(strings.text("update.version")) \(UpdateShown.version(update.installation))")
                    TimelineView(.periodic(from: .now, by: 30)) { context in
                        Text(said(now: context.date))
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
            }
            if update.offer != nil {
                UpdateStrip(update: update, strings: strings)
            }
        }
    }

    /// When it last checked, and what it found when it offered nothing.
    private func said(now: Date) -> String {
        let last = UpdateShown.lastCheck(update.lastCheck, now: now, strings)
        guard update.offer == nil, let outcome = UpdateShown.outcome(update.outcome, checking: update.checking, strings)
        else {
            return last
        }
        return "\(last) · \(outcome)"
    }
}
