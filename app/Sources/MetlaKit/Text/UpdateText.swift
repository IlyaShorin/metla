import Foundation

/// What the sidebar's strip, the menu-bar panel's line and Settings show of
/// an offer: one title, an optional note, progress, and the buttons.
public struct UpdateShown: Equatable, Sendable {
    public enum Action: Equatable, Sendable {
        case install(enabled: Bool)
        case whatsNew(URL)
        case later
        case releasePage(URL)
    }

    public enum Progress: Equatable, Sendable {
        /// The share done, 0 to 1.
        case share(Double)
        /// Under way, no measure.
        case working
    }

    public let title: String
    /// The reason the copy cannot install, or that a cleanup runs.
    public let note: String?
    public let progress: Progress?
    public let actions: [Action]

    /// The offer, as the copy can act on it; nil without one.
    public static func of(
        _ offer: UpdateModel.Offer?, installation: Installation, cleanupRunning: Bool, _ strings: Strings
    ) -> UpdateShown? {
        guard let offer else {
            return nil
        }
        let release = offer.release
        let available = strings.format("update.available", release.version.description)
        switch offer.phase {
        case .offered:
            if let block = installation.block {
                return UpdateShown(
                    title: available, note: text(block, strings), progress: nil,
                    actions: [.releasePage(release.page), .later])
            }
            let notes = release.notes.map { [Action.whatsNew($0)] } ?? []
            return UpdateShown(
                title: available, note: cleanupRunning ? strings.text("update.afterCleanup") : nil, progress: nil,
                actions: [.install(enabled: !cleanupRunning)] + notes + [.later])
        case .downloading(let received, let total):
            let title = strings.format(
                "update.downloading", Format.bytes(UInt64(max(0, received)), strings),
                Format.bytes(UInt64(max(0, total)), strings))
            let share = Format.share(UInt64(max(0, received)), of: UInt64(max(0, total)))
            return UpdateShown(title: title, note: nil, progress: .share(share), actions: [])
        case .verifying:
            return UpdateShown(title: strings.text("update.verifying"), note: nil, progress: .working, actions: [])
        case .installing:
            return UpdateShown(title: strings.text("update.installing"), note: nil, progress: .working, actions: [])
        case .failed(let failure):
            return UpdateShown(
                title: text(failure, strings), note: nil, progress: nil, actions: [.releasePage(release.page), .later])
        }
    }

    /// Why an install did not happen, in one sentence.
    public static func text(_ failure: UpdateFailure, _ strings: Strings) -> String {
        switch failure {
        case .noConnection: strings.text("update.failed.noConnection")
        case .downloadFailed: strings.text("update.failed.downloadFailed")
        case .mismatch: strings.text("update.failed.mismatch")
        case .notSigned: strings.text("update.failed.notSigned")
        case .cannotWrite: strings.text("update.failed.cannotWrite")
        case .cleanupRunning: strings.text("update.failed.cleanupRunning")
        }
    }

    /// Why this copy cannot replace itself, in one line.
    public static func text(_ block: InstallBlock, _ strings: Strings) -> String {
        switch block {
        case .adHoc: strings.text("update.block.adHoc")
        case .translocated: strings.text("update.block.translocated")
        case .readOnlyVolume: strings.text("update.block.readOnlyVolume")
        case .notWritable: strings.text("update.block.notWritable")
        }
    }

    /// Settings: the running version and build, "0.8.0 (11)".
    public static func version(_ installation: Installation) -> String {
        "\(installation.version) (\(installation.build))"
    }

    /// Settings: when a check last got an answer.
    public static func lastCheck(_ date: Date?, now: Date, _ strings: Strings) -> String {
        guard let date else {
            return strings.text("update.neverChecked")
        }
        return strings.format("update.lastChecked", Format.relative(date, now: now, strings))
    }

    /// Settings: what the last check found when it offered nothing, or
    /// that one runs.
    public static func outcome(_ outcome: UpdateModel.Outcome?, checking: Bool, _ strings: Strings) -> String? {
        if checking {
            return strings.text("update.checking")
        }
        switch outcome {
        case .upToDate?: return strings.text("update.upToDate")
        case .failed(.noConnection)?: return strings.text("update.check.noConnection")
        case .failed(.answered(let status))?: return strings.format("update.check.answered", status)
        case .failed(.unreadable)?: return strings.text("update.check.unreadable")
        case nil: return nil
        }
    }
}
