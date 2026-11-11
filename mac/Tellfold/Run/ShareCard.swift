import AppKit
import SwiftUI

/// What a share card shows, taken from a finished run: the report's headline and confidence, the sentiment of each
/// round, and two posts the report cites, with who wrote them. Built without any view so the choice can be tested.
struct ShareCardContent: Equatable {
    /// One round's mean sentiment.
    struct SentimentPoint: Equatable {
        var round: Int
        var value: Double
    }

    struct Quote: Equatable {
        var postID: Int
        var text: String
        var name: String
        var role: String
        var handle: String
        var sentiment: Double?
        var persona: Persona?
    }

    var headline: String
    var confidence: ReportConfidence
    /// Every round the sentiment pass scored, in round order.
    var sentiment: [SentimentPoint]
    var quotes: [Quote]

    var finalSentiment: Double? { sentiment.last?.value }

    /// What VoiceOver reads for the card: the headline, the confidence, the final sentiment and who is quoted.
    var spokenDescription: String {
        var parts = [headline, confidence == .unknown ? "Confidence unknown" : "\(confidence.title) confidence"]
        if let finalSentiment { parts.append("final sentiment \(RunUI.sentimentText(finalSentiment))") }
        if !quotes.isEmpty { parts.append("quotes from " + ListFormatter.localizedString(byJoining: quotes.map(\.name))) }
        return parts.joined(separator: ". ")
    }

    static let maxHeadline = 120
    static let maxRole = 42

    init(headline: String, confidence: ReportConfidence, sentiment: [SentimentPoint], quotes: [Quote]) {
        self.headline = headline
        self.confidence = confidence
        self.sentiment = sentiment
        self.quotes = quotes
    }

