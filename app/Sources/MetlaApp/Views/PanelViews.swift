import AppKit
import MetlaCoreFFI
import MetlaKit
import SwiftUI

/// The first-run sheet: what Metla does, that it reads only metadata, why
/// Full Disk Access matters. The first scan runs either way.
struct FirstRunSheet: View {
    let access: AccessModel
    let strings: Strings
    var done: () -> Void = {}

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(strings.text("firstrun.title")).font(.title2.weight(.semibold))
            Text(strings.text("firstrun.what"))
            Text(strings.text("firstrun.metadata")).foregroundStyle(.secondary)
            Text(strings.text("firstrun.fda"))
            HStack {
                Spacer()
                Button(strings.text("firstrun.skip")) { finish() }
                    .keyboardShortcut(.cancelAction)
                Button(strings.text("firstrun.open")) {
                    SystemSettings.openFullDiskAccess()
                    finish()
                }
                .keyboardShortcut(.defaultAction)
            }
        }
        .fixedSize(horizontal: false, vertical: true)
        .padding(24)
        .frame(width: 460)
    }

    private func finish() {
        access.finishFirstRun()
        done()
    }
}

/// The menu-bar panel, about 360 pt wide: it reads the cached snapshot
/// and never waits for the engine.
struct MenuBarPanel: View {
    let model: AppModel
    let actions: PanelActions

    var body: some View {
        let strings = model.strings
        VStack(alignment: .leading, spacing: 10) {
            if let summary = summary {
                HStack(alignment: .firstTextBaseline) {
                    Text(summary.volume).font(.headline)
                    Spacer()
                    Text(summary.free).foregroundStyle(.secondary)
                }
                ShareBar(share: summary.used, tint: .accentColor)
                HStack {
                    Text(summary.usedLine)
                    Spacer()
                    Text(summary.canBeFreed).fontWeight(.medium)
                }
                .font(.caption)
                if let toTrash = summary.toTrash {
                    Text(toTrash).font(.caption).foregroundStyle(.secondary)
                }
                Divider()
                ForEach(Array(summary.quick.enumerated()), id: \.element.id) { index, row in
                    HStack(spacing: 8) {
                        RoundedRectangle(cornerRadius: 3).fill(Palette.category(index)).frame(width: 9, height: 9)
                        Text(row.name)
                        Spacer()
                        Text(row.size).monospacedDigit()
                    }
                }
                HStack(spacing: 8) {
                    Text(summary.rest.name).foregroundStyle(.secondary).lineLimit(2)
                    Spacer()
                    Text(summary.rest.size).monospacedDigit().foregroundStyle(.secondary)
                }
                .font(.callout)
                Button {
                    actions.clean()
                } label: {
                    HStack {
                        Text(summary.cleanTitle)
                        Spacer()
                        if let hotkey = summary.hotkey {
                            Text(hotkey).opacity(0.8)
                        }
                    }
                    .frame(maxWidth: .infinity)
                }
                .buttonStyle(.borderedProminent)
                .controlSize(.large)
            } else {
                Text(strings.text("menubar.noPicture")).foregroundStyle(.secondary)
            }
            HStack {
                Button(strings.text("menubar.open"), action: actions.open)
                Button(strings.text("menubar.refresh"), action: actions.refresh)
            }
            Divider()
            Text(summary?.status ?? "").font(.caption).foregroundStyle(.secondary).lineLimit(2)
            Toggle(strings.text("menubar.background"), isOn: Binding(
                get: { model.overview?.profile != .off },
                set: { actions.background($0) }))
                .toggleStyle(.symbolCheckbox)
            HStack {
                Button(strings.text("menubar.settings"), action: actions.settings)
                Spacer()
                Button(strings.text("menubar.quit"), action: actions.quit)
            }
            .buttonStyle(.link)
        }
        .padding(14)
        .frame(width: 360)
    }

    private var summary: MenuBarSummary? {
        model.overview.map {
            MenuBarSummary.make(
                $0, volume: model.volume.name, mode: model.hotkeys?.mode ?? .confirm,
                hotkey: model.hotkeys?.bindings[.clean], status: model.status, model.names)
        }
    }
}

/// What the panel's buttons do; the app supplies them.
struct PanelActions {
    var clean: () -> Void = {}
    var open: () -> Void = {}
    var refresh: () -> Void = {}
    var background: (Bool) -> Void = { _ in }
    var settings: () -> Void = {}
    var quit: () -> Void = {}
}

/// The Settings window: General, Hotkeys, Exclusions, Access.
struct SettingsView: View {
    let model: AppModel
    /// The tab shown: the bridge's, so that whoever opens Settings can
    /// choose it (the capture shows each tab of the real window).
    @Binding var chosen: SettingsTab

