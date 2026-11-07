import Foundation
import Testing
@testable import Shoal

// Unit tests for the canvas engine: Barnes-Hut quadtree, force simulation stepping, the layout engine (knowledge
// graph), camera maths, the sentiment scale, the influence ramp and the crowd graph's analytic layouts.

private func randomPoints(_ n: Int, seed: UInt64, spread: Double = 500) -> [SIMD2<Double>] {
    var rng = SplitMix64(seed: seed)
    return (0..<n).map { _ in SIMD2(rng.unit() * spread - spread / 2, rng.unit() * spread - spread / 2) }
}

private func bruteForce(on i: Int, _ points: [SIMD2<Double>], _ weights: [Double], distanceMin: Double = 1) -> SIMD2<Double> {
    var f = SIMD2<Double>.zero
    for j in points.indices where j != i {
        let d = points[j] - points[i]
        var l = d.x * d.x + d.y * d.y
        if l < distanceMin * distanceMin { l = (distanceMin * distanceMin * l).squareRoot() }
        f += d * weights[j] / l
    }
    return f
}

private func length(_ v: SIMD2<Double>) -> Double { (v.x * v.x + v.y * v.y).squareRoot() }

@Suite("QuadTree")
struct QuadTreeTests {
    @Test func emptyTreeHasNoCells() {
        let tree = QuadTree(positions: [])
        #expect(tree.isEmpty)
        var visited = 0
        tree.forEachPoint(near: .zero, within: 100) { _ in visited += 1 }
        #expect(visited == 0)
    }

    @Test func aggregatesCountMassAndCentreOfMass() throws {
        let points = randomPoints(300, seed: 1)
        let weights = points.indices.map { -1 - Double($0 % 5) }
        let tree = QuadTree(positions: points, weights: weights)
        let root = try #require(tree.root)
        #expect(root.count == 300)
        #expect(abs(root.mass - weights.reduce(0, +)) < 1e-9)
        let total = weights.map(abs).reduce(0, +)
        let com = zip(points, weights).reduce(SIMD2<Double>.zero) { $0 + $1.0 * abs($1.1) } / total
        #expect(abs(root.centerX - com.x) < 1e-6)
        #expect(abs(root.centerY - com.y) < 1e-6)
    }

    @Test func everyPointLiesInsideItsLeaf() {
        let points = randomPoints(500, seed: 2)
        let tree = QuadTree(positions: points)
        var seen = 0
        for cell in tree.cells where cell.isLeaf {
            var j = cell.head
            while j >= 0 {
                let p = points[Int(j)]
                #expect(p.x >= cell.minX && p.x <= cell.minX + cell.size)
                #expect(p.y >= cell.minY && p.y <= cell.minY + cell.size)
                seen += 1
                j = tree.next[Int(j)]
            }
        }
        #expect(seen == 500)
    }

    @Test func coincidentPointsShareALeaf() {
        var points = Array(repeating: SIMD2<Double>(3, 4), count: 6)
        points.append(SIMD2(10, -2))
        let tree = QuadTree(positions: points)
        #expect(tree.root?.count == 7)
        var stack: [Int32] = []
        // Forces between coincident points stay finite (jiggled).
        let f = tree.manyBody(on: 0, theta: 0.9, stack: &stack, jiggle: { 1e-7 })
        #expect(f.x.isFinite && f.y.isFinite)
    }

    @Test func skipsNonFinitePoints() {
        let points: [SIMD2<Double>] = [SIMD2(0, 0), SIMD2(.nan, 1), SIMD2(5, 5), SIMD2(.infinity, 0)]
        let tree = QuadTree(positions: points)
        #expect(tree.root?.count == 2)
    }

    @Test func rangeQueryMatchesBruteForce() {
        let points = randomPoints(400, seed: 3)
        let tree = QuadTree(positions: points)
        var rng = SplitMix64(seed: 99)
        for _ in 0..<40 {
            let c = SIMD2(rng.unit() * 500 - 250, rng.unit() * 500 - 250)
            let r = rng.unit() * 80 + 5
            var found = Set<Int>()
            tree.forEachPoint(near: c, within: r) { found.insert($0) }
            let expected = Set(points.indices.filter { length(points[$0] - c) <= r })
            #expect(found == expected)
        }
    }

