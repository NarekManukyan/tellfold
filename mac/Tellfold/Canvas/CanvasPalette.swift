import SwiftUI

/// An sRGB colour as plain numbers, so scales can be computed and unit-tested without SwiftUI.
nonisolated struct RGB: Sendable, Equatable {
    var r: Double
    var g: Double
    var b: Double

    init(_ r: Double, _ g: Double, _ b: Double) {
        self.r = r
        self.g = g
        self.b = b
    }

    init(hex: UInt32) {
        self.init(Double((hex >> 16) & 0xFF) / 255, Double((hex >> 8) & 0xFF) / 255, Double(hex & 0xFF) / 255)
    }

    var color: Color { Color(.sRGB, red: r, green: g, blue: b, opacity: 1) }

    // OKLab (Björn Ottosson): perceptually even interpolation, so the diverging scale has no muddy midpoint.
    private static func toLinear(_ c: Double) -> Double { c <= 0.04045 ? c / 12.92 : pow((c + 0.055) / 1.055, 2.4) }
    private static func toSRGB(_ c: Double) -> Double {
        let v = c <= 0.0031308 ? c * 12.92 : 1.055 * pow(c, 1 / 2.4) - 0.055
        return min(1, max(0, v))
    }

    var oklab: SIMD3<Double> {
        let lr = Self.toLinear(r), lg = Self.toLinear(g), lb = Self.toLinear(b)
        let l = cbrt(0.4122214708 * lr + 0.5363325363 * lg + 0.0514459929 * lb)
        let m = cbrt(0.2119034982 * lr + 0.6806995451 * lg + 0.1073969566 * lb)
        let s = cbrt(0.0883024619 * lr + 0.2817188376 * lg + 0.6299787005 * lb)
        return SIMD3(0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
                     1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
                     0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s)
    }

    init(oklab v: SIMD3<Double>) {
        let l_ = v.x + 0.3963377774 * v.y + 0.2158037573 * v.z
        let m_ = v.x - 0.1055613458 * v.y - 0.0638541728 * v.z
        let s_ = v.x - 0.0894841775 * v.y - 1.2914855480 * v.z
        let l = l_ * l_ * l_, m = m_ * m_ * m_, s = s_ * s_ * s_
        self.init(Self.toSRGB(4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s),
                  Self.toSRGB(-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s),
                  Self.toSRGB(-0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s))
    }

    func mixed(with other: RGB, _ t: Double) -> RGB {
        let a = oklab, b = other.oklab
        return RGB(oklab: a + (b - a) * min(1, max(0, t)))
    }
}

/// Diverging sentiment scale: red (negative) → graphite grey (neutral) → green (positive), the contract's
/// accent jobs. Red and green differ in lightness, and sentiment is never colour alone: tooltips, the run log,
/// the cast sheet and the feed always print the signed number.
nonisolated struct SentimentScale: Sendable, Equatable {
    var negative: RGB
    var neutral: RGB
    var positive: RGB

    static let dark = SentimentScale(negative: RGB(hex: Theme.Hex.negative.dark), neutral: RGB(hex: Theme.Hex.neutral.dark),
                                     positive: RGB(hex: Theme.Hex.positive.dark))
    static let light = SentimentScale(negative: RGB(hex: Theme.Hex.negative.light), neutral: RGB(hex: Theme.Hex.neutral.light),
                                      positive: RGB(hex: Theme.Hex.positive.light))

    static func forScheme(_ scheme: ColorScheme) -> SentimentScale { scheme == .dark ? .dark : .light }

    /// `score` in -1...1 (clamped). nil (agent has not posted yet) is the neutral colour.
    func rgb(_ score: Double?) -> RGB {
        guard let score, score.isFinite else { return neutral }
        let s = min(1, max(-1, score))
        return s < 0 ? neutral.mixed(with: negative, -s) : neutral.mixed(with: positive, s)
    }

    func color(_ score: Double?) -> Color { rgb(score).color }

    /// Short words for VoiceOver and tooltips.
    static func describe(_ score: Double?) -> String {
        guard let score else { return "no posts yet" }
        switch score {
        case ..<(-0.5): return "strongly negative"
        case ..<(-0.15): return "negative"
        case ..<0.15: return "neutral"
        case ..<0.5: return "positive"
        default: return "strongly positive"
        }
    }
}

