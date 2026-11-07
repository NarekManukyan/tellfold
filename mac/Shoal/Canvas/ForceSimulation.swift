import Foundation

/// SplitMix64: small, fast, deterministic. Every random choice in the canvas goes through a seeded instance
/// so layouts and effect timings are reproducible for a given run.
nonisolated struct SplitMix64: RandomNumberGenerator, Sendable {
    private var state: UInt64

    init(seed: UInt64) { state = seed }

    mutating func next() -> UInt64 {
        state &+= 0x9E37_79B9_7F4A_7C15
        var z = state
        z = (z ^ (z >> 30)) &* 0xBF58_476D_1CE4_E5B9
        z = (z ^ (z >> 27)) &* 0x94D0_49BB_1331_11EB
        return z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    mutating func unit() -> Double { Double(next() >> 11) / Double(UInt64(1) << 53) }

    /// Stable 64-bit FNV-1a hash of a string, for seeding per-id choices.
    static func hash(_ s: String) -> UInt64 {
        var h: UInt64 = 0xCBF2_9CE4_8422_2325
        for b in s.utf8 { h = (h ^ UInt64(b)) &* 0x0000_0100_0000_01B3 }
        return h
    }
}

nonisolated struct ForceNode: Sendable, Equatable {
    var position: SIMD2<Double>
    var velocity: SIMD2<Double> = .zero
    /// Collision radius (drawn radius plus nothing; padding is a parameter).
    var radius: Double = 5
    /// Many-body strength; negative repels.
    var charge: Double = -30
    /// When set, the node is held at this position.
    var pinned: SIMD2<Double>?
    /// Heading for the ambient drift, radians.
    var heading: Double = 0
}

nonisolated struct ForceLink: Sendable, Hashable {
    var source: Int
    var target: Int
    var distance: Double = 30
    /// nil uses the default `1 / min(degree(source), degree(target))`.
    var strength: Double?
}

nonisolated struct ForceParameters: Sendable, Equatable {
    var alphaMin = 0.001
    var alphaDecay = 1 - pow(0.001, 1.0 / 300.0)
    var velocityDecay = 0.4
    /// Barnes-Hut opening criterion.
    var theta = 0.9
    var distanceMin = 1.0
    var distanceMax = Double.infinity
    var collisionPadding = 1.5
    var collisionStrength = 0.7
    /// Weak pull of every node toward `center`, scaled by alpha (keeps disconnected nodes in view).
    var gravity = 0.05
    /// Half extents of a rectangle the nodes are kept inside (a soft bounds force), or nil for no confinement.
    /// Repulsion spreads the layout until it fills the rectangle, so a wide panel is filled by the layout itself.
    var box: SIMD2<Double>?
    var center = SIMD2<Double>.zero
    /// Shift the mean position to `center` after each step.
    var recenter = true
    /// Ambient drift acceleration per tick (0 disables). Applied independently of alpha.
    var drift = 0.0
    /// Standard deviation of the per-tick heading change, radians.
    var driftTurn = 0.22
    /// How strongly linked nodes align their headings (schooling), 0...1 per tick.
    var driftAlignment = 0.04
}

/// A d3-style velocity-Verlet force simulation: many-body (Barnes-Hut), links, collision, gravity, centring and
/// a low ambient "shoal" drift. A plain value type: it does no scheduling and is stepped by its owner
/// (`ForceLayoutEngine`) off the main actor, or directly by tests.
nonisolated struct ForceSimulation: Sendable {
    var nodes: [ForceNode] = []
    private(set) var links: [ForceLink] = []
    var parameters: ForceParameters
    private(set) var alpha: Double = 1
    var alphaTarget: Double = 0
    private(set) var tickCount = 0

    private var rng: SplitMix64
    private var degree: [Int] = []
    private var linkStrength: [Double] = []
    private var linkBias: [Double] = []
    private var stack: [Int32] = []

    init(nodes: [ForceNode] = [], links: [ForceLink] = [], parameters: ForceParameters = ForceParameters(),
         seed: UInt64 = 7) {
        self.parameters = parameters
        self.rng = SplitMix64(seed: seed)
        self.nodes = nodes
        setLinks(links)
    }

    /// True once the simulation has cooled and has no ambient motion to keep it alive.
    var isSettled: Bool { alpha < parameters.alphaMin && alphaTarget < parameters.alphaMin && parameters.drift == 0 }

    mutating func setLinks(_ newLinks: [ForceLink]) {
        links = newLinks.filter { $0.source != $0.target && nodes.indices.contains($0.source) && nodes.indices.contains($0.target) }
        degree = Array(repeating: 0, count: nodes.count)
        for l in links {
            degree[l.source] += 1
            degree[l.target] += 1
        }
        linkStrength = links.map { l in l.strength ?? 1 / Double(max(1, min(degree[l.source], degree[l.target]))) }
        linkBias = links.map { l in
            let s = Double(degree[l.source]), t = Double(degree[l.target])
            return s + t > 0 ? s / (s + t) : 0.5
        }
    }

    mutating func reheat(_ value: Double = 1) { alpha = max(alpha, min(1, value)) }
    mutating func setAlpha(_ value: Double) { alpha = max(0, min(1, value)) }

    /// Initial positions on a phyllotaxis spiral (even density, no overlaps, deterministic).
    static func phyllotaxis(index: Int, spacing: Double = 10, center: SIMD2<Double> = .zero) -> SIMD2<Double> {
        let r = spacing * (0.5 + Double(index)).squareRoot()
        let a = Double(index) * Double.pi * (3 - 5.0.squareRoot())
        return center + SIMD2(r * cos(a), r * sin(a))
    }

    mutating func step(_ count: Int = 1) {
        for _ in 0..<max(0, count) { tick() }
    }

    private mutating func jiggle() -> Double { (rng.unit() - 0.5) * 1e-6 }

    private mutating func tick() {
        guard !nodes.isEmpty else { return }
        tickCount += 1
        alpha += (alphaTarget - alpha) * parameters.alphaDecay
        let a = alpha
        if a > 1e-9 {
            applyLinks(alpha: a)
            applyManyBody(alpha: a)
            applyGravity(alpha: a)
            applyCollision()
        }
        applyBox()
        if parameters.drift > 0 { applyDrift() }
        integrate()
    }

    private mutating func applyLinks(alpha: Double) {
        for (k, l) in links.enumerated() {
            let s = nodes[l.source], t = nodes[l.target]
            var d = (t.position + t.velocity) - (s.position + s.velocity)
            if d.x == 0 { d.x = jiggle() }
            if d.y == 0 { d.y = jiggle() }
            let len = (d.x * d.x + d.y * d.y).squareRoot()
            let f = (len - l.distance) / len * alpha * linkStrength[k]
            d *= f
            let b = linkBias[k]
            nodes[l.target].velocity -= d * b
            nodes[l.source].velocity += d * (1 - b)
        }
    }

    private mutating func applyManyBody(alpha: Double) {
        let tree = QuadTree(positions: nodes.map(\.position), weights: nodes.map(\.charge))
        var stack = self.stack
        var rng = self.rng
        for i in nodes.indices {
            let f = tree.manyBody(on: i, theta: parameters.theta, distanceMin: parameters.distanceMin,
                                  distanceMax: parameters.distanceMax, stack: &stack,
                                  jiggle: { (rng.unit() - 0.5) * 1e-6 })
            nodes[i].velocity += f * alpha
        }
        self.stack = stack
        self.rng = rng
    }

    private mutating func applyGravity(alpha: Double) {
        let g = parameters.gravity * alpha
        guard g > 0 else { return }
        for i in nodes.indices {
            nodes[i].velocity += (parameters.center - nodes[i].position) * g
        }
    }

    /// Pushes nodes back toward the rectangle `parameters.box` when they cross it.
    private mutating func applyBox() {
        guard let box = parameters.box else { return }
        for i in nodes.indices where nodes[i].pinned == nil {
            let limit = SIMD2(max(4, box.x - nodes[i].radius - 2), max(4, box.y - nodes[i].radius - 2))
            let p = nodes[i].position
            if p.x > limit.x { nodes[i].velocity.x -= (p.x - limit.x) * 0.35 }
            if p.x < -limit.x { nodes[i].velocity.x -= (p.x + limit.x) * 0.35 }
            if p.y > limit.y { nodes[i].velocity.y -= (p.y - limit.y) * 0.35 }
            if p.y < -limit.y { nodes[i].velocity.y -= (p.y + limit.y) * 0.35 }
        }
    }

    private mutating func applyCollision() {
        let pad = parameters.collisionPadding
        let strength = parameters.collisionStrength
        guard strength > 0 else { return }
        let predicted = nodes.map { $0.position + $0.velocity }
        let radii = nodes.map { $0.radius + pad }
        let tree = QuadTree(positions: predicted, radii: radii)
        let maxR = tree.root?.maxRadius ?? 0
        var rng = self.rng
        var hits: [Int] = []
        hits.reserveCapacity(16)
        for i in nodes.indices {
            let ri = radii[i]
            let pi = predicted[i]
            hits.removeAll(keepingCapacity: true)
            tree.forEachPoint(near: pi, within: ri + maxR) { j in if j > i { hits.append(j) } }
            for j in hits {
                let rj = radii[j]
                var d = pi - predicted[j]
                var l2 = d.x * d.x + d.y * d.y
                let r = ri + rj
                guard l2 < r * r else { continue }
                if d.x == 0 { d.x = (rng.unit() - 0.5) * 1e-6; l2 += d.x * d.x }
                if d.y == 0 { d.y = (rng.unit() - 0.5) * 1e-6; l2 += d.y * d.y }
                let l = l2.squareRoot()
                d *= (r - l) / l * strength
                let wi = (rj * rj) / (ri * ri + rj * rj)
                nodes[i].velocity += d * wi
                nodes[j].velocity -= d * (1 - wi)
            }
        }
        self.rng = rng
    }

    private mutating func applyDrift() {
        let turn = parameters.driftTurn
        // Schooling: linked nodes nudge their headings toward each other.
        if parameters.driftAlignment > 0 {
            let k = parameters.driftAlignment
            for l in links {
                let hs = nodes[l.source].heading, ht = nodes[l.target].heading
                let diff = atan2(sin(ht - hs), cos(ht - hs))
                nodes[l.source].heading += diff * k
                nodes[l.target].heading -= diff * k
            }
        }
        for i in nodes.indices {
            // Box-Muller-free approximation: sum of two uniforms is triangular, close enough for a heading walk.
            let n = (rng.unit() + rng.unit() - 1) * turn * 1.7
            nodes[i].heading = (nodes[i].heading + n).truncatingRemainder(dividingBy: 2 * .pi)
            let h = nodes[i].heading
            nodes[i].velocity += SIMD2(cos(h), sin(h)) * parameters.drift
        }
    }

    private mutating func integrate() {
        let keep = 1 - parameters.velocityDecay
        var anyPinned = false
        for i in nodes.indices {
            if let pin = nodes[i].pinned {
                nodes[i].position = pin
                nodes[i].velocity = .zero
                anyPinned = true
            } else {
                nodes[i].velocity *= keep
                if !nodes[i].velocity.x.isFinite || !nodes[i].velocity.y.isFinite { nodes[i].velocity = .zero }
                nodes[i].position += nodes[i].velocity
            }
        }
        guard parameters.recenter, !anyPinned else { return }
        var mean = SIMD2<Double>.zero
        for n in nodes { mean += n.position }
        mean /= Double(nodes.count)
        let shift = parameters.center - mean
        for i in nodes.indices { nodes[i].position += shift }
    }
}