    @Test func barnesHutWithThetaZeroIsExact() {
        let points = randomPoints(120, seed: 4)
        let weights = Array(repeating: -30.0, count: points.count)
        let tree = QuadTree(positions: points, weights: weights)
        var stack: [Int32] = []
        for i in stride(from: 0, to: points.count, by: 7) {
            let approx = tree.manyBody(on: i, theta: 0, stack: &stack, jiggle: { 0 })
            let exact = bruteForce(on: i, points, weights)
            #expect(length(approx - exact) < 1e-9 * max(1, length(exact)))
        }
    }

    @Test func barnesHutApproximationIsClose() {
        let points = randomPoints(400, seed: 5)
        let weights = Array(repeating: -30.0, count: points.count)
        let tree = QuadTree(positions: points, weights: weights)
        var stack: [Int32] = []
        var totalError = 0.0, totalMagnitude = 0.0
        for i in points.indices {
            let approx = tree.manyBody(on: i, theta: 0.9, stack: &stack, jiggle: { 0 })
            let exact = bruteForce(on: i, points, weights)
            totalError += length(approx - exact)
            totalMagnitude += length(exact)
        }
        #expect(totalError / totalMagnitude < 0.08)
    }
}

@Suite("ForceSimulation")
struct ForceSimulationTests {
    private func grid(_ n: Int) -> [ForceNode] {
        (0..<n).map { ForceNode(position: ForceSimulation.phyllotaxis(index: $0)) }
    }

    @Test func alphaCoolsMonotonicallyAndSettles() {
        var sim = ForceSimulation(nodes: grid(30))
        var last = sim.alpha
        for _ in 0..<320 {
            sim.step()
            #expect(sim.alpha < last)
            last = sim.alpha
        }
        #expect(sim.isSettled)
        #expect(sim.tickCount == 320)
    }

    @Test func steppingIsDeterministicForASeed() {
        let links = (1..<40).map { ForceLink(source: $0, target: ($0 * 7) % $0) }
        var a = ForceSimulation(nodes: grid(40), links: links, seed: 42)
        var b = ForceSimulation(nodes: grid(40), links: links, seed: 42)
        a.step(150)
        b.step(150)
        #expect(a.nodes == b.nodes)
    }

    @Test func linkConvergesToItsDistance() {
        var p = ForceParameters()
        p.gravity = 0
        p.collisionStrength = 0
        let nodes = [ForceNode(position: SIMD2(-5, 0), charge: 0), ForceNode(position: SIMD2(5, 1), charge: 0)]
        var sim = ForceSimulation(nodes: nodes, links: [ForceLink(source: 0, target: 1, distance: 60)], parameters: p)
        sim.step(300)
        let d = length(sim.nodes[0].position - sim.nodes[1].position)
        #expect(abs(d - 60) < 1.5)
    }

    @Test func chargeRepels() {
        var p = ForceParameters()
        p.gravity = 0
        p.collisionStrength = 0
        let nodes = [ForceNode(position: SIMD2(-1, 0)), ForceNode(position: SIMD2(1, 0))]
        var sim = ForceSimulation(nodes: nodes, parameters: p)
        sim.step(60)
        #expect(length(sim.nodes[0].position - sim.nodes[1].position) > 20)
    }

    @Test func collisionSeparatesOverlappingNodes() {
        var p = ForceParameters()
        p.gravity = 0.02
        let nodes = (0..<12).map { i in
            ForceNode(position: SIMD2(Double(i) * 0.01, 0), radius: 8, charge: 0)
        }
        var sim = ForceSimulation(nodes: nodes, parameters: p)
        sim.step(300)
        var minGap = Double.infinity
        for i in sim.nodes.indices {
            for j in sim.nodes.indices where j > i {
                minGap = min(minGap, length(sim.nodes[i].position - sim.nodes[j].position))
            }
        }
        #expect(minGap > 2 * 8 * 0.85)
    }