    var body: some View {
        TabView(selection: $chosen) {
            Group {
                if let general = model.general {
                    GeneralPane(general: general, strings: model.strings)
                }
            }
            .tabItem { Label(model.strings.text("settings.general"), systemImage: "gearshape") }
            .tag(SettingsTab.general)
            Group {
                if let hotkeys = model.hotkeys {
                    HotkeysPane(hotkeys: hotkeys, names: model.names)
                }
            }
            .tabItem { Label(model.strings.text("settings.hotkeys"), systemImage: "keyboard") }
            .tag(SettingsTab.hotkeys)
            Group {
                if let exclusions = model.exclusions {
                    ExclusionsPane(exclusions: exclusions, strings: model.strings)
                }
            }
            .tabItem { Label(model.strings.text("settings.exclusions"), systemImage: "nosign") }
            .tag(SettingsTab.exclusions)
            AccessPane(access: model.access, strings: model.strings)
                .tabItem { Label(model.strings.text("settings.access"), systemImage: "lock.shield") }
                .tag(SettingsTab.access)
        }
        // Tall enough for the longest tab (Hotkeys, in Russian) without
        // scrolling; the window keeps one size across the tabs.
        .frame(width: 560, height: 500)
        .task {
            await model.general?.load()
            await model.exclusions?.load()
            await model.access.check()
        }
    }
}

enum SettingsTab: Hashable, CaseIterable {
    case general, hotkeys, exclusions, access
}

struct GeneralPane: View {
    let general: GeneralModel
    let strings: Strings

    var body: some View {
        Form {
            Toggle(isOn: Binding(get: { general.settings?.background ?? true },
                                 set: { on in Task { await general.setBackground(on) } })) {
                Titled(strings.text("general.background"), note: strings.text("general.backgroundNote"))
            }
            Toggle(strings.text("general.battery"), isOn: Binding(
                get: { general.settings?.reduceOnBattery ?? true },
                set: { on in Task { await general.setReduceOnBattery(on) } }))
            Toggle(strings.text("general.login"), isOn: Binding(
                get: { general.launchAtLogin }, set: { general.setLaunchAtLogin($0) }))
            if let failure = general.loginFailure {
                Text(failure).font(.caption).foregroundStyle(.red)
            }
            DaysField(title: strings.text("general.stale"), value: strings.format("general.days", general.staleDays)) { step in
                Task { await general.setStaleDays(general.staleDays + step) }
            }
            DaysField(title: strings.text("general.recent"), value: strings.format("general.days", general.recentDays)) { step in
                Task { await general.setRecentDays(general.recentDays + step) }
            }
            Toggle(isOn: Binding(get: { general.showInMenuBar }, set: { general.setShowInMenuBar($0) })) {
                Titled(strings.text("general.menuBar"), note: strings.text("general.menuBarNote"))
            }
        }
        .formStyle(.grouped)
        .toggleStyle(.symbolCheckbox)
    }
}

struct HotkeysPane: View {
    let hotkeys: HotkeysModel
    let names: Names
    private var strings: Strings { names.strings }

    var body: some View {
        Form {
            Section {
                recorder(.clean, title: strings.text("hotkeys.clean"))
                recorder(.show, title: strings.text("hotkeys.show"))
            }
            Section(strings.text("hotkeys.mode")) {
                Picker(selection: Binding(
                    get: { hotkeys.mode == .immediate || hotkeys.askingForImmediate },
                    set: { immediate in Task { await hotkeys.choose(immediate ? .immediate : .confirm) } })) {
                    Text(strings.text("hotkeys.confirm")).tag(false)
                    VStack(alignment: .leading) {
                        Text(strings.text("hotkeys.immediate"))
                        Text(strings.text("hotkeys.immediateNote")).font(.caption).foregroundStyle(.secondary)
                    }
                    .tag(true)
                } label: {
                    EmptyView()
                }
                .pickerStyle(.radioGroup)
            }
            Section(strings.text("hotkeys.quick")) {
                ForEach(QuickCategory.all, id: \.self) { quick in
                    Toggle(names.category(quick.category), isOn: Binding(
                        get: { hotkeys.quick.contains(quick) },
                        set: { on in Task { await hotkeys.setQuick(quick, on) } }))
                        .disabled(hotkeys.mode != .immediate)
                }
                Text(strings.text("hotkeys.never")).font(.caption).foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
        .toggleStyle(.symbolCheckbox)
        .alert(strings.text("hotkeys.alertTitle"), isPresented: Binding(
            get: { hotkeys.askingForImmediate }, set: { if !$0 { hotkeys.cancelImmediate() } })) {
            Button(strings.text("preview.cancel"), role: .cancel) { hotkeys.cancelImmediate() }
            Button(strings.text("hotkeys.alertConfirm"), role: .destructive) {
                Task { await hotkeys.confirmImmediate() }
            }
        } message: {
            Text(strings.text("hotkeys.alertText"))
        }
    }

    private func recorder(_ action: HotkeyAction, title: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            LabeledContent(title) {
                HStack {
                    KeyRecorder(combo: hotkeys.bindings[action], strings: strings) { key, modifiers in
                        hotkeys.record(key: key, modifiers: modifiers, for: action)
                    }
                    Button(strings.text("hotkeys.clear")) { hotkeys.clear(action) }
                        .disabled(hotkeys.bindings[action] == nil)
                }
            }
            if let problem = hotkeys.problem(action) {
                Text(problem).font(.caption).foregroundStyle(.red)
            }
        }
    }
}

/// Records a key combination: click, then press the keys.
struct KeyRecorder: View {
    let combo: KeyCombo?
    let strings: Strings
    let recorded: (UInt32, KeyCombo.Modifiers) -> Void
    @State private var recording = false
    @FocusState private var focused: Bool

