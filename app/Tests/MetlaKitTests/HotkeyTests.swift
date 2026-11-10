import Foundation
import MetlaCoreFFI
import Testing
@testable import MetlaKit

/// A registrar that answers as told and remembers what it was asked.
@MainActor
final class FakeRegistrar: HotkeyRegistrar {
    var taken: Set<KeyCombo> = []
    var asked: [(KeyCombo?, HotkeyAction)] = []

    func register(_ combo: KeyCombo?, for action: HotkeyAction) -> Registration {
        asked.append((combo, action))
        guard let combo else { return .unassigned }
        return taken.contains(combo) ? .failed(status: -9878) : .registered
    }
}

@MainActor
@Suite struct HotkeyTests {
    let strings = Strings(language: "en", region: "US")

    @Test func aCombinationNeedsCommandOptionOrControl() {
        #expect(KeyCombo(key: 0, modifiers: []) == nil)
        #expect(KeyCombo(key: 0, modifiers: [.shift]) == nil, "shift alone would steal typing")
        #expect(KeyCombo(key: 0, modifiers: [.option]) != nil)
        #expect(KeyCombo(key: 0, modifiers: [.command, .shift]) != nil)
        #expect(KeyCombo(key: 0, modifiers: [.control]) != nil)
    }

    @Test func combinationsReadAsMenusShowThem() throws {
        #expect(KeyCombo.clean.display == "⌃⌥⌘⌫")
        let all = try #require(KeyCombo(key: 0x2E, modifiers: [.command, .shift, .option, .control]))
        #expect(all.display == "⌃⌥⇧⌘M", "Apple's order")
        #expect(KeyCombo(key: 0x12, modifiers: [.command])?.display == "⌘1")
        #expect(KeyCombo(key: 0x7A, modifiers: [.option])?.display == "⌥F1")
        #expect(KeyCombo(key: 0x31, modifiers: [.control])?.display == "⌃Space")
        #expect(KeyCombo(key: 0x2A, modifiers: [.control])?.display == "⌃\\")
        #expect(KeyCombo.name(of: 0x0A) == "#10", "a key with no name says its code")
        #expect(KeyCombo.name(of: 0x24) == "↩")
    }

    @Test func combinationsAreKeptAndReadBackExactly() {
        for combo in [KeyCombo.clean, KeyCombo(key: 0x2E, modifiers: [.command, .shift])!] {
            #expect(KeyCombo.decode(combo.encoded) == combo)
        }
        #expect(KeyCombo.clean.encoded == "11:51")
        for broken in ["", "11", "11:", ":51", "x:51", "11:y", "4:51", "16:51", "11:51:1"] {
            #expect(KeyCombo.decode(broken) == nil, "\(broken)")
        }
    }

    @Test func bindingsStartAsDefaultsAndAreKept() {
        let preferences = MemoryPreferences()
        let registrar = FakeRegistrar()
        let model = HotkeysModel(preferences: preferences, registrar: registrar, client: nil, strings: strings)
        #expect(model.bindings[.clean] == .clean && model.bindings[.show] == nil, "Clean ⌃⌥⌘⌫, Show unassigned")
        model.registerAll()
        #expect(model.registrations == [.clean: .registered, .show: .unassigned])
        model.record(key: 0x2E, modifiers: [.command, .option], for: .show)
        model.clear(.clean)
        let again = HotkeysModel(preferences: preferences, registrar: registrar, client: nil, strings: strings)
        #expect(again.bindings[.show]?.display == "⌥⌘M")
        #expect(again.bindings[.clean] == nil, "a cleared binding stays cleared")
        preferences.set("garbage", forKey: "hotkey.clean")
        #expect(HotkeysModel.kept(.clean, preferences) == .clean, "what cannot be read is the default")
    }

    @Test func theRecorderRefusesAKeyWithoutAModifierAndSaysWhy() {
        let registrar = FakeRegistrar()
        let model = HotkeysModel(preferences: MemoryPreferences(), registrar: registrar, client: nil, strings: strings)
        model.record(key: 0x2E, modifiers: [.shift], for: .clean)
        #expect(model.problem(.clean) == "Use ⌘, ⌥ or ⌃ with the key.")
        #expect(model.bindings[.clean] == .clean, "the old binding stays")
        #expect(registrar.asked.isEmpty, "nothing was registered")
        model.record(key: 0x2E, modifiers: [.control], for: .clean)
        #expect(model.problem(.clean) == nil)
    }

    @Test func aCombinationAnotherAppHoldsIsShownNotSwallowed() throws {
        let registrar = FakeRegistrar()
        let taken = try #require(KeyCombo(key: 0x2E, modifiers: [.command]))
        registrar.taken = [taken]
        let model = HotkeysModel(preferences: MemoryPreferences(), registrar: registrar, client: nil, strings: strings)
        model.record(key: taken.key, modifiers: taken.modifiers, for: .show)
        #expect(model.registrations[.show] == .failed(status: -9878))
        #expect(model.problem(.show) == "Not available: another app uses it (error -9878).")
    }

    @Test func cleaningImmediatelyTakesAnExplicitOptIn() async throws {
        let model = try await started(Fixture.settings())
        defer { model.stop() }
        await model.attachHotkeys(FakeRegistrar())
        let hotkeys = try #require(model.hotkeys)
        #expect(hotkeys.mode == .confirm, "the default")
        await hotkeys.choose(.immediate)
        #expect(hotkeys.askingForImmediate && hotkeys.mode == .confirm, "asked, not switched")
        hotkeys.cancelImmediate()
        #expect(!hotkeys.askingForImmediate && hotkeys.mode == .confirm)
        await hotkeys.confirmImmediate()
        #expect(hotkeys.mode == .confirm, "no confirmation without the question")
        await hotkeys.choose(.immediate)
        await hotkeys.confirmImmediate()
        #expect(hotkeys.mode == .immediate)
        await hotkeys.setQuick(.buildArtifact, true)
        await hotkeys.setQuick(.appCache, true)
        await hotkeys.setQuick(.buildArtifact, false)
        let kept = try await #require(model.client).settings()
        #expect(kept.hotkey == .immediate && kept.quick == [.appCache], "kept by the engine")
        await hotkeys.choose(.confirm)
        #expect(hotkeys.mode == .confirm && !hotkeys.askingForImmediate)
        #expect(await model.cleanHotkey() == .showPreview, "showing and confirming")
    }
}
