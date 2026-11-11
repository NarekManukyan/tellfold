import AppKit
import Foundation
import SwiftUI
import Testing
@testable import Tellfold

// The share card shows the report's headline and confidence, the rounds' sentiment and two posts the report itself
// cites, with who wrote them. Its PNG has the exact pixel size of each format, in both appearances.
@MainActor
@Suite("Share card")
struct ShareCardTests {
    private static func decode<T: Decodable>(_ type: T.Type, _ json: String) throws -> T {
        try TellfoldAPI.decoder.decode(T.self, from: Data(json.utf8))
    }

    private static let personas: [Persona] = [
        Persona(id: "a01", handle: "krisp", name: "Krisp",
                bio: "AI noise cancellation for calls, meetings, and recording. Clear audio, anywhere."),
        Persona(id: "a02", handle: "dan_callcoach", name: "Dan Ortega", bio: "Sales coach. Zoom fatigue real."),
        Persona(id: "a03", handle: "mira_ops", name: "Mira Chen", bio: "Ops at a 12-person remote shop | coffee"),
        Persona(id: "a04", handle: "quiet", name: "", bio: ""),
    ]

    private static func post(_ id: Int, _ author: String, _ content: String, sentiment: Double?,
                             repostOf: Int? = nil, likes: Int = 0) -> PostOut {
        PostOut(id: id, round: 1, authorID: author, content: content, parentID: nil, repostOf: repostOf, likes: likes,
                authorHandle: personas.first { $0.id == author }?.handle ?? author, sentiment: sentiment)
    }

    private static let posts: [PostOut] = [
        post(3, "a01", "We will add a hard stop on the signup screen.", sentiment: 0.6),
        post(5, "a01", "Reminders go out three days before the trial ends.", sentiment: 0.5),
        post(8, "a02", "A card for a free trial?\nNo thanks. I'm out.", sentiment: -0.8),
        post(9, "a03", "Fine by me if cancelling is one tap.", sentiment: 0.2),
        post(12, "a02", "", sentiment: -0.3, repostOf: 3),
        post(20, "a04", "Most liked post of all.", sentiment: 0.1, likes: 40),
    ]

    private static func report(reasons: [String], prediction: String = "", keyVoices: [String] = [],
                               summary: String = "Mostly calm (#9).", section: String = "See #20.") throws -> Report {
        let body: [String: Any] = [
            "title": "Customers tolerate card trials only with live safeguards", "summary": summary,
            "prediction": prediction, "confidence": "medium", "reasons": reasons, "key_voices": keyVoices,
            "sections": [["title": "Detail", "body_markdown": section]],
        ]
        return try TellfoldAPI.decoder.decode(Report.self, from: JSONSerialization.data(withJSONObject: body))
    }

    private static let stats: [RoundStats] = [
        RoundStats(round: 2, sentiment: 0.34), RoundStats(round: 0, sentiment: -0.1), RoundStats(round: 1, sentiment: nil),
    ]

    @Test func citationsAreReadInTheReportsOrderOfWeight() throws {
        let report = try Self.report(reasons: ["Hard stops won (#3, #12).", "A#7 is another run; ##4 is not a cite"],
                                     prediction: "Critics stay (#8) but #3 again.")
        #expect(ShareCardContent.citedPostIDs(in: report) == [3, 12, 8, 9, 20])
    }

    @Test func quotesComeFromTheCitedEvidenceAndShowBothSides() throws {
        // #3 and #5 are both by a01; #12 is a repost; #8 is the first cited post by someone else pointing the other way.
        let report = try Self.report(reasons: ["Pledges (#3, #5, #12).", "Fine if easy (#9)."], prediction: "Holdouts (#8).")
        let content = ShareCardContent(report: report, stats: Self.stats, posts: Self.posts, personas: Self.personas)
        #expect(content.headline == "Customers tolerate card trials only with live safeguards")
        #expect(content.confidence == .medium)
        #expect(content.quotes.map(\.postID) == [3, 8])
        let critic = try #require(content.quotes.last)
        #expect(critic.text == "A card for a free trial? No thanks. I'm out.")
        #expect(critic.name == "Dan Ortega")
        #expect(critic.role == "Sales coach")
        #expect(content.quotes.first?.role == "AI noise cancellation for calls, meetings…")
        #expect(content.sentiment.map(\.round) == [0, 2])
        #expect(content.finalSentiment == 0.34)
        // VoiceOver hears what the card shows, not only the headline
        let spoken = content.spokenDescription
        #expect(spoken.hasPrefix("Customers tolerate card trials only with live safeguards. Medium confidence. "))
        #expect(spoken.contains(RunUI.sentimentText(0.34)) && spoken.contains("Krisp") && spoken.contains("Dan Ortega"))
    }

    @Test func withoutAnOppositeVoiceTheNextCitedPersonIsQuoted() throws {
        let report = try Self.report(reasons: ["#3", "#5", "#9"])
        let content = ShareCardContent(report: report, stats: [], posts: Self.posts, personas: Self.personas)
        #expect(content.quotes.map(\.postID) == [3, 9])
        #expect(content.quotes.last?.role == "Ops at a 12-person remote shop")
        #expect(content.sentiment.isEmpty && content.finalSentiment == nil)
    }