    /// The card for a run. `quoteLength` caps each quote (the square card has room for longer ones).
    init(report: Report, stats: [RoundStats], posts: [PostOut], personas: [Persona], quoteLength: Int = 170) {
        headline = Self.shorten(report.title, to: Self.maxHeadline)
        confidence = report.confidence
        sentiment = stats.sorted { $0.round < $1.round }.compactMap { s in
            s.sentiment.flatMap { $0.isFinite ? SentimentPoint(round: s.round, value: $0) : nil }
        }
        let byID = Dictionary(personas.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a })
        quotes = Self.pickQuotes(report: report, posts: posts).map { post in
            let persona = byID[post.authorID]
            return Quote(postID: post.id, text: Self.shorten(Self.flatten(post.content), to: quoteLength),
                         name: persona?.name.nonEmpty ?? "@\(post.authorHandle)",
                         role: Self.role(of: persona, handle: post.authorHandle),
                         handle: persona?.handle ?? post.authorHandle, sentiment: post.sentiment, persona: persona)
        }
    }

    // MARK: choosing the quotes

    /// `#12` post citations in the report's own order of weight: the reasons first, then the prediction, the summary, the
    /// sections, the next steps and the risks. Study refs such as `A#12` are not this run's posts and are skipped.
    // `nonisolated(unsafe)`: NSRegularExpression is immutable once made and documented as safe to use from any thread.
    nonisolated(unsafe) private static let citation = try! NSRegularExpression(pattern: #"(?<![A-Za-z0-9#])#(\d{1,9})(?![0-9])"#)

    static func citedPostIDs(in report: Report) -> [Int] {
        let texts = report.reasons + [report.prediction, report.summary] + report.sections.map(\.bodyMarkdown)
            + report.recommendations.flatMap { $0.evidence + [$0.why] } + report.risks
        var seen = Set<Int>()
        var ids: [Int] = []
        for text in texts {
            let range = NSRange(text.startIndex..., in: text)
            for match in citation.matches(in: text, range: range) {
                guard let digits = Range(match.range(at: 1), in: text), let id = Int(text[digits]) else { continue }
                if seen.insert(id).inserted { ids.append(id) }
            }
        }
        return ids
    }

    /// Two posts from the report's evidence, by different people: the first one cited, then the next one cited whose
    /// sentiment points the other way (the card then shows both sides), else simply the next one by someone else. Reposts
    /// and empty posts are passed over. Only when the report cites fewer than two usable posts do the key voices' most
    /// engaged posts fill in.
    static func pickQuotes(report: Report, posts: [PostOut]) -> [PostOut] {
        let byID = Dictionary(posts.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a })
        let usable: (PostOut) -> Bool = { !$0.isRepost && !flatten($0.content).isEmpty }
        var candidates = citedPostIDs(in: report).compactMap { byID[$0] }.filter(usable)
        if Set(candidates.map(\.authorID)).count < 2 {
            let voices = Set(report.keyVoices)
            let engaged = posts.filter { usable($0) && (voices.contains($0.authorID) || voices.contains($0.authorHandle)) }
                .sorted { ($0.likes + 2 * $0.reposts + $0.replies, -$0.id) > ($1.likes + 2 * $1.reposts + $1.replies, -$1.id) }
            candidates += engaged.filter { post in !candidates.contains { $0.id == post.id } }
        }
        guard let first = candidates.first else { return [] }
        let others = candidates.dropFirst().filter { $0.authorID != first.authorID }
        let opposite = others.first { post in
            guard let a = first.sentiment, let b = post.sentiment else { return false }
            return (a > 0 && b < 0) || (a < 0 && b > 0)
        }
        return [first] + [opposite ?? others.first].compactMap { $0 }
    }

    // MARK: text

    /// What someone does, from the first sentence of their bio ("Sales coach. Zoom fatigue real." -> "Sales coach"), cut
    /// at a word; their handle when the bio is empty.
    static func role(of persona: Persona?, handle: String) -> String {
        let bio = flatten(persona?.bio ?? "")
        let first = bio.split(whereSeparator: { ".!?|;·—".contains($0) }).first.map {
            $0.trimmingCharacters(in: .whitespaces)
        } ?? ""
        return first.isEmpty ? "@\(persona?.handle ?? handle)" : shorten(first, to: maxRole)
    }

    /// One line: runs of whitespace (newlines included) become one space.
    static func flatten(_ text: String) -> String {
        text.split(whereSeparator: \.isWhitespace).joined(separator: " ")
    }

    /// `text` cut at the last word that fits in `limit` characters, with an ellipsis.
    static func shorten(_ text: String, to limit: Int) -> String {
        let text = flatten(text)
        guard text.count > limit else { return text }
        let cut = text.prefix(limit - 1)
        // Already at a word's end when the next character is a space or punctuation; otherwise drop the broken word.
        let next = text[cut.endIndex]
        let atWord = next.isWhitespace || next.isPunctuation ? cut : (cut.lastIndex(of: " ").map { cut[..<$0] } ?? cut)
        return atWord.trimmingCharacters(in: CharacterSet(charactersIn: " ,;:.-\u{2013}\u{2014}")) + "\u{2026}"
    }
}

private extension String {
    var nonEmpty: String? { isEmpty ? nil : self }
}

// MARK: - formats

/// The two card sizes, in pixels: a link-preview landscape and an Instagram-style square.
enum ShareCardFormat: String, CaseIterable, Identifiable {
    case landscape, square

    var id: String { rawValue }

    var size: CGSize {
        switch self {
        case .landscape: CGSize(width: 1200, height: 630)
        case .square: CGSize(width: 1080, height: 1080)
        }
    }

    var title: String {
        switch self {
        case .landscape: "Landscape (1200 × 630)"
        case .square: "Square (1080 × 1080)"
        }
    }

    /// How long a quote may be before it is cut.
    var quoteLength: Int {
        switch self {
        case .landscape: 150
        case .square: 210
        }
    }
}

// MARK: - the card

/// The card itself, drawn at its pixel size with the app's tokens resolved for one appearance, so the PNG matches
/// what the app shows whatever the system appearance is.
struct ShareCardView: View {
    let content: ShareCardContent
    let format: ShareCardFormat
    let scheme: ColorScheme

    private func c(_ pair: Theme.Pair) -> Color { pair.color(scheme) }

