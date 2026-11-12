import Foundation

/// Why a transfer ended without its body.
public enum TransportFailure: Error, Equatable, Sendable {
    /// No connection, before any byte came.
    case offline
    /// An HTTP status other than 200.
    case status(Int)
    /// More bytes than allowed, declared or received.
    case tooLarge
    /// Anything else: cut short, a redirect to anything but https, a file
    /// that could not be written.
    case failed
}

/// GETs, each in a session of its own made from one ephemeral
/// configuration: no cookies, no cache, no credentials kept, nothing on
/// disk but the body asked for. A redirect is followed only to https.
/// Every request says `Accept-Language: en`: otherwise the system sends
/// the user's languages.
public final class SessionTransport: @unchecked Sendable {
    // Never changed after init; each transfer's session copies it.
    private let configuration: URLSessionConfiguration

    public init(configuration: URLSessionConfiguration = SessionTransport.configuration()) {
        self.configuration = configuration
    }

    /// The configuration the app uses.
    public static func configuration() -> URLSessionConfiguration {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.httpCookieStorage = nil
        configuration.httpShouldSetCookies = false
        configuration.httpCookieAcceptPolicy = .never
        configuration.urlCredentialStorage = nil
        configuration.urlCache = nil
        configuration.requestCachePolicy = .reloadIgnoringLocalAndRemoteCacheData
        configuration.timeoutIntervalForRequest = 30
        configuration.timeoutIntervalForResource = 600
        return configuration
    }

    /// The body of `url`, at most `limit` bytes.
    public func fetch(_ url: URL, headers: [String: String], limit: Int64) async throws(TransportFailure) -> Data {
        try await transfer(url, headers: headers, limit: limit, into: nil, progress: nil).body
    }

    /// Writes the body of `url` to a new file at `file`, at most `limit`
    /// bytes, telling the bytes received so far; the bytes written.
    public func download(
        _ url: URL, to file: URL, limit: Int64, headers: [String: String], progress: @escaping @Sendable (Int64) -> Void
    ) async throws(TransportFailure) -> Int64 {
        guard FileManager.default.createFile(atPath: file.path, contents: nil),
            let handle = try? FileHandle(forWritingTo: file)
        else {
            throw .failed
        }
        defer { try? handle.close() }
        return try await transfer(url, headers: headers, limit: limit, into: handle, progress: progress).received
    }

    private func transfer(
        _ url: URL, headers: [String: String], limit: Int64, into file: FileHandle?,
        progress: (@Sendable (Int64) -> Void)?
    ) async throws(TransportFailure) -> (body: Data, received: Int64) {
        var request = URLRequest(url: url)
        request.setValue("en", forHTTPHeaderField: "Accept-Language")
        for (name, value) in headers {
            request.setValue(value, forHTTPHeaderField: name)
        }
        let queue = OperationQueue()
        queue.maxConcurrentOperationCount = 1
        let result = await withCheckedContinuation { (done: CheckedContinuation<Transfer.Outcome, Never>) in
            let transfer = Transfer(limit: limit, file: file, progress: progress, done: done)
            let session = URLSession(configuration: configuration, delegate: transfer, delegateQueue: queue)
            session.dataTask(with: request).resume()
            session.finishTasksAndInvalidate()
        }
        return try result.get()
    }

    /// Whether a redirect may be followed: only to https.
    static func follows(_ redirect: URL?) -> Bool {
        redirect?.scheme?.lowercased() == "https"
    }

    /// What a session's error means for the update.
    static func failure(_ error: any Error, received: Int64) -> TransportFailure {
        let offline: Set<URLError.Code> = [
            .notConnectedToInternet, .networkConnectionLost, .cannotFindHost, .cannotConnectToHost, .timedOut,
            .dnsLookupFailed, .internationalRoamingOff, .dataNotAllowed,
        ]
        guard received == 0, let code = (error as? URLError)?.code, offline.contains(code) else {
            return .failed
        }
        return .offline
    }
}

/// One transfer's delegate. Its state is touched only on the session's
/// serial delegate queue.
private final class Transfer: NSObject, URLSessionDataDelegate, @unchecked Sendable {
    typealias Outcome = Result<(body: Data, received: Int64), TransportFailure>

    private let limit: Int64
    private let file: FileHandle?
    private let progress: (@Sendable (Int64) -> Void)?
    private let done: CheckedContinuation<Outcome, Never>
    private var body = Data()
    private var received: Int64 = 0
    private var failure: TransportFailure?

    init(
        limit: Int64, file: FileHandle?, progress: (@Sendable (Int64) -> Void)?,
        done: CheckedContinuation<Outcome, Never>
    ) {
        self.limit = limit
        self.file = file
        self.progress = progress
        self.done = done
    }

    func urlSession(
        _ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse,
        newRequest request: URLRequest, completionHandler: @escaping @Sendable (URLRequest?) -> Void
    ) {
        guard SessionTransport.follows(request.url) else {
            failure = failure ?? .failed
            completionHandler(nil)
            task.cancel()
            return
        }
        completionHandler(request)
    }

    func urlSession(
        _ session: URLSession, dataTask: URLSessionDataTask, didReceive response: URLResponse,
        completionHandler: @escaping @Sendable (URLSession.ResponseDisposition) -> Void
    ) {
        if let status = (response as? HTTPURLResponse)?.statusCode, status != 200 {
            failure = failure ?? .status(status)
        } else if response.expectedContentLength > limit {
            failure = failure ?? .tooLarge
        }
        completionHandler(failure == nil ? .allow : .cancel)
    }

    func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive data: Data) {
        guard failure == nil else {
            return
        }
        received += Int64(data.count)
        guard received <= limit else {
            failure = .tooLarge
            dataTask.cancel()
            return
        }
        if let file {
            do {
                try file.write(contentsOf: data)
            } catch {
                failure = .failed
                dataTask.cancel()
                return
            }
        } else {
            body.append(data)
        }
        progress?(received)
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: (any Error)?) {
        if let failure {
            done.resume(returning: .failure(failure))
        } else if let error {
            done.resume(returning: .failure(SessionTransport.failure(error, received: received)))
        } else {
            done.resume(returning: .success((body, received)))
        }
    }
}

/// The feed over the transport: GitHub's answer, or why there is none.
public struct FeedClient: UpdateFeed {
    public let source: UpdateSource
    private let transport: SessionTransport
    private let headers: [String: String]
    /// A release record is a few kilobytes.
    static let limit: Int64 = 1_000_000

    /// `version` goes into the User-Agent, the only thing that names the
    /// app.
    public init(source: UpdateSource, transport: SessionTransport, version: String) {
        self.source = source
        self.transport = transport
        headers = Self.headers(version: version)
    }

    /// Everything the app sets on the feed's request.
    public static func headers(version: String) -> [String: String] {
        ["User-Agent": "Metla/\(version)", "Accept": "application/vnd.github+json"]
    }

    public func latest() async throws(CheckFailure) -> Data {
        do {
            return try await transport.fetch(source.feed, headers: headers, limit: Self.limit)
        } catch {
            switch error {
            case .offline, .failed: throw .noConnection
            case .status(let status): throw .answered(status)
            case .tooLarge: throw .unreadable
            }
        }
    }
}
