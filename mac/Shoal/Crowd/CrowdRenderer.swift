import SwiftUI

/// Draws one frame of the crowd graph into a `GraphicsContext`. A plain value: everything it needs is captured, so
/// the canvas closure never reaches back into observable state.
struct CrowdRenderer {
    var scene: CrowdScene
    var mode: CrowdMode
    /// World positions at this instant.
    var positions: [String: CGPoint]
    var camera: CanvasCamera
    /// How present the camp regions, the Shoal axis and the Influence rings are (0...1 each).
    var weights: [Double]
    var layout: CrowdLayout
    var palette: CanvasPalette
    /// Cached avatar bitmaps by agent id; missing ones are drawn as vectors.
    var images: [String: Image]
    var hoveredID: String?
    var hoveredEdge: CrowdEdge?
    var compact: Bool

    // MARK: geometry shared with hit testing

    /// The curve of an aggregated edge, from rim to rim.
    static func edgeCurve(from a: CGPoint, to b: CGPoint, fromRadius: Double, toRadius: Double)
        -> (p0: CGPoint, c: CGPoint, p1: CGPoint) {
        let p0 = CanvasGeometry.inset(a, toward: b, by: fromRadius + 1)
        let p1 = CanvasGeometry.inset(b, toward: a, by: toRadius + 3)
        return (p0, CanvasGeometry.control(p0, p1, bend: 0.14), p1)
    }

    /// The tinted region behind a camp: its members' bounds plus padding, with room above for the label.
    static func regionRect(group gi: Int, nodes: [CrowdNode], camera: CanvasCamera, scale ns: Double,
                           label: CrowdLabel? = nil, screen: (String) -> CGPoint?) -> CGRect? {
        let pad = CrowdLayout.regionPadding * min(1, max(0.5, camera.scale))
        let room = CrowdLayout.labelRoom(lines: label?.lines ?? 1)
        let minWidth = label?.width ?? 0
        var lo = CGPoint(x: Double.infinity, y: Double.infinity), hi = CGPoint(x: -Double.infinity, y: -Double.infinity)
        var count = 0
        for n in nodes where n.groupIndex == gi {
            guard let p = screen(n.id) else { continue }
            let r = n.radius * ns
            lo.x = min(lo.x, p.x - r); hi.x = max(hi.x, p.x + r)
            lo.y = min(lo.y, p.y - r); hi.y = max(hi.y, p.y + r)
            count += 1
        }
        guard count > 0 else { return nil }
        let width = max(hi.x - lo.x + 2 * pad, minWidth)
        return CGRect(x: (lo.x + hi.x) / 2 - width / 2, y: lo.y - pad - room, width: width, height: hi.y - lo.y + 2 * pad + room)
    }

    // MARK: draw

    func draw(in ctx: inout GraphicsContext, size: CGSize) {
        guard !scene.nodes.isEmpty else { return }
        let ns = CrowdModel.nodeScale(camera.scale)
        func screen(_ id: String) -> CGPoint? {
            positions[id].map { camera.toScreen(SIMD2($0.x, $0.y), size: size) }
        }
        func radius(_ id: String) -> Double { (scene.node(id)?.radius ?? 11) * ns }

        // Selection: an agent dims everyone else; otherwise a camp dims everyone outside it.
        let focusAgent = scene.selectedAgentID
        let focusMembers: Set<String>? = focusAgent == nil
            ? scene.selectedGroupID.flatMap { gid in scene.groups.first { $0.id == gid }.map { Set($0.members) } }
            : nil
        func dimmed(_ id: String) -> Bool {
            if let focusAgent { return id != focusAgent }
            if let focusMembers { return !focusMembers.contains(id) }
            return false
        }
        let anyFocus = focusAgent != nil || focusMembers != nil

        let wCamps = weights[0], wShoal = weights[1], wInfluence = weights[2]
        if wCamps > 0.01 { drawRegions(in: &ctx, size: size, weight: wCamps, scale: ns, focusAgent: focusAgent, focusMembers: focusMembers, screen: screen) }
        if wShoal > 0.01 { drawShoalAxis(in: &ctx, size: size, weight: wShoal) }
        if wInfluence > 0.01 { drawRings(in: &ctx, size: size, weight: wInfluence) }

        if wCamps > 0.01 { drawCampEdges(in: &ctx, weight: wCamps, focusAgent: focusAgent, dimmed: dimmed, anyFocus: anyFocus, screen: screen, radius: radius) }
        if wInfluence > 0.01 { drawInfluenceEdges(in: &ctx, weight: wInfluence, dimmed: dimmed, anyFocus: anyFocus, screen: screen, radius: radius) }

        // Avatars: dimmed ones first, big voices last, the selected one on top.
        let order = scene.nodes.indices.sorted { a, b in
            let na = scene.nodes[a], nb = scene.nodes[b]
            let sa = na.id == focusAgent, sb = nb.id == focusAgent
            if sa != sb { return sb }
            let da = dimmed(na.id), db = dimmed(nb.id)
            if da != db { return da }
            return na.radius < nb.radius
        }
        for i in order {
            let node = scene.nodes[i]
            guard let c = screen(node.id) else { continue }
            drawNode(node, at: c, scale: ns, dim: dimmed(node.id), in: ctx)
        }
        drawLabels(in: &ctx, size: size, screen: screen, scale: ns, dimmed: dimmed)
    }