    private var padding: CGFloat { format == .landscape ? 60 : 72 }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Text(content.headline)
                .font(.system(size: format == .landscape ? 50 : 62, weight: .bold))
                .foregroundStyle(c(Theme.Hex.text))
                .lineSpacing(2)
                .lineLimit(format == .landscape ? 2 : 4)
                .minimumScaleFactor(0.7)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, format == .landscape ? 18 : 28)
            if format == .square {
                sentimentBlock(width: format.size.width - 2 * padding, height: 150)
                    .padding(.top, 40)
            }
            Spacer(minLength: format == .landscape ? 24 : 36)
            quotes
            Spacer(minLength: format == .landscape ? 24 : 36)
            footer
        }
        .padding(padding)
        .frame(width: format.size.width, height: format.size.height, alignment: .topLeading)
        .background(c(Theme.Hex.ground))
        .environment(\.colorScheme, scheme)
    }

    private var header: some View {
        HStack(spacing: 12) {
            Text("Simulated crowd reaction")
                .font(.system(size: 20, weight: .semibold))
                .foregroundStyle(c(Theme.Hex.muted))
            Spacer(minLength: 12)
            confidencePill
        }
    }

    private var confidencePill: some View {
        let level = switch content.confidence {
        case .low: 1
        case .medium: 2
        case .high: 3
        case .unknown: 0
        }
        return HStack(spacing: 10) {
            Text(content.confidence == .unknown ? "Confidence unknown" : "\(content.confidence.title) confidence")
                .font(.system(size: 19, weight: .medium))
                .foregroundStyle(c(Theme.Hex.secondary))
            HStack(spacing: 3) {
                ForEach(0..<3, id: \.self) { i in
                    RoundedRectangle(cornerRadius: 1.5)
                        .fill(i < level ? c(Theme.Hex.accent) : c(Theme.Hex.seamStrong))
                        .frame(width: 7, height: 18)
                }
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
        .overlay(Capsule().strokeBorder(c(Theme.Hex.seamStrong), lineWidth: 1.5))
    }

    @ViewBuilder private var quotes: some View {
        let cards = ForEach(Array(content.quotes.enumerated()), id: \.offset) { _, quote in
            quoteCard(quote)
        }
        if format == .landscape {
            HStack(alignment: .top, spacing: 20) { cards }
        } else {
            VStack(alignment: .leading, spacing: 20) { cards }
        }
    }

    private func quoteCard(_ quote: ShareCardContent.Quote) -> some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("\u{201C}\(quote.text)\u{201D}")
                .font(.system(size: format == .landscape ? 21 : 25))
                .foregroundStyle(c(Theme.Hex.text))
                .lineSpacing(3)
                .lineLimit(format == .landscape ? 4 : 5)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
            HStack(spacing: 12) {
                if let persona = quote.persona {
                    AvatarView(persona: persona, size: 40)
                }
                VStack(alignment: .leading, spacing: 2) {
                    Text(quote.name)
                        .font(.system(size: 18, weight: .semibold))
                        .foregroundStyle(c(Theme.Hex.text))
                    Text(quote.role)
                        .font(.system(size: 16))
                        .foregroundStyle(c(Theme.Hex.muted))
                }
                .lineLimit(1)
            }
        }
        .padding(22)
        .frame(maxWidth: .infinity, maxHeight: format == .landscape ? .infinity : nil, alignment: .topLeading)
        .background(c(Theme.Hex.card), in: RoundedRectangle(cornerRadius: Theme.cardRadius * 1.6))
        .overlay(RoundedRectangle(cornerRadius: Theme.cardRadius * 1.6).strokeBorder(c(Theme.Hex.seam), lineWidth: 1.5))
    }

    private var footer: some View {
        HStack(alignment: .bottom, spacing: 16) {
            if format == .landscape {
                sentimentBlock(width: 300, height: 54)
            }
            Spacer(minLength: 16)
            HStack(spacing: 10) {
                BrandMark().frame(width: 24, height: 24)
                Text("Made with Tellfold")
                    .font(.system(size: 18, weight: .medium))
                    .foregroundStyle(c(Theme.Hex.muted))
            }
        }
    }

    /// "Sentiment by round" with the final value, over a small line on a fixed −1…1 scale (the zero line dashed).
    private func sentimentBlock(width: CGFloat, height: CGFloat) -> some View {
        let final = content.finalSentiment
        let value = final ?? 0
        // the number is text (AA tokens); the line is a mark
        let ink: Theme.Pair = value > 0.05 ? Theme.Hex.positiveText : value < -0.05 ? Theme.Hex.negative : Theme.Hex.neutral
        let tone: Theme.Pair = value > 0.05 ? Theme.Hex.positive : ink
        return VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                Text("Sentiment by round")
                    .font(.system(size: 16, weight: .medium))
                    .foregroundStyle(c(Theme.Hex.muted))
                if let final {
                    Text(RunUI.sentimentText(final))
                        .font(Theme.mono(18, .semibold))
                        .foregroundStyle(c(ink))
                }
            }
            SentimentSparkline(values: content.sentiment.map(\.value), line: c(tone), zero: c(Theme.Hex.seamStrong))
                .frame(width: width, height: height)
        }
    }
}

