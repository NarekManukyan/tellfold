import Observation
import SwiftUI

/// Main-actor state of the crowd graph: which view is showing, the animated positions, the camera, hover and the
/// shared pointer handling. Layouts are computed analytically (`CrowdLayout`), so switching view or round is a short
/// tween between two sets of positions, not a physics simulation.
@MainActor
@Observable
final class CrowdModel: CanvasGesturing {
    /// The view the user picked. The effective view falls back to Camps when the run has no sentiment (see `effective`).
    var mode: CrowdMode = .camps

    var hoveredID: String?
    var hoveredEdge: CrowdEdge?
    var hoverLocation: CGPoint?
    var isHovering = false
    /// Set once the user pans or zooms; nil means "frame the picture automatically".
    var userCamera: CanvasCamera?
    /// True while a transition is running (keeps the frame timeline ticking).
    private(set) var isAnimating = false
    /// Bumped when avatar bitmaps become available so the view redraws with them.
    private(set) var avatarVersion = 0
    /// Bumped whenever a layout is applied, so a view at rest redraws with it.
    private(set) var layoutVersion = 0

    // Hit testing and drawing read these every frame.
    @ObservationIgnored private(set) var nodes: [CrowdNode] = []
    @ObservationIgnored private(set) var edges: [CrowdEdge] = []
    @ObservationIgnored private(set) var layout = CrowdLayout()
    @ObservationIgnored var viewSize: CGSize = .zero

    @ObservationIgnored private var from: [String: CGPoint] = [:]
    @ObservationIgnored private var start = 0.0
    @ObservationIgnored private var duration = 0.0
    @ObservationIgnored private var cameraFrom = CanvasCamera()
    @ObservationIgnored private var cameraTo = CanvasCamera()
    @ObservationIgnored private var weightsFrom = [1.0, 0, 0]
    @ObservationIgnored private var weightsTo = [1.0, 0, 0]
    @ObservationIgnored private var appliedContentKey: Int?
    @ObservationIgnored private var appliedSize: CGSize = .zero
    @ObservationIgnored private var endTask: Task<Void, Never>?
    @ObservationIgnored private var panStart: CanvasCamera?
    @ObservationIgnored private var magnifyStart: CanvasCamera?

    static let transition = 0.4

    /// The view actually drawn: Shoal needs per-agent sentiment, so without any it shows the Camps layout.
    func effective(for scene: CrowdScene) -> CrowdMode {
        mode == .shoal && !scene.runHasSentiment ? .camps : mode
    }

    // MARK: layout and animation

    /// Computes the layout for `mode` and starts a transition from wherever the avatars are now. A change of size alone
    /// (a window resize) snaps instead of animating.
    func relayout(scene: CrowdScene, mode: CrowdMode, size: CGSize, reduceMotion: Bool, compact: Bool = false) {
        guard size.width > 1, size.height > 1 else { return }
        viewSize = size
        effectiveIsInfluence = mode == .influence
        nodes = scene.nodes
        edges = scene.edges
        let key = scene.layoutSignature(mode: mode)
        let now = ProcessInfo.processInfo.systemUptime
        let target = CrowdLayout.compute(nodes: scene.nodes, groupCount: scene.groups.count, sentiment: scene.sentiment,
                                         size: size, mode: mode, labels: scene.groups.map { scene.label($0, compact: compact) })
        let first = appliedContentKey == nil
        let sizeOnly = appliedContentKey == key
        let animate = !first && !sizeOnly && !reduceMotion
        from = animate ? positions(at: now) : target.positions
        cameraFrom = animate ? autoCamera(at: now) : Self.camera(for: target)
        cameraTo = Self.camera(for: target)
        weightsFrom = animate ? weights(at: now) : Self.oneHot(mode)
        weightsTo = Self.oneHot(mode)
        layout = target
        start = now
        duration = animate ? Self.transition : 0
        appliedContentKey = key
        appliedSize = size
        layoutVersion += 1
        endTask?.cancel()
        if animate {
            isAnimating = true
            endTask = Task { [weak self] in
                try? await Task.sleep(for: .seconds(Self.transition + 0.08))
                guard !Task.isCancelled else { return }
                self?.isAnimating = false
            }
        } else if isAnimating {
            isAnimating = false
        }
    }

    private static func oneHot(_ mode: CrowdMode) -> [Double] {
        [mode == .camps ? 1 : 0, mode == .shoal ? 1 : 0, mode == .influence ? 1 : 0]
    }

    private static func camera(for layout: CrowdLayout) -> CanvasCamera {
        let s = layout.fitScale
        return CanvasCamera(scale: s, offset: SIMD2(-layout.center.x * s, -layout.center.y * s))
    }

    /// 0 at the start of the transition, 1 at its end (eased).
    private func progress(at now: Double) -> Double {
        guard duration > 0 else { return 1 }
        let t = min(1, max(0, (now - start) / duration))
        return t < 0.5 ? 4 * t * t * t : 1 - pow(-2 * t + 2, 3) / 2
    }

    func positions(at now: Double) -> [String: CGPoint] {
        let t = progress(at: now)
        if t >= 1 { return layout.positions }
        var out: [String: CGPoint] = [:]
        out.reserveCapacity(layout.positions.count)
        for (id, to) in layout.positions {
            let f = from[id] ?? to
            out[id] = CGPoint(x: f.x + (to.x - f.x) * t, y: f.y + (to.y - f.y) * t)
        }
        return out
    }

