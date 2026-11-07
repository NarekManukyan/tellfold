import AppKit
import SwiftUI

// Illustrated, deterministic avatars for simulated people and monogram tiles for organisations.
//
// A port of `shoal/web/static/js/avatar.js`, shape for shape: the same persona gets the same face on the web and in
// the Mac app, so keep the two in step. Faces are plain circles, ellipses and rounded rectangles in a 64-unit box.
// People are framed on 5...59 and clipped to a circle of radius 27 at (32, 32); organisations are a rounded square
// (radius 14/64) in a tile colour with white initials.

/// The face as data: every renderer draws exactly these choices.
nonisolated struct AvatarSpec: Equatable, Sendable {
    var back: String
    var skin: String
    var hair: String
    var shirt: String
    /// 0 short, 1 side part, 2 bob, 3 long, 4 bun, 5 curls, 6 buzz, 7 bald with beard.
    var style: Int
    var glasses: Bool
    /// 0 flat, 1 smile, 2 open.
    var smile: Int
}

nonisolated enum Avatar {
    static let skin = ["#F6D3B8", "#EBBE9B", "#D8A07C", "#B98059", "#8E5B3C", "#5F3C28"]
    static let hair = ["#231A14", "#4A3426", "#7A4E2D", "#B5813E", "#DDB874", "#9A9A9A", "#B4452C"]
    static let shirt = ["#6366F1", "#0EA5E9", "#14B8A6", "#F59E0B", "#EC4899", "#8B5CF6", "#64748B", "#22C55E"]
    static let back = ["#E0E7FF", "#E0F2FE", "#CCFBF1", "#FEF3C7", "#FCE7F3", "#EDE9FE", "#F1F5F9", "#DCFCE7"]
    static let tile = ["#4F46E5", "#0369A1", "#0F766E", "#B45309", "#BE185D", "#6D28D9", "#334155", "#15803D"]
    static let ink = "#1F2937"

    /// FNV-1a (32 bit) over the UTF-16 code units of `"<id>:<handle>"`; never 0.
    static func seed(id: String, handle: String = "") -> UInt32 {
        var h: UInt32 = 0x811c_9dc5
        for unit in "\(id):\(handle)".utf16 {
            h ^= UInt32(unit)
            h = h &* 0x0100_0193
        }
        return h == 0 ? 1 : h
    }

    /// The face for a persona: xorshift32 steps, one choice per step in the order back, skin, hair, shirt, style,
    /// glasses, smile.
    static func spec(id: String, handle: String = "") -> AvatarSpec {
        var x = seed(id: id, handle: handle)
        func pick(_ n: Int) -> Int {
            x ^= x << 13
            x ^= x >> 17
            x ^= x << 5
            return Int(x % UInt32(n))
        }
        return AvatarSpec(
            back: back[pick(back.count)],
            skin: skin[pick(skin.count)],
            hair: hair[pick(hair.count)],
            shirt: shirt[pick(shirt.count)],
            style: pick(8),
            glasses: pick(10) < 3,
            smile: pick(3))
    }

    /// Initials for a monogram: "App Store" is "AS", "revenuecat" is "RE".
    static func initials(_ name: String, _ handle: String = "") -> String {
        let source = !name.isEmpty ? name : (!handle.isEmpty ? handle : "?")
        let words = String(source.map { $0 == "_" || $0 == "@" ? " " : $0 })
            .split(whereSeparator: { $0.isWhitespace })
            .map(String.init)
        if words.count >= 2, let a = words[0].first, let b = words[1].first {
            return (String(a) + String(b)).uppercased()
        }
        return String((words.first ?? "?").prefix(2)).uppercased()
    }

    static func tileColor(id: String, handle: String = "") -> String {
        tile[Int(seed(id: id, handle: handle) % UInt32(tile.count))]
    }

    static func color(_ hex: String) -> Color {
        Color(hex: hexValue(hex))
    }

    static func hexValue(_ hex: String) -> UInt32 {
        UInt32(hex.trimmingCharacters(in: CharacterSet(charactersIn: "#")), radix: 16) ?? 0
    }
}