/// The rounds' sentiment as a line on a −1…1 scale, with the zero line dashed and the last round marked.
struct SentimentSparkline: View {
    let values: [Double]
    let line: Color
    let zero: Color

    var body: some View {
        Canvas { ctx, size in
            func point(_ i: Int) -> CGPoint {
                let x = values.count > 1 ? size.width * CGFloat(i) / CGFloat(values.count - 1) : size.width / 2
                let v = min(1, max(-1, values[i]))
                return CGPoint(x: x, y: (1 - CGFloat(v + 1) / 2) * (size.height - 8) + 4)
            }
            var base = Path()
            base.move(to: CGPoint(x: 0, y: size.height / 2))
            base.addLine(to: CGPoint(x: size.width, y: size.height / 2))
            ctx.stroke(base, with: .color(zero), style: StrokeStyle(lineWidth: 1.5, dash: [5, 5]))
            guard !values.isEmpty else { return }
            var path = Path()
            path.move(to: point(0))
            for i in values.indices.dropFirst() { path.addLine(to: point(i)) }
            ctx.stroke(path, with: .color(line), style: StrokeStyle(lineWidth: 3, lineCap: .round, lineJoin: .round))
            let last = point(values.count - 1)
            ctx.fill(Path(ellipseIn: CGRect(x: last.x - 5, y: last.y - 5, width: 10, height: 10)), with: .color(line))
        }
        .accessibilityHidden(true)
    }
}

// MARK: - rendering

enum ShareCardRenderer {
    enum RenderError: LocalizedError {
        case render

        var errorDescription: String? { "The share card could not be drawn." }
    }

    /// The card as PNG data at exactly the format's pixel size.
    @MainActor
    static func png(_ content: ShareCardContent, format: ShareCardFormat, scheme: ColorScheme) throws -> Data {
        let renderer = ImageRenderer(content: ShareCardView(content: content, format: format, scheme: scheme))
        renderer.scale = 1
        renderer.proposedSize = ProposedViewSize(format.size)
        guard let image = renderer.cgImage else { throw RenderError.render }
        let rep = NSBitmapImageRep(cgImage: image)
        rep.size = format.size
        guard let data = rep.representation(using: .png, properties: [:]) else { throw RenderError.render }
        return data
    }

    /// The suggested file name: the report title, then the format, as PNG.
    static func suggestedFileName(title: String, format: ShareCardFormat, scheme: ColorScheme) -> String {
        let name = ReportPDFExporter.baseName(title: title)
        return "\(name) (\(format.rawValue), \(scheme == .dark ? "dark" : "light")).png"
    }

    /// Puts the PNG on the clipboard, as PNG data and as an image, so it pastes into chats, mail and documents.
    static func copy(_ png: Data) {
        let board = NSPasteboard.general
        board.clearContents()
        if let image = NSImage(data: png) { board.writeObjects([image]) }
        board.setData(png, forType: .png)
    }
}
