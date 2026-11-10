import Foundation
import Testing
@testable import MetlaKit

@Suite struct FormatTests {
    let english = Strings(language: "en", region: "US")
    let russian = Strings(language: "ru", region: "RU")

    @Test func sizesAreDecimalAsFinderShowsThem() {
        #expect(Format.bytes(1_500_000, english) == "1.5 MB")
        #expect(Format.bytes(1_000, english) == "1 kB")
        #expect(Format.bytes(2_500_000_000, russian) == "2,5 ГБ")
        #expect(Format.bytes(0, english) == "0 bytes", "zero is a number, not a word")
    }

    @Test func countsAreCompact() {
        #expect(Format.count(1_900_000, english) == "1.9M")
        #expect(Format.count(512, english) == "512")
        #expect(Format.count(1_900_000, russian) == "1,9\u{00A0}млн")
    }

    @Test func datesAreRelativeToNow() {
        let now = Date(timeIntervalSince1970: 1_000_000)
        let earlier = now.addingTimeInterval(-120)
        #expect(Format.relative(earlier, now: now, english) == "2 min. ago")
        #expect(Format.relative(earlier, now: now, russian) == "2 мин. назад")
        #expect(Format.relative(now, now: now, english) == "1 sec. ago", "never in the future, not even by zero")
        #expect(Format.relative(now.addingTimeInterval(60), now: now, english) == Format.relative(now, now: now, english),
                "a time ahead of now reads as now")
        #expect(Format.date(86_400) == Date(timeIntervalSince1970: 86_400))
    }

    @Test func sharesStayBetweenNothingAndAll() {
        #expect(Format.share(5, of: 10) == 0.5)
        #expect(Format.share(5, of: 0) == 0)
        #expect(Format.share(20, of: 10) == 1)
    }

    @Test func pathsBelowHomeStartWithATilde() {
        #expect(Format.path("/Users/me/code/app", home: "/Users/me") == "~/code/app")
        #expect(Format.path("/Users/me", home: "/Users/me") == "~")
        #expect(Format.path("/Users/meadow/x", home: "/Users/me") == "/Users/meadow/x")
        #expect(Format.path("/Applications", home: "/Users/me") == "/Applications")
        #expect(Format.path("/x", home: "/") == "/x", "a home of / shortens nothing")
    }
}
