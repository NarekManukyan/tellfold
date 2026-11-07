import AppKit
import SwiftUI

/// The single token source for Tellfold's calm report look: a light ground with white cards, one indigo accent, SF Pro,
/// and a dark mirror under the system appearance. Each accent has exactly one job:
///
/// - `accent` (indigo): selection, links, brand
/// - `positive` / `negative` / `neutral`: sentiment, always printed as a signed number next to the colour
/// - `event` (amber): injected events, nothing else
/// - `repost` (violet): reposts
///
/// Colours are dynamic (dark and light resolve per appearance, no asset catalog entries). Cards are 10 pt radius
/// with a 1 px border and, in light mode only, a hairline shadow. There are no gradients, glass or glow.
enum Theme {
    // MARK: colour

    /// A colour that resolves to `dark` or `light` with the window's appearance.
    static func dynamic(dark: UInt32, light: UInt32, darkAlpha: Double = 1, lightAlpha: Double = 1) -> Color {
        Color(nsColor: NSColor(name: nil) { appearance in
            let isDark = appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
            return NSColor(hex: isDark ? dark : light, alpha: isDark ? darkAlpha : lightAlpha)
        })
    }

    // MARK: raw values (the canvases resolve these per colour scheme)

    struct Pair: Sendable {
        let dark: UInt32
        let light: UInt32

        func color(_ scheme: ColorScheme) -> Color { Color(nsColor: NSColor(hex: scheme == .dark ? dark : light)) }
        func nsColor(dark isDark: Bool) -> NSColor { NSColor(hex: isDark ? dark : light) }
    }

    enum Hex {
        static let ground = Pair(dark: 0x0E1013, light: 0xF7F8FA)
        static let card = Pair(dark: 0x16191E, light: 0xFFFFFF)
        /// Kept for the canvases: the surface they sit on is a card.
        static let panel = card
        static let elevated = card
        static let hover = Pair(dark: 0x1E2228, light: 0xF3F4F6)
        static let seam = Pair(dark: 0x262A31, light: 0xE5E7EB)
        static let seamStrong = Pair(dark: 0x343944, light: 0xD1D5DB)
        static let text = Pair(dark: 0xF3F4F6, light: 0x111827)
        static let secondary = Pair(dark: 0xC7CBD1, light: 0x4B5563)
        static let muted = Pair(dark: 0x9CA3AF, light: 0x6B7280)
        static let faint = Pair(dark: 0x6B7280, light: 0x9CA3AF)
        static let accent = Pair(dark: 0x818CF8, light: 0x6366F1)
        static let accentInk = Pair(dark: 0xFFFFFF, light: 0xFFFFFF)
        /// Fill of primary buttons: the same indigo in both appearances so white text keeps its contrast.
        static let accentFill = Pair(dark: 0x4F46E5, light: 0x4F46E5)
        /// Links and citations (text): AA on the ground in both appearances. `accent` stays for non-text marks.
        static let link = Pair(dark: 0xA5B4FC, light: 0x4F46E5)
        static let positive = Pair(dark: 0x4ADE80, light: 0x16A34A)
        static let negative = Pair(dark: 0xF87171, light: 0xDC2626)
        static let repost = Pair(dark: 0xC084FC, light: 0xA855F7)
        static let event = Pair(dark: 0xFBBF24, light: 0xD97706)
        static let follow = Pair(dark: 0x3A4150, light: 0xC9CED6)
        static let neutral = Pair(dark: 0x9CA3AF, light: 0x6B7280)
        static let post = secondary
        static let node = Pair(dark: 0x2A2F37, light: 0xE5E7EB)
    }

    static func dynamic(_ pair: Pair) -> Color { dynamic(dark: pair.dark, light: pair.light) }

    // Surfaces.
    static let ground = dynamic(Hex.ground)
    static let card = dynamic(Hex.card)
    /// Older name for the surface panels sit on: sheets and settings use the ground.
    static let panel = ground
    static let elevated = card
    static let hover = dynamic(Hex.hover)
    static let seam = dynamic(Hex.seam)
    static let seamStrong = dynamic(Hex.seamStrong)

    // Text.
    static let text = dynamic(Hex.text)
    static let secondary = dynamic(Hex.secondary)
    static let muted = dynamic(Hex.muted)
    static let faint = dynamic(Hex.faint)