    // MARK: avatars

    private func drawNode(_ node: CrowdNode, at c: CGPoint, scale ns: Double, dim: Bool, in canvas: GraphicsContext) {
        var ctx = canvas
        let selected = node.id == scene.selectedAgentID
        let hovered = node.id == hoveredID
        let unscored = weights[1] > 0.5 && scene.sentiment[node.id] == nil
        let r = node.radius * ns * (selected ? 1.08 : 1)
        let alpha = dim ? 0.25 : (unscored ? 0.5 : 1)
        ctx.opacity = alpha
        if unscored { ctx.addFilter(.grayscale(1)) }

        // Ring: the camp colour in the Camps view, the card colour elsewhere (it keeps overlapping avatars apart).
        let ringColor: Color = selected ? palette.accent : (weights[0] > 0.5 ? (scene.group(of: node)?.color ?? palette.ground) : palette.ground)
        let ring = shape(center: c, radius: r + 1, organisation: node.isOrganisation)
        ctx.stroke(ring, with: .color(ringColor), lineWidth: selected ? 2.5 : 2)

        let rect = CGRect(x: c.x - r, y: c.y - r, width: 2 * r, height: 2 * r)
        if let image = images[node.id] {
            ctx.draw(image, in: rect)
        } else {
            var layer = ctx
            layer.translateBy(x: rect.minX, y: rect.minY)
            AvatarArt.draw(node.identity, in: &layer, size: rect.width)
        }
        if hovered, !selected {
            ctx.stroke(shape(center: c, radius: r + 3.5, organisation: node.isOrganisation),
                       with: .color(palette.text.opacity(0.35)), lineWidth: 1.2)
        }
    }

    /// A circle for a person, a rounded square for an organisation (the tile's 14/64 corner).
    private func shape(center c: CGPoint, radius r: Double, organisation: Bool) -> Path {
        let rect = CGRect(x: c.x - r, y: c.y - r, width: 2 * r, height: 2 * r)
        return organisation ? Path(roundedRect: rect, cornerRadius: r * 14 / 32) : Path(ellipseIn: rect)
    }

    // MARK: camps

