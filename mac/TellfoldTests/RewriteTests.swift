import Foundation
import Testing
@testable import Tellfold

@Suite("Rewrite suggestion: line diff and decoding")
struct RewriteTests {
    private func kinds(_ lines: [LineDiff.Line]) -> String {
        lines.map { $0.kind == .same ? "=" : $0.kind == .added ? "+" : "-" }.joined()
    }

    @Test func identicalTextIsAllSame() {
        let d = LineDiff.diff(old: "a\nb\nc", new: "a\nb\nc")
        #expect(kinds(d) == "===")
        #expect(LineDiff.counts(d) == (0, 0))
    }

    @Test func replacedLineIsRemovedThenAdded() {
        let d = LineDiff.diff(old: "Title\nPrices rise 20%.\nThanks", new: "Title\nPrices rise 20%, regulars keep 10% off.\nThanks")
        #expect(kinds(d) == "=-+=")
        #expect(d[1].text == "Prices rise 20%." && d[2].text == "Prices rise 20%, regulars keep 10% off.")
        #expect(d.map(\.id) == [0, 1, 2, 3])
    }

    @Test func insertionsDeletionsAndEdges() {
        #expect(kinds(LineDiff.diff(old: "a\nc", new: "a\nb\nc")) == "=+=")
        #expect(kinds(LineDiff.diff(old: "a\nb\nc", new: "a\nc")) == "=-=")
        #expect(kinds(LineDiff.diff(old: [], new: ["x", "y"])) == "++")
        #expect(kinds(LineDiff.diff(old: ["x"], new: [])) == "-")
        #expect(kinds(LineDiff.diff(old: "a\r\nb", new: "a\nb")) == "==")
        let reordered = LineDiff.diff(old: "a\nb\nc\nd", new: "d\na\nb\nc")
        #expect(LineDiff.counts(reordered) == (1, 1))
    }

    @Test func diffRebuildsBothTexts() {
        let old = "One\nTwo\nThree\nFour\nFive", new = "One\nTwo and a half\nThree\nFive\nSix"
        let d = LineDiff.diff(old: old, new: new)
        #expect(d.filter { $0.kind != .added }.map(\.text).joined(separator: "\n") == old)
        #expect(d.filter { $0.kind != .removed }.map(\.text).joined(separator: "\n") == new)
    }

    @Test func longSharedHeadAndTailAreMatchedWithoutTheTable() {
        let head = (0..<3000).map { "head \($0)" }, tail = (0..<3000).map { "tail \($0)" }
        let d = LineDiff.diff(old: head + ["old middle"] + tail, new: head + ["new middle", "added"] + tail)
        #expect(d.count == 6003 && LineDiff.counts(d) == (1, 2))
        #expect(d[2999].kind == .same && d[3000].text == "old middle" && d[3001].kind == .added && d[3003].kind == .same)
        #expect(d.map(\.id) == Array(0..<6003))
        #expect(kinds(LineDiff.diff(old: ["a", "a"], new: ["a"])) == "=-")  // head and tail never overlap
    }

    @Test func decodesASuggestion() throws {
        let s = try TellfoldAPI.decoder.decode(RewriteSuggestion.self, from: Data(#"""
        {"revised_text":"New notice","source_name":"notice.md","created_at":"2026-11-10T00:00:00Z",
         "changes":[{"what":"Keep the discount","why":"@priya asked for it in #12.","cites":[12]},{"what":"x"}],
         "kept_on_purpose":["The price"],
         "citation_audit":{"checked":3,"unknown_posts":[],"unknown_handles":[],"misquotes":[],"marked":0}}
        """#.utf8))
        #expect(s.revisedText == "New notice" && s.sourceName == "notice.md")
        #expect(s.changes.map(\.cites) == [[12], []])
        #expect(s.changes[1].why == "")
        #expect(s.keptOnPurpose == ["The price"])
        #expect(s.citationAudit?.summaryLine == "All 3 citations checked against the posts")
        let bare = try TellfoldAPI.decoder.decode(RewriteSuggestion.self, from: Data(#"{"revised_text":"t"}"#.utf8))
        #expect(bare.changes.isEmpty && bare.sourceName.isEmpty && bare.citationAudit == nil)
    }
}
