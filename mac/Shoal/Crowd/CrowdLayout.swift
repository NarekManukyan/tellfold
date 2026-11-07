import CoreGraphics
import Foundation

/// The three layouts of the crowd graph. The same people and avatars in each.
nonisolated enum CrowdMode: String, CaseIterable, Identifiable, Sendable {
    case camps, shoal, influence

    var id: String { rawValue }
    var title: String {
        switch self {
        case .camps: "Camps"
        case .shoal: "Shoal"
        case .influence: "Influence"
        }
    }
}

/// One agent as the crowd graph needs it.
nonisolated struct CrowdNode: Sendable, Hashable {
    var id: String
    var handle: String
    var name: String
    var influence: Double
    var isOrganisation: Bool
    /// Index into the scene's camp groups.
    var groupIndex: Int

    /// Drawn radius from the fixed influence ramp (diameter 22 / 26 / 30 / 36 / 44).
    var radius: Double { InfluenceRamp.radius(influence) }
    var identity: AvatarIdentity { AvatarIdentity(id: id, handle: handle, name: name, isOrganisation: isOrganisation) }
}

/// Positions for one mode, in points around the origin (the view centre at scale 1).
nonisolated struct CrowdLayout: Sendable, Equatable {
    var positions: [String: CGPoint] = [:]
    /// Scale that makes the picture fit the view; 1 when it already does.
    var fitScale: Double = 1
    /// Centre of the picture, so the camera can put it in the middle of the view.
    var center: CGPoint = .zero
    /// Shoal: half width of the sentiment axis.
    var shoalHalf: Double = 0
    /// Influence: ring radii (vertical) for influence 0.75, 0.5 and 0.25, and the horizontal stretch.
    var ringRadii: [Double] = []
    var ringStretch: Double = 1
    /// Shoal: world y of the sentiment axis, and of the top of the "No posts yet" lane when someone is in it.
    var shoalAxisY: Double = 0
    var shoalLaneY: Double?

    static let viewPadding = 28.0
    /// Margin around a camp's cluster inside its tinted region.
    static let regionPadding = 20.0
    static let regionLabelRoom = 20.0
    /// Height above a camp's avatars for its label: one line, or two when the figures wrap.
    static func labelRoom(lines: Int) -> Double { regionLabelRoom + 15 * Double(max(0, lines - 1)) + 4 }
    /// Space kept between avatars.
    static let gap = 7.0

    static func compute(nodes: [CrowdNode], groupCount: Int, sentiment: [String: Double], size: CGSize,
                        mode: CrowdMode, labels: [CrowdLabel] = []) -> CrowdLayout {
        guard !nodes.isEmpty else { return CrowdLayout() }
        let size = CGSize(width: max(size.width, 240), height: max(size.height, 200))
        switch mode {
        case .camps: return camps(nodes, groupCount: groupCount, size: size, labels: labels)
        case .shoal: return shoal(nodes, sentiment: sentiment, size: size)
        case .influence: return influence(nodes, groupCount: groupCount, size: size)
        }
    }

    // MARK: camps

    /// Labels keep their size on screen while the picture is scaled to fit, so a picture that had to shrink needs more
    /// world room per label: lay out again with the scale the last pass came to.
    private static func camps(_ nodes: [CrowdNode], groupCount: Int, size: CGSize, labels: [CrowdLabel]) -> CrowdLayout {
        var layout = campsPass(nodes, groupCount: groupCount, size: size, labels: labels, labelScale: 1)
        for _ in 0..<2 where layout.fitScale < 0.97 {
            layout = campsPass(nodes, groupCount: groupCount, size: size, labels: labels, labelScale: max(0.45, layout.fitScale))
        }
        return layout
    }

    private static func campsPass(_ nodes: [CrowdNode], groupCount: Int, size: CGSize, labels: [CrowdLabel],
                                  labelScale: Double) -> CrowdLayout {
        var clusters: [[CrowdNode]] = Array(repeating: [], count: max(1, groupCount))
        for n in nodes { clusters[min(max(0, n.groupIndex), clusters.count - 1)].append(n) }
        clusters = clusters.map { $0.sorted { ($0.influence, $1.id) > ($1.influence, $0.id) } }
        let used = clusters.indices.filter { !clusters[$0].isEmpty }

        // Each cluster: a sunflower with the big voices in the middle, relaxed so no two avatars touch.
        var local: [String: CGPoint] = [:]
        var clusterRadius: [Int: Double] = [:]
        for g in used {
            let members = clusters[g]
            let rMax = members.map(\.radius).max() ?? 11
            let c = (2 * rMax + gap * 2) * 0.56
            var pts = members.indices.map { k -> CGPoint in
                let r = c * (Double(k) + 0.5).squareRoot()
                let a = Double(k) * 2.399_963
                return CGPoint(x: r * cos(a), y: r * sin(a))
            }
            let radii = members.map { $0.radius + 1.5 }
            relax(&pts, radii: radii, targets: pts, gap: gap, iterations: 80) { _, _ in (CGVector(dx: 1, dy: 0), 0.06, 0.06) }
            // Centre the cluster on its middle.
            let box = bounds(pts, radii)
            let mid = CGPoint(x: (box.minX + box.maxX) / 2, y: (box.minY + box.maxY) / 2)
            var radius = 0.0
            for (k, m) in members.enumerated() {
                let p = CGPoint(x: pts[k].x - mid.x, y: pts[k].y - mid.y)
                local[m.id] = p
                radius = max(radius, hypot(p.x, p.y) + m.radius + 2)
            }
            // A camp's region is at least as wide as its label.
            // A wide label gives its camp some extra room, but not so much that a small camp is spread thin.
            let label = labels.indices.contains(g) ? min(200, labels[g].width) / labelScale : 0
            clusterRadius[g] = max(radius, label / 2 - regionPadding)
        }

        // Cluster centres on an ellipse, wider than tall, pushed apart until their regions clear each other.
        let availW = size.width - 2 * viewPadding
        let availH = size.height - 2 * viewPadding - labelRoom(lines: 2)
        let rMaxCluster = clusterRadius.values.max() ?? 0
        let rx = max(availW * 0.2, availW / 2 - rMaxCluster - regionPadding)
        let ry = max(availH * 0.2, availH / 2 - rMaxCluster - regionPadding)
        var targets: [CGPoint] = []
        let total = used.map { clusterRadius[$0] ?? 1 }.reduce(0, +)
        var cumulative = 0.0
        for (i, g) in used.enumerated() {
            let w = clusterRadius[g] ?? 1
            switch used.count {
            case 1: targets.append(.zero)
            case 2: targets.append(CGPoint(x: (i == 0 ? -0.5 : 0.5) * rx, y: 0))
            default:
                let angle = -Double.pi / 2 - Double.pi * 0.25 + 2 * Double.pi * (cumulative + w / 2) / total
                targets.append(CGPoint(x: rx * cos(angle), y: ry * sin(angle)))
            }
            cumulative += w
        }
        var centres = targets
        let radii = used.map { (clusterRadius[$0] ?? 1) + regionPadding + 14 }
        relax(&centres, radii: radii, targets: targets, gap: 0, iterations: 160) { _, _ in (CGVector(dx: 1, dy: 0), 0.05, 0.05) }

        var positions: [String: CGPoint] = [:]
        for (i, g) in used.enumerated() {
            for m in clusters[g] {
                let l = local[m.id] ?? .zero
                positions[m.id] = CGPoint(x: centres[i].x + l.x, y: centres[i].y + l.y)
            }
        }
        var layout = CrowdLayout(positions: positions)
        // The picture is the camps' regions: their avatars plus padding, and at least as wide as their labels. Labels
        // keep their size on screen while the picture is scaled, so the fit is found by iterating. Each pass also
        // pushes overlapping regions (label box and cluster) apart, so no two camps touch.
        func regions(at scale: Double) -> [Int: CGRect] {
            let pad = regionPadding * min(1, max(0.5, scale)) / scale
            var out: [Int: CGRect] = [:]
            for g in used {
                var gl = CGPoint(x: Double.infinity, y: Double.infinity), gh = CGPoint(x: -Double.infinity, y: -Double.infinity)
                for m in clusters[g] {
                    guard let p = positions[m.id] else { continue }
                    gl.x = min(gl.x, p.x - m.radius); gh.x = max(gh.x, p.x + m.radius)
                    gl.y = min(gl.y, p.y - m.radius); gh.y = max(gh.y, p.y + m.radius)
                }
                let label = labels.indices.contains(g) ? labels[g] : nil
                // Label rows keep their size on screen, so the room above the avatars does not shrink with the picture.
                let room = labelRoom(lines: label?.lines ?? 1) / scale
                let mid = (gl.x + gh.x) / 2
                let half = max((gh.x - gl.x) / 2 + pad, (label?.width ?? 0) / 2 / scale)
                out[g] = CGRect(x: mid - half, y: gl.y - pad - room, width: 2 * half, height: gh.y + pad - (gl.y - pad - room))
            }
            return out
        }
        func shift(_ g: Int, dx: Double, dy: Double) {
            for m in clusters[g] {
                if let p = positions[m.id] { positions[m.id] = CGPoint(x: p.x + dx, y: p.y + dy) }
            }
        }
        var scale = 1.0
        for _ in 0..<6 {
            let gap = 14 / scale
            for _ in 0..<40 {
                let rects = regions(at: scale)
                var moved = false
                for (ii, i) in used.enumerated() {
                    for j in used[(ii + 1)...] {
                        guard let ri = rects[i], let rj = rects[j] else { continue }
                        let a = ri.insetBy(dx: -gap / 2, dy: -gap / 2), b = rj.insetBy(dx: -gap / 2, dy: -gap / 2)
                        guard a.intersects(b) else { continue }
                        let ox = min(a.maxX, b.maxX) - max(a.minX, b.minX)
                        let oy = min(a.maxY, b.maxY) - max(a.minY, b.minY)
                        moved = true
                        if ox <= oy {
                            let d = (ox / 2 + 0.5) * (ri.midX <= rj.midX ? 1 : -1)
                            shift(i, dx: -d, dy: 0); shift(j, dx: d, dy: 0)
                        } else {
                            let d = (oy / 2 + 0.5) * (ri.midY <= rj.midY ? 1 : -1)
                            shift(i, dx: 0, dy: -d); shift(j, dx: 0, dy: d)
                        }
                    }
                }
                if !moved { break }
            }
            var lo = CGPoint(x: Double.infinity, y: Double.infinity), hi = CGPoint(x: -Double.infinity, y: -Double.infinity)
            for r in regions(at: scale).values {
                lo.x = min(lo.x, r.minX); hi.x = max(hi.x, r.maxX)
                lo.y = min(lo.y, r.minY); hi.y = max(hi.y, r.maxY)
            }
            layout.frame(lo: lo, hi: hi, size: size)
            scale = layout.fitScale
        }
        layout.positions = positions
        return layout
    }

    // MARK: shoal

    private static func shoal(_ nodes: [CrowdNode], sentiment: [String: Double], size: CGSize) -> CrowdLayout {
        let half = max(80, size.width / 2 - viewPadding - 30)
        let scored = nodes.filter { sentiment[$0.id] != nil }.sorted { ($0.influence, $1.id) > ($1.influence, $0.id) }
        let unscored = nodes.filter { sentiment[$0.id] == nil }.sorted { ($0.influence, $1.id) > ($1.influence, $0.id) }
        var placed: [(x: Double, y: Double, r: Double)] = []
        var positions: [String: CGPoint] = [:]
        for n in scored {
            let score = min(1, max(-1, sentiment[n.id] ?? 0))
            let x = score * half
            let r = n.radius + 1.5
            var candidates: [Double] = [0]
            for p in placed {
                let need = r + p.r + gap
                let dx = abs(p.x - x)
                guard dx < need else { continue }
                let dy = (need * need - dx * dx).squareRoot()
                candidates.append(p.y + dy)
                candidates.append(p.y - dy)
            }
            candidates.sort { abs($0) != abs($1) ? abs($0) < abs($1) : $0 > $1 }
            let y = candidates.first { y in
                !placed.contains { p in hypot(p.x - x, p.y - y) < r + p.r + gap - 0.01 }
            } ?? 0
            placed.append((x, y, r))
            positions[n.id] = CGPoint(x: x, y: y)
        }
        var layout = CrowdLayout(positions: positions)
        layout.shoalHalf = half
        var lo = CGPoint(x: -half - 24, y: Double.infinity), hi = CGPoint(x: half + 24, y: -Double.infinity)
        for n in scored {
            guard let p = positions[n.id] else { continue }
            lo.y = min(lo.y, p.y - n.radius - 6); hi.y = max(hi.y, p.y + n.radius + 6)
        }
        if !lo.y.isFinite { lo.y = -30; hi.y = 30 }
        // The axis sits under the swarm; agents with no scored post yet wait in their own lane under the axis, not on
        // the 0 line where they would read as neutral.
        layout.shoalAxisY = hi.y + 14
        lo.y -= 22
        hi.y = layout.shoalAxisY + 34
        if !unscored.isEmpty {
            let laneY = layout.shoalAxisY + 40
            layout.shoalLaneY = laneY
            var x = -half, rowTop = laneY + 16, rowHeight = 0.0
            for n in unscored {
                let d = 2 * (n.radius + 1.5) + gap
                if x + d > half + 1 { x = -half; rowTop += rowHeight; rowHeight = 0 }
                positions[n.id] = CGPoint(x: x + d / 2, y: rowTop + d / 2)
                x += d
                rowHeight = max(rowHeight, d)
            }
            layout.positions = positions
            hi.y = rowTop + rowHeight + 8
        }
        layout.frame(lo: lo, hi: hi, size: size)
        return layout
    }

    // MARK: influence

    private static func influence(_ nodes: [CrowdNode], groupCount: Int, size: CGSize) -> CrowdLayout {
        let stretch = min(1.5, max(1, size.width / size.height * 0.85))
        let radius = max(60, min(size.height / 2 - viewPadding - 12, (size.width / 2 - viewPadding - 12) / stretch))
        var groups: [[CrowdNode]] = Array(repeating: [], count: max(1, groupCount))
        for n in nodes { groups[min(max(0, n.groupIndex), groups.count - 1)].append(n) }
        groups = groups.map { $0.sorted { ($0.influence, $1.id) > ($1.influence, $0.id) } }
        var ordered: [CrowdNode] = []
        var targets: [CGPoint] = []
        var start = -Double.pi / 2
        for g in groups where !g.isEmpty {
            let width = 2 * Double.pi * Double(g.count) / Double(nodes.count)
            for (k, n) in g.enumerated() {
                let a = start + (Double(k) + 0.5) / Double(g.count) * width
                let d = (1 - min(1, max(0, n.influence))) * radius
                ordered.append(n)
                targets.append(CGPoint(x: d * cos(a) * stretch, y: d * sin(a)))
            }
            start += width
        }
        var pts = targets.enumerated().map { i, t in
            CGPoint(x: t.x + sin(Double(i) * 1.7) * 0.8, y: t.y + cos(Double(i) * 2.3) * 0.8)
        }
        let radii = ordered.map { $0.radius + 1.5 }
        relax(&pts, radii: radii, targets: targets, gap: gap, iterations: 200) { _, p in
            let len = max(0.001, hypot(p.x / stretch, p.y))
            return (CGVector(dx: p.x / stretch / len, dy: p.y / len), 0.45, 0.025)
        }
        var positions: [String: CGPoint] = [:]
        for (i, n) in ordered.enumerated() { positions[n.id] = pts[i] }
        var layout = CrowdLayout(positions: positions)
        layout.ringRadii = [0.75, 0.5, 0.25].map { (1 - $0) * radius }
        layout.ringStretch = stretch
        let lo = CGPoint(x: -radius * stretch - 14, y: -radius - 14), hi = CGPoint(x: radius * stretch + 14, y: radius + 14)
        var box = (lo: lo, hi: hi)
        for (i, n) in ordered.enumerated() {
            box.lo.x = min(box.lo.x, pts[i].x - n.radius); box.hi.x = max(box.hi.x, pts[i].x + n.radius)
            box.lo.y = min(box.lo.y, pts[i].y - n.radius); box.hi.y = max(box.hi.y, pts[i].y + n.radius)
        }
        layout.frame(lo: box.lo, hi: box.hi, size: size)
        return layout
    }

    // MARK: helpers

    /// Sets `center` and `fitScale` so the box `lo...hi` fits inside `size` with the view padding.
    private mutating func frame(lo: CGPoint, hi: CGPoint, size: CGSize) {
        guard lo.x.isFinite, hi.x.isFinite else { return }
        center = CGPoint(x: (lo.x + hi.x) / 2, y: (lo.y + hi.y) / 2)
        let w = max(1, hi.x - lo.x), h = max(1, hi.y - lo.y)
        let availW = size.width - 2 * 12, availH = size.height - 2 * 12
        fitScale = min(1.5, max(0.3, min(availW / w, availH / h)))
    }

    private static func bounds(_ pts: [CGPoint], _ radii: [Double]) -> CGRect {
        var lo = CGPoint(x: Double.infinity, y: Double.infinity), hi = CGPoint(x: -Double.infinity, y: -Double.infinity)
        for (i, p) in pts.enumerated() {
            lo.x = min(lo.x, p.x - radii[i]); hi.x = max(hi.x, p.x + radii[i])
            lo.y = min(lo.y, p.y - radii[i]); hi.y = max(hi.y, p.y + radii[i])
        }
        return CGRect(x: lo.x, y: lo.y, width: hi.x - lo.x, height: hi.y - lo.y)
    }

    /// Pushes overlapping discs apart while a pull toward `targets` keeps the arrangement: `pull` gives, for a point,
    /// the unit axis along which the pull is strong (`along`) and the strength across it (`perp`).
    private static func relax(_ pts: inout [CGPoint], radii: [Double], targets: [CGPoint], gap: Double, iterations: Int,
                              pull: (Int, CGPoint) -> (u: CGVector, along: Double, perp: Double)) {
        let n = pts.count
        guard n > 0 else { return }
        var dx = [Double](repeating: 0, count: n), dy = [Double](repeating: 0, count: n)
        // The last stretch has no pull at all, so whatever the arrangement wanted, nothing is left touching.
        let finishing = max(20, iterations / 3)
        for step in 0..<(iterations + finishing) {
            let free = step >= iterations
            for i in 0..<n { dx[i] = 0; dy[i] = 0 }
            for i in 0..<n {
                for j in (i + 1)..<max(i + 1, n) {
                    var vx = pts[j].x - pts[i].x, vy = pts[j].y - pts[i].y
                    let need = radii[i] + radii[j] + gap
                    var d = (vx * vx + vy * vy).squareRoot()
                    guard d < need else { continue }
                    if d < 0.001 {
                        let a = Double(i * 31 + j * 17)
                        vx = cos(a); vy = sin(a); d = 1
                    }
                    let push = (need - d) / 2 * 0.8
                    let ux = vx / d, uy = vy / d
                    dx[i] -= ux * push; dy[i] -= uy * push
                    dx[j] += ux * push; dy[j] += uy * push
                }
            }
            for i in 0..<n {
                guard !free else {
                    pts[i].x += dx[i]; pts[i].y += dy[i]
                    continue
                }
                let (u, along, perp) = pull(i, pts[i])
                let tx = targets[i].x - pts[i].x, ty = targets[i].y - pts[i].y
                let a = tx * u.dx + ty * u.dy
                let px = tx - a * u.dx, py = ty - a * u.dy
                dx[i] += u.dx * a * along + px * perp
                dy[i] += u.dy * a * along + py * perp
                pts[i].x += dx[i]; pts[i].y += dy[i]
            }
        }
    }
}
