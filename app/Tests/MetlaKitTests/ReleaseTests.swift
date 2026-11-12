import Foundation
import Testing
@testable import MetlaKit

/// GitHub's answer for the latest release, in the shape the API gave for
/// v0.7.3 (the owner's and uploader's records left out), as 0.8.1.
enum FeedFixture {
    static let json = """
        {
          "url": "https://api.github.com/repos/IlyaShorin/metla/releases/443007844",
          "html_url": "https://github.com/IlyaShorin/metla/releases/tag/v0.8.1",
          "id": 443007844,
          "tag_name": "v0.8.1",
          "target_commitish": "main",
          "name": "Metla 0.8.1",
          "draft": false,
          "immutable": false,
          "prerelease": false,
          "created_at": "2026-11-12T13:42:36Z",
          "published_at": "2026-11-12T13:46:45Z",
          "assets": [
            {
              "url": "https://api.github.com/repos/IlyaShorin/metla/releases/assets/739654765",
              "id": 739654765,
              "name": "Metla.zip",
              "label": "",
              "content_type": "application/zip",
              "state": "uploaded",
              "size": 2730275,
              "digest": "sha256:9f6c7313b9a76a6f4cb6b761186c065cceabd779163dca834221854610b3b8f4",
              "download_count": 0,
              "browser_download_url": "https://github.com/IlyaShorin/metla/releases/download/v0.8.1/Metla.zip"
            }
          ],
          "tarball_url": "https://api.github.com/repos/IlyaShorin/metla/tarball/v0.8.1",
          "zipball_url": "https://api.github.com/repos/IlyaShorin/metla/zipball/v0.8.1",
          "body": "Notes in Markdown: never shown in the app."
        }
        """

    /// The fixture with `edit` applied to its top level, as JSON.
    static func with(_ edit: (inout [String: Any]) -> Void) -> Data {
        var object = (try? JSONSerialization.jsonObject(with: Data(json.utf8))) as? [String: Any] ?? [:]
        edit(&object)
        return (try? JSONSerialization.data(withJSONObject: object)) ?? Data()
    }

    /// The fixture with `edit` applied to its first asset.
    static func asset(_ edit: @escaping (inout [String: Any]) -> Void) -> Data {
        with { object in
            var assets = object["assets"] as? [[String: Any]] ?? []
            edit(&assets[0])
            object["assets"] = assets
        }
    }

    static let release = Release(
        version: Version("0.8.1")!,
        download: URL(string: "https://github.com/IlyaShorin/metla/releases/download/v0.8.1/Metla.zip")!,
        size: 2_730_275,
        digest: Data(hex: "9f6c7313b9a76a6f4cb6b761186c065cceabd779163dca834221854610b3b8f4"),
        notes: URL(string: "https://github.com/IlyaShorin/metla/releases/tag/v0.8.1")!,
        page: URL(string: "https://github.com/IlyaShorin/metla/releases/tag/v0.8.1")!)
}

@Suite struct VersionTests {
    @Test func aTagIsAVPrefixAndThreeNumbers() {
        #expect(Version(tag: "v0.8.1") == Version("0.8.1"))
        #expect(Version(tag: "v10.0.12")?.description == "10.0.12")
        for tag in ["0.8.1", "v0.8", "v0.8.1.2", "v0.8.1-beta", "v0.08.1", "v-1.0.0", "v+1.0.0", "V0.8.1", "v0..1",
                    "v1.0.0 ", "v١.٢.٣", "v1234567890.0.0", ""] {
            #expect(Version(tag: tag) == nil, "\(tag)")
        }
        #expect(Version("0.0.0") != nil)
    }

    @Test func versionsCompareByNumberNotByText() throws {
        let order = ["0.7.3", "0.8.0", "0.8.1", "0.9.0", "0.10.0", "1.0.0", "1.0.10"].map { Version($0)! }
        #expect(order == order.sorted())
        #expect(try #require(Version("0.10.0")) > #require(Version("0.9.9")))
        #expect(!(try #require(Version("0.8.0")) < #require(Version("0.8.0"))))
    }
}

@Suite struct FeedTests {
    let running = Version("0.8.0")!
    let github = UpdateSource.github

    @Test func theFixedFeedIsGitHubsLatestReleaseOfTheOneRepository() {
        #expect(github.feed.absoluteString == "https://api.github.com/repos/IlyaShorin/metla/releases/latest")
        #expect(github.downloads == "https://github.com/IlyaShorin/metla/releases/download/")
        #expect(github.pages == "https://github.com/IlyaShorin/metla/releases/")
        #expect(UpdateSource.repository == "IlyaShorin/metla")
    }