    @Test func recentresOnCentre() {
        var p = ForceParameters()
        p.center = SIMD2(100, -50)
        let nodes = (0..<20).map { ForceNode(position: ForceSimulation.phyllotaxis(index: $0) + SIMD2(400, 400)) }
        var sim = ForceSimulation(nodes: nodes, parameters: p)
        sim.step()
        let mean = sim.nodes.reduce(SIMD2<Double>.zero) { $0 + $1.position } / Double(sim.nodes.count)
        #expect(length(mean - p.center) < 1e-6)
    }

    @Test func pinnedNodeStaysPut() {
        var nodes = grid(10)
        nodes[3].pinned = SIMD2(77, 33)
        var sim = ForceSimulation(nodes: nodes, links: [ForceLink(source: 3, target: 4)])
        sim.step(50)
        #expect(sim.nodes[3].position == SIMD2(77, 33))
        #expect(sim.nodes[3].velocity == .zero)
    }

    @Test func coincidentStartStaysFinite() {
        let nodes = Array(repeating: ForceNode(position: .zero), count: 25)
        var sim = ForceSimulation(nodes: nodes, links: [ForceLink(source: 0, target: 1)])
        sim.step(100)
        #expect(sim.nodes.allSatisfy { $0.position.x.isFinite && $0.position.y.isFinite })
        let spread = sim.nodes.map { length($0.position) }.max() ?? 0
        #expect(spread > 5)
    }

    @Test func ambientDriftKeepsMovingButStaysBounded() {
        var p = ForceParameters()
        p.drift = 0.03
        let links = (1..<30).map { ForceLink(source: $0, target: $0 / 2) }
        var sim = ForceSimulation(nodes: grid(30), links: links, parameters: p)
        sim.alphaTarget = 0.012
        sim.step(400)
        #expect(!sim.isSettled)
        let before = sim.nodes.map(\.position)
        sim.step(60)
        let moved = zip(before, sim.nodes.map(\.position)).map { length($0 - $1) }.max() ?? 0
        #expect(moved > 0.05)
        sim.step(1500)
        let extent = sim.nodes.map { length($0.position) }.max() ?? 0
        #expect(extent < 400)
    }

    @Test func linksToMissingNodesAreDropped() {
        let sim = ForceSimulation(nodes: grid(3), links: [ForceLink(source: 0, target: 9), ForceLink(source: 1, target: 1),
                                                        ForceLink(source: 0, target: 2)])
        #expect(sim.links.count == 1)
    }

    @Test func twoHundredAgentsStepQuickly() {
        var rng = SplitMix64(seed: 5)
        let links = (0..<600).map { _ in ForceLink(source: Int(rng.next() % 200), target: Int(rng.next() % 200)) }
        var sim = ForceSimulation(nodes: grid(200), links: links)
        let clock = ContinuousClock()
        let elapsed = clock.measure { sim.step(120) }
        #expect(sim.nodes.allSatisfy { $0.position.x.isFinite })
        // Generous bound for unoptimised Debug builds; Release is well under a millisecond per tick.
        #expect(elapsed < .seconds(6))
    }
}

@Suite("ForceLayoutEngine")
struct ForceLayoutEngineTests {
    @Test func configurePublishesASnapshot() async {
        let engine = ForceLayoutEngine()
        let nodes = (0..<12).map { LayoutNodeSpec(id: "a\($0)", radius: 5, charge: -30) }
        let links = (1..<12).map { LayoutLinkSpec(source: "a\($0)", target: "a0", distance: 30) }
        await engine.configure(nodes: nodes, links: links, seed: 1, warmup: 30)
        let snap = engine.buffer.read()
        #expect(snap.ids == nodes.map(\.id))
        #expect(snap.current.count == 12)
        #expect(snap.current.allSatisfy { $0.x.isFinite && $0.y.isFinite })
        #expect(snap.index["a5"] == 5)
    }

