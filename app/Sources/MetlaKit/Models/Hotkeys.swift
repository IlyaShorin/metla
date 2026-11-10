import Foundation
import MetlaCoreFFI
import Observation

/// A global key combination: a virtual key code (Carbon's `kVK_…`) and
/// modifiers. Only combinations with ⌘, ⌥ or ⌃ can be made: a key with
/// no modifier (or only ⇧) would steal ordinary typing.
public struct KeyCombo: Equatable, Hashable, Sendable {
    public struct Modifiers: OptionSet, Hashable, Sendable {
        public let rawValue: UInt32
        public init(rawValue: UInt32) {
            self.rawValue = rawValue
        }

        public static let control = Modifiers(rawValue: 1 << 0)
        public static let option = Modifiers(rawValue: 1 << 1)
        public static let shift = Modifiers(rawValue: 1 << 2)
        public static let command = Modifiers(rawValue: 1 << 3)
    }

    public let key: UInt32
    public let modifiers: Modifiers

    /// `nil` without ⌘, ⌥ or ⌃.
    public init?(key: UInt32, modifiers: Modifiers) {
        guard !modifiers.isDisjoint(with: [.command, .option, .control]) else {
            return nil
        }
        self.key = key
        self.modifiers = modifiers
    }

    /// ⌃⌥⌘⌫: the default for Clean.
    public static let clean = KeyCombo(key: 0x33, modifiers: [.control, .option, .command])!

    /// "⌃⌥⌘⌫": modifiers in Apple's order, then the key.
    public var display: String {
        let marks: [(Modifiers, String)] = [(.control, "⌃"), (.option, "⌥"), (.shift, "⇧"), (.command, "⌘")]
        let prefix = marks.filter { modifiers.contains($0.0) }.map(\.1).joined()
        return prefix + KeyCombo.name(of: key)
    }

    /// As kept in the preferences: "modifiers:key".
    public var encoded: String {
        "\(modifiers.rawValue):\(key)"
    }

    /// A kept combination, or `nil` for anything else.
    public static func decode(_ text: String) -> KeyCombo? {
        let parts = text.split(separator: ":", omittingEmptySubsequences: false)
        guard parts.count == 2, let flags = UInt32(parts[0]), let key = UInt32(parts[1]),
              flags & ~0b1111 == 0
        else {
            return nil
        }
        return KeyCombo(key: key, modifiers: Modifiers(rawValue: flags))
    }

    /// The key's name on a US keyboard, as menus show it.
    public static func name(of key: UInt32) -> String {
        if let special = special[key] {
            return special
        }
        if let letter = letters[key] {
            return String(letter)
        }
        return "#\(key)"
    }

    static let special: [UInt32: String] = [
        0x33: "⌫", 0x75: "⌦", 0x24: "↩", 0x30: "⇥", 0x31: "Space", 0x35: "⎋",
        0x7B: "←", 0x7C: "→", 0x7D: "↓", 0x7E: "↑",
        0x7A: "F1", 0x78: "F2", 0x63: "F3", 0x76: "F4", 0x60: "F5", 0x61: "F6",
        0x62: "F7", 0x64: "F8", 0x65: "F9", 0x6D: "F10", 0x67: "F11", 0x6F: "F12",
    ]

    /// Letters, digits and punctuation by their ANSI key code.
    static let letters: [UInt32: Character] = {
        let layout = "asdfhgzxcv\0bqweryt123465=97-80]ou[ip\0lj'k;\\,/nm."
        var table: [UInt32: Character] = [:]
        for (code, char) in layout.enumerated() where char != "\0" {
            table[UInt32(code)] = Character(char.uppercased())
        }
        return table
    }()
}

/// The two actions a global hotkey can start.
public enum HotkeyAction: String, CaseIterable, Sendable {
    case clean
    case show
}

/// Whether a hotkey is registered with the system.
public enum Registration: Equatable, Sendable {
    case unassigned
    case registered
    /// The system refused it, usually because another app has it.
    case failed(status: Int32)
}

/// Registers global hotkeys (the app: Carbon's `RegisterEventHotKey`).
@MainActor
public protocol HotkeyRegistrar: AnyObject {
    /// Replaces what `action` is bound to; `nil` unbinds it.
    func register(_ combo: KeyCombo?, for action: HotkeyAction) -> Registration
}

/// Where the app keeps its own preferences (the hotkeys, the menu bar
/// switch, the first run). The engine keeps the rest.
public protocol Preferences: AnyObject, Sendable {
    func string(forKey key: String) -> String?
    func set(_ value: String?, forKey key: String)
}

/// The app's own preferences in `UserDefaults`.
public final class StandardPreferences: Preferences, @unchecked Sendable {
    // UserDefaults is documented as thread-safe.
    private let defaults: UserDefaults

