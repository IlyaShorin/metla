import Foundation
import Testing
@testable import MetlaKit

/// Every key the code uses has a text in English and in Russian, and the
/// two languages have the same keys.
@Suite struct StringsTests {
    static let sources: URL = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        .appendingPathComponent("Sources")

    /// `text("…")` and `format("…"` keys in the sources.
    static func usedKeys() throws -> Set<String> {
        var keys = Set<String>()
        let files = FileManager.default.enumerator(at: sources, includingPropertiesForKeys: nil)
        let pattern = try Regex(#"(?:text|format)\("([a-zA-Z]+\.[a-zA-Z.]+)""#)
        while let file = files?.nextObject() as? URL {
            guard file.pathExtension == "swift", !file.path.contains("/generated/") else { continue }
            let source = try String(contentsOf: file, encoding: .utf8)
            for match in source.matches(of: pattern) {
                if let key = match.output[1].substring {
                    keys.insert(String(key))
                }
            }
        }
        return keys
    }

    static func keys(_ language: String) throws -> Set<String> {
        let folder = sources.appendingPathComponent("MetlaKit/Resources/\(language).lproj")
        let strings = try PropertyListSerialization.propertyList(
            from: Data(contentsOf: folder.appendingPathComponent("Localizable.strings")), format: nil)
        let plurals = try PropertyListSerialization.propertyList(
            from: Data(contentsOf: folder.appendingPathComponent("Localizable.stringsdict")), format: nil)
        let all = Set((strings as? [String: String] ?? [:]).keys)
            .union((plurals as? [String: Any] ?? [:]).keys)
        return all.filter { !$0.hasPrefix("rule.") }
    }

    @Test func everyKeyUsedHasATextInBothLanguages() throws {
        let used = try Self.usedKeys()
        #expect(used.count > 60)
        let english = try Self.keys("en")
        let russian = try Self.keys("ru")
        let generated = Set(["category.appCache", "category.devCache", "category.buildArtifact",
                             "category.log", "category.gitWorktree", "category.temp"])
        #expect(used.union(generated).subtracting(english).isEmpty, "missing in English: \(used.subtracting(english))")
        #expect(english == russian, "only English: \(english.subtracting(russian)), only Russian: \(russian.subtracting(english))")
    }

    @Test func pluralsFollowEachLanguagesRules() {
        let english = Strings(language: "en", region: "US")
        let russian = Strings(language: "ru", region: "RU")
        #expect(english.format("status.files", 1, "1") == "1 file")
        #expect(english.format("status.files", 3, "3") == "3 files")
        #expect(russian.format("status.files", 1, "1") == "1 файл")
        #expect(russian.format("status.files", 3, "3") == "3 файла")
        #expect(russian.format("status.files", 1_900_000, "1,9\u{00A0}млн") == "1,9\u{00A0}млн файлов")
        #expect(russian.format("overview.partialDenied", 14) == "Нет доступа к 14 папкам.")
        #expect(english.format("overview.partialDenied", 1) == "No access to 1 folder.")
        #expect(russian.format("preview.titleTrash", 2, "4 МБ") == "Переместить 2 объекта в Корзину — около 4 МБ?")
        #expect(russian.format("preview.title", 9, "2 МБ") == "Удалить 9 объектов — около 2 МБ?")
    }

    /// The app's bundle holds MetlaKit's resources in its Resources; the
    /// strings come from there, not from the package build's directory.
    @Test func theResourcesInsideTheAppAreTheOnesRead() throws {
        let app = Fixture.scratch().appendingPathComponent("Test.app")
        let resources = app.appendingPathComponent("Contents/Resources")
        try FileManager.default.createDirectory(at: resources, withIntermediateDirectories: true)
        try Data("<plist version=\"1.0\"><dict/></plist>".utf8).write(to: app.appendingPathComponent("Contents/Info.plist"))
        let inside = resources.appendingPathComponent("Metla_MetlaKit.bundle")
        try FileManager.default.copyItem(at: Bundle.module.bundleURL, to: inside)
        let found = Strings.bundle(inside: try #require(Bundle(url: app)))
        #expect(found.bundleURL.resolvingSymlinksInPath().path == inside.resolvingSymlinksInPath().path)
        let bare = try #require(Bundle(url: Fixture.scratch()))
        #expect(Strings.bundle(inside: bare).bundleURL == Bundle.module.bundleURL, "the package's, in a test")
    }

    @Test func aKeyWithoutATextReadsAsItself() {
        #expect(Strings(language: "en", region: "US").text("no.such.key") == "no.such.key")
    }

    @Test func theUsersPreferredLanguageIsChosenAmongThoseTheAppHas() {
        #expect(Strings.preferred(among: ["en", "ru"], ["ru-RU", "en"]) == "ru")
        #expect(Strings.preferred(among: ["en", "ru"], ["de-DE"]) == "en")
        #expect(Strings(language: "ru", region: "RU").locale.language.languageCode == "ru")
    }
}
