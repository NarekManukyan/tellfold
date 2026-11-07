import AppKit
import SwiftUI

/// Small shared vocabulary for the run views: pipeline stages, action kinds, sentiment colours and formatting.
/// Enum-or-string fields from the shared models (state, stage, action type, confidence) are read through
/// `"\(value)"`, which yields the raw JSON word for both a `String` and a `String`-backed enum case.
enum RunUI {
    // MARK: pipeline

    struct Stage: Identifiable, Hashable, Sendable {
        let id: String
        let title: String
        let symbol: String
    }

    static let stages: [Stage] = [
        Stage(id: "reading", title: "Reading", symbol: "doc.text"),
        Stage(id: "graph", title: "Graph", symbol: "point.3.connected.trianglepath.dotted"),
        Stage(id: "personas", title: "Personas", symbol: "person.2"),
        Stage(id: "seeding", title: "Seeding", symbol: "leaf"),
        Stage(id: "rounds", title: "Rounds", symbol: "arrow.triangle.2.circlepath"),
        Stage(id: "report", title: "Report", symbol: "doc.richtext"),
    ]

    /// Index of a stage word in `stages`; `finished` is past the end.
    static func stageIndex(_ word: String) -> Int {
        if word == "finished" { return stages.count }
        return stages.firstIndex { $0.id == word } ?? 0
    }

    // MARK: actions

    struct ActionKind: Identifiable, Hashable, Sendable {
        let id: String
        let title: String
        let symbol: String
        let color: Color
    }

    /// Action types drawn in the timeline's action-mix bars (no-ops are left out). Colours follow the accent jobs:
    /// replies blue, reposts violet, likes green, dislikes red; posts and follows are structural greys.
    static let actionKinds: [ActionKind] = [
        ActionKind(id: "post", title: "Post", symbol: "square.and.pencil", color: Theme.muted),
        ActionKind(id: "reply", title: "Reply", symbol: "arrowshape.turn.up.left", color: Theme.accent),
        ActionKind(id: "repost", title: "Repost", symbol: "arrow.2.squarepath", color: Theme.repost),
        ActionKind(id: "like", title: "Like", symbol: "hand.thumbsup", color: Theme.positive),
        ActionKind(id: "dislike", title: "Dislike", symbol: "hand.thumbsdown", color: Theme.negative),
        ActionKind(id: "follow", title: "Follow", symbol: "person.badge.plus", color: Theme.follow),
    ]

    /// The colour of one action word (run log, legend), by accent job.
    static func actionColor(_ word: String) -> Color {
        switch word {
        case "reply": Theme.accent
        case "repost": Theme.repost
        case "like": Theme.positive
        case "dislike": Theme.negative
        case "follow": Theme.muted
        case "nothing": Theme.faint
        case "post": Theme.postInk
        default: Theme.text
        }
    }

    // MARK: sentiment (red -> grey -> green; the signed number is always printed too)

    /// The canvas's `SentimentScale`, resolved per appearance so feed chips, charts and the network agree.
    static func sentimentColor(_ score: Double?) -> Color {
        let rounded = score.map { ($0 * 50).rounded() / 50 }  // 0.02 steps keep the colour cache small
        return Color(nsColor: NSColor(name: nil) { appearance in
            let dark = appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
            let rgb = (dark ? SentimentScale.dark : SentimentScale.light).rgb(rounded)
            let color = NSColor(srgbRed: rgb.r, green: rgb.g, blue: rgb.b, alpha: 1)
            return score == nil ? color.withAlphaComponent(0.7) : color
        })
    }

    /// Signed two-decimal text, e.g. `-0.66` / `+0.12`; `n/a` only when no score exists at all.
    static func sentimentText(_ score: Double?) -> String {
        guard let score, score.isFinite else { return "n/a" }
        // Rounded half up on the two-decimal value (what the web page does), not on the binary expansion: 0.305 reads +0.31.
        let rounded = (score * 100 + 0.5).rounded(.down) / 100
        let text = rounded.formatted(.number.precision(.fractionLength(2)).sign(strategy: .always(includingZero: false)))
        return text.replacingOccurrences(of: "-", with: "\u{2212}")
    }

    /// The mean sentiment of one round: the engine's per-round figure, else the mean of that round's scored posts.
    /// The timeline line, its readout and the feed's round headers all read this, so they cannot disagree.
    @MainActor
    static func roundSentiment(_ round: Int, in store: RunStore) -> Double? {
        if let value = store.stats(for: round)?.sentiment, value.isFinite { return value }
        let scores = store.posts.compactMap { $0.round == round ? $0.sentiment : nil }
        return scores.isEmpty ? nil : scores.reduce(0, +) / Double(scores.count)
    }