    private func drawRegions(in ctx: inout GraphicsContext, size: CGSize, weight: Double, scale ns: Double, focusAgent: String?,
                             focusMembers: Set<String>?, screen: (String) -> CGPoint?) {
        for (gi, group) in scene.groups.enumerated() {
            guard let rect = Self.regionRect(group: gi, nodes: scene.nodes, camera: camera, scale: ns, label: scene.label(group, compact: compact), screen: screen) else { continue }
            let selectedGroup = focusAgent.flatMap { scene.node($0)?.groupIndex } == gi
                || scene.selectedGroupID == group.id
            let dim = (focusAgent != nil || focusMembers != nil) && !selectedGroup ? 0.45 : 1
            let shape = Path(roundedRect: rect, cornerRadius: 16)
            ctx.fill(shape, with: .color(group.color.opacity(0.10 * weight * dim)))
            ctx.stroke(shape, with: .color(group.color.opacity(0.35 * weight * dim)), lineWidth: 1)

            let label = scene.label(group, compact: compact)
            let figureColor = palette.textDim.opacity(weight * dim)
            // In the compact card a label wider than its region slides left so it stays whole inside the card.
            let x = compact ? max(6, min(rect.minX + 10, size.width - label.width + 10 - 2)) : rect.minX + 10
            for (i, row) in label.rows.enumerated() {
                let name = Text(row.name).font(Theme.ui(11.5, .semibold)).foregroundStyle(palette.text.opacity(weight * dim))
                let tail = Text(row.tail).font(Theme.mono(11)).foregroundStyle(figureColor)
                ctx.draw(ctx.resolve(Text("\(name)\(tail)")), at: CGPoint(x: x, y: rect.minY + 8 + 15 * Double(i)), anchor: .topLeading)
            }
        }
    }

    private func drawCampEdges(in ctx: inout GraphicsContext, weight: Double, focusAgent: String?,
                               dimmed: (String) -> Bool, anyFocus: Bool, screen: (String) -> CGPoint?,
                               radius: (String) -> Double) {
        // Follows: faint hairlines; the ones touching the selected agent are lit.
        var rest = Path(), lit = Path()
        for f in scene.follows {
            guard let a = screen(f.follower), let b = screen(f.followee) else { continue }
            if let focusAgent, f.follower == focusAgent || f.followee == focusAgent {
                lit.move(to: a); lit.addLine(to: b)
            } else if !anyFocus || !(dimmed(f.follower) || dimmed(f.followee)) {
                rest.move(to: a); rest.addLine(to: b)
            }
        }
        ctx.stroke(rest, with: .color(palette.edge.opacity(weight * (anyFocus ? 0.3 : 0.5))), lineWidth: 0.8)
        ctx.stroke(lit, with: .color(palette.accent.opacity(0.5 * weight)), lineWidth: 1.2)

        // This round's interactions: subtle directed curves.
        for (k, arc) in scene.arcs.enumerated() {
            guard let a = screen(arc.from), let b = screen(arc.to) else { continue }
            let involved = focusAgent.map { arc.from == $0 || arc.to == $0 } ?? (!anyFocus || !(dimmed(arc.from) && dimmed(arc.to)))
            let p0 = CanvasGeometry.inset(a, toward: b, by: radius(arc.from) + 1)
            let p1 = CanvasGeometry.inset(b, toward: a, by: radius(arc.to) + 3)
            let bend = 0.12 + (Double(k % 7) - 3) * 0.03
            let c = CanvasGeometry.control(p0, p1, bend: bend)
            let color = palette.arc(arc.kind).opacity((involved ? 0.6 : 0.12) * weight)
            var path = Path()
            path.move(to: p0)
            path.addQuadCurve(to: p1, control: c)
            ctx.stroke(path, with: .color(color), style: StrokeStyle(lineWidth: arc.kind.lineWidth * 0.8, lineCap: .round,
                                                                   lineJoin: .round, dash: arc.kind.dash))
            arrowhead(at: p1, from: CanvasGeometry.quadTangent(p0, c, p1, 1), size: 5, color: color, in: &ctx)
        }
    }

    // MARK: influence

    private func drawInfluenceEdges(in ctx: inout GraphicsContext, weight: Double, dimmed: (String) -> Bool,
                                    anyFocus: Bool, screen: (String) -> CGPoint?, radius: (String) -> Double) {
        for e in scene.edges {
            guard let a = screen(e.from), let b = screen(e.to) else { continue }
            let hot = e == hoveredEdge
            let faded = anyFocus && dimmed(e.from) && dimmed(e.to)
            let curve = Self.edgeCurve(from: a, to: b, fromRadius: radius(e.from), toRadius: radius(e.to))
            let base = e.replies >= e.reposts ? palette.accent : palette.arc(.repost)
            let color = base.opacity((hot ? 0.95 : (faded ? 0.1 : 0.5)) * weight)
            var path = Path()
            path.move(to: curve.p0)
            path.addQuadCurve(to: curve.p1, control: curve.c)
            let width = 0.9 + 1.1 * Double(e.count).squareRoot() + (hot ? 0.8 : 0)
            ctx.stroke(path, with: .color(color), style: StrokeStyle(lineWidth: width, lineCap: .round))
            arrowhead(at: curve.p1, from: CanvasGeometry.quadTangent(curve.p0, curve.c, curve.p1, 1),
                      size: 6 + min(3, Double(e.count)), color: color, in: &ctx)
        }
    }

