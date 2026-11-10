import Foundation

/// Where engine calls may run: never on the main thread. Every engine call
/// blocks its thread until the engine answers, so one on the main thread
/// would stall the interface; the client checks each call here.
public struct MainThreadGuard: Sendable {
    /// What a blocking call on the main thread does.
    let report: @Sendable (String) -> Void

    public init(report: @escaping @Sendable (String) -> Void) {
        self.report = report
    }

    /// Debug builds stop at a blocking call on the main thread; release
    /// builds let it pass.
    public static let standard = MainThreadGuard { call in
        assertionFailure("blocking engine call on the main thread: \(call)")
    }

    /// Reports `call` when it is made on the main thread.
    public func check(_ call: String) {
        if pthread_main_np() != 0 {
            report(call)
        }
    }
}
