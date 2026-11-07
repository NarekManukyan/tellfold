import SwiftUI

/// Static per-entity drawing attributes.
nonisolated struct EntityVisual: Sendable, Equatable {
    var id: String
    var name: String
    var kind: EntityKind
    var summary: String
    var radius: Double
    var degree: Int
    var embodiedBy: [String]
}

nonisolated struct KnowledgeGraphScene: Sendable {
    var entities: [EntityVisual] = []
    var byID: [String: Int] = [:]
    var relations: [CanvasRelation] = []
    var neighbours: [String: Set<String>] = [:]
    var kindCounts: [(kind: EntityKind, count: Int)] = []

    init() {}

    init(input: KnowledgeGraphInput) {
        let ids = Set(input.entities.map(\.id))
        relations = input.relations.filter { ids.contains($0.source) && ids.contains($0.target) && $0.source != $0.target }
        var degree: [String: Int] = [:]
        for r in relations {
            degree[r.source, default: 0] += 1
            degree[r.target, default: 0] += 1
            neighbours[r.source, default: []].insert(r.target)
            neighbours[r.target, default: []].insert(r.source)
        }
        var counts: [EntityKind: Int] = [:]
        for e in input.entities where byID[e.id] == nil {
            let kind = EntityKind(raw: e.kind)
            counts[kind, default: 0] += 1
            let d = degree[e.id] ?? 0
            byID[e.id] = entities.count
            entities.append(EntityVisual(id: e.id, name: e.name, kind: kind, summary: e.summary,
                                         radius: 6 + 2.6 * Double(d).squareRoot(), degree: d,
                                         embodiedBy: input.embodiedBy[e.id] ?? []))
        }
        kindCounts = EntityKind.allCases.compactMap { k in counts[k].map { (k, $0) } }
    }

    /// Entities get room for their name labels: a wider collision radius and more repulsion for hubs.
    var layoutNodes: [LayoutNodeSpec] {
        entities.map { LayoutNodeSpec(id: $0.id, radius: $0.radius + 10, charge: -140 - 30 * Double($0.degree)) }
    }

    /// One link per connected pair, longer around hubs so their spokes and labels spread out.
    var layoutLinks: [LayoutLinkSpec] {
        var seen = Set<String>()
        return relations.compactMap { r -> LayoutLinkSpec? in
            let key = r.source < r.target ? "\(r.source)|\(r.target)" : "\(r.target)|\(r.source)"
            guard seen.insert(key).inserted else { return nil }
            let degree = { (id: String) in self.byID[id].map { self.entities[$0].degree } ?? 0 }
            let hub = Double(max(degree(r.source), degree(r.target)))
            return LayoutLinkSpec(source: r.source, target: r.target, distance: 50 + 22 * hub.squareRoot(), strength: nil)
        }
    }
}

/// Glyph outlines per entity kind, so kinds stay distinguishable without colour.
nonisolated enum EntityGlyphPath {
    static func path(_ glyph: EntityKind.Glyph, center c: CGPoint, radius r: Double) -> Path {
        switch glyph {
        case .circle:
            return Path(ellipseIn: CGRect(x: c.x - r, y: c.y - r, width: 2 * r, height: 2 * r))
        case .square:
            let s = r * 0.9
            return Path(roundedRect: CGRect(x: c.x - s, y: c.y - s, width: 2 * s, height: 2 * s), cornerRadius: r * 0.22)
        case .diamond:
            return polygon(c, r * 1.15, sides: 4, rotation: 0)
        case .hexagon:
            return polygon(c, r * 1.05, sides: 6, rotation: .pi / 6)
        case .triangle:
            return polygon(c, r * 1.25, sides: 3, rotation: -.pi / 2)
        case .invertedTriangle:
            return polygon(c, r * 1.25, sides: 3, rotation: .pi / 2)
        case .pentagon:
            return polygon(c, r * 1.1, sides: 5, rotation: -.pi / 2)
        case .octagon:
            return polygon(c, r * 1.05, sides: 8, rotation: .pi / 8)
        }
    }

    private static func polygon(_ c: CGPoint, _ r: Double, sides: Int, rotation: Double) -> Path {
        var p = Path()
        for k in 0..<sides {
            let a = rotation + Double(k) * 2 * .pi / Double(sides)
            let pt = CGPoint(x: c.x + r * cos(a), y: c.y + r * sin(a))
            if k == 0 { p.move(to: pt) } else { p.addLine(to: pt) }
        }
        p.closeSubpath()
        return p
    }
}