    // Accent jobs.
    static let accent = dynamic(Hex.accent)
    static let accentSoft = dynamic(dark: 0x818CF8, light: 0x6366F1, darkAlpha: 0.16, lightAlpha: 0.10)
    static let accentInk = dynamic(Hex.accentInk)
    static let accentFill = dynamic(Hex.accentFill)
    static let link = dynamic(Hex.link)
    static let positive = dynamic(Hex.positive)
    static let negative = dynamic(Hex.negative)
    static let negativeSoft = dynamic(dark: 0xF87171, light: 0xDC2626, darkAlpha: 0.14, lightAlpha: 0.08)
    static let repost = dynamic(Hex.repost)
    static let event = dynamic(Hex.event)
    static let eventSoft = dynamic(dark: 0xFBBF24, light: 0xD97706, darkAlpha: 0.14, lightAlpha: 0.10)
    /// Follows are structure, not an accent job.
    static let follow = dynamic(Hex.follow)
    static let neutral = dynamic(Hex.neutral)
    /// Neutral node body.
    static let node = dynamic(Hex.node)
    static let postInk = dynamic(Hex.post)
    /// `0 1px 2px rgba(17,24,39,.04)` in light mode, nothing in dark.
    static let cardShadow = dynamic(dark: 0x000000, light: 0x111827, darkAlpha: 0, lightAlpha: 0.04)

    static let popoverShadow = dynamic(dark: 0x000000, light: 0x111827, darkAlpha: 0.4, lightAlpha: 0.12)

    // MARK: camps

    /// Camp colours in camp order: indigo, sky, teal, violet, pink. "Other" and the fallback groups are slate.
    static let campHexes: [UInt32] = [0x6366F1, 0x0EA5E9, 0x14B8A6, 0xA855F7, 0xEC4899]
    static let campSlate: UInt32 = 0x94A3B8

    // MARK: type

    /// SF Pro, fixed sizes (macOS has no Dynamic Type).
    static func ui(_ size: CGFloat, _ weight: Font.Weight = .regular) -> Font {
        .system(size: size, weight: weight)
    }

    /// SF Mono for #ids, numbers in tables and provenance. Digits are tabular.
    static func mono(_ size: CGFloat, _ weight: Font.Weight = .regular) -> Font {
        .system(size: size, weight: weight, design: .monospaced).monospacedDigit()
    }

    static let caption = ui(11)
    static let small = ui(12)
    static let body = ui(13)
    static let bodyStrong = ui(13, .semibold)
    static let large = ui(15)
    static let headline = ui(15, .semibold)
    static let title = ui(20, .semibold)
    static let display = ui(28, .semibold)
    /// Section titles: 13 semibold ink, no small caps.
    static let sectionTitle = ui(13, .semibold)

    // MARK: space

    static let space1: CGFloat = 4
    static let space2: CGFloat = 8
    static let space3: CGFloat = 12
    static let space4: CGFloat = 16
    static let space5: CGFloat = 24
    static let space6: CGFloat = 32
    /// Radius of controls (buttons, fields, rows).
    static let radius: CGFloat = 6
    static let cardRadius: CGFloat = 10
    /// A 1 px hairline (1 point on 2x displays is two pixels, so use a device pixel).
    static let seamWidth: CGFloat = 1
}

extension NSColor {
    convenience init(hex: UInt32, alpha: Double = 1) {
        self.init(srgbRed: Double((hex >> 16) & 0xFF) / 255, green: Double((hex >> 8) & 0xFF) / 255,
                  blue: Double(hex & 0xFF) / 255, alpha: alpha)
    }
}

extension Color {
    init(hex: UInt32, opacity: Double = 1) {
        self.init(.sRGB, red: Double((hex >> 16) & 0xFF) / 255, green: Double((hex >> 8) & 0xFF) / 255,
                  blue: Double(hex & 0xFF) / 255, opacity: opacity)
    }
}

// MARK: - building blocks

/// A 1 px horizontal or vertical hairline.
struct Seam: View {
    var vertical = false

    var body: some View {
        Rectangle()
            .fill(Theme.seam)
            .frame(width: vertical ? Theme.seamWidth : nil, height: vertical ? nil : Theme.seamWidth)
            .accessibilityHidden(true)
    }
}