/// Who an avatar stands for. Equal identities draw the same picture.
nonisolated struct AvatarIdentity: Hashable, Sendable {
    var id: String
    var handle: String
    var name: String
    var isOrganisation: Bool

    init(id: String, handle: String, name: String, isOrganisation: Bool) {
        self.id = id
        self.handle = handle
        self.name = name
        self.isOrganisation = isOrganisation
    }

    init(_ persona: Persona) {
        self.init(id: persona.id, handle: persona.handle, name: persona.name, isOrganisation: persona.entityID != nil)
    }
}

/// Draws an avatar into a square of `size` points. People use the 5...59 frame clipped to a circle; organisations a
/// rounded-square monogram.
nonisolated enum AvatarArt {
    static func draw(_ identity: AvatarIdentity, in context: inout GraphicsContext, size: CGFloat) {
        if identity.isOrganisation {
            drawTile(identity, in: &context, size: size)
        } else {
            drawFace(Avatar.spec(id: identity.id, handle: identity.handle), in: &context, size: size)
        }
    }

    private static func drawTile(_ identity: AvatarIdentity, in context: inout GraphicsContext, size: CGFloat) {
        let tile = Avatar.color(Avatar.tileColor(id: identity.id, handle: identity.handle))
        let rect = CGRect(x: 0, y: 0, width: size, height: size)
        context.fill(Path(roundedRect: rect, cornerRadius: size * 14 / 64), with: .color(tile))
        let text = Avatar.initials(identity.name, identity.handle)
        let fontSize = (text.count > 1 ? 24.0 : 30.0) * size / 64
        context.draw(
            Text(text).font(.system(size: fontSize, weight: .semibold)).foregroundStyle(.white),
            at: CGPoint(x: size / 2, y: size * 33 / 64), anchor: .center)
    }

    private static func drawFace(_ spec: AvatarSpec, in context: inout GraphicsContext, size: CGFloat) {
        var g = context
        g.scaleBy(x: size / 54, y: size / 54)
        g.translateBy(x: -5, y: -5)
        g.clip(to: Path(ellipseIn: CGRect(x: 5, y: 5, width: 54, height: 54)))

        let skin = Avatar.color(spec.skin), hair = Avatar.color(spec.hair), ink = Avatar.color(Avatar.ink)

        func circle(_ cx: Double, _ cy: Double, _ r: Double, _ color: Color, opacity: Double = 1) {
            g.fill(Path(ellipseIn: CGRect(x: cx - r, y: cy - r, width: 2 * r, height: 2 * r)), with: .color(color.opacity(opacity)))
        }
        func ellipse(_ cx: Double, _ cy: Double, _ rx: Double, _ ry: Double, _ color: Color, opacity: Double = 1) {
            g.fill(Path(ellipseIn: CGRect(x: cx - rx, y: cy - ry, width: 2 * rx, height: 2 * ry)), with: .color(color.opacity(opacity)))
        }
        func rect(_ x: Double, _ y: Double, _ w: Double, _ h: Double, _ rr: Double, _ color: Color) {
            g.fill(Path(roundedRect: CGRect(x: x, y: y, width: w, height: h), cornerRadius: rr), with: .color(color))
        }

        rect(0, 0, 64, 64, 0, Avatar.color(spec.back))
        ellipse(32, 66, 23, 15, Avatar.color(spec.shirt)) // shoulders
        if spec.style == 2 { rect(13, 24, 9, 20, 4.5, hair); rect(42, 24, 9, 20, 4.5, hair) } // bob sides
        if spec.style == 3 { rect(12, 22, 10, 32, 5, hair); rect(42, 22, 10, 32, 5, hair) } // long sides
        rect(28, 44, 8, 9, 3, skin) // neck
        circle(16.5, 35, 3.6, skin); circle(47.5, 35, 3.6, skin) // ears
        circle(32, 34, 15.5, skin) // face
        if spec.style == 7 { // beard along the jaw
            var beard = Path()
            beard.move(to: CGPoint(x: 18.5, y: 37))
            beard.addQuadCurve(to: CGPoint(x: 45.5, y: 37), control: CGPoint(x: 32, y: 57))
            g.stroke(beard, with: .color(hair), style: StrokeStyle(lineWidth: 5, lineCap: .round))
        }
        switch spec.style {
        case 0: ellipse(32, 23.5, 16.5, 8.5, hair)
        case 1: ellipse(29.5, 23, 17, 8.5, hair); circle(44.5, 26.5, 5, hair)
        case 2: ellipse(32, 23, 18, 9.5, hair)
        case 3: ellipse(32, 22.5, 18, 9.5, hair)
        case 4: ellipse(32, 24, 16.5, 8, hair); circle(32, 12.5, 6, hair)
        case 5:
            for (x, y) in [(18.0, 27.0), (20, 20), (26, 16), (32, 14.5), (38, 16), (44, 20), (46, 27)] { circle(x, y, 6, hair) }
        case 6: ellipse(32, 24, 15.5, 7.5, hair, opacity: 0.55)
        default: break
        }
        circle(26.5, 34.5, 1.9, ink); circle(37.5, 34.5, 1.9, ink) // eyes
        switch spec.smile {
        case 0:
            rect(28.5, 41, 7, 1.8, 0.9, ink)
        case 1:
            var smile = Path()
            smile.move(to: CGPoint(x: 27.5, y: 40.2))
            smile.addQuadCurve(to: CGPoint(x: 36.5, y: 40.2), control: CGPoint(x: 32, y: 44.6))
            g.stroke(smile, with: .color(ink), style: StrokeStyle(lineWidth: 1.8, lineCap: .round))
        default: // open smile
            var open = Path()
            open.move(to: CGPoint(x: 27.8, y: 40))
            open.addQuadCurve(to: CGPoint(x: 36.2, y: 40), control: CGPoint(x: 32, y: 46.2))
            open.closeSubpath()
            g.fill(open, with: .color(ink))
        }
        if spec.glasses {
            let line = StrokeStyle(lineWidth: 1.4)
            g.stroke(Path(ellipseIn: CGRect(x: 26.5 - 4.6, y: 34.5 - 4.6, width: 9.2, height: 9.2)), with: .color(ink), style: line)
            g.stroke(Path(ellipseIn: CGRect(x: 37.5 - 4.6, y: 34.5 - 4.6, width: 9.2, height: 9.2)), with: .color(ink), style: line)
            var bridge = Path()
            bridge.move(to: CGPoint(x: 31.1, y: 34.5))
            bridge.addLine(to: CGPoint(x: 32.9, y: 34.5))
            g.stroke(bridge, with: .color(ink), style: line)
        }
    }
}