    func autoCamera(at now: Double) -> CanvasCamera {
        let t = progress(at: now)
        return t >= 1 ? cameraTo : cameraFrom.approaching(cameraTo, t)
    }

    /// How present each decoration is: camp regions, the Shoal axis, the Influence rings.
    func weights(at now: Double) -> [Double] {
        let t = progress(at: now)
        return (0..<3).map { weightsFrom[$0] + (weightsTo[$0] - weightsFrom[$0]) * t }
    }

    var camera: CanvasCamera { userCamera ?? autoCamera(at: ProcessInfo.processInfo.systemUptime) }

    /// Makes sure every avatar bitmap exists, then asks for a redraw.
    func warmAvatars(_ nodes: [CrowdNode]) {
        var added = false
        for n in nodes where AvatarImageCache.shared.cached(for: n.identity) == nil {
            if AvatarImageCache.shared.image(for: n.identity) != nil { added = true }
        }
        if added { avatarVersion += 1 }
    }

    // MARK: hit testing

    /// Drawn radius factor: avatars follow the camera scale, kept readable.
    nonisolated static func nodeScale(_ cameraScale: Double) -> Double { min(2.5, max(0.35, cameraScale)) }

    func hitNode(_ point: CGPoint) -> String? {
        let now = ProcessInfo.processInfo.systemUptime
        let pos = positions(at: now)
        let cam = camera
        let ns = Self.nodeScale(cam.scale)
        var best: (String, Double)?
        for n in nodes {
            guard let p = pos[n.id] else { continue }
            let s = cam.toScreen(SIMD2(p.x, p.y), size: viewSize)
            let d = hypot(s.x - point.x, s.y - point.y)
            if d <= n.radius * ns + 3, d < (best?.1 ?? .infinity) { best = (n.id, d) }
        }
        return best?.0
    }

    /// The aggregated edge under `point` (Influence view), if any.
    func hitEdge(_ point: CGPoint) -> CrowdEdge? {
        let now = ProcessInfo.processInfo.systemUptime
        let pos = positions(at: now)
        let cam = camera
        let ns = Self.nodeScale(cam.scale)
        var radius: [String: Double] = [:]
        for n in nodes { radius[n.id] = n.radius * ns }
        var best: (CrowdEdge, Double)?
        for e in edges {
            guard let a = pos[e.from], let b = pos[e.to] else { continue }
            let sa = cam.toScreen(SIMD2(a.x, a.y), size: viewSize), sb = cam.toScreen(SIMD2(b.x, b.y), size: viewSize)
            let curve = CrowdRenderer.edgeCurve(from: sa, to: sb, fromRadius: radius[e.from] ?? 10, toRadius: radius[e.to] ?? 10)
            var prev = curve.p0
            for k in 1...12 {
                let q = CanvasGeometry.quadPoint(curve.p0, curve.c, curve.p1, Double(k) / 12)
                let d = CanvasGeometry.distance(from: point, toSegment: prev, q)
                if d < 6, d < (best?.1 ?? .infinity) { best = (e, d) }
                prev = q
            }
        }
        return best?.0
    }

    // MARK: CanvasGesturing

    var hasUserCamera: Bool { userCamera != nil }

    func hover(_ phase: HoverPhase) {
        switch phase {
        case .active(let location):
            isHovering = true
            hoverLocation = location
            let hit = hitNode(location)
            if hit != hoveredID { hoveredID = hit }
            let edge = hit == nil && effectiveIsInfluence ? hitEdge(location) : nil
            if edge != hoveredEdge { hoveredEdge = edge }
        case .ended:
            isHovering = false
            hoverLocation = nil
            hoveredID = nil
            hoveredEdge = nil
        }
    }

    @ObservationIgnored var effectiveIsInfluence = false

    func zoom(by factor: Double, anchor: CGPoint?) {
        var cam = camera
        cam.zoom(by: factor, anchor: anchor ?? CGPoint(x: viewSize.width / 2, y: viewSize.height / 2), size: viewSize)
        userCamera = cam
    }

    func pan(by delta: CGSize) {
        var cam = camera
        cam.pan(by: delta)
        userCamera = cam
    }

    func fit() { userCamera = nil }

    func dragChanged(start: CGPoint, location: CGPoint, translation: CGSize) {
        if panStart == nil { panStart = camera }
        guard var cam = panStart else { return }
        cam.pan(by: translation)
        userCamera = cam
    }

    func dragEnded() { panStart = nil }

    func magnifyChanged(_ magnification: Double, anchor: CGPoint) {
        if magnifyStart == nil { magnifyStart = camera }
        guard var cam = magnifyStart else { return }
        cam.zoom(by: magnification, anchor: anchor, size: viewSize)
        userCamera = cam
    }

    func magnifyEnded() { magnifyStart = nil }

    func scroll(dx: Double, dy: Double, precise: Bool, zoomModifier: Bool) {
        if zoomModifier || !precise {
            zoom(by: exp(dy * (precise ? 0.01 : 0.08)), anchor: hoverLocation)
        } else {
            pan(by: CGSize(width: dx, height: dy))
        }
    }
}
