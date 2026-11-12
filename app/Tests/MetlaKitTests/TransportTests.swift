import Foundation
import Testing
@testable import MetlaKit

/// Answers a test session's requests as the test says, without the
/// network, and records each request it was given. Each test uses hosts
/// of its own, so tests running at once do not meet.
final class StubProtocol: URLProtocol, @unchecked Sendable {
    enum Reply: Sendable {
        case answer(status: Int, body: Data)
        case redirect(to: String)
        /// Declares `declared` bytes, sends `sent`, then loses the connection.
        case cut(sent: Data, declared: Int)
        case fail(URLError.Code)
    }

    private static let lock = NSLock()
    nonisolated(unsafe) private static var replies: [String: Reply] = [:]
    nonisolated(unsafe) private static var requests: [String: URLRequest] = [:]

    static func reply(_ url: String, _ reply: Reply) {
        lock.withLock { replies[url] = reply }
    }

    static func request(_ url: String) -> URLRequest? {
        lock.withLock { requests[url] }
    }

    /// A transport whose sessions go through this stub only.
    static func transport() -> SessionTransport {
        let configuration = SessionTransport.configuration()
        configuration.protocolClasses = [StubProtocol.self]
        return SessionTransport(configuration: configuration)
    }

    override class func canInit(with request: URLRequest) -> Bool {
        true
    }

    override class func canonicalRequest(for request: URLRequest) -> URLRequest {
        request
    }

    override func startLoading() {
        let url = request.url!.absoluteString
        let reply = Self.lock.withLock {
            Self.requests[url] = request
            return Self.replies[url]
        }
        let client = client!
        switch reply ?? .answer(status: 404, body: Data()) {
        case .answer(let status, let body):
            let response = HTTPURLResponse(
                url: request.url!, statusCode: status, httpVersion: "HTTP/1.1",
                headerFields: ["Content-Length": String(body.count)])!
            client.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
            client.urlProtocol(self, didLoad: body)
            client.urlProtocolDidFinishLoading(self)
        case .redirect(let target):
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 302, httpVersion: "HTTP/1.1", headerFields: ["Location": target])!
            client.urlProtocol(self, wasRedirectedTo: URLRequest(url: URL(string: target)!), redirectResponse: response)
        case .cut(let sent, let declared):
            let response = HTTPURLResponse(
                url: request.url!, statusCode: 200, httpVersion: "HTTP/1.1",
                headerFields: ["Content-Length": String(declared)])!
            client.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
            client.urlProtocol(self, didLoad: sent)
            // The loss a moment later, on this loading thread's run loop: the
            // bytes sent reach the delegate first.
            let later = Later(protocol: self, client: client)
            Timer.scheduledTimer(withTimeInterval: 0.2, repeats: false) { _ in later.fail() }
        case .fail(let code):
            client.urlProtocol(self, didFailWithError: URLError(code))
        }
    }

    override func stopLoading() {}

    /// What the cut connection does once its bytes are out.
    struct Later: @unchecked Sendable {
        let `protocol`: URLProtocol
        let client: any URLProtocolClient

        func fail() {
            client.urlProtocol(`protocol`, didFailWithError: URLError(.networkConnectionLost))
        }
    }
}

/// The transport and the feed client, against the stub or file URLs:
/// nothing here touches the network.
@Suite struct TransportTests {
    @Test func theSessionKeepsNothing() {
        let configuration = SessionTransport.configuration()
        #expect(configuration.httpCookieStorage == nil && !configuration.httpShouldSetCookies)
        #expect(configuration.httpCookieAcceptPolicy == .never)
        #expect(configuration.urlCredentialStorage == nil && configuration.urlCache == nil)
        #expect(configuration.requestCachePolicy == .reloadIgnoringLocalAndRemoteCacheData)
    }