    static func sentimentWord(_ score: Double?) -> String { SentimentScale.describe(score) }

    // MARK: links inside rendered text

    static let linkScheme = "shoal"

    static func agentURL(_ agentID: String) -> URL? {
        var c = URLComponents()
        c.scheme = linkScheme
        c.host = "agent"
        c.path = "/" + agentID
        return c.url
    }

    static func postURL(_ postID: Int) -> URL? { URL(string: "\(linkScheme)://post/\(postID)") }

    enum LinkTarget: Equatable {
        case agent(String)
        case post(Int)
    }

    static func linkTarget(_ url: URL) -> LinkTarget? {
        guard url.scheme == linkScheme, let host = url.host() else { return nil }
        let value = url.path().trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        switch host {
        case "agent" where !value.isEmpty: return .agent(value)
        case "post": return Int(value).map(LinkTarget.post)
        default: return nil
        }
    }
}

// MARK: - small reusable views

/// A tappable `@handle` in mono accent (selection is indigo); selects the agent.
struct RunHandleChip: View {
    let handle: String
    let sentiment: Double?
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 5) {
                if sentiment != nil {
                    // Fill carries the sign; the number is printed next to it in the feed.
                    Image(systemName: (sentiment ?? 0) < -0.15 ? "minus.circle.fill" : ((sentiment ?? 0) > 0.15 ? "plus.circle.fill" : "circle.fill"))
                        .font(.system(size: 8))
                        .foregroundStyle(RunUI.sentimentColor(sentiment))
                }
                Text("@\(handle)")
                    .font(Theme.mono(12, .medium))
                    .foregroundStyle(Theme.link)
                    .underline(hovering, color: Theme.link)
                    .lineLimit(1)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .help("Show @\(handle) in the inspector")
        .accessibilityLabel("@\(handle), sentiment \(RunUI.sentimentWord(sentiment))")
        .accessibilityHint("Shows this agent in the inspector")
        .accessibilityAddTraits(.isButton)
    }
}

/// Wrapping horizontal layout for chips and tags.
///
/// The reported size never exceeds the proposed width: a chip wider than the row is offered the row width (so its
/// text wraps) instead of being placed at its ideal width and running off the edge of the inspector column.
struct RunFlowLayout: Layout {
    var spacing: CGFloat = 6
    var lineSpacing: CGFloat = 6

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let maxWidth = max(0, proposal.width ?? .infinity)
        let rows = arrange(maxWidth: maxWidth, subviews: subviews)
        let used = rows.map(\.width).max() ?? 0
        let height = rows.reduce(0) { $0 + $1.height } + lineSpacing * CGFloat(max(0, rows.count - 1))
        return CGSize(width: maxWidth.isFinite ? min(used, maxWidth) : used, height: height)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var y = bounds.minY
        for row in arrange(maxWidth: bounds.width, subviews: subviews) {
            var x = bounds.minX
            for (index, size) in zip(row.indices, row.sizes) {
                subviews[index].place(at: CGPoint(x: x, y: y + (row.height - size.height) / 2), proposal: ProposedViewSize(size))
                x += size.width + spacing
            }
            y += row.height + lineSpacing
        }
    }

    private struct Row {
        var indices: [Int] = []
        var sizes: [CGSize] = []
        var width: CGFloat = 0
        var height: CGFloat = 0
    }

    private func arrange(maxWidth: CGFloat, subviews: Subviews) -> [Row] {
        var rows: [Row] = []
        var current = Row()
        for index in subviews.indices {
            var size = subviews[index].sizeThatFits(.unspecified)
            if size.width > maxWidth {
                // Too wide for any row: offer exactly the row width and let the chip wrap or truncate.
                size = subviews[index].sizeThatFits(ProposedViewSize(width: maxWidth, height: nil))
                size.width = min(size.width, maxWidth)
            }
            let extra = current.indices.isEmpty ? size.width : current.width + spacing + size.width
            if extra > maxWidth, !current.indices.isEmpty {
                rows.append(current)
                current = Row()
            }
            current.width = current.indices.isEmpty ? size.width : current.width + spacing + size.width
            current.height = max(current.height, size.height)
            current.indices.append(index)
            current.sizes.append(size)
        }
        if !current.indices.isEmpty { rows.append(current) }
        return rows
    }
}
