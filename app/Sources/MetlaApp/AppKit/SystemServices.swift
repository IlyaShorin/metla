import AppKit
import Carbon.HIToolbox
import IOKit.ps
import MetlaKit
import ServiceManagement
import SwiftUI

/// Global hotkeys through Carbon's `RegisterEventHotKey`: no Accessibility
/// permission, and they work while the app is in the background. A thin
/// adapter: the combinations, their names and their keeping are
/// MetlaKit's (`KeyCombo`, `HotkeysModel`).
@MainActor
final class HotkeyCenter: HotkeyRegistrar {
    private var refs: [HotkeyAction: EventHotKeyRef] = [:]
    private var handler: EventHandlerRef?
    private let pressed: @MainActor (HotkeyAction) -> Void
    /// Carbon's signature for Metla's hotkeys ("Mtla").
    static let signature: OSType = 0x4D74_6C61

    init(pressed: @escaping @MainActor (HotkeyAction) -> Void) {
        self.pressed = pressed
        install()
    }

    private func install() {
        var spec = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        let me = Unmanaged.passUnretained(self).toOpaque()
        InstallEventHandler(GetApplicationEventTarget(), { _, event, context in
            var id = EventHotKeyID()
            GetEventParameter(
                event, EventParamName(kEventParamDirectObject), EventParamType(typeEventHotKeyID), nil,
                MemoryLayout<EventHotKeyID>.size, nil, &id)
            guard let context, let action = HotkeyCenter.action(id.id) else { return noErr }
            let center = Unmanaged<HotkeyCenter>.fromOpaque(context).takeUnretainedValue()
            MainActor.assumeIsolated { center.pressed(action) }
            return noErr
        }, 1, &spec, me, &handler)
    }

    static func action(_ id: UInt32) -> HotkeyAction? {
        HotkeyAction.allCases.indices.contains(Int(id)) ? HotkeyAction.allCases[Int(id)] : nil
    }

    func register(_ combo: KeyCombo?, for action: HotkeyAction) -> Registration {
        if let old = refs.removeValue(forKey: action) {
            UnregisterEventHotKey(old)
        }
        guard let combo else {
            return .unassigned
        }
        let index = UInt32(HotkeyAction.allCases.firstIndex(of: action) ?? 0)
        var ref: EventHotKeyRef?
        let status = RegisterEventHotKey(
            combo.key, Self.carbon(combo.modifiers), EventHotKeyID(signature: Self.signature, id: index),
            GetApplicationEventTarget(), 0, &ref)
        guard status == noErr, let ref else {
            return .failed(status: status)
        }
        refs[action] = ref
        return .registered
    }

    static func carbon(_ modifiers: KeyCombo.Modifiers) -> UInt32 {
        var flags = 0
        if modifiers.contains(.command) { flags |= cmdKey }
        if modifiers.contains(.option) { flags |= optionKey }
        if modifiers.contains(.control) { flags |= controlKey }
        if modifiers.contains(.shift) { flags |= shiftKey }
        return UInt32(flags)
    }
}

/// The small panel the hotkey shows: it never takes focus and goes away
/// by itself.
@MainActor
final class HUD {
    private var panel: NSPanel?
    private var hide: Task<Void, Never>?

    func show(_ message: HUDMessage, strings: Strings, report: @escaping () -> Void) {
        let content = NSHostingView(rootView: HUDView(message: message, strings: strings) { [weak self] in
            self?.close()
            report()
        })
        let panel = self.panel ?? NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 300, height: 120),
            styleMask: [.nonactivatingPanel, .hudWindow, .utilityWindow, .titled, .fullSizeContentView],
            backing: .buffered, defer: false)
        // No empty title bar band above the message: the panel has no
        // title and no buttons.
        panel.titlebarAppearsTransparent = true
        panel.titleVisibility = .hidden
        content.safeAreaRegions = []
        panel.isFloatingPanel = true
        panel.level = .statusBar
        panel.hidesOnDeactivate = false
        panel.contentView = content
        panel.setContentSize(content.fittingSize)
        if let screen = NSScreen.main?.visibleFrame {
            panel.setFrameOrigin(NSPoint(x: screen.maxX - panel.frame.width - 20, y: screen.maxY - panel.frame.height - 20))
        }
        panel.orderFrontRegardless()
        self.panel = panel
        hide?.cancel()
        hide = Task { [weak self] in
            try? await Task.sleep(for: .seconds(6))
            if !Task.isCancelled { self?.close() }
        }
    }

    func close() {
        panel?.orderOut(nil)
    }
}

struct HUDView: View {
    let message: HUDMessage
    let strings: Strings
    let report: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(message.title).font(.headline)
            ForEach(message.lines, id: \.self) { Text($0).monospacedDigit() }
            if message.report {
                Button(strings.text("hud.showReport"), action: report).buttonStyle(.link)
            }
        }
        .padding(16)
        .frame(width: 280, alignment: .leading)
    }
}

/// The Mac's power state from IOKit's power-source notifications, Low
/// Power Mode and the thermal state; it calls back only when one changes.
/// No timers, no polling.
@MainActor
final class PowerMonitor {
    private let changed: @MainActor (PowerState) -> Void
    private var source: CFRunLoopSource?
    private var observers: [NSObjectProtocol] = []

    init(changed: @escaping @MainActor (PowerState) -> Void) {
        self.changed = changed
        let me = Unmanaged.passUnretained(self).toOpaque()
        if let source = IOPSNotificationCreateRunLoopSource({ context in
            guard let context else { return }
            let monitor = Unmanaged<PowerMonitor>.fromOpaque(context).takeUnretainedValue()
            MainActor.assumeIsolated { monitor.tell() }
        }, me)?.takeRetainedValue() {
            CFRunLoopAddSource(CFRunLoopGetMain(), source, .defaultMode)
            self.source = source
        }
        let center = NotificationCenter.default
        for name in [Notification.Name.NSProcessInfoPowerStateDidChange, ProcessInfo.thermalStateDidChangeNotification] {
            observers.append(center.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { self?.tell() }
            })
        }
    }

    /// Tells the current state.
    func tell() {
        changed(Self.now())
    }

    static func now() -> PowerState {
        let info = IOPSCopyPowerSourcesInfo()?.takeRetainedValue()
        let providing = info.flatMap { IOPSGetProvidingPowerSourceType($0)?.takeUnretainedValue() as String? }
        let process = ProcessInfo.processInfo
        let thermal: PowerState.Thermal = switch process.thermalState {
        case .nominal: .nominal
        case .fair: .fair
        case .serious: .serious
        case .critical: .critical
        @unknown default: .serious
        }
        return PowerState(
            onBattery: providing == kIOPMBatteryPowerKey, lowPowerMode: process.isLowPowerModeEnabled,
            thermal: thermal)
    }
}

/// Launch at login through `SMAppService.mainApp`.
@MainActor
final class SystemLoginItem: LoginItem {
    var enabled: Bool {
        SMAppService.mainApp.status == .enabled
    }

    func set(_ enabled: Bool) throws {
        if enabled {
            try SMAppService.mainApp.register()
        } else {
            try SMAppService.mainApp.unregister()
        }
    }
}
