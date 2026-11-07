import SwiftUI

/// Turns report text into `AttributedString`s: inline markdown per line, with `@handle`, persona ids,
/// `#123` and `post 123` turned into `tellfold://` links that the run view routes to agent / post selection.
struct RunLinkifier: Sendable {
    let handles: [String: String]   // lowercased handle -> agent id
    let agentIDs: Set<String>
    /// Agent id -> handle, so a bare id in the prose reads as `@handle`.
    let handleByID: [String: String]
    let postIDs: Set<Int>

    init(personas: [Persona], postIDs: Set<Int>) {
        var handles: [String: String] = [:]
        for p in personas { handles[p.handle.lowercased()] = p.id }
        self.handles = handles
        self.agentIDs = Set(personas.map(\.id))
        self.handleByID = Dictionary(personas.map { ($0.id, $0.handle) }, uniquingKeysWith: { a, _ in a })
        self.postIDs = postIDs
    }

    private static let pattern = #"(?<![\w\[/@])@([A-Za-z0-9_][A-Za-z0-9_.]{0,63})|(?<![\w\[&/])#(\d{1,8})\b|\b[Pp]osts? #?(\d{1,8})\b|\b(a\d{2,3})\b"#

    /// Rewrites known references as markdown links.
    func linkify(_ text: String) -> String {
        guard let regex = try? NSRegularExpression(pattern: Self.pattern) else { return text }
        let ns = text as NSString
        var out = text
        let matches = regex.matches(in: text, range: NSRange(location: 0, length: ns.length))
        for match in matches.reversed() {
            guard let range = Range(match.range, in: out) else { continue }
            if let r = Range(match.range(at: 1), in: text) {
                var handle = String(text[r])
                var trailing = ""
                while handle.hasSuffix(".") { handle.removeLast(); trailing += "." }
                guard let id = handles[handle.lowercased()], let url = RunUI.agentURL(id) else { continue }
                out.replaceSubrange(range, with: "[@\(handle)](\(url.absoluteString))\(trailing)")
            } else if let r = Range(match.range(at: 2), in: text) ?? Range(match.range(at: 3), in: text),
                      let id = Int(text[r]) {
                guard postIDs.isEmpty || postIDs.contains(id), let url = RunUI.postURL(id) else { continue }
                out.replaceSubrange(range, with: "[\(text[range])](\(url.absoluteString))")
            } else if let r = Range(match.range(at: 4), in: text) {
                let id = String(text[r])
                guard agentIDs.contains(id), let url = RunUI.agentURL(id) else { continue }
                out.replaceSubrange(range, with: "[@\(handleByID[id] ?? id)](\(url.absoluteString))")
            }
        }
        return out
    }

    /// Inline markdown with citations (`@handle`, `#post`, agent ids) drawn as mono links in the accent colour; the run
    /// view routes them to agent / post selection.
    func inline(_ text: String, size: CGFloat = 14) -> AttributedString {
        let source = linkify(text)
        let options = AttributedString.MarkdownParsingOptions(
            allowsExtendedAttributes: false,
            interpretedSyntax: .inlineOnlyPreservingWhitespace,
            failurePolicy: .returnPartiallyParsedIfPossible
        )
        var result = (try? AttributedString(markdown: source, options: options)) ?? AttributedString(text)
        for run in result.runs where run.link != nil {
            result[run.range].font = Theme.mono(size * 0.92)
            result[run.range].foregroundColor = Theme.link
        }
        return result
    }
}

/// Minimal block-level markdown: headings, bullet and numbered lists, block quotes and paragraphs.
struct RunMarkdownView: View {
    let markdown: String
    let linkifier: RunLinkifier
    var size: CGFloat = 14
    var spacing: CGFloat = 8

    private enum Block: Hashable {
        case heading(level: Int, text: String)
        case bullet(marker: String, text: String)
        case quote(String)
        case paragraph(String)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: spacing) {
            ForEach(Array(blocks.enumerated()), id: \.offset) { _, block in
                view(for: block)
            }
        }
        .textSelection(.enabled)
    }

    @ViewBuilder
    private func view(for block: Block) -> some View {
        switch block {
        case let .heading(level, text):
            Text(linkifier.inline(text, size: size))
                .font(level <= 2 ? Theme.ui(size + 3, .semibold) : Theme.ui(size + 1, .semibold))
                .padding(.top, 4)
                .accessibilityAddTraits(.isHeader)
        case let .bullet(marker, text):
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Text(marker)
                    .font(Theme.mono(size - 1))
                    .foregroundStyle(Theme.faint)
                    .frame(minWidth: 14, alignment: .trailing)
                Text(linkifier.inline(text, size: size))
                    .font(Theme.ui(size))
                    .lineSpacing(3)
                    .fixedSize(horizontal: false, vertical: true)
            }
        case let .quote(text):
            Text(linkifier.inline(text, size: size))
                .font(Theme.ui(size))
                .foregroundStyle(Theme.muted)
                .padding(.leading, 10)
                .overlay(alignment: .leading) {
                    Rectangle().fill(Theme.seamStrong).frame(width: 1)
                }
        case let .paragraph(text):
            Text(linkifier.inline(text, size: size))
                .font(Theme.ui(size))
                .fixedSize(horizontal: false, vertical: true)
                .lineSpacing(size >= 17 ? 5 : 4)
        }
    }

    private var blocks: [Block] {
        var blocks: [Block] = []
        var paragraph: [String] = []
        func flush() {
            if !paragraph.isEmpty {
                blocks.append(.paragraph(paragraph.joined(separator: " ")))
                paragraph.removeAll()
            }
        }
        for raw in markdown.components(separatedBy: .newlines) {
            let line = raw.trimmingCharacters(in: .whitespaces)
            if line.isEmpty {
                flush()
                continue
            }
            if let heading = Self.heading(line) {
                flush()
                blocks.append(heading)
            } else if let bullet = Self.bullet(line) {
                flush()
                blocks.append(bullet)
            } else if line.hasPrefix(">") {
                flush()
                blocks.append(.quote(String(line.dropFirst()).trimmingCharacters(in: .whitespaces)))
            } else {
                paragraph.append(line)
            }
        }
        flush()
        return blocks
    }

    private static func heading(_ line: String) -> Block? {
        let hashes = line.prefix { $0 == "#" }.count
        guard (1...6).contains(hashes) else { return nil }
        let rest = line.dropFirst(hashes)
        guard rest.first == " " else { return nil }
        return .heading(level: hashes, text: rest.trimmingCharacters(in: .whitespaces))
    }

    private static func bullet(_ line: String) -> Block? {
        for marker in ["- ", "* ", "+ ", "• "] where line.hasPrefix(marker) {
            return .bullet(marker: "•", text: String(line.dropFirst(marker.count)))
        }
        let digits = line.prefix { $0.isNumber }
        if !digits.isEmpty, digits.count <= 3 {
            let rest = line.dropFirst(digits.count)
            if rest.hasPrefix(". ") || rest.hasPrefix(") ") {
                return .bullet(marker: "\(digits).", text: String(rest.dropFirst(2)))
            }
        }
        return nil
    }
}
