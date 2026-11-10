import Foundation

/// A line diff for the suggested rewrite: the longest common subsequence of lines, then removed and added lines in
/// reading order (removals before the additions that replace them).
nonisolated enum LineDiff {
    enum Kind: Equatable, Sendable { case same, removed, added }

    struct Line: Equatable, Sendable, Identifiable {
        var kind: Kind
        var text: String
        /// Position in the diff, stable for SwiftUI lists.
        var id: Int
    }

    static func lines(_ text: String) -> [String] {
        text.replacingOccurrences(of: "\r\n", with: "\n").components(separatedBy: "\n")
    }

    static func diff(old: String, new: String) -> [Line] {
        diff(old: lines(old), new: lines(new))
    }

    /// The common first and last lines are matched directly; only the lines in between go through the LCS table,
    /// which is quadratic in their count.
    static func diff(old a: [String], new b: [String]) -> [Line] {
        var head = 0
        while head < a.count, head < b.count, a[head] == b[head] { head += 1 }
        var tail = 0
        while tail < a.count - head, tail < b.count - head, a[a.count - 1 - tail] == b[b.count - 1 - tail] { tail += 1 }
        var out: [Line] = []
        func add(_ kind: Kind, _ text: String) { out.append(Line(kind: kind, text: text, id: out.count)) }
        for line in a[..<head] { add(.same, line) }
        let midA = Array(a[head..<(a.count - tail)]), midB = Array(b[head..<(b.count - tail)])
        let n = midA.count, m = midB.count
        // lcs[i][j]: length of the LCS of midA[i...] and midB[j...]
        var lcs = Array(repeating: Array(repeating: 0, count: m + 1), count: n + 1)
        if n > 0 && m > 0 {
            for i in stride(from: n - 1, through: 0, by: -1) {
                for j in stride(from: m - 1, through: 0, by: -1) {
                    lcs[i][j] = midA[i] == midB[j] ? lcs[i + 1][j + 1] + 1 : max(lcs[i + 1][j], lcs[i][j + 1])
                }
            }
        }
        var i = 0, j = 0
        while i < n || j < m {
            if i < n, j < m, midA[i] == midB[j] {
                add(.same, midA[i]); i += 1; j += 1
            } else if i < n, j == m || lcs[i + 1][j] >= lcs[i][j + 1] {
                add(.removed, midA[i]); i += 1
            } else {
                add(.added, midB[j]); j += 1
            }
        }
        for line in a[(a.count - tail)...] { add(.same, line) }
        return out
    }

    /// Lines removed and added, for the sheet's summary ("3 lines changed").
    static func counts(_ lines: [Line]) -> (removed: Int, added: Int) {
        (lines.filter { $0.kind == .removed }.count, lines.filter { $0.kind == .added }.count)
    }
}