    @Test func reconfigureKeepsExistingPositions() async {
        let engine = ForceLayoutEngine()
        let nodes = (0..<8).map { LayoutNodeSpec(id: "n\($0)", radius: 5, charge: -30) }
        await engine.configure(nodes: nodes, links: [], seed: 1, warmup: 40)
        let before = await engine.positions()
        await engine.configure(nodes: nodes + [LayoutNodeSpec(id: "new", radius: 5, charge: -30)],
                               links: [LayoutLinkSpec(source: "new", target: "n0", distance: 30)], seed: 1)
        let after = await engine.positions()
        for id in before.keys { #expect(after[id] == before[id]) }
        #expect(after["new"] != nil)
    }

    @Test func settleRunsToRestWithoutDrift() async {
        let engine = ForceLayoutEngine()
        await engine.setDrift(enabled: false)
        let nodes = (0..<20).map { LayoutNodeSpec(id: "n\($0)", radius: 5, charge: -30) }
        await engine.configure(nodes: nodes, links: [], seed: 3)
        await engine.settle(maxTicks: 1000)
        #expect(engine.buffer.read().settled)
    }

    @Test func runLoopAppliesPinsAndStopsOnCancel() async throws {
        let engine = ForceLayoutEngine()
        let nodes = (0..<6).map { LayoutNodeSpec(id: "n\($0)", radius: 5, charge: -30) }
        await engine.configure(nodes: nodes, links: [], seed: 3)
        let task = Task { await engine.run() }
        engine.controls.pin("n2", at: SIMD2(123, 45))
        try await Task.sleep(for: .milliseconds(150))
        let p = await engine.positions()["n2"]
        #expect(p == SIMD2(123, 45))
        engine.controls.unpin("n2")
        task.cancel()
        await task.value
    }

    @Test func snapshotInterpolatesBetweenTicks() {
        var snap = LayoutSnapshot()
        snap.ids = ["a"]
        snap.index = ["a": 0]
        snap.previous = [SIMD2(0, 0)]
        snap.current = [SIMD2(10, 0)]
        snap.time = 100
        snap.interval = 0.1
        #expect(snap.position(0, at: 100) == SIMD2(0, 0))
        #expect(abs(snap.position(0, at: 100.05).x - 5) < 1e-9)
        #expect(snap.position(0, at: 101) == SIMD2(10, 0))
    }
}

@Suite("Canvas camera and colour")
struct CanvasCameraTests {
    @Test func screenAndWorldRoundTrip() {
        let size = CGSize(width: 800, height: 600)
        let cam = CanvasCamera(scale: 1.7, offset: SIMD2(30, -12))
        let p = SIMD2<Double>(42, -17)
        let back = cam.toWorld(cam.toScreen(p, size: size), size: size)
        #expect(length(back - p) < 1e-9)
    }

    @Test func zoomKeepsAnchorFixed() {
        let size = CGSize(width: 800, height: 600)
        var cam = CanvasCamera(scale: 1, offset: SIMD2(10, 20))
        let anchor = CGPoint(x: 600, y: 140)
        let w = cam.toWorld(anchor, size: size)
        cam.zoom(by: 2.5, anchor: anchor, size: size)
        let s = cam.toScreen(w, size: size)
        #expect(abs(s.x - anchor.x) < 1e-9 && abs(s.y - anchor.y) < 1e-9)
        cam.zoom(by: 1000, anchor: anchor, size: size)
        #expect(cam.scale == CanvasCamera.maxScale)
    }

    @Test func fittingFramesTheBounds() {
        let size = CGSize(width: 400, height: 300)
        let cam = CanvasCamera.fitting(min: SIMD2(-100, -50), max: SIMD2(300, 150), size: size, padding: 20)
        for corner in [SIMD2<Double>(-100, -50), SIMD2(300, 150), SIMD2(-100, 150), SIMD2(300, -50)] {
            let s = cam.toScreen(corner, size: size)
            #expect(s.x >= 19.99 && s.x <= 380.01 && s.y >= 19.99 && s.y <= 280.01)
        }
    }

