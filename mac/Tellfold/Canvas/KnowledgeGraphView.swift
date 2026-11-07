import SwiftUI

/// Entities and relations extracted from the seed documents, laid out with the same force engine as the network.
struct KnowledgeGraphView: View {
    let store: RunStore

    @State private var canvas = GraphCanvasController(
        parameters: ForceParameters(distanceMin: 4, collisionPadding: 4, gravity: 0.08),
        ambientDrift: 0.012, seed: 11)
    @State private var selectedEntityID: String?
    @AppStorage("canvas.graph.legend") private var showLegend = true
    @Environment(\.colorScheme) private var colorScheme
    @Environment(\.colorSchemeContrast) private var contrast
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        let input = KnowledgeGraphInput(store: store)
        let scene = KnowledgeGraphScene(input: input)
        let palette = CanvasPalette(scheme: colorScheme, increasedContrast: contrast == .increased)
        // A persona selected elsewhere highlights the entity it embodies.
        let agentEntity = input.selectedAgentID.flatMap { aid in input.embodiedBy.first { $0.value.contains(aid) }?.key }
        let selected = selectedEntityID ?? agentEntity
        let paused = reduceMotion && canvas.layoutSettled && canvas.draggingID == nil

        GeometryReader { geo in
            TimelineView(.animation(paused: paused)) { _ in
                let now = ProcessInfo.processInfo.systemUptime
                let (snapshot, camera) = canvas.frameState(now: now, size: geo.size, reduceMotion: reduceMotion)
                let hoveredRelation = canvas.hoveredID == nil
                    ? relation(near: canvas.hoverLocation, scene: scene, snapshot: snapshot, camera: camera, now: now, size: geo.size)
                    : nil
                let renderer = KnowledgeGraphRenderer(
                    scene: scene, snapshot: snapshot, camera: camera, now: now, palette: palette, accent: palette.accent,
                    focusID: canvas.hoveredID ?? selected, selectedID: selected, hoveredID: canvas.hoveredID,
                    hoveredRelation: hoveredRelation)
                ZStack(alignment: .topLeading) {
                    Canvas { ctx, size in renderer.draw(in: &ctx, size: size) }
                        .accessibilityHidden(true)
                    if canvas.draggingID == nil, let id = canvas.hoveredID, let i = scene.byID[id],
                       let p = snapshot.position(of: id, at: now) {
                        CanvasTooltip(anchor: camera.toScreen(p, size: geo.size),
                                      nodeRadius: scene.entities[i].radius * camera.scale, container: geo.size) {
                            EntityTooltipContent(entity: scene.entities[i], palette: palette, agentNames: input.agentNames)
                        }
                    }
                }
            }
            .graphCanvasInteraction(canvas, onTap: { point in
                let id = canvas.hitTest(point)
                selectedEntityID = id
                if let id, let agent = input.embodiedBy[id]?.first {
                    canvasSelectAgent(store, agent)
                }
            }, onEscape: {
                selectedEntityID = nil
            })
            .accessibilityElement(children: .contain)
            .accessibilityLabel("Knowledge graph")
            .accessibilityValue("\(scene.entities.count) entities, \(scene.relations.count) relations")
            .accessibilityChildren {
                KnowledgeGraphAccessibilityList(scene: scene, selectedID: selected) { id in
                    selectedEntityID = id
                    if let agent = input.embodiedBy[id]?.first { canvasSelectAgent(store, agent) }
                }
            }
        }
        .overlay(alignment: .bottomTrailing) {
            CanvasZoomControls(controller: canvas, hasUserCamera: canvas.userCamera != nil).padding(10)
        }
        .overlay(alignment: .bottomLeading) {
            KnowledgeGraphLegend(scene: scene, palette: palette, isExpanded: $showLegend).padding(10)
        }
        .clipped()
        .task { await canvas.runEngine() }
        .task(id: input.layoutKey) { await configure(scene) }
        .task(id: reduceMotion) {
            await canvas.engine.setDrift(enabled: !reduceMotion)
            if reduceMotion { await canvas.engine.settle() }
        }
    }

    private func configure(_ scene: KnowledgeGraphScene) async {
        for e in scene.entities { canvas.radii[e.id] = e.radius }
        let nodes = scene.layoutNodes
        let links = scene.layoutLinks
        let seed = SplitMix64.hash(scene.entities.map(\.id).joined(separator: ","))
        await canvas.engine.setDrift(enabled: !reduceMotion)
        await canvas.engine.configure(nodes: nodes, links: links, seed: seed, warmup: reduceMotion ? 0 : 80)
        if reduceMotion { await canvas.engine.settle() }
    }

    /// The relation whose line passes within a few points of the pointer.
    private func relation(near point: CGPoint?, scene: KnowledgeGraphScene, snapshot: LayoutSnapshot,
                          camera: CanvasCamera, now: Double, size: CGSize) -> Int? {
        guard let point else { return nil }
        var best: (Int, Double)?
        for (k, r) in scene.relations.enumerated() {
            guard let a = snapshot.position(of: r.source, at: now), let b = snapshot.position(of: r.target, at: now) else { continue }
            let d = CanvasGeometry.distance(from: point, toSegment: camera.toScreen(a, size: size), camera.toScreen(b, size: size))
            if d < 5, d < (best?.1 ?? .infinity) { best = (k, d) }
        }
        return best?.0
    }
}

struct KnowledgeGraphAccessibilityList: View {
    var scene: KnowledgeGraphScene
    var selectedID: String?
    var select: (String) -> Void

    var body: some View {
        let names = Dictionary(scene.entities.map { ($0.id, $0.name) }, uniquingKeysWith: { a, _ in a })
        let sorted = scene.entities.sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
        ForEach(sorted, id: \.id) { e in
            Button {
                select(e.id)
            } label: {
                Text("\(e.name), \(e.kind.label)")
            }
            .accessibilityValue(describe(e, names: names))
            .accessibilityAddTraits(e.id == selectedID ? .isSelected : [])
        }
    }

    private func describe(_ e: EntityVisual, names: [String: String]) -> String {
        let rels = scene.relations.filter { $0.source == e.id || $0.target == e.id }.prefix(8).map { r in
            r.source == e.id ? "\(r.label) \(names[r.target] ?? r.target)" : "\(names[r.source] ?? r.source) \(r.label) this"
        }
        var parts = [e.summary]
        if !rels.isEmpty { parts.append(rels.joined(separator: "; ")) }
        return parts.filter { !$0.isEmpty }.joined(separator: ". ")
    }
}
