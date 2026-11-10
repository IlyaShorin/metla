import Foundation
import MetlaCoreFFI
import os

/// The one owner of the Rust engine. Every call runs on the actor, off the
/// main thread, and is checked by a `MainThreadGuard`; the engine's
/// notifications arrive as a stream of "something changed" signals and
/// the topics since the last look (`drain`).
public actor EngineClient {
    private let engine: Engine
    private let mainThread: MainThreadGuard
    private let relay: Relay
    /// Ends when the client is released.
    public nonisolated let changes: AsyncStream<Void>

    init(engine: Engine, mainThread: MainThreadGuard, relay: Relay, changes: AsyncStream<Void>) {
        self.engine = engine
        self.mainThread = mainThread
        self.relay = relay
        self.changes = changes
    }

    /// Opens the engine off the main thread; `trash` moves things to the
    /// Trash (the system's, unless a test or a scenario gives another).
    @concurrent
    public static func open(
        _ options: EngineOptions,
        trash: any TrashHost = SystemTrash(),
        mainThread: MainThreadGuard = .standard
    ) async throws -> EngineClient {
        let engine = try perform("open", mainThread) {
            try Engine.open(options: options, trash: trash)
        }
        let (changes, continuation) = AsyncStream.makeStream(
            of: Void.self, bufferingPolicy: .bufferingNewest(1))
        let relay = Relay(continuation: continuation)
        engine.setObserver(observer: relay)
        return EngineClient(engine: engine, mainThread: mainThread, relay: relay, changes: changes)
    }

    /// Runs one blocking engine call, after checking where it runs.
    nonisolated static func perform<T>(
        _ call: String,
        _ mainThread: MainThreadGuard,
        _ body: () throws -> T
    ) rethrows -> T {
        mainThread.check(call)
        return try body()
    }

    private func run<T>(_ call: String, _ body: (Engine) throws -> T) rethrows -> T {
        try Self.perform(call, mainThread) { try body(engine) }
    }

    /// The topics told since the last call; empty when a signal was
    /// already handled.
    public nonisolated func drain() -> Set<Changed> {
        relay.drain()
    }

    /// How many cleanups have ended (a report, or why none ran) since the
    /// engine opened: a cleanup asked for has ended once this grows.
    public nonisolated var cleanupsEnded: Int {
        relay.cleanupsEnded
    }

    public func snapshot() throws -> Overview {
        try run("snapshot") { try $0.snapshot() }
    }

    public func freshness() throws -> Freshness {
        try run("freshness") { try $0.freshness() }
    }

    public func progress() -> ProgressSnapshot? {
        run("progress") { $0.progress() }
    }

    public func largestFolders(limit: UInt32) throws -> [TopFolder] {
        try run("largestFolders") { try $0.largestFolders(limit: limit) }
    }

    public func candidates(
        _ category: Category?, order: Order, offset: UInt64, limit: UInt32
    ) throws -> CandidatePage {
        try run("candidates") {
            try $0.candidates(category: category, order: order, offset: offset, limit: limit)
        }
    }

    public func children(_ dir: DirRef, offset: UInt64, limit: UInt32) throws -> FolderPage {
        try run("children") { try $0.children(dir: dir, offset: offset, limit: limit) }
    }

    public func startScan() throws {
        try run("startScan") { try $0.startScan() }
    }

    public func cancelScan() {
        run("cancelScan") { $0.cancelScan() }
    }

    public func setAttention(_ looking: Bool) throws {
        try run("setAttention") { try $0.setAttention(looking: looking) }
    }

    public func exclusions() throws -> [String] {
        try run("exclusions") { try $0.exclusions(offset: 0, limit: 1000) }
    }

    public func addExclusion(_ path: String) throws -> Bool {
        try run("addExclusion") { try $0.addExclusion(path: path) }
    }

    public func removeExclusion(_ path: String) throws -> Bool {
        try run("removeExclusion") { try $0.removeExclusion(path: path) }
    }

    /// The candidates keeping `path` hides.
    public func hiddenBy(_ path: String) throws -> Tally {
        try run("hiddenBy") { try $0.hiddenBy(path: path) }
    }

    public func settings() -> EngineSettings {
        run("settings") { $0.settings() }
    }

    public func setSettings(_ settings: EngineSettings) throws {
        try run("setSettings") { try $0.setSettings(settings: settings) }
    }

    public func setPowerProfile(_ profile: PowerProfile) throws {
        try run("setPowerProfile") { try $0.setPowerProfile(profile: profile) }
    }

    /// The preview of cleaning `selection`; it runs Git for worktrees.
    public func preview(_ selection: [DirId]) throws -> CleanupPreview {
        try run("preview") { try $0.preview(selection: selection) }
    }

    public func executeConfirmed(_ preview: PreviewId, _ confirmed: Confirmations) throws {
        try run("executeConfirmed") { try $0.executeConfirmed(preview: preview, confirmed: confirmed) }
    }

    public func quickClean(_ categories: [QuickCategory]) throws {
        try run("quickClean") { try $0.quickClean(categories: categories) }
    }

    public func cleanupProgress() -> CleanProgress? {
        run("cleanupProgress") { $0.cleanupProgress() }
    }

    public func cancelCleanup() {
        run("cancelCleanup") { $0.cancelCleanup() }
    }

    /// Cancels the running cleanup from any thread at once: it only sets
    /// the engine's flag.
    public nonisolated func cancelNow() {
        engine.cancelCleanup()
    }

    public func lastReport() -> CleanupResult? {
        run("lastReport") { $0.lastReport() }
    }
}

/// Hands the engine's notifications, which come on its own threads, to
/// the client: the topics are kept until drained, and one pending signal
/// says there is something to drain.
final class Relay: EngineObserver, Sendable {
    private let topics = OSAllocatedUnfairLock<Set<Changed>>(initialState: [])
    private let cleanups = OSAllocatedUnfairLock(initialState: 0)
    private let continuation: AsyncStream<Void>.Continuation

    init(continuation: AsyncStream<Void>.Continuation) {
        self.continuation = continuation
    }

    deinit {
        continuation.finish()
    }

    func changed(topic: Changed) {
        if topic == .cleanup {
            cleanups.withLock { $0 += 1 }
        }
        topics.withLock { _ = $0.insert(topic) }
        continuation.yield()
    }

    var cleanupsEnded: Int {
        cleanups.withLock { $0 }
    }

    func drain() -> Set<Changed> {
        topics.withLock { topics in
            defer { topics = [] }
            return topics
        }
    }
}

/// The system's Trash: `FileManager.trashItem`, so that Finder's "Put
/// Back" works. The engine calls it on its own thread, never the main one.
public final class SystemTrash: TrashHost, Sendable {
    public init() {}

    public func trash(path: String) -> TrashOutcome {
        do {
            try FileManager.default.trashItem(at: URL(fileURLWithPath: path), resultingItemURL: nil)
            return .moved
        } catch {
            return .refused(reason: error.localizedDescription)
        }
    }
}
