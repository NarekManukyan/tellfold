import Observation
import SwiftUI

/// Main-actor state shared by the network and knowledge-graph canvases: the layout engine, the camera (automatic
/// framing until the user pans or zooms), hover, node dragging and hit testing.
@MainActor
@Observable
final class GraphCanvasController {
    @ObservationIgnored let engine: ForceLayoutEngine

    /// Set once the user pans or zooms; nil means "frame the graph automatically".
    var userCamera: CanvasCamera?
    var hoveredID: String?
    var hoverLocation: CGPoint?
    var isHovering = false
    /// True while the layout is at rest (only possible with drift off, i.e. Reduce Motion).
    var layoutSettled = false
    var draggingID: String?
    /// Nodes keep a fixed on-screen size (the influence ramp) instead of scaling with the camera.
    @ObservationIgnored var fixedNodeSize = false
    /// How far the automatic framing may widen the picture to fill a wide panel (1 = never).
    @ObservationIgnored var maxStretch = 1.0
    /// Points kept clear around the automatically framed graph.
    @ObservationIgnored var fitPadding = 44.0

    @ObservationIgnored var viewSize: CGSize = .zero
    @ObservationIgnored private(set) var autoCamera = CanvasCamera()
    @ObservationIgnored private var lastFrameTime: Double = 0
    /// Drawn radius per node id from the last frame, for hit testing.
    @ObservationIgnored var radii: [String: Double] = [:]
    @ObservationIgnored private var panStartCamera: CanvasCamera?
    @ObservationIgnored private var magnifyStartCamera: CanvasCamera?
    @ObservationIgnored private var dragStartWorld: SIMD2<Double>?
    @ObservationIgnored private var dragNodeStart: SIMD2<Double>?

    init(parameters: ForceParameters = ForceParameters(), ambientDrift: Double = 0.035, seed: UInt64 = 7) {
        engine = ForceLayoutEngine(parameters: parameters, ambientDrift: ambientDrift, seed: seed)
    }

    var camera: CanvasCamera { userCamera ?? autoCamera }

    /// Current snapshot and the camera to draw it with. Eases the automatic camera toward a frame that fits the
    /// graph (snaps under Reduce Motion). Called once per rendered frame.
    func frameState(now: Double, size: CGSize, reduceMotion: Bool) -> (LayoutSnapshot, CanvasCamera) {
        viewSize = size
        let snapshot = engine.buffer.read()
        if userCamera == nil {
            let points = snapshot.positions(at: now)
            let r = snapshot.ids.map { radii[$0] ?? 6 }
            if let b = CanvasGeometry.bounds(points, radii: r) {
                let target = CanvasCamera.fitting(min: b.min, max: b.max, size: size, padding: fitPadding, maxScale: fixedNodeSize ? 6 : 2.4,
                                                    maxStretch: maxStretch)
                let dt = lastFrameTime > 0 ? min(0.25, max(0, now - lastFrameTime)) : 1
                let first = autoCamera == CanvasCamera() || reduceMotion
                autoCamera = first ? target : autoCamera.approaching(target, 1 - exp(-dt * 3.5))
            }
        }
        lastFrameTime = now
        return (snapshot, camera)
    }

    /// The node id under `point` (view coordinates), with a few points of slack.
    func hitTest(_ point: CGPoint) -> String? {
        let snapshot = engine.buffer.read()
        let now = ProcessInfo.processInfo.systemUptime
        let cam = camera
        var best: (String, Double)?
        for (i, id) in snapshot.ids.enumerated() {
            let s = cam.toScreen(snapshot.position(i, at: now), size: viewSize)
            let d = hypot(s.x - point.x, s.y - point.y)
            let r = (radii[id] ?? 5) * (fixedNodeSize ? CanvasGeometry.nodeScale(cam.scale) : cam.scale) + 4
            if d <= r, d < (best?.1 ?? .infinity) { best = (id, d) }
        }
        return best?.0
    }

    func screenPosition(of id: String) -> CGPoint? {
        let snapshot = engine.buffer.read()
        guard let p = snapshot.position(of: id, at: ProcessInfo.processInfo.systemUptime) else { return nil }
        return camera.toScreen(p, size: viewSize)
    }

    // MARK: camera

    func zoom(by factor: Double, anchor: CGPoint? = nil) {
        var cam = camera
        cam.zoom(by: factor, anchor: anchor ?? CGPoint(x: viewSize.width / 2, y: viewSize.height / 2), size: viewSize)
        userCamera = cam
    }

    func pan(by delta: CGSize) {
        var cam = camera
        cam.pan(by: delta)
        userCamera = cam
    }

    /// Back to automatic framing.
    func fit() { userCamera = nil }

    // MARK: gestures

    func hover(_ phase: HoverPhase) {
        switch phase {
        case .active(let location):
            isHovering = true
            hoverLocation = location
            if draggingID == nil {
                let hit = hitTest(location)
                if hit != hoveredID { hoveredID = hit }
            }
        case .ended:
            isHovering = false
            hoverLocation = nil
            if draggingID == nil { hoveredID = nil }
        }
    }

    func dragChanged(start: CGPoint, location: CGPoint, translation: CGSize) {
        if panStartCamera == nil, draggingID == nil {
            if let id = hitTest(start), let p = engine.buffer.read().position(of: id, at: ProcessInfo.processInfo.systemUptime) {
                draggingID = id
                dragNodeStart = p
                dragStartWorld = camera.toWorld(start, size: viewSize)
            } else {
                panStartCamera = camera
            }
        }
        if let id = draggingID, let startWorld = dragStartWorld, let nodeStart = dragNodeStart {
            let w = camera.toWorld(location, size: viewSize)
            engine.controls.pin(id, at: nodeStart + (w - startWorld))
        } else if var cam = panStartCamera {
            cam.pan(by: translation)
            userCamera = cam
        }
    }

    func dragEnded() {
        if let id = draggingID { engine.controls.unpin(id) }
        draggingID = nil
        dragStartWorld = nil
        dragNodeStart = nil
        panStartCamera = nil
    }

    func magnifyChanged(_ magnification: Double, anchor: CGPoint) {
        if magnifyStartCamera == nil { magnifyStartCamera = camera }
        guard var cam = magnifyStartCamera else { return }
        cam.zoom(by: magnification, anchor: anchor, size: viewSize)
        userCamera = cam
    }

    func magnifyEnded() { magnifyStartCamera = nil }

    /// Scroll wheel / trackpad: precise (trackpad) deltas pan, a mouse wheel or ⌘-scroll zooms about the pointer.
    func scroll(dx: Double, dy: Double, precise: Bool, zoomModifier: Bool) {
        if zoomModifier || !precise {
            let factor = exp(dy * (precise ? 0.01 : 0.08))
            zoom(by: factor, anchor: hoverLocation)
        } else {
            pan(by: CGSize(width: dx, height: dy))
        }
    }

    /// Ties the engine lifetime and the settled flag to a view's `.task`; returns when the task is cancelled.
    func runEngine() async {
        await withDiscardingTaskGroup { group in
            group.addTask { [engine] in await engine.run() }
            while !Task.isCancelled {
                let settled = engine.buffer.read().settled
                if settled != layoutSettled { layoutSettled = settled }
                do { try await Task.sleep(for: .milliseconds(100)) } catch { break }
            }
            group.cancelAll()
        }
    }
}
