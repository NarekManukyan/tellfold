import Foundation
import Synchronization

/// One node handed to the layout engine.
nonisolated struct LayoutNodeSpec: Sendable, Hashable {
    var id: String
    var radius: Double
    var charge: Double
}

/// One link handed to the layout engine (by node id).
nonisolated struct LayoutLinkSpec: Sendable, Hashable {
    var source: String
    var target: String
    var distance: Double
    var strength: Double?
}

/// The latest published layout. Holds the previous frame too, so the renderer can interpolate between engine ticks
/// and stay smooth on displays faster than the 60 Hz simulation.
nonisolated struct LayoutSnapshot: Sendable {
    var ids: [String] = []
    var index: [String: Int] = [:]
    var previous: [SIMD2<Double>] = []
    var current: [SIMD2<Double>] = []
    /// `ProcessInfo.systemUptime` when `current` was published.
    var time: Double = 0
    var interval: Double = 1.0 / 60.0
    var version: UInt64 = 0
    var alpha: Double = 0
    var settled = true

    static let empty = LayoutSnapshot()

    /// Interpolated position of node `i` at uptime `now` (one tick behind the engine, which keeps motion smooth).
    func position(_ i: Int, at now: Double) -> SIMD2<Double> {
        guard current.indices.contains(i) else { return .zero }
        guard previous.count == current.count, interval > 0 else { return current[i] }
        let t = min(1, max(0, (now - time) / interval))
        return previous[i] + (current[i] - previous[i]) * t
    }

    func position(of id: String, at now: Double) -> SIMD2<Double>? {
        guard let i = index[id] else { return nil }
        return position(i, at: now)
    }

    /// All interpolated positions at `now`.
    func positions(at now: Double) -> [SIMD2<Double>] {
        guard previous.count == current.count, interval > 0 else { return current }
        let t = min(1, max(0, (now - time) / interval))
        if t >= 1 { return current }
        var out = current
        for i in out.indices { out[i] = previous[i] + (current[i] - previous[i]) * t }
        return out
    }
}

/// Lock-protected mailbox between the engine (writer, off the main actor) and the renderer (reader, main actor).
/// Reads are a copy-on-write struct copy under the lock, so drawing never waits on a simulation step.
nonisolated final class LayoutBuffer: Sendable {
    private let state = Mutex(LayoutSnapshot.empty)

    func read() -> LayoutSnapshot { state.withLock { $0 } }
    func write(_ snapshot: LayoutSnapshot) { state.withLock { $0 = snapshot } }
}

/// Pins written by the main actor while the user drags a node, read by the engine before each tick.
/// A shared box instead of actor messages, so 60 Hz drag updates can never arrive out of order.
nonisolated final class LayoutControls: Sendable {
    private struct State {
        var pins: [String: SIMD2<Double>] = [:]
        var reheat: Double = 0
        var revision: UInt64 = 0
    }

    private let state = Mutex(State())

    func pin(_ id: String, at position: SIMD2<Double>) {
        state.withLock {
            $0.pins[id] = position
            $0.reheat = max($0.reheat, 0.12)
            $0.revision &+= 1
        }
    }

    func unpin(_ id: String) {
        state.withLock {
            $0.pins[id] = nil
            $0.reheat = max($0.reheat, 0.12)
            $0.revision &+= 1
        }
    }

    func requestReheat(_ alpha: Double) {
        state.withLock {
            $0.reheat = max($0.reheat, alpha)
            $0.revision &+= 1
        }
    }

    /// Returns pins and pending reheat if anything changed since `revision`, clearing the reheat request.
    func take(since revision: UInt64) -> (pins: [String: SIMD2<Double>], reheat: Double, revision: UInt64)? {
        state.withLock { s in
            guard s.revision != revision else { return nil }
            let out = (s.pins, s.reheat, s.revision)
            s.reheat = 0
            return out
        }
    }
}