struct KnowledgeGraphRenderer {
    var scene: KnowledgeGraphScene
    var snapshot: LayoutSnapshot
    var camera: CanvasCamera
    var now: Double
    var palette: CanvasPalette
    var accent: Color
    var focusID: String?
    var selectedID: String?
    var hoveredID: String?
    var hoveredRelation: Int?

    func draw(in ctx: inout GraphicsContext, size: CGSize) {
        guard !snapshot.ids.isEmpty else { return }
        let positions = snapshot.positions(at: now)
        func screen(_ id: String) -> CGPoint? {
            guard let i = snapshot.index[id], positions.indices.contains(i) else { return nil }
            return camera.toScreen(positions[i], size: size)
        }
        func radius(_ id: String) -> Double {
            guard let i = scene.byID[id] else { return 5 }
            return max(2, scene.entities[i].radius * camera.scale)
        }
        let focusSet: Set<String>? = focusID.map { scene.neighbours[$0, default: []].union([$0]) }
        func dimmed(_ id: String) -> Bool { focusSet.map { !$0.contains(id) } ?? false }

        // Relations: a line and a small arrowhead toward the target.
        var labelled: [(Int, CGPoint, CGPoint)] = []
        for (k, rel) in scene.relations.enumerated() {
            guard let a = screen(rel.source), let b = screen(rel.target) else { continue }
            let p0 = CanvasGeometry.inset(a, toward: b, by: radius(rel.source) + 1)
            let p1 = CanvasGeometry.inset(b, toward: a, by: radius(rel.target) + 2)
            let touching = focusID.map { rel.source == $0 || rel.target == $0 } ?? false
            let hot = touching || hoveredRelation == k
            let opacity = hot ? 0.85 : (focusID == nil ? (palette.increasedContrast ? 0.6 : 0.4) : 0.1)
            let color = hot ? accent.opacity(opacity) : palette.edge.opacity(opacity)
            var line = Path()
            line.move(to: p0)
            line.addLine(to: p1)
            ctx.stroke(line, with: .color(color), lineWidth: hot ? 1.5 : 1)
            let dx = p1.x - p0.x, dy = p1.y - p0.y
            let len = max(0.001, hypot(dx, dy))
            if len > 14 {
                let ux = dx / len, uy = dy / len, s = 5.5
                var arrow = Path()
                arrow.move(to: p1)
                arrow.addLine(to: CGPoint(x: p1.x - ux * s - uy * s * 0.5, y: p1.y - uy * s + ux * s * 0.5))
                arrow.addLine(to: CGPoint(x: p1.x - ux * s + uy * s * 0.5, y: p1.y - uy * s - ux * s * 0.5))
                arrow.closeSubpath()
                ctx.fill(arrow, with: .color(color))
            }
            if hot { labelled.append((k, p0, p1)) }
        }

        // Entities.
        let order = scene.entities.indices.sorted { a, b in
            let da = dimmed(scene.entities[a].id), db = dimmed(scene.entities[b].id)
            if da != db { return da }
            return scene.entities[a].radius < scene.entities[b].radius
        }
        for i in order {
            let e = scene.entities[i]
            guard let c = screen(e.id) else { continue }
            let r = radius(e.id)
            let alpha = dimmed(e.id) ? 0.25 : 1
            let shape = EntityGlyphPath.path(e.kind.glyph, center: c, radius: r)
            let color = palette.entity(e.kind)
            ctx.fill(shape, with: .color(color.opacity(0.88 * alpha)))
            ctx.stroke(shape, with: .color(palette.nodeHalo.opacity(alpha)), lineWidth: 1)
            if !e.embodiedBy.isEmpty {
                let o = EntityGlyphPath.path(e.kind.glyph, center: c, radius: r + 3)
                ctx.stroke(o, with: .color(palette.text.opacity(0.7 * alpha)), lineWidth: 1.1)
            }
            if e.id == selectedID || e.id == hoveredID {
                let o = EntityGlyphPath.path(e.kind.glyph, center: c, radius: r + (e.embodiedBy.isEmpty ? 3.5 : 6))
                ctx.stroke(o, with: .color(e.id == selectedID ? accent : palette.text.opacity(0.6)),
                           lineWidth: e.id == selectedID ? 2 : 1.2)
            }
        }

        // Names: all when the graph is small or zoomed in, otherwise the best-connected ones and the focus.
        let showAll = scene.entities.count <= 40 || camera.scale >= 1.3
        let topIDs: Set<String> = showAll ? [] : Set(scene.entities.sorted { $0.degree > $1.degree }.prefix(12).map(\.id))
        for e in scene.entities {
            let visible = showAll || topIDs.contains(e.id) || focusSet?.contains(e.id) == true
            guard visible, let c = screen(e.id) else { continue }
            let isFocus = e.id == focusID
            let faded = dimmed(e.id)
            let text = Text(clippedLabel(e.name, 36))
                .font(.system(size: isFocus ? 11.5 : 10.5, weight: isFocus ? .semibold : .regular))
                .foregroundStyle((isFocus ? palette.text : palette.textDim).opacity(faded ? 0.35 : 1))
            let resolved = ctx.resolve(text)
            let p = CGPoint(x: c.x, y: c.y + radius(e.id) + (e.embodiedBy.isEmpty ? 3 : 6))
            let sz = resolved.measure(in: CGSize(width: 2000, height: 100))
            let bg = Path(roundedRect: CGRect(x: p.x - sz.width / 2 - 3, y: p.y - 0.5, width: sz.width + 6, height: sz.height + 1),
                          cornerRadius: 3)
            ctx.fill(bg, with: .color(palette.nodeHalo.opacity(faded ? 0.25 : 0.7)))
            ctx.draw(resolved, at: p, anchor: .top)
        }

        // Relation labels for the focus or the hovered relation, at the midpoint.
        // Labels sit closer to the far end of each relation, so a hub's labels fan out instead of piling up.
        let maxLabels = hoveredRelation != nil && labelled.count > 12 ? 1 : 12
        let shown = labelled.sorted { a, _ in a.0 == hoveredRelation }.prefix(maxLabels)
        for (k, p0, p1) in shown {
            let rel = scene.relations[k]
            let t = focusID == rel.source ? 0.62 : (focusID == rel.target ? 0.38 : 0.5)
            let mid = CGPoint(x: p0.x + (p1.x - p0.x) * t, y: p0.y + (p1.y - p0.y) * t)
            let text = Text(clippedLabel(scene.relations[k].label, 48))
                .font(.system(size: 10, weight: .medium))
                .foregroundStyle(palette.text)
            let resolved = ctx.resolve(text)
            let sz = resolved.measure(in: CGSize(width: 2000, height: 100))
            let bg = Path(roundedRect: CGRect(x: mid.x - sz.width / 2 - 4, y: mid.y - sz.height / 2 - 1.5,
                                              width: sz.width + 8, height: sz.height + 3), cornerRadius: 4)
            ctx.fill(bg, with: .color(palette.nodeHalo.opacity(0.92)))
            ctx.stroke(bg, with: .color(accent.opacity(0.5)), lineWidth: 0.6)
            ctx.draw(resolved, at: mid, anchor: .center)
        }
    }
}