    /// The feed's request carries the app's version and GitHub's media
    /// type, a fixed language instead of the user's, and nothing else the
    /// app sets.
    @Test func theFeedIsAskedWithTheAppsTwoHeadersOnly() async throws {
        let feed = "https://headers.test/repos/x/releases/latest"
        StubProtocol.reply(feed, .answer(status: 200, body: Data(FeedFixture.json.utf8)))
        let source = UpdateSource(feed: URL(string: feed)!, downloads: "https://headers.test/d/", pages: "https://headers.test/p/")
        let client = FeedClient(source: source, transport: StubProtocol.transport(), version: "0.8.0")
        let answer = try await client.latest()
        #expect(answer == Data(FeedFixture.json.utf8))
        let request = try #require(StubProtocol.request(feed))
        #expect(request.httpMethod == "GET")
        #expect(request.allHTTPHeaderFields == [
            "User-Agent": "Metla/0.8.0", "Accept": "application/vnd.github+json", "Accept-Language": "en",
        ])
        #expect(request.httpBody == nil)
    }

    @Test func aFeedThatCannotBeUsedSaysWhy() async {
        let cases: [(String, StubProtocol.Reply, CheckFailure)] = [
            ("https://status.test/latest", .answer(status: 403, body: Data("rate limited".utf8)), .answered(403)),
            ("https://offline.test/latest", .fail(.notConnectedToInternet), .noConnection),
            ("https://dns.test/latest", .fail(.cannotFindHost), .noConnection),
            ("https://tls.test/latest", .fail(.serverCertificateUntrusted), .noConnection),
            ("https://big.test/latest", .answer(status: 200, body: Data(count: 1_000_001)), .unreadable),
            ("https://hop.test/latest", .redirect(to: "http://hop.test/insecure"), .noConnection),
        ]
        for (url, reply, failure) in cases {
            StubProtocol.reply(url, reply)
            let source = UpdateSource(feed: URL(string: url)!, downloads: "https://x.test/", pages: "https://x.test/")
            let client = FeedClient(source: source, transport: StubProtocol.transport(), version: "0.8.0")
            await #expect(throws: failure, "\(url)") { try await client.latest() }
        }
        #expect(StubProtocol.request("http://hop.test/insecure") == nil, "the http hop is never asked")
    }

    @Test func aRedirectIsFollowedOnlyToHTTPS() async throws {
        StubProtocol.reply("https://from.test/a", .redirect(to: "https://to.test/b"))
        StubProtocol.reply("https://to.test/b", .answer(status: 200, body: Data("moved".utf8)))
        let body = try await StubProtocol.transport().fetch(URL(string: "https://from.test/a")!, headers: [:], limit: 100)
        #expect(body == Data("moved".utf8))
        #expect(SessionTransport.follows(URL(string: "https://x/y")) && SessionTransport.follows(URL(string: "HTTPS://x/y")))
        for url in ["http://x/y", "file:///x", "ftp://x/y"] {
            #expect(!SessionTransport.follows(URL(string: url)), "\(url)")
        }
        #expect(!SessionTransport.follows(nil))
    }

    @Test func aDownloadIsWrittenWithItsProgress() async throws {
        let box = Sandbox()
        defer { box.remove() }
        let source = box("source.bin")
        let bytes = Data((0..<300_000).map { UInt8($0 % 251) })
        try bytes.write(to: source)
        let seen = Seen()
        let written = try await SessionTransport().download(
            source, to: box("copy.bin"), limit: 300_000, headers: [:], progress: { seen.add($0) })
        #expect(written == 300_000)
        #expect(try Data(contentsOf: box("copy.bin")) == bytes)
        #expect(seen.last == 300_000)
        await #expect(throws: TransportFailure.tooLarge) {
            try await SessionTransport().download(source, to: box("small.bin"), limit: 299_999, headers: [:], progress: { _ in })
        }
        await #expect(throws: TransportFailure.failed) {
            try await SessionTransport().download(source, to: box("missing/x.bin"), limit: 300_000, headers: [:], progress: { _ in })
        }
    }

    /// The download says the app's version and the fixed language only.
    @Test func theDownloadIsAskedWithTheVersionOnly() async throws {
        let url = "https://download.test/v0.8.1/Metla.zip"
        StubProtocol.reply(url, .answer(status: 200, body: Data(count: 10)))
        let box = Sandbox()
        defer { box.remove() }
        let installer = Installer(requirement: "", transport: StubProtocol.transport(), version: "0.8.0", relaunch: { _ in })
        let release = Release(
            version: Version("0.8.1")!, download: URL(string: url)!, size: 10, digest: nil, notes: nil,
            page: URL(string: "https://download.test/")!)
        _ = try await installer.download(release, into: box.url, progress: { _ in })
        let request = try #require(StubProtocol.request(url))
        #expect(request.allHTTPHeaderFields == ["User-Agent": "Metla/0.8.0", "Accept-Language": "en"])
    }

    @Test func aDownloadCutShortOrTooLongFails() async {
        StubProtocol.reply("https://cut.test/Metla.zip", .cut(sent: Data(count: 1000), declared: 5000))
        StubProtocol.reply("https://long.test/Metla.zip", .answer(status: 200, body: Data(count: 5001)))
        StubProtocol.reply("https://gone.test/Metla.zip", .answer(status: 404, body: Data()))
        StubProtocol.reply("https://down.test/Metla.zip", .fail(.networkConnectionLost))
        let box = Sandbox()
        defer { box.remove() }
        let cases: [(String, TransportFailure)] = [
            ("https://cut.test/Metla.zip", .failed), ("https://long.test/Metla.zip", .tooLarge),
            ("https://gone.test/Metla.zip", .status(404)), ("https://down.test/Metla.zip", .offline),
        ]
        for (index, (url, failure)) in cases.enumerated() {
            await #expect(throws: failure, "\(url)") {
                try await StubProtocol.transport().download(
                    URL(string: url)!, to: box("\(index).zip"), limit: 5000, headers: [:], progress: { _ in })
            }
        }
    }

    @Test func aConnectionLostMidwayIsAFailedDownloadNotAnOfflineOne() {
        #expect(SessionTransport.failure(URLError(.networkConnectionLost), received: 0) == .offline)
        #expect(SessionTransport.failure(URLError(.networkConnectionLost), received: 1) == .failed)
        #expect(SessionTransport.failure(URLError(.badServerResponse), received: 0) == .failed)
        #expect(SessionTransport.failure(CocoaError(.fileWriteNoPermission), received: 0) == .failed)
    }
}

/// Progress values seen, from any thread.
final class Seen: @unchecked Sendable {
    private let lock = NSLock()
    private var values: [Int64] = []

    func add(_ value: Int64) {
        lock.withLock { values.append(value) }
    }

    var last: Int64? {
        lock.withLock { values.last }
    }
}
