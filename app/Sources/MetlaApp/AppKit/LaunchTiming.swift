import Darwin
import Foundation
import MetlaCoreFFI
import MetlaKit

/// With `METLA_PRINT_LAUNCH_TIMING=1` the app prints how long after the
/// process started its first window appeared and the saved picture was
/// shown, and its resident memory then.
enum LaunchTiming {
    @MainActor
    static func firstWindow(print enabled: Bool) {
        if enabled {
            #if DEBUG
                let build = "debug"
            #else
                let build = "release"
            #endif
            let library = cleansBeyondFixtures() ? "production library: cleans real data" : "plain library: fixtures only"
            Swift.print("launch: \(build) build, \(library)")
        }
        report("first window", enabled)
    }

    @MainActor
    static func firstPicture(print enabled: Bool) {
        report("first picture", enabled)
    }

    static func report(_ what: String, _ enabled: Bool) {
        guard enabled, let started = processStart() else {
            return
        }
        let elapsed = Date().timeIntervalSince(started) * 1000
        let (rss, peak) = (Double(residentBytes()) / 1_000_000, Double(peakResidentBytes()) / 1_000_000)
        Swift.print(String(format: "launch: %@ after %.0f ms, RSS %.1f MB (peak %.1f MB)", what, elapsed, rss, peak))
        fflush(stdout)
    }

    /// When the kernel started this process.
    static func processStart() -> Date? {
        var info = kinfo_proc()
        var size = MemoryLayout<kinfo_proc>.stride
        var name: [Int32] = [CTL_KERN, KERN_PROC, KERN_PROC_PID, getpid()]
        guard sysctl(&name, 4, &info, &size, nil, 0) == 0 else {
            return nil
        }
        let start = info.kp_proc.p_un.__p_starttime
        return Date(timeIntervalSince1970: TimeInterval(start.tv_sec) + TimeInterval(start.tv_usec) / 1e6)
    }

    /// The process's resident memory.
    static func residentBytes() -> UInt64 {
        basicInfo()?.resident_size ?? 0
    }

    /// The most resident memory the process has had.
    static func peakResidentBytes() -> UInt64 {
        basicInfo()?.resident_size_max ?? 0
    }

    private static func basicInfo() -> mach_task_basic_info? {
        var info = mach_task_basic_info()
        var count = mach_msg_type_number_t(MemoryLayout<mach_task_basic_info>.size / MemoryLayout<natural_t>.size)
        let result = withUnsafeMutablePointer(to: &info) { pointer in
            pointer.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
                task_info(mach_task_self_, task_flavor_t(MACH_TASK_BASIC_INFO), $0, &count)
            }
        }
        return result == KERN_SUCCESS ? info : nil
    }
}

/// With `METLA_PRINT_LAUNCH_TIMING=1`, what a person would watch after
/// the first picture, for measuring (docs/PERFORMANCE.md): when a scan's
/// first numbers show and when a full scan has ended (the first one into
/// an empty data directory, or the one `--rescan` starts as Refresh
/// does), the memory then and one minute after the first picture, and how
/// late the main thread was: a 50 ms heartbeat counts the stalls of
/// 250 ms or more (what macOS calls a hang) and keeps the longest.
@MainActor
final class Measure {
    private var last = Date()
    private var longest: TimeInterval = 0
    private var hangs = 0
    private var timer: Timer?

    static func follow(_ model: AppModel) async {
        let measure = Measure()
        measure.beat()
        let before = model.freshness?.fullScan
        if model.settings.rescanAtLaunch {
            await model.rescan()
        }
        let started = Date()
        var numbers = false
        var scanned = false
        var ended = !model.scanning && !model.settings.rescanAtLaunch
        var minute = false
        while !(ended && minute) {
            try? await Task.sleep(for: .milliseconds(100))
            if case .scanning(let progress, _)? = model.freshness?.state {
                scanned = true
                if !numbers, progress.files > 0 {
                    numbers = true
                    LaunchTiming.report("first numbers", true)
                }
            }
            let newer = model.freshness?.fullScan.map { $0 != before } ?? false
            if !ended, scanned || newer, case .idle? = model.freshness?.state {
                ended = true
                LaunchTiming.report("full scan ended", true)
                measure.print("during the scan")
            }
            if !minute, Date().timeIntervalSince(started) >= 60 {
                minute = true
                LaunchTiming.report("one minute after the first picture", true)
                measure.print("in that minute")
            }
        }
        // The heartbeat would wake the app 20 times a second from now on.
        measure.timer?.invalidate()
    }

    private func beat() {
        last = Date()
        let timer = Timer(timeInterval: 0.05, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.tick() }
        }
        RunLoop.main.add(timer, forMode: .common)
        self.timer = timer
    }

    private func tick() {
        let now = Date()
        let late = now.timeIntervalSince(last) - 0.05
        last = now
        longest = max(longest, late)
        if late >= 0.25 {
            hangs += 1
        }
    }

    private func print(_ when: String) {
        Swift.print(String(format: "main thread %@: longest stall %.0f ms, %d stalls of 250 ms or more",
                           when, longest * 1000, hangs))
        fflush(stdout)
        longest = 0
        hangs = 0
    }
}