    @Test func keyVoicesFillInOnlyWhenTheReportCitesTooLittle() throws {
        // Only a01 is cited: a04, a key voice, fills in with their most engaged post.
        let sparse = try Self.report(reasons: ["Only one person cited (#3, #5)."], keyVoices: ["a04"], summary: "", section: "")
        let posts = Self.posts + [Self.post(21, "a04", "A quieter post.", sentiment: 0.0, likes: 1)]
        let content = ShareCardContent(report: sparse, stats: [], posts: posts, personas: Self.personas)
        #expect(content.quotes.map(\.postID) == [3, 20])
        let quiet = try #require(content.quotes.last)
        #expect(quiet.name == "@quiet" && quiet.role == "@quiet")

        let none = try Self.report(reasons: [])
        let nothing = ShareCardContent(report: none, stats: [], posts: [], personas: [])
        #expect(nothing.quotes.isEmpty)
    }

    @Test func longTextIsCutAtAWord() {
        #expect(ShareCardContent.shorten("short", to: 10) == "short")
        let cut = ShareCardContent.shorten("one two three four five six seven", to: 16)
        #expect(cut == "one two three…" && cut.count <= 16)
        #expect(ShareCardContent.shorten(String(repeating: "x", count: 30), to: 10).count == 10)
        let long = Self.post(30, "a02", String(repeating: "word ", count: 80), sentiment: -0.5)
        let report = try? Self.report(reasons: ["#30"])
        let content = ShareCardContent(report: report!, stats: [], posts: [long], personas: Self.personas,
                                       quoteLength: ShareCardFormat.landscape.quoteLength)
        #expect((content.quotes.first?.text.count ?? 0) <= ShareCardFormat.landscape.quoteLength)
        #expect(content.quotes.first?.text.hasSuffix("…") == true)
    }

    @Test func aFixtureRunGivesTwoQuotesByDifferentPeople() throws {
        let detail = try Fixtures.decode(RunDetail.self, "run_detail.json")
        let posts = try Fixtures.decode([PostOut].self, "posts.json")
        let report = try #require(detail.report)
        let content = ShareCardContent(report: report, stats: detail.stats, posts: posts, personas: detail.personas)
        let cited = Set(ShareCardContent.citedPostIDs(in: report))
        let authors = Dictionary(posts.map { ($0.id, $0.authorID) }, uniquingKeysWith: { a, _ in a })
        #expect(content.quotes.allSatisfy { quote in
            cited.contains(quote.postID) || report.keyVoices.contains(authors[quote.postID] ?? "")
        })
        if content.quotes.count == 2 { #expect(content.quotes[0].handle != content.quotes[1].handle) }
        #expect(!content.headline.isEmpty)
    }

    @Test(arguments: [ShareCardFormat.landscape, .square])
    func pngHasTheFormatsPixelSizeInBothAppearances(format: ShareCardFormat) throws {
        let report = try Self.report(reasons: ["#3", "#8"])
        let content = ShareCardContent(report: report, stats: Self.stats, posts: Self.posts, personas: Self.personas,
                                       quoteLength: format.quoteLength)
        var corners: [NSColor] = []
        for scheme in [ColorScheme.light, .dark] {
            let png = try ShareCardRenderer.png(content, format: format, scheme: scheme)
            #expect(png.starts(with: Data([0x89, 0x50, 0x4E, 0x47])))
            let rep = try #require(NSBitmapImageRep(data: png))
            #expect(rep.pixelsWide == Int(format.size.width) && rep.pixelsHigh == Int(format.size.height))
            corners.append(try #require(rep.colorAt(x: 2, y: 2)?.usingColorSpace(.sRGB)))
        }
        // The ground token of each appearance fills the corner: near white, then near black.
        #expect(corners[0].brightnessComponent > 0.9)
        #expect(corners[1].brightnessComponent < 0.15)
        #expect(format.size == (format == .landscape ? CGSize(width: 1200, height: 630) : CGSize(width: 1080, height: 1080)))
    }

    @Test func fileNameNamesTheFormatAndAppearance() {
        #expect(ShareCardRenderer.suggestedFileName(title: "Plus: yes/no?", format: .square, scheme: .dark)
            == "Plus yes no? (square, dark).png")
    }
}

/// The four cards of a real run, written as PNGs to look at. Opt-in: point it at a running engine and a finished run,
///
///     TEST_RUNNER_TELLFOLD_CARD_PORT=<port> TEST_RUNNER_TELLFOLD_CARD_TOKEN=<token> TEST_RUNNER_TELLFOLD_CARD_RUN=<id> \
///     TEST_RUNNER_TELLFOLD_CARD_OUT=<folder> xcodebuild test -only-testing:TellfoldTests/ShareCardRealRunTests ...
@MainActor
@Suite(.enabled(if: ProcessInfo.processInfo.environment["TELLFOLD_CARD_RUN"] != nil))
struct ShareCardRealRunTests {
    @Test func writesTheCardsOfARealRun() async throws {
        let env = ProcessInfo.processInfo.environment
        let api = TellfoldAPI(port: Int(env["TELLFOLD_CARD_PORT"] ?? "") ?? 0, token: env["TELLFOLD_CARD_TOKEN"])
        let id = try #require(env["TELLFOLD_CARD_RUN"])
        let out = URL(filePath: try #require(env["TELLFOLD_CARD_OUT"]), directoryHint: .isDirectory)
        let detail = try await api.run(id)
        let posts = try await api.posts(runID: id)
        let report = try #require(detail.report)
        for format in ShareCardFormat.allCases {
            let content = ShareCardContent(report: report, stats: detail.stats, posts: posts, personas: detail.personas,
                                           quoteLength: format.quoteLength)
            #expect(content.quotes.count == 2)
            for scheme in [ColorScheme.light, .dark] {
                let png = try ShareCardRenderer.png(content, format: format, scheme: scheme)
                try png.write(to: out.appending(path: "\(format.rawValue)-\(scheme == .dark ? "dark" : "light").png"))
            }
        }
    }
}
