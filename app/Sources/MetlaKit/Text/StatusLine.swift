import Foundation
import MetlaCoreFFI

/// The toolbar's scan status: a coloured dot and a few words, for every
/// state the engine can be in. A picture that is incomplete, deferred or
/// out of date never reads as current.
public struct StatusLine: Equatable, Sendable {
    /// The dot's colour.
    public enum Tone: Equatable, Sendable {
        /// A scan or an update runs.
        case working
        /// Complete and followed.
        case current
        /// Partial, changes waiting, a full scan needed, or paused.
        case stale
        /// The last scan or update failed.
        case problem
        /// Nothing is watched.
        case off
    }

    public let text: String
    public let tone: Tone
    /// A Stop button belongs next to it.
    public let canStop: Bool

    /// The line for `freshness`; `expected` is the size the scan is
    /// heading for (the last picture's), if known.
    public static func of(
        _ freshness: Freshness, expected: UInt64?, now: Date, _ strings: Strings
    ) -> StatusLine {
        switch freshness.state {
        case .scanning(let progress, _):
            let refreshing = freshness.picture != .missing
            return StatusLine(
                text: scanning(progress, expected, refreshing: refreshing, strings), tone: .working, canStop: true)
        case .updating:
            return StatusLine(text: strings.text("status.updating"), tone: .working, canStop: false)
        case .cleaning:
            return StatusLine(text: strings.text("status.cleaning"), tone: .working, canStop: false)
        case .paused:
            return StatusLine(text: strings.text("status.paused"), tone: .stale, canStop: false)
        case .failed(let failure):
            return StatusLine(text: failed(failure, strings), tone: .problem, canStop: false)
        case .off:
            return StatusLine(text: strings.text("status.off"), tone: .off, canStop: false)
        case .idle:
            return idle(freshness, now: now, strings)
        }
    }

    /// "Scanning 61% · 1.9M files", without the share when the size it is
    /// heading for is not known; "Refreshing …" when a picture exists,
    /// which stays shown and usable meanwhile.
    static func scanning(
        _ progress: ProgressSnapshot, _ expected: UInt64?, refreshing: Bool, _ strings: Strings
    ) -> String {
        let files = strings.format("status.files", Int(clamping: progress.files), Format.count(progress.files, strings))
        let key = refreshing ? "status.refreshing" : "status.scanning"
        guard let expected, expected > 0 else {
            return strings.format(key, files)
        }
        let share = Int(Format.share(progress.bytes, of: expected) * 100)
        return strings.format(key + "Share", min(share, 99), files)
    }

    static func failed(_ failure: Failure, _ strings: Strings) -> String {
        switch failure {
        case .noSpace: strings.text("status.noSpace")
        case .index: strings.text("status.indexFailed")
        case .scan: strings.text("status.scanFailed")
        }
    }

    /// "Updated 2 min ago", marked when the picture is partial or needs a
    /// full scan; "Never scanned" without a picture.
    static func idle(_ freshness: Freshness, now: Date, _ strings: Strings) -> StatusLine {
        guard let updated = freshness.updated, freshness.picture != .missing else {
            return StatusLine(text: strings.text("status.never"), tone: .stale, canStop: false)
        }
        let when = Format.relative(Format.date(updated), now: now, strings)
        var text = strings.format("status.updated", when)
        if freshness.picture == .partial {
            text += strings.text("status.partialSuffix")
        }
        if freshness.needsFullScan {
            text += strings.text("status.outdatedSuffix")
        }
        let current = freshness.picture == .complete && !freshness.needsFullScan && freshness.watching
        return StatusLine(text: text, tone: current ? .current : .stale, canStop: false)
    }
}