    private func drawRings(in ctx: inout GraphicsContext, size: CGSize, weight: Double) {
        let c = camera.toScreen(.zero, size: size)
        let values = ["0.75", "0.5", "0.25"]
        for (k, r) in layout.ringRadii.enumerated() {
            let ry = r * camera.scale, rx = ry * layout.ringStretch
            let rect = CGRect(x: c.x - rx, y: c.y - ry, width: 2 * rx, height: 2 * ry)
            ctx.stroke(Path(ellipseIn: rect), with: .color(palette.edge.opacity(0.9 * weight)),
                       style: StrokeStyle(lineWidth: 1, dash: [2, 5]))
            if !compact || k == 1 {
                let label = Text(values[k]).font(Theme.mono(10)).foregroundStyle(palette.textDim.opacity(0.8 * weight))
                ctx.draw(ctx.resolve(label), at: CGPoint(x: c.x, y: c.y - ry - 1), anchor: .bottom)
            }
        }
        let title = Text("influence").font(Theme.ui(10)).foregroundStyle(palette.textDim.opacity(0.8 * weight))
        ctx.draw(ctx.resolve(title), at: CGPoint(x: c.x, y: c.y + 12), anchor: .top)
    }

    // MARK: shoal

    private func drawShoalAxis(in ctx: inout GraphicsContext, size: CGSize, weight: Double) {
        let half = layout.shoalHalf
        let x0 = camera.toScreen(.zero, size: size).x
        let xl = camera.toScreen(SIMD2(-half, 0), size: size).x
        let xr = camera.toScreen(SIMD2(half, 0), size: size).x
        let axisY = camera.toScreen(SIMD2(0, layout.shoalAxisY), size: size).y
        var centre = Path()
        centre.move(to: CGPoint(x: x0, y: 10))
        centre.addLine(to: CGPoint(x: x0, y: axisY))
        ctx.stroke(centre, with: .color(palette.edge.opacity(weight)), style: StrokeStyle(lineWidth: 1, dash: [3, 4]))
        var axis = Path()
        axis.move(to: CGPoint(x: xl, y: axisY))
        axis.addLine(to: CGPoint(x: xr, y: axisY))
        for v in [-1.0, -0.5, 0, 0.5, 1] {
            let x = camera.toScreen(SIMD2(v * half, 0), size: size).x
            axis.move(to: CGPoint(x: x, y: axisY - 3))
            axis.addLine(to: CGPoint(x: x, y: axisY + 3))
            if !compact || v.truncatingRemainder(dividingBy: 1) == 0 {
                let text = v == 0 ? "0" : (v > 0 ? "+" : "−") + (abs(v) == 1 ? "1" : "0.5")
                ctx.draw(ctx.resolve(Text(text).font(Theme.mono(10)).foregroundStyle(palette.textDim.opacity(weight))),
                         at: CGPoint(x: x, y: axisY + 5), anchor: .top)
            }
        }
        ctx.stroke(axis, with: .color(palette.edge.opacity(weight * 1.2)), lineWidth: 1)
        ctx.draw(ctx.resolve(Text("← Against").font(Theme.ui(11, .medium)).foregroundStyle(palette.text.opacity(weight))),
                 at: CGPoint(x: xl, y: 10), anchor: .topLeading)
        if let laneY = layout.shoalLaneY {
            let y = camera.toScreen(SIMD2(0, laneY), size: size).y
            var rule = Path()
            rule.move(to: CGPoint(x: xl, y: y)); rule.addLine(to: CGPoint(x: xr, y: y))
            ctx.stroke(rule, with: .color(palette.edge.opacity(weight)), style: StrokeStyle(lineWidth: 1, dash: [2, 4]))
            let count = scene.nodes.filter { scene.sentiment[$0.id] == nil }.count
            ctx.draw(ctx.resolve(Text("No posts yet · \(count)").font(Theme.ui(11, .medium)).foregroundStyle(palette.textDim.opacity(weight))),
                     at: CGPoint(x: xl, y: y + 4), anchor: .topLeading)
        }
        ctx.draw(ctx.resolve(Text("For →").font(Theme.ui(11, .medium)).foregroundStyle(palette.text.opacity(weight))),
                 at: CGPoint(x: xr, y: 10), anchor: .topTrailing)
    }