/// Interaction kinds drawn as arcs. Each keeps its own stroke so the kinds stay distinct without colour.
nonisolated enum ArcKind: String, Sendable, CaseIterable {
    case reply, repost, like, dislike, follow

    init?(actionType: String) { self.init(rawValue: actionType.lowercased()) }

    var label: String {
        switch self {
        case .reply: "Reply"
        case .repost: "Repost"
        case .like: "Like"
        case .dislike: "Dislike"
        case .follow: "Follow"
        }
    }

    var lineWidth: Double {
        switch self {
        case .reply: 1.8
        case .repost: 1.1
        case .like: 1.9
        case .dislike: 1.5
        case .follow: 1.0
        }
    }

    /// Dash pattern in points (empty = solid).
    var dash: [CGFloat] {
        switch self {
        case .like: [0.1, 4]
        case .dislike: [5, 3.5]
        default: []
        }
    }

    var isDouble: Bool { self == .repost }
    /// Every arc ends in an arrowhead: who acted on whom is the point of the picture.
    var hasArrowhead: Bool { true }
}

/// The fixed five-step influence ramp: node size never encodes anything else, and the legend shows the same steps.
nonisolated enum InfluenceRamp {
    struct Step: Sendable {
        var max: Double
        var radius: Double
        var label: String
    }

    /// Avatar diameters 22 / 26 / 30 / 36 / 44 pt for influence under 0.2 / 0.2 / 0.4 / 0.6 / 0.8 and up.
    static let steps: [Step] = [
        Step(max: 0.2, radius: 11, label: "<0.2"),
        Step(max: 0.4, radius: 13, label: "0.2"),
        Step(max: 0.6, radius: 15, label: "0.4"),
        Step(max: 0.8, radius: 18, label: "0.6"),
        Step(max: .infinity, radius: 22, label: "0.8+"),
    ]

    static func step(_ influence: Double) -> Int {
        let v = influence.isFinite ? min(1, max(0, influence)) : 0.5
        return steps.firstIndex { v < $0.max } ?? steps.count - 1
    }

    static func radius(_ influence: Double) -> Double { steps[step(influence)].radius }

    /// Half-side of an organisation tile drawn at ramp radius `r`: a tile fills the same box as a face.
    static func squareHalfSide(_ r: Double) -> Double { r }
}

/// Entity kinds in the knowledge graph: a distinct glyph per kind, so kinds read without colour.
nonisolated enum EntityKind: String, Sendable, CaseIterable {
    case person, organization, group, product, place, concept, event, other

    init(raw: String) { self = EntityKind(rawValue: raw.lowercased()) ?? .other }

    var label: String { rawValue.capitalized }

    enum Glyph: Sendable { case circle, square, hexagon, diamond, triangle, pentagon, invertedTriangle, octagon }

    var glyph: Glyph {
        switch self {
        case .person: .circle
        case .organization: .square
        case .group: .hexagon
        case .product: .diamond
        case .place: .triangle
        case .concept: .pentagon
        case .event: .invertedTriangle
        case .other: .octagon
        }
    }
}

/// All colours the canvases use, resolved for one colour scheme and contrast setting from `Theme`.
nonisolated struct CanvasPalette: Sendable {
    var scheme: ColorScheme
    var increasedContrast: Bool
    var sentiment: SentimentScale
    var text: Color
    var textDim: Color
    var edge: Color
    var event: Color
    var label: Color
    /// Backing behind labels and node outlines: the panel ground.
    var ground: Color
    var accent: Color
    var nodeBody: Color

    init(scheme: ColorScheme, increasedContrast: Bool = false) {
        self.scheme = scheme
        self.increasedContrast = increasedContrast
        sentiment = .forScheme(scheme)
        text = Theme.Hex.text.color(scheme)
        textDim = Theme.Hex.muted.color(scheme)
        edge = Theme.Hex.follow.color(scheme)
        event = Theme.Hex.event.color(scheme)
        label = text
        ground = Theme.Hex.panel.color(scheme)
        accent = Theme.Hex.accent.color(scheme)
        nodeBody = Theme.Hex.node.color(scheme)
    }

    var nodeHalo: Color { ground }

    /// Opacity of the resting follow graph.
    var edgeOpacity: Double { increasedContrast ? 0.75 : (scheme == .dark ? 0.55 : 0.6) }

    /// Arc colours follow the accent jobs: replies blue, reposts violet, likes green, dislikes red, follows muted.
    func arc(_ kind: ArcKind) -> Color {
        switch kind {
        case .reply: Theme.Hex.accent.color(scheme)
        case .repost: Theme.Hex.repost.color(scheme)
        case .like: Theme.Hex.positive.color(scheme)
        case .dislike: Theme.Hex.negative.color(scheme)
        case .follow: Theme.Hex.muted.color(scheme)
        }
    }

    /// Knowledge-graph entity kinds are told apart by glyph, not hue: colour is reserved for state and action
    /// type, so every kind draws in a graphite ink.
    func entity(_ kind: EntityKind) -> Color {
        switch kind {
        case .person, .organization: text
        case .group, .product, .place: textDim
        case .concept, .event, .other: edge
        }
    }
}
