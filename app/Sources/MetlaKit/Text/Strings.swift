import Foundation

/// The app's words in one language, from MetlaKit's resources: English is
/// the base, Russian a translation. Keys are the English text's id.
public struct Strings: Sendable {
    let bundle: Bundle
    public let locale: Locale

    /// The language the system prefers among those the app has.
    public static let current = Strings(language: nil)

    /// `language` is "en" or "ru"; `nil` takes the first of the user's
    /// preferred languages the app has. Numbers and dates follow the same
    /// language, in `region` (the user's by default).
    public init(language: String?, region: String? = nil) {
        let base = Strings.resources
        let chosen = language ?? Strings.preferred(among: base.localizations, Locale.preferredLanguages)
        let path = base.path(forResource: chosen, ofType: "lproj")
        bundle = path.flatMap(Bundle.init(path:)) ?? base
        let place = region.map { Locale.Region($0) } ?? Locale.current.region
        locale = Locale(languageCode: Locale.LanguageCode(chosen), languageRegion: place)
    }

    /// The first of `preferences` the app has a translation for; English
    /// otherwise.
    static func preferred(among available: [String], _ preferences: [String]) -> String {
        Bundle.preferredLocalizations(from: available, forPreferences: preferences)
            .first { $0 != "Base" } ?? "en"
    }

    /// The text for `key`; the key itself when it has none.
    public func text(_ key: String) -> String {
        bundle.localizedString(forKey: key, value: key, table: nil)
    }

    /// The text for `key` with `arguments` put in, plural rules applied.
    public func format(_ key: String, _ arguments: any CVarArg...) -> String {
        String(format: text(key), locale: locale, arguments: arguments)
    }

    /// MetlaKit's resource bundle: inside the app's Resources, or where
    /// the package build put it.
    static let resources: Bundle = {
        let inApp = Bundle.main.resourceURL?.appendingPathComponent("Metla_MetlaKit.bundle")
        return inApp.flatMap(Bundle.init(url:)) ?? Bundle.module
    }()
}
