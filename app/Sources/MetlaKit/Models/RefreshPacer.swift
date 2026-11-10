import Foundation

/// How often the views refresh on the engine's notifications: at most once
/// per `interval`, so a scan that notifies all the time costs a few
/// refreshes a second.
public struct RefreshPacer: Sendable {
    public let interval: Duration
    private var last: ContinuousClock.Instant?

    public init(interval: Duration = .milliseconds(250)) {
        self.interval = interval
    }

    /// How long to wait at `now` before the next refresh may run.
    public func wait(at now: ContinuousClock.Instant) -> Duration {
        guard let last else {
            return .zero
        }
        let next = last + interval
        return next > now ? next - now : .zero
    }

    /// Notes that a refresh ran at `now`.
    public mutating func ran(at now: ContinuousClock.Instant) {
        last = now
    }
}