/// Owns a `ForceSimulation` and steps it at ~60 Hz on its own actor, publishing positions to `buffer`.
/// The loop is structured: call `run()` from a view's `.task`, and cancelling the task stops it.
actor ForceLayoutEngine {
    nonisolated let buffer = LayoutBuffer()
    nonisolated let controls = LayoutControls()

    private var simulation: ForceSimulation
    private var ids: [String] = []
    private var index: [String: Int] = [:]
    private var ambientDrift: Double
    private var ambientAlpha: Double
    private var driftEnabled = true
    private var version: UInt64 = 0
    private let tickInterval: Double = 1.0 / 60.0
    private var controlsRevision: UInt64 = 0
    private var pinnedIDs: Set<String> = []
    private var layoutAspect: Double? = nil

    /// - Parameters:
    ///   - parameters: force parameters (drift is controlled separately through `setDrift`).
    ///   - ambientDrift: drift acceleration when drift is enabled.
    ///   - ambientAlpha: alpha floor while drifting, so links and repulsion keep the crowd's shape.
    init(parameters: ForceParameters = ForceParameters(), ambientDrift: Double = 0.035,
         ambientAlpha: Double = 0.012, seed: UInt64 = 7) {
        var p = parameters
        p.drift = 0
        simulation = ForceSimulation(parameters: p, seed: seed)
        self.ambientDrift = ambientDrift
        self.ambientAlpha = ambientAlpha
    }

    var alpha: Double { simulation.alpha }
    var nodeCount: Int { simulation.nodes.count }

    /// Replaces the graph. Nodes whose id was already present keep their position and velocity; new nodes start
    /// next to a linked, already-placed neighbour or on a phyllotaxis spiral. `warmup` ticks run immediately
    /// (use a few hundred under Reduce Motion so the first frame is already at rest).
    func configure(nodes: [LayoutNodeSpec], links: [LayoutLinkSpec], seed: UInt64, warmup: Int = 0,
                   reheat: Double = 1) {
        let oldIndex = index
        let oldNodes = simulation.nodes
        var rng = SplitMix64(seed: seed)
        var newIndex: [String: Int] = [:]
        for (i, n) in nodes.enumerated() where newIndex[n.id] == nil { newIndex[n.id] = i }
        let unique = nodes.enumerated().filter { newIndex[$0.element.id] == $0.offset }.map(\.element)
        newIndex = [:]
        for (i, n) in unique.enumerated() { newIndex[n.id] = i }

        // Neighbour lookup for placing new nodes.
        var neighbours: [String: [String]] = [:]
        for l in links {
            neighbours[l.source, default: []].append(l.target)
            neighbours[l.target, default: []].append(l.source)
        }
        let spacing = max(8, (unique.map(\.radius).max() ?? 5) * 2.2)
        var built: [ForceNode] = []
        built.reserveCapacity(unique.count)
        for (i, spec) in unique.enumerated() {
            if let old = oldIndex[spec.id], oldNodes.indices.contains(old) {
                var n = oldNodes[old]
                n.radius = spec.radius
                n.charge = spec.charge
                built.append(n)
                continue
            }
            var pos = ForceSimulation.phyllotaxis(index: i, spacing: spacing)
            if let anchor = neighbours[spec.id]?.first(where: { oldIndex[$0] != nil }), let k = oldIndex[anchor] {
                let a = rng.unit() * 2 * .pi
                pos = oldNodes[k].position + SIMD2(cos(a), sin(a)) * (spacing * 1.5)
            }
            built.append(ForceNode(position: pos, radius: spec.radius, charge: spec.charge,
                                   heading: rng.unit() * 2 * .pi))
        }
        simulation.nodes = built
        simulation.setLinks(links.compactMap { l in
            guard let s = newIndex[l.source], let t = newIndex[l.target] else { return nil }
            return ForceLink(source: s, target: t, distance: l.distance, strength: l.strength)
        })
        ids = unique.map(\.id)
        index = newIndex
        let fresh = oldNodes.isEmpty || unique.contains { oldIndex[$0.id] == nil }
        updateBox()
        simulation.reheat(fresh ? reheat : min(reheat, 0.3))
        applyDrift()
        if warmup > 0 { simulation.step(warmup) }
        publish(resetPrevious: true)
    }

    /// Turns the ambient drift on or off (off under Reduce Motion).
    func setDrift(enabled: Bool) {
        driftEnabled = enabled
        applyDrift()
    }

    func reheat(_ value: Double = 0.3) { simulation.reheat(value) }

    /// Shapes the layout for a panel of the given width / height ratio: nodes are confined to a rectangle of that
    /// aspect (sized from the node count), and repulsion spreads them until they fill it. Returns true when the
    /// layout was reheated.
    @discardableResult
    func setAspect(_ aspect: Double) -> Bool {
        let a = min(3.5, max(1, aspect.isFinite ? aspect : 1))
        guard abs(a - (layoutAspect ?? 0)) > 0.01 || simulation.parameters.box == nil else { return false }
        layoutAspect = a
        updateBox()
        simulation.reheat(0.6)
        return true
    }

    private func updateBox() {
        guard let a = layoutAspect else { return }
        let n = Double(max(1, simulation.nodes.count))
        let h = max(70, 30 * n.squareRoot())
        simulation.parameters.box = SIMD2(h * a, h)
    }

    /// Runs the simulation to rest right now (or up to `maxTicks`), then publishes.
    func settle(maxTicks: Int = 400) {
        var n = 0
        while n < maxTicks, simulation.alpha > max(simulation.parameters.alphaMin, simulation.alphaTarget * 1.5) {
            simulation.step()
            n += 1
        }
        publish(resetPrevious: true)
    }

    /// Advances `count` ticks without sleeping and publishes (tests, warm-up).
    func step(_ count: Int = 1) {
        simulation.step(count)
        publish(resetPrevious: false)
    }

    func positions() -> [String: SIMD2<Double>] {
        var out: [String: SIMD2<Double>] = [:]
        for (i, id) in ids.enumerated() { out[id] = simulation.nodes[i].position }
        return out
    }

    /// The stepping loop. Returns when the calling task is cancelled.
    func run() async {
        let clock = ContinuousClock()
        while !Task.isCancelled {
            applyControls()
            let active = !simulation.nodes.isEmpty && !simulation.isSettled
            if active {
                simulation.step()
                publish(resetPrevious: false)
            }
            do {
                try await clock.sleep(for: active ? .seconds(tickInterval) : .milliseconds(100))
            } catch {
                return
            }
        }
    }

    private func applyControls() {
        guard let update = controls.take(since: controlsRevision) else { return }
        controlsRevision = update.revision
        for id in pinnedIDs where update.pins[id] == nil {
            if let i = index[id] { simulation.nodes[i].pinned = nil }
        }
        for (id, p) in update.pins {
            guard let i = index[id] else { continue }
            simulation.nodes[i].pinned = p
            simulation.nodes[i].position = p
        }
        pinnedIDs = Set(update.pins.keys)
        if update.reheat > 0 { simulation.reheat(update.reheat) }
    }

    private func applyDrift() {
        var p = simulation.parameters
        p.drift = driftEnabled ? ambientDrift : 0
        simulation.parameters = p
        simulation.alphaTarget = driftEnabled ? ambientAlpha : 0
    }

    private func publish(resetPrevious: Bool) {
        let old = buffer.read()
        let positions = simulation.nodes.map(\.position)
        version &+= 1
        var snap = LayoutSnapshot()
        snap.ids = ids
        snap.index = index
        snap.current = positions
        snap.previous = (resetPrevious || old.current.count != positions.count || old.ids != ids) ? positions : old.current
        snap.time = ProcessInfo.processInfo.systemUptime
        snap.interval = tickInterval
        snap.version = version
        snap.alpha = simulation.alpha
        snap.settled = simulation.isSettled
        buffer.write(snap)
    }
}