/// An avatar at any size, drawn as vectors. The same persona gets the same face everywhere.
struct AvatarView: View {
    let identity: AvatarIdentity
    var size: CGFloat = 32

    init(persona: Persona, size: CGFloat = 32) {
        identity = AvatarIdentity(persona)
        self.size = size
    }

    init(identity: AvatarIdentity, size: CGFloat = 32) {
        self.identity = identity
        self.size = size
    }

    var body: some View {
        Canvas { context, canvasSize in
            var context = context
            AvatarArt.draw(identity, in: &context, size: min(canvasSize.width, canvasSize.height))
        }
        .frame(width: size, height: size)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(identity.name.isEmpty ? identity.handle : identity.name)
    }
}

/// Bitmaps of avatars, rendered once per identity, so the network canvas can draw dozens of them per frame cheaply.
@MainActor
final class AvatarImageCache {
    static let shared = AvatarImageCache()

    /// Side of the cached bitmap in pixels: crisp up to the largest graph avatar at 2x with room for zooming in.
    static let pixels: CGFloat = 192

    private var images: [AvatarIdentity: CGImage] = [:]

    /// The bitmap if it was already rendered; never renders.
    func cached(for identity: AvatarIdentity) -> CGImage? { images[identity] }

    func image(for identity: AvatarIdentity) -> CGImage? {
        if let cached = images[identity] { return cached }
        let renderer = ImageRenderer(content: AvatarView(identity: identity, size: 64))
        renderer.scale = Self.pixels / 64
        guard let image = renderer.cgImage else { return nil }
        images[identity] = image
        return image
    }

    var count: Int { images.count }
}