    @Test func sentimentScaleHitsItsAnchors() {
        for scale in [SentimentScale.dark, .light] {
            let close = { (a: RGB, b: RGB) in abs(a.r - b.r) < 0.01 && abs(a.g - b.g) < 0.01 && abs(a.b - b.b) < 0.01 }
            #expect(close(scale.rgb(-1), scale.negative))
            #expect(close(scale.rgb(0), scale.neutral))
            #expect(close(scale.rgb(1), scale.positive))
            #expect(close(scale.rgb(nil), scale.neutral))
            #expect(close(scale.rgb(-7), scale.negative))
            #expect(close(scale.rgb(.nan), scale.neutral))
        }
        // OKLab round trip.
        let c = RGB(hex: 0x3FC7B5)
        let back = RGB(oklab: c.oklab)
        #expect(abs(back.r - c.r) < 1e-6 && abs(back.g - c.g) < 1e-6 && abs(back.b - c.b) < 1e-6)
    }

    @Test func arcKindsParseActionTypes() {
        #expect(ArcKind(actionType: "reply") == .reply)
        #expect(ArcKind(actionType: "FOLLOW") == .follow)
        #expect(ArcKind(actionType: "post") == nil)
        #expect(EntityKind(raw: "Organization") == .organization)
        #expect(EntityKind(raw: "spaceship") == .other)
    }
}

@Suite("Influence ramp and crowd layout")
struct CrowdLayoutTests {
    @Test func rampGivesTheFixedAvatarDiameters() {
        #expect([0.0, 0.19, 0.2, 0.39, 0.4, 0.59, 0.6, 0.79, 0.8, 1.0].map { InfluenceRamp.radius($0) * 2 }
                == [22, 22, 26, 26, 30, 30, 36, 36, 44, 44])
        #expect(InfluenceRamp.step(0.95) == 4)
        #expect(InfluenceRamp.step(.nan) == InfluenceRamp.step(0.5))
    }

    private func nodes(_ n: Int, groups: Int) -> [CrowdNode] {
        (0..<n).map { i in
            CrowdNode(id: "a\(i)", handle: "h\(i)", name: "Agent \(i)", influence: Double((i * 37) % 100) / 100,
                      isOrganisation: i % 7 == 0, groupIndex: i % groups)
        }
    }

    private func overlaps(_ layout: CrowdLayout, _ nodes: [CrowdNode]) -> Int {
        var count = 0
        for (i, a) in nodes.enumerated() {
            for b in nodes[(i + 1)...] {
                let p = layout.positions[a.id]!, q = layout.positions[b.id]!
                if hypot(p.x - q.x, p.y - q.y) < a.radius + b.radius - 1 { count += 1 }
            }
        }
        return count
    }