/// The card surface: white (graphite in dark), 1 px border, 10 pt radius, hairline shadow in light mode only.
struct CardStyle: ViewModifier {
    var padding: CGFloat = Theme.space4
    /// Clip the content to the card's rounded shape (for cards whose content runs edge to edge).
    var clips = false

    func body(content: Content) -> some View {
        content
            .padding(padding)
            .clipShape(RoundedRectangle(cornerRadius: clips ? Theme.cardRadius : 0))
            .background(Theme.card, in: RoundedRectangle(cornerRadius: Theme.cardRadius))
            .overlay(RoundedRectangle(cornerRadius: Theme.cardRadius).strokeBorder(Theme.seam, lineWidth: 1))
            .shadow(color: Theme.cardShadow, radius: 1, x: 0, y: 1)
    }
}

extension View {
    func card(padding: CGFloat = Theme.space4, clips: Bool = false) -> some View {
        modifier(CardStyle(padding: padding, clips: clips))
    }

    /// Panel ground: the window ground.
    func panelGround() -> some View { background(Theme.ground) }

    /// A floating popover surface (tooltips, zoom controls): card fill, strong border, no heavy shadow.
    func elevatedSurface() -> some View {
        background(Theme.card, in: RoundedRectangle(cornerRadius: Theme.radius + 2))
            .overlay(RoundedRectangle(cornerRadius: Theme.radius + 2).strokeBorder(Theme.seamStrong, lineWidth: 1))
            .shadow(color: Theme.popoverShadow, radius: 6, x: 0, y: 2)
    }

    func consoleField() -> some View { modifier(ConsoleFieldStyle()) }
}

/// A card section title: 13 semibold ink with an optional trailing accessory.
struct SectionTitle<Trailing: View>: View {
    let title: String
    @ViewBuilder var trailing: Trailing

    var body: some View {
        HStack(spacing: Theme.space2) {
            Text(title)
                .font(Theme.sectionTitle)
                .foregroundStyle(Theme.text)
                .accessibilityAddTraits(.isHeader)
            Spacer(minLength: Theme.space2)
            trailing
        }
    }
}

extension SectionTitle where Trailing == EmptyView {
    init(_ title: String) {
        self.title = title
        self.trailing = EmptyView()
    }
}

extension SectionTitle {
    init(_ title: String, @ViewBuilder trailing: () -> Trailing) {
        self.title = title
        self.trailing = trailing()
    }
}

/// Button: 28 pt, 6 pt radius. Primary is the indigo fill; secondary is a card with a border; ghost has no chrome.
struct ConsoleButtonStyle: ButtonStyle {
    enum Kind { case primary, secondary, ghost, danger, event }
    var kind: Kind = .secondary
    var compact = false

    func makeBody(configuration: Configuration) -> some View {
        ButtonBody(configuration: configuration, kind: kind, compact: compact)
    }

    private struct ButtonBody: View {
        let configuration: Configuration
        let kind: Kind
        let compact: Bool
        @Environment(\.isEnabled) private var isEnabled
        @State private var hovering = false

        var body: some View {
            let pressed = configuration.isPressed
            configuration.label
                .font(Theme.ui(compact ? 12 : 13, .medium))
                .lineLimit(1)
                .padding(.horizontal, compact ? 8 : 12)
                .frame(height: compact ? 24 : 28)
                .foregroundStyle(foreground)
                .background(background(pressed: pressed), in: RoundedRectangle(cornerRadius: Theme.radius))
                .overlay(RoundedRectangle(cornerRadius: Theme.radius).strokeBorder(border, lineWidth: 1))
                .opacity(isEnabled ? 1 : 0.45)
                .contentShape(RoundedRectangle(cornerRadius: Theme.radius))
                .onHover { hovering = $0 }
        }

        private var foreground: Color {
            switch kind {
            case .primary: Theme.accentInk
            case .secondary: Theme.text
            case .ghost: Theme.secondary
            case .danger: Theme.negative
            case .event: Theme.event
            }
        }

        private func background(pressed: Bool) -> Color {
            switch kind {
            case .primary: Theme.accentFill.opacity(pressed ? 0.78 : (hovering ? 0.9 : 1))
            case .secondary: pressed ? Theme.seam : (hovering ? Theme.hover : Theme.card)
            case .ghost: pressed ? Theme.seam : (hovering ? Theme.hover : .clear)
            case .danger: pressed || hovering ? Theme.negativeSoft : .clear
            case .event: pressed || hovering ? Theme.eventSoft : .clear
            }
        }