    @Test func aGoodReleaseIsOffered() {
        #expect(github.read(Data(FeedFixture.json.utf8), running: running) == .offer(FeedFixture.release))
    }

    @Test func anAnswerThatIsNotAReleaseIsUnreadable() {
        for body in ["", "not json", "[]", "{\"tag_name\": 8}", "{\"assets\": {}}"] {
            #expect(github.read(Data(body.utf8), running: running) == .unreadable, "\(body)")
        }
    }

    @Test func aDraftAPrereleaseOrAnUnmarkedReleaseIsNoUpdate() {
        let cases: [(String, (inout [String: Any]) -> Void)] = [
            ("draft", { $0["draft"] = true }),
            ("prerelease", { $0["prerelease"] = true }),
            ("no draft flag", { $0["draft"] = nil }),
            ("no prerelease flag", { $0["prerelease"] = nil }),
        ]
        for (name, edit) in cases {
            #expect(github.read(FeedFixture.with(edit), running: running) == .noUpdate, "\(name)")
        }
    }

    @Test func onlyAStrictlyGreaterVersionIsOffered() {
        for tag in ["v0.8.0", "v0.7.3", "v0.0.1", "0.8.1", "v0.8.1-rc1", "latest"] {
            #expect(github.read(FeedFixture.with { $0["tag_name"] = tag }, running: running) == .noUpdate, "\(tag)")
        }
        #expect(github.read(FeedFixture.with { $0["tag_name"] = nil }, running: running) == .noUpdate)
        guard case .offer(let release) = github.read(Data(FeedFixture.json.utf8), running: Version("0.7.3")!) else {
            Issue.record("0.8.1 over 0.7.3")
            return
        }
        #expect(release.version == Version("0.8.1"))
    }

    @Test func withoutAnAssetNamedExactlyMetlaZipThereIsNoUpdate() {
        #expect(github.read(FeedFixture.with { $0["assets"] = [] }, running: running) == .noUpdate)
        #expect(github.read(FeedFixture.with { $0["assets"] = nil }, running: running) == .noUpdate)
        for name in ["metla.zip", "Metla.zip ", "Metla.app.zip", "Metla.dmg"] {
            #expect(github.read(FeedFixture.asset { $0["name"] = name }, running: running) == .noUpdate, "\(name)")
        }
    }

    @Test func theDownloadMustBeHTTPSWithinTheRepositorysReleaseDownloads() {
        let urls = [
            "http://github.com/IlyaShorin/metla/releases/download/v0.8.1/Metla.zip",
            "https://example.com/IlyaShorin/metla/releases/download/v0.8.1/Metla.zip",
            "https://github.com.example.com/IlyaShorin/metla/releases/download/v0.8.1/Metla.zip",
            "https://github.com/someone/metla/releases/download/v0.8.1/Metla.zip",
            "https://github.com/IlyaShorin/metla/releases/download/../../../someone/x/Metla.zip",
            "https://github.com/IlyaShorin/metla/releases/download/%2e%2e/x/Metla.zip",
            "https://github.com/IlyaShorin/metla/releases/download/",
            "https://user@github.com/IlyaShorin/metla/releases/download/v0.8.1/Metla.zip",
            "https://github.com:8443/IlyaShorin/metla/releases/download/v0.8.1/Metla.zip",
            "file:///IlyaShorin/metla/releases/download/v0.8.1/Metla.zip",
            "not a url",
        ]
        for url in urls {
            #expect(github.read(FeedFixture.asset { $0["browser_download_url"] = url }, running: running) == .noUpdate, "\(url)")
        }
        #expect(github.read(FeedFixture.asset { $0["browser_download_url"] = nil }, running: running) == .noUpdate)
        let upper = "https://GITHUB.com/IlyaShorin/metla/releases/download/v0.8.1/Metla.zip"
        #expect(github.read(FeedFixture.asset { $0["browser_download_url"] = upper }, running: running) != .noUpdate,
                "the host's case does not matter")
    }

    @Test func theDeclaredSizeIsPositiveAndAtMost100MB() {
        for size in [0, -1, 100_000_001] {
            #expect(github.read(FeedFixture.asset { $0["size"] = size }, running: running) == .noUpdate, "\(size)")
        }
        #expect(github.read(FeedFixture.asset { $0["size"] = nil }, running: running) == .noUpdate)
        guard case .offer(let release) = github.read(FeedFixture.asset { $0["size"] = 100_000_000 }, running: running)
        else {
            Issue.record("100 MB is allowed")
            return
        }
        #expect(release.size == 100_000_000)
    }

    @Test func aDigestIsSHA256OrAbsent() {
        guard case .offer(let release) = github.read(FeedFixture.asset { $0["digest"] = nil }, running: running) else {
            Issue.record("a release without a digest is offered")
            return
        }
        #expect(release.digest == nil)
        for digest in ["sha512:9f6c", "sha256:9f6c", "sha256:" + String(repeating: "g", count: 64), "9f6c7313"] {
            #expect(github.read(FeedFixture.asset { $0["digest"] = digest }, running: running) == .noUpdate, "\(digest)")
        }
    }

    @Test func whatsNewIsOnlyAPageOfTheRepositorysReleases() {
        for url in ["http://github.com/IlyaShorin/metla/releases/tag/v0.8.1", "https://example.com/x",
                    "https://github.com/IlyaShorin/other/releases/tag/v0.8.1", "javascript:alert(1)"] {
            guard case .offer(let release) = github.read(FeedFixture.with { $0["html_url"] = url }, running: running) else {
                Issue.record("\(url): the release is still offered")
                continue
            }
            #expect(release.notes == nil, "\(url)")
            #expect(release.page.absoluteString == "https://github.com/IlyaShorin/metla/releases/", "\(url)")
        }
    }
}

