import AppKit
import MetlaCoreFFI
import MetlaKit

/// The in-app update as this process makes it: the running bundle, the
/// core's stamp (a production build reads GitHub only), the running app's
/// own signature, and a relaunch that waits for this process to end.
@MainActor
enum Updates {
    /// The update for this copy, or nil; says on stderr which feed it reads.
    static func make(_ model: AppModel) -> UpdateModel? {
        let made = UpdateModel.app(
            bundle: Bundle.main.bundleURL, stamp: coreStamp(), environment: ProcessInfo.processInfo.environment,
            signature: Signature.running(), preferences: model.preferences,
            cleanupRunning: { [weak model] in model?.cleanup?.running ?? false }, relaunch: relaunch)
        FileHandle.standardError.write(Data((made.said + "\n").utf8))
        return made.model
    }

    /// Starts a helper that opens `bundle` once this process has ended,
    /// with this process's own `METLA_` variables, then quits. When the
    /// helper cannot start, the app stays.
    static func relaunch(_ bundle: URL) {
        do {
            let environment = Relaunch.carried(ProcessInfo.processInfo.environment)
            try Relaunch.after(pid: getpid(), open: bundle, environment: environment)
        } catch {
            FileHandle.standardError.write(Data("update: installed, but the relaunch could not start: \(error)\n".utf8))
            return
        }
        FileHandle.standardError.write(Data("update: installed; quitting, the new copy starts once this one has ended\n".utf8))
        NSApp.terminate(nil)
    }
}
