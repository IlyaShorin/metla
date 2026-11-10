import Foundation

/// Numbers and dates as the app shows them, in the strings' locale.
public enum Format {
    /// Allocated bytes in decimal units, as Finder shows sizes.
    public static func bytes(_ value: UInt64, _ strings: Strings) -> String {
        let style = ByteCountFormatStyle(style: .file, spellsOutZero: false)
        return Int64(clamping: value).formatted(style.locale(strings.locale))
    }

    /// A count in a few characters: "1.9M".
    public static func count(_ value: UInt64, _ strings: Strings) -> String {
        Int64(clamping: value).formatted(.number.notation(.compactName).locale(strings.locale))
    }

    /// When something happened, relative to `now`: "2 min. ago".
    public static func relative(_ date: Date, now: Date, _ strings: Strings) -> String {
        let formatter = RelativeDateTimeFormatter()
        formatter.locale = strings.locale
        formatter.unitsStyle = .short
        return formatter.localizedString(for: min(date, now), relativeTo: now)
    }

    /// A Unix time in seconds as a date.
    public static func date(_ seconds: Int64) -> Date {
        Date(timeIntervalSince1970: TimeInterval(seconds))
    }

    /// A share of `total` as a fraction from 0 to 1.
    public static func share(_ part: UInt64, of total: UInt64) -> Double {
        total == 0 ? 0 : min(1, Double(part) / Double(total))
    }

    /// The path below the home directory as `~/…`; others as they are.
    public static func path(_ path: String, home: String) -> String {
        guard path.hasPrefix(home), home.count > 1 else {
            return path
        }
        let rest = path.dropFirst(home.count)
        if rest.isEmpty {
            return "~"
        }
        return rest.hasPrefix("/") ? "~" + rest : path
    }
}