    var body: some View {
        Button {
            recording = true
            focused = true
        } label: {
            Text(recording ? strings.text("hotkeys.recording") : combo?.display ?? strings.text("hotkeys.none"))
                .frame(minWidth: 110)
        }
        .focused($focused)
        .onKeyPress(phases: .down) { press in
            guard recording else { return .ignored }
            recording = false
            recorded(KeyRecorder.code(press), KeyRecorder.modifiers(press.modifiers))
            return .handled
        }
        .help(strings.text("hotkeys.record"))
    }

    static func modifiers(_ flags: EventModifiers) -> KeyCombo.Modifiers {
        var modifiers: KeyCombo.Modifiers = []
        if flags.contains(.control) { modifiers.insert(.control) }
        if flags.contains(.option) { modifiers.insert(.option) }
        if flags.contains(.shift) { modifiers.insert(.shift) }
        if flags.contains(.command) { modifiers.insert(.command) }
        return modifiers
    }

    /// The virtual key code of the press, from the current event.
    static func code(_ press: KeyPress) -> UInt32 {
        UInt32(NSApp.currentEvent?.keyCode ?? 0)
    }
}

struct ExclusionsPane: View {
    let exclusions: ExclusionsModel
    let strings: Strings
    @State private var selected: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(strings.text("exclusions.title")).foregroundStyle(.secondary)
            List(selection: $selected) {
                if exclusions.rows.isEmpty {
                    Text(strings.text("exclusions.empty")).foregroundStyle(.secondary)
                }
                ForEach(exclusions.rows) { row in
                    VStack(alignment: .leading, spacing: 1) {
                        Text(row.shownPath).font(.system(.callout, design: .monospaced))
                            .lineLimit(1).truncationMode(.middle)
                        Text(row.hides).font(.caption).foregroundStyle(.secondary)
                    }
                    .tag(row.path)
                }
            }
            .frame(minHeight: 180)
            .dropDestination(for: URL.self) { urls, _ in
                Task { await exclusions.add(urls.map(\.path)) }
                return !urls.isEmpty
            }
            HStack {
                Button(strings.text("exclusions.add")) { choose() }
                Button(strings.text("exclusions.remove")) {
                    if let selected { Task { await exclusions.remove(selected) } }
                }
                .disabled(selected == nil)
                Spacer()
                Text(strings.text("exclusions.drop")).font(.caption).foregroundStyle(.secondary)
            }
        }
        .padding(20)
    }

    private func choose() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = true
        guard panel.runModal() == .OK else { return }
        let paths = panel.urls.map(\.path)
        Task { await exclusions.add(paths) }
    }
}

struct AccessPane: View {
    let access: AccessModel
    let strings: Strings

    var body: some View {
        Form {
            LabeledContent(strings.text("access.title")) {
                HStack(spacing: 6) {
                    Circle().fill(tint).frame(width: 8, height: 8).accessibilityHidden(true)
                    Text(state).foregroundStyle(tint)
                }
            }
            Text(strings.text("access.why")).fixedSize(horizontal: false, vertical: true)
            HStack {
                Button(strings.text("access.open")) { SystemSettings.openFullDiskAccess() }
                Button(strings.text("access.recheck")) { Task { await access.check() } }
            }
            Text(strings.text("access.restart")).font(.caption).foregroundStyle(.secondary)
            Text(strings.text("access.never")).font(.caption).foregroundStyle(.secondary)
        }
        .formStyle(.grouped)
    }

    private var state: String {
        switch access.state {
        case .granted: strings.text("access.granted")
        case .notGranted: strings.text("access.notGranted")
        case .unknown: strings.text("access.unknown")
        }
    }

    private var tint: Color {
        switch access.state {
        case .granted: .green
        case .notGranted: .orange
        case .unknown: .secondary
        }
    }
}

/// A setting's title with a note under it.
struct Titled: View {
    let title: String
    let note: String

    init(_ title: String, note: String) {
        self.title = title
        self.note = note
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title)
            Text(note).font(.caption).foregroundStyle(.secondary)
        }
    }
}