        private var border: Color {
            switch kind {
            case .primary: Theme.accentFill
            case .secondary: Theme.seamStrong
            case .ghost: .clear
            case .danger: Theme.negative.opacity(0.4)
            case .event: Theme.event.opacity(0.4)
            }
        }
    }
}

extension ButtonStyle where Self == ConsoleButtonStyle {
    static var consolePrimary: ConsoleButtonStyle { .init(kind: .primary) }
    static var console: ConsoleButtonStyle { .init(kind: .secondary) }
    static var consoleGhost: ConsoleButtonStyle { .init(kind: .ghost) }
    static var consoleDanger: ConsoleButtonStyle { .init(kind: .danger) }
    static func console(_ kind: ConsoleButtonStyle.Kind, compact: Bool = false) -> ConsoleButtonStyle {
        .init(kind: kind, compact: compact)
    }
}

/// A text field on a card with a 1 px control border.
struct ConsoleFieldStyle: ViewModifier {
    func body(content: Content) -> some View {
        content
            .textFieldStyle(.plain)
            .font(Theme.body)
            .padding(.horizontal, 8)
            .frame(minHeight: 28)
            .background(Theme.card, in: RoundedRectangle(cornerRadius: Theme.radius))
            .overlay(RoundedRectangle(cornerRadius: Theme.radius).strokeBorder(Theme.seamStrong, lineWidth: 1))
    }
}

/// A plain run-state chip: an icon and the state in words on a quiet pill.
struct StatusChip: View {
    let state: RunState
    var compact = false

    var body: some View {
        HStack(spacing: 4) {
            Image(systemName: symbol)
                .font(.system(size: compact ? 9 : 10, weight: .bold))
                .foregroundStyle(tint)
            Text(state.title)
                .font(Theme.ui(compact ? 11 : 12, .medium))
                .foregroundStyle(Theme.secondary)
        }
        .padding(.horizontal, compact ? 6 : 8)
        .padding(.vertical, compact ? 2 : 3)
        .background(Theme.hover, in: Capsule())
        .fixedSize()
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("State: \(state.title)")
    }

    private var symbol: String {
        switch state {
        case .running: "circle.dotted"
        case .queued: "clock"
        case .done: "checkmark.circle.fill"
        case .failed: "exclamationmark.triangle.fill"
        case .stopped: "stop.circle.fill"
        case .unknown: "questionmark.circle"
        }
    }

    private var tint: Color {
        switch state {
        case .running, .queued: Theme.accent
        case .done: Theme.positive
        case .failed: Theme.negative
        case .stopped, .unknown: Theme.muted
        }
    }
}

// MARK: - form building blocks

/// A titled block of a sheet or settings pane: a 13 semibold heading, rows below, optional footnote.
struct ConsoleSection<Trailing: View, Content: View>: View {
    let title: String
    var footer: String?
    @ViewBuilder var trailing: Trailing
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: Theme.space3) {
            SectionTitle(title: title) { trailing }
            VStack(alignment: .leading, spacing: Theme.space3) {
                content
            }
            if let footer {
                Text(footer)
                    .font(Theme.caption)
                    .foregroundStyle(Theme.muted)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(.horizontal, Theme.space5)
        .padding(.vertical, Theme.space4)
        .frame(maxWidth: .infinity, alignment: .leading)
        .overlay(alignment: .bottom) { Seam() }
    }
}

extension ConsoleSection where Trailing == EmptyView {
    init(_ title: String, footer: String? = nil, @ViewBuilder content: () -> Content) {
        self.title = title
        self.footer = footer
        self.trailing = EmptyView()
        self.content = content()
    }
}

extension ConsoleSection {
    init(_ title: String, footer: String? = nil, @ViewBuilder trailing: () -> Trailing, @ViewBuilder content: () -> Content) {
        self.title = title
        self.footer = footer
        self.trailing = trailing()
        self.content = content()
    }
}

/// A text-style link button for section headers ("Choose Files…", "Add Event").
struct ConsoleLinkStyle: ButtonStyle {
    var tint: Color = Theme.link

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(Theme.ui(12, .medium))
            .foregroundStyle(tint.opacity(configuration.isPressed ? 0.7 : 1))
    }
}

extension ButtonStyle where Self == ConsoleLinkStyle {
    static var consoleLink: ConsoleLinkStyle { .init() }
}