    // MARK: labels

    /// Handles under the bigger avatars, the selected one and the hovered one; a label that would collide with one
    /// already placed (or leave the canvas) is dropped.
    private func drawLabels(in ctx: inout GraphicsContext, size: CGSize, screen: (String) -> CGPoint?, scale ns: Double,
                            dimmed: (String) -> Bool) {
        var ids: [String] = []
        if let s = scene.selectedAgentID { ids.append(s) }
        if let h = hoveredID, !ids.contains(h) { ids.append(h) }
        let threshold = compact ? 0.6 : 0.4
        for n in scene.nodes.sorted(by: { ($0.influence, $1.id) > ($1.influence, $0.id) })
        where n.influence >= threshold && !ids.contains(n.id) && !dimmed(n.id) {
            ids.append(n.id)
        }
        var placed: [CGRect] = []
        let bounds = CGRect(origin: .zero, size: size).insetBy(dx: 2, dy: 2)
        for id in ids {
            guard let node = scene.node(id), let c = screen(id) else { continue }
            let emphasised = id == scene.selectedAgentID || id == hoveredID
            let text = Text("@" + clippedLabel(node.handle, 22))
                .font(Theme.mono(emphasised ? 11 : 10, emphasised ? .semibold : .regular))
                .foregroundStyle(emphasised ? palette.text : palette.textDim)
            let resolved = ctx.resolve(text)
            let sz = resolved.measure(in: CGSize(width: 2000, height: 100))
            let w = sz.width + 8, h = sz.height + 2
            let rect = CGRect(x: c.x - w / 2, y: c.y + node.radius * ns + 4, width: w, height: h)
            let touchesAvatar = scene.nodes.contains { other in
                guard other.id != id, let p = screen(other.id) else { return false }
                let r = other.radius * ns + 3
                return rect.insetBy(dx: -1, dy: -1).intersects(CGRect(x: p.x - r, y: p.y - r, width: 2 * r, height: 2 * r))
            }
            guard bounds.contains(rect), emphasised || (!touchesAvatar && !placed.contains(where: { $0.insetBy(dx: -2, dy: -1).intersects(rect) })) else { continue }
            placed.append(rect)
            ctx.fill(Path(roundedRect: rect, cornerRadius: 4), with: .color(palette.ground.opacity(0.88)))
            ctx.draw(resolved, at: CGPoint(x: rect.midX, y: rect.midY), anchor: .center)
        }
    }

    // MARK: helpers

    private func arrowhead(at tip: CGPoint, from tangent: CGVector, size s: Double, color: Color, in ctx: inout GraphicsContext) {
        let len = max(0.001, hypot(tangent.dx, tangent.dy))
        let ux = tangent.dx / len, uy = tangent.dy / len
        var arrow = Path()
        arrow.move(to: tip)
        arrow.addLine(to: CGPoint(x: tip.x - ux * s - uy * s * 0.5, y: tip.y - uy * s + ux * s * 0.5))
        arrow.addLine(to: CGPoint(x: tip.x - ux * s + uy * s * 0.5, y: tip.y - uy * s - ux * s * 0.5))
        arrow.closeSubpath()
        ctx.fill(arrow, with: .color(color))
    }
}

/// Shortens a canvas label to `limit` characters with an ellipsis.
nonisolated func clippedLabel(_ s: String, _ limit: Int) -> String {
    s.count > limit ? String(s.prefix(max(1, limit - 1))) + "…" : s
}