@Suite struct SourceTests {
    @Test func aProductionBuildIgnoresTheTestFeed() {
        let environment = [UpdateSource.variable: "file:///tmp/feed/latest.json"]
        let chosen = UpdateSource.chosen(stamp: "62f142ca7975e910-production", environment: environment)
        #expect(chosen.source == .github)
        #expect(chosen.said == "update: feed https://api.github.com/repos/IlyaShorin/metla/releases/latest (fixed in this build; METLA_UPDATE_FEED ignored)")
        let quiet = UpdateSource.chosen(stamp: "62f142ca7975e910-production", environment: [:])
        #expect(quiet.source == .github && quiet.said.hasSuffix("(fixed in this build)"))
    }

    @Test func otherBuildsTakeAFileOrHTTPFeed() throws {
        for stamp in ["62f142ca7975e910-plain", "unstamped"] {
            let file = UpdateSource.chosen(stamp: stamp, environment: [UpdateSource.variable: "file:///tmp/feed/latest.json"])
            #expect(file.source.feed.absoluteString == "file:///tmp/feed/latest.json")
            #expect(file.source.downloads == "file:///tmp/feed/" && file.source.pages == "file:///tmp/feed/")
            #expect(file.said == "update: feed file:///tmp/feed/latest.json (METLA_UPDATE_FEED)")
        }
        let http = UpdateSource.chosen(stamp: "x-plain", environment: [UpdateSource.variable: "http://127.0.0.1:8000/f/latest.json"])
        #expect(http.source.downloads == "http://127.0.0.1:8000/f/")
        let local = try #require(UpdateSource.test(URL(fileURLWithPath: "/tmp/feed/latest.json")))
        #expect(local.feed.isFileURL)
    }

    @Test func anUnusableTestFeedLeavesTheFixedOne() {
        for value in ["", "latest.json", "ftp://host/latest.json", "https:///latest.json", "/tmp/feed/latest.json"] {
            let chosen = UpdateSource.chosen(stamp: "x-plain", environment: [UpdateSource.variable: value])
            #expect(chosen.source == .github, "\(value)")
        }
    }

    /// A test feed's downloads are files beside it, read as GitHub's are.
    @Test func aTestFeedOffersWhatLiesBesideIt() {
        let source = UpdateSource.test(URL(string: "file:///tmp/feed/latest.json")!)!
        let beside = FeedFixture.asset { $0["browser_download_url"] = "file:///tmp/feed/Metla.zip" }
        guard case .offer(let release) = source.read(beside, running: Version("0.8.0")!) else {
            Issue.record("a file beside the feed")
            return
        }
        #expect(release.download.absoluteString == "file:///tmp/feed/Metla.zip")
        let elsewhere = FeedFixture.asset { $0["browser_download_url"] = "file:///tmp/other/Metla.zip" }
        #expect(source.read(elsewhere, running: Version("0.8.0")!) == .noUpdate)
        #expect(source.read(Data(FeedFixture.json.utf8), running: Version("0.8.0")!) == .noUpdate, "GitHub's URL is not beside it")
    }
}
