import CoreGraphics
import Foundation

/// World-to-screen transform for a graph canvas: `screen = viewCentre + offset + world * scale * (stretch, 1)`.
/// `stretch` widens the picture horizontally (nodes stay round) so a wide panel is filled instead of
/// framing a round cluster in its middle.
nonisolated struct CanvasCamera: Equatable, Sendable {
    var scale: Double = 1
    var offset: SIMD2<Double> = .zero
    var stretch: Double = 1

    static let minScale = 0.12
    static let maxScale = 8.0

    func toScreen(_ p: SIMD2<Double>, size: CGSize) -> CGPoint {
        CGPoint(x: size.width / 2 + offset.x + p.x * scale * stretch, y: size.height / 2 + offset.y + p.y * scale)
    }

    func toWorld(_ s: CGPoint, size: CGSize) -> SIMD2<Double> {
        SIMD2((s.x - size.width / 2 - offset.x) / (scale * stretch), (s.y - size.height / 2 - offset.y) / scale)
    }

    /// Zooms by `factor` keeping the world point under `anchor` fixed on screen.
    mutating func zoom(by factor: Double, anchor: CGPoint, size: CGSize) {
        guard factor.isFinite, factor > 0 else { return }
        let w = toWorld(anchor, size: size)
        scale = min(Self.maxScale, max(Self.minScale, scale * factor))
        offset = SIMD2(anchor.x - size.width / 2 - w.x * scale * stretch, anchor.y - size.height / 2 - w.y * scale)
    }

    mutating func pan(by delta: CGSize) {
        offset += SIMD2(delta.width, delta.height)
    }

    /// A camera that frames `bounds` (world min/max) inside `size` with `padding` points on each side.
    /// With `maxStretch` > 1 the picture is also widened (up to that factor) to match the panel's aspect.
    static func fitting(min lo: SIMD2<Double>, max hi: SIMD2<Double>, size: CGSize, padding: Double = 36,
                        maxScale: Double = 2.4, maxStretch: Double = 1) -> CanvasCamera {
        guard size.width > 1, size.height > 1, lo.x.isFinite, hi.x.isFinite, lo.x <= hi.x, lo.y <= hi.y else {
            return CanvasCamera()
        }
        let w = max(hi.x - lo.x, 1), h = max(hi.y - lo.y, 1)
        let availW = max(size.width - 2 * padding, 40), availH = max(size.height - 2 * padding, 40)
        // Stretch first (to the panel's aspect, within the cap), then size the picture to what is left.
        let stretch = Swift.min(Swift.max(1, maxStretch), Swift.max(1, (availW / availH) / (w / h)))
        let s = min(maxScale, max(minScale, min(availW / (w * stretch), availH / h)))
        let c = (lo + hi) / 2
        return CanvasCamera(scale: s, offset: SIMD2(-c.x * s * stretch, -c.y * s), stretch: stretch)
    }

    /// Exponential approach toward `target`; `t` in 0...1.
    func approaching(_ target: CanvasCamera, _ t: Double) -> CanvasCamera {
        let k = min(1, max(0, t))
        // Interpolate scale geometrically so zoom speed feels even.
        let s = exp(log(scale) + (log(target.scale) - log(scale)) * k)
        return CanvasCamera(scale: s, offset: offset + (target.offset - offset) * k,
                            stretch: stretch + (target.stretch - stretch) * k)
    }
}

/// Small geometry helpers shared by the renderers.
nonisolated enum CanvasGeometry {
    /// Zoom factor for fixed-size nodes: they swell a little when zoomed in and shrink a little when zoomed out,
    /// but stay readable steps of the influence ramp.
    static func nodeScale(_ cameraScale: Double) -> Double { min(1.45, max(0.85, cameraScale.squareRoot())) }

    /// Point on the quadratic Bézier p0 → (c) → p1 at t.
    static func quadPoint(_ p0: CGPoint, _ c: CGPoint, _ p1: CGPoint, _ t: Double) -> CGPoint {
        let u = 1 - t
        return CGPoint(x: u * u * p0.x + 2 * u * t * c.x + t * t * p1.x,
                       y: u * u * p0.y + 2 * u * t * c.y + t * t * p1.y)
    }

    /// Tangent of the quadratic Bézier at t (not normalised).
    static func quadTangent(_ p0: CGPoint, _ c: CGPoint, _ p1: CGPoint, _ t: Double) -> CGVector {
        CGVector(dx: 2 * (1 - t) * (c.x - p0.x) + 2 * t * (p1.x - c.x),
                 dy: 2 * (1 - t) * (c.y - p0.y) + 2 * t * (p1.y - c.y))
    }

    /// Control point bending the chord p0 → p1 sideways by `bend` × its length.
    static func control(_ p0: CGPoint, _ p1: CGPoint, bend: Double) -> CGPoint {
        let mx = (p0.x + p1.x) / 2, my = (p0.y + p1.y) / 2
        let dx = p1.x - p0.x, dy = p1.y - p0.y
        return CGPoint(x: mx - dy * bend, y: my + dx * bend)
    }

    /// Moves `p` toward `toward` by `distance` points (to start/end arcs at node rims).
    static func inset(_ p: CGPoint, toward: CGPoint, by distance: Double) -> CGPoint {
        let dx = toward.x - p.x, dy = toward.y - p.y
        let len = (dx * dx + dy * dy).squareRoot()
        guard len > distance * 2, len > 0 else { return p }
        return CGPoint(x: p.x + dx / len * distance, y: p.y + dy / len * distance)
    }

    /// Distance from `p` to the segment a–b.
    static func distance(from p: CGPoint, toSegment a: CGPoint, _ b: CGPoint) -> Double {
        let abx = b.x - a.x, aby = b.y - a.y
        let len2 = abx * abx + aby * aby
        var t = len2 > 0 ? ((p.x - a.x) * abx + (p.y - a.y) * aby) / len2 : 0
        t = min(1, max(0, t))
        let qx = a.x + abx * t - p.x, qy = a.y + aby * t - p.y
        return (qx * qx + qy * qy).squareRoot()
    }

    /// Bounds of a set of world points expanded by per-point radii.
    static func bounds(_ points: [SIMD2<Double>], radii: [Double] = []) -> (min: SIMD2<Double>, max: SIMD2<Double>)? {
        guard !points.isEmpty else { return nil }
        var lo = SIMD2<Double>(repeating: .infinity), hi = SIMD2<Double>(repeating: -.infinity)
        for (i, p) in points.enumerated() where p.x.isFinite && p.y.isFinite {
            let r = radii.indices.contains(i) ? radii[i] : 0
            lo = pointwiseMin(lo, p - SIMD2(repeating: r))
            hi = pointwiseMax(hi, p + SIMD2(repeating: r))
        }
        return lo.x.isFinite ? (lo, hi) : nil
    }
}