    public init(_ defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    public func string(forKey key: String) -> String? {
        defaults.string(forKey: key)
    }

    public func set(_ value: String?, forKey key: String) {
        defaults.set(value, forKey: key)
    }
}

/// The Hotkeys settings: the two bindings with their registration, the
/// hotkey mode and its quick categories (kept by the engine). Turning on
/// "clean immediately" takes an explicit opt-in.
@MainActor
@Observable
public final class HotkeysModel {
    public private(set) var bindings: [HotkeyAction: KeyCombo] = [:]
    public private(set) var registrations: [HotkeyAction: Registration] = [:]
    /// The recorder refused a combination: why.
    public private(set) var refused: [HotkeyAction: String] = [:]
    public private(set) var mode: HotkeyMode = .confirm
    public private(set) var quick: Set<QuickCategory> = []
    /// "Clean immediately" was asked for and waits for the confirmation
    /// in the alert.
    public private(set) var askingForImmediate = false

    private let preferences: any Preferences
    private let registrar: any HotkeyRegistrar
    private let client: EngineClient?
    private let strings: Strings
    static let unbound = "none"

    public init(
        preferences: any Preferences, registrar: any HotkeyRegistrar, client: EngineClient?, strings: Strings
    ) {
        self.preferences = preferences
        self.registrar = registrar
        self.client = client
        self.strings = strings
        for action in HotkeyAction.allCases {
            bindings[action] = Self.kept(action, preferences)
        }
    }

    static func key(_ action: HotkeyAction) -> String {
        "hotkey." + action.rawValue
    }

    /// What is kept for `action`: its combination, unbound, or the default.
    static func kept(_ action: HotkeyAction, _ preferences: any Preferences) -> KeyCombo? {
        switch preferences.string(forKey: key(action)) {
        case unbound?: nil
        case let text?: KeyCombo.decode(text) ?? fallback(action)
        case nil: fallback(action)
        }
    }

    static func fallback(_ action: HotkeyAction) -> KeyCombo? {
        action == .clean ? .clean : nil
    }

    /// Registers every binding with the system.
    public func registerAll() {
        for action in HotkeyAction.allCases {
            registrations[action] = registrar.register(bindings[action], for: action)
        }
    }

    /// Reads the mode and quick categories the engine keeps.
    public func load() async {
        guard let client else {
            return
        }
        let settings = await client.settings()
        mode = settings.hotkey
        quick = Set(settings.quick)
    }

    /// The recorder caught `key` with `modifiers` for `action`.
    public func record(key: UInt32, modifiers: KeyCombo.Modifiers, for action: HotkeyAction) {
        guard let combo = KeyCombo(key: key, modifiers: modifiers) else {
            refused[action] = strings.text("hotkeys.needsModifier")
            return
        }
        refused[action] = nil
        bind(combo, to: action)
    }

    /// The recorder's clear button.
    public func clear(_ action: HotkeyAction) {
        refused[action] = nil
        bind(nil, to: action)
    }

    private func bind(_ combo: KeyCombo?, to action: HotkeyAction) {
        bindings[action] = combo
        preferences.set(combo?.encoded ?? Self.unbound, forKey: Self.key(action))
        registrations[action] = registrar.register(combo, for: action)
    }

    /// What to say next to a recorder: nothing, or why it is not working.
    public func problem(_ action: HotkeyAction) -> String? {
        if let refused = refused[action] {
            return refused
        }
        if case .failed(let status)? = registrations[action] {
            return strings.format("hotkeys.taken", String(status))
        }
        return nil
    }

    /// "Show and confirm" needs nothing; "Clean immediately" first asks.
    public func choose(_ wanted: HotkeyMode) async {
        switch wanted {
        case .confirm:
            askingForImmediate = false
            await save(mode: .confirm, quick: quick)
        case .immediate:
            askingForImmediate = mode != .immediate
        }
    }

    /// The alert's destructive button: clean immediately from now on.
    public func confirmImmediate() async {
        guard askingForImmediate else {
            return
        }
        askingForImmediate = false
        await save(mode: .immediate, quick: quick)
    }

    /// The alert's Cancel: the mode stays as it was.
    public func cancelImmediate() {
        askingForImmediate = false
    }

    /// Checks or unchecks a quick category; there is no way to add
    /// anything else.
    public func setQuick(_ category: QuickCategory, _ on: Bool) async {
        var chosen = quick
        if on {
            chosen.insert(category)
        } else {
            chosen.remove(category)
        }
        await save(mode: mode, quick: chosen)
    }

    private func save(mode: HotkeyMode, quick: Set<QuickCategory>) async {
        guard let client else {
            return
        }
        var settings = await client.settings()
        settings.hotkey = mode
        settings.quick = QuickCategory.all.filter(quick.contains)
        try? await client.setSettings(settings)
        self.mode = mode
        self.quick = quick
    }
}

extension QuickCategory {
    /// Every quick category, in the engine's order.
    public static let all: [QuickCategory] = [.appCache, .devCache, .buildArtifact]

    /// The category it is.
    public var category: Category {
        switch self {
        case .appCache: .appCache
        case .devCache: .devCache
        case .buildArtifact: .buildArtifact
        }
    }
}