    @Test(arguments: CrowdMode.allCases)
    func everyViewPlacesEveryoneWithoutOverlap(mode: CrowdMode) {
        let agents = nodes(40, groups: 5)
        let sentiment = Dictionary(uniqueKeysWithValues: agents.enumerated().map { ($1.id, Double($0 % 21) / 10 - 1) })
        let layout = CrowdLayout.compute(nodes: agents, groupCount: 5, sentiment: sentiment,
                                         size: CGSize(width: 900, height: 600), mode: mode)
        #expect(layout.positions.count == agents.count)
        #expect(overlaps(layout, agents) == 0)
        #expect(layout.fitScale > 0 && layout.fitScale <= 1.5)
        for p in layout.positions.values { #expect(p.x.isFinite && p.y.isFinite) }
    }

    @Test func layoutsAreDeterministic() {
        let agents = nodes(25, groups: 3)
        let a = CrowdLayout.compute(nodes: agents, groupCount: 3, sentiment: [:], size: CGSize(width: 700, height: 500), mode: .camps)
        let b = CrowdLayout.compute(nodes: agents, groupCount: 3, sentiment: [:], size: CGSize(width: 700, height: 500), mode: .camps)
        #expect(a == b)
    }

    @Test func shoalPutsAgentsAtTheirSentimentAndUnscoredInALaneBelow() {
        let agents = nodes(5, groups: 2)
        let layout = CrowdLayout.compute(nodes: agents, groupCount: 2, sentiment: ["a0": -1, "a1": 1, "a2": 0.5],
                                         size: CGSize(width: 800, height: 500), mode: .shoal)
        let half = layout.shoalHalf
        #expect(half > 0)
        #expect(abs(layout.positions["a0"]!.x + half) < 0.001)
        #expect(abs(layout.positions["a1"]!.x - half) < 0.001)
        #expect(abs(layout.positions["a2"]!.x - half / 2) < 0.001)
        #expect(layout.positions["a3"]!.y > layout.shoalAxisY && layout.positions["a4"]!.y > layout.shoalAxisY)
    }

    @Test func influencePutsBigVoicesNearTheCentre() {
        var agents = nodes(12, groups: 3)
        agents[0].influence = 0.95
        agents[1].influence = 0.05
        let layout = CrowdLayout.compute(nodes: agents, groupCount: 3, sentiment: [:], size: CGSize(width: 800, height: 600),
                                         mode: .influence)
        let big = layout.positions["a0"]!, small = layout.positions["a1"]!
        #expect(hypot(big.x, big.y) < hypot(small.x, small.y))
        #expect(layout.ringRadii.count == 3 && layout.ringRadii[0] < layout.ringRadii[1] && layout.ringRadii[1] < layout.ringRadii[2])  // 0.75, 0.5, 0.25 outward
    }

    @Test func campClustersAreSeparatedFromEachOther() {
        let agents = nodes(30, groups: 3)
        let layout = CrowdLayout.compute(nodes: agents, groupCount: 3, sentiment: [:], size: CGSize(width: 900, height: 600),
                                         mode: .camps)
        func centre(_ g: Int) -> CGPoint {
            let pts = agents.filter { $0.groupIndex == g }.map { layout.positions[$0.id]! }
            return CGPoint(x: pts.map(\.x).reduce(0, +) / Double(pts.count), y: pts.map(\.y).reduce(0, +) / Double(pts.count))
        }
        for (a, b) in [(0, 1), (0, 2), (1, 2)] {
            #expect(hypot(centre(a).x - centre(b).x, centre(a).y - centre(b).y) > 60)
        }
    }

    @Test func emptyCrowdLaysOutNothing() {
        let layout = CrowdLayout.compute(nodes: [], groupCount: 0, sentiment: [:], size: CGSize(width: 400, height: 300), mode: .camps)
        #expect(layout.positions.isEmpty)
    }

    @Test func knowledgeGraphSceneBuildsLayoutSpecs() {
        var input = KnowledgeGraphInput()
        input.entities = [CanvasEntity(id: "hub", name: "Hub", kind: "product", summary: ""),
                          CanvasEntity(id: "x", name: "X", kind: "Person", summary: ""),
                          CanvasEntity(id: "y", name: "Y", kind: "weird", summary: "")]
        input.relations = [CanvasRelation(source: "hub", target: "x", label: "uses"),
                           CanvasRelation(source: "x", target: "hub", label: "likes"),
                           CanvasRelation(source: "hub", target: "missing", label: "dropped")]
        input.embodiedBy = ["x": ["a01"]]
        let scene = KnowledgeGraphScene(input: input)
        #expect(scene.relations.count == 2)
        #expect(scene.layoutLinks.count == 1)  // one link per connected pair
        #expect(scene.layoutNodes.count == 3)
        #expect(scene.entities[scene.byID["y"]!].kind == .other)
        #expect(scene.entities[scene.byID["x"]!].embodiedBy == ["a01"])
        #expect(scene.kindCounts.map(\.kind) == [.person, .product, .other])
    }
}
