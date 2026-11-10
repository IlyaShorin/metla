import Darwin
import Foundation

/// With `METLA_PRINT_LAUNCH_TIMING=1` the app prints how long after the
/// process started its first window appeared and the saved picture was
/// shown, and its resident memory then.
enum LaunchTiming {
    @MainActor
    static func firstWindow(print enabled: Bool) {
        report("first window", enabled)
    }

    @MainActor
    static func firstPicture(print enabled: Bool) {
        report("first picture", enabled)
    }

    private static func report(_ what: String, _ enabled: Bool) {
        guard enabled, let started = processStart() else {
            return
        }
        let elapsed = Date().timeIntervalSince(started) * 1000
        let rss = Double(residentBytes()) / 1_000_000
        Swift.print(String(format: "launch: %@ after %.0f ms, RSS %.1f MB", what, elapsed, rss))
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
        var info = mach_task_basic_info()
        var count = mach_msg_type_number_t(MemoryLayout<mach_task_basic_info>.size / MemoryLayout<natural_t>.size)
        let result = withUnsafeMutablePointer(to: &info) { pointer in
            pointer.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
                task_info(mach_task_self_, task_flavor_t(MACH_TASK_BASIC_INFO), $0, &count)
            }
        }
        return result == KERN_SUCCESS ? info.resident_size : 0
    }
}
