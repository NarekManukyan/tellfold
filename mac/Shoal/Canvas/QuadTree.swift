import Foundation

/// A region quadtree over a fixed set of points, used for Barnes-Hut many-body approximation and for
/// neighbourhood queries (collision). Cells live in one flat array; children are always appended after their
/// parent, so a reverse walk over `cells` is a valid post-order for aggregation.
///
/// Coincident points (or points below `maxDepth`) share a leaf and are chained through `next`.
nonisolated struct QuadTree: Sendable {
    struct Cell: Sendable {
        var minX: Double
        var minY: Double
        var size: Double
        /// Child cell indices by quadrant (bit 0 = east half, bit 1 = south half); -1 when absent.
        var children: SIMD4<Int32> = SIMD4(repeating: -1)
        /// First point of this leaf's chain, -1 when empty or internal.
        var head: Int32 = -1
        var isLeaf = true
        /// Sum of point weights (for many-body this is the summed charge strength, usually negative).
        var mass: Double = 0
        /// Sum of |weight|, used to place the centre of mass.
        var absMass: Double = 0
        var centerX: Double = 0
        var centerY: Double = 0
        var maxRadius: Double = 0
        var count: Int32 = 0
    }

    static let maxDepth = 40

    private(set) var cells: [Cell] = []
    private(set) var next: [Int32]
    let positions: [SIMD2<Double>]
    let weights: [Double]
    let radii: [Double]

    /// - Parameters:
    ///   - positions: the points; NaN or infinite points are skipped.
    ///   - weights: per-point weight (charge). Defaults to 1 for every point.
    ///   - radii: per-point radius, aggregated as `maxRadius` per cell. Defaults to 0.
    init(positions: [SIMD2<Double>], weights: [Double]? = nil, radii: [Double]? = nil) {
        self.positions = positions
        self.weights = weights ?? Array(repeating: 1, count: positions.count)
        self.radii = radii ?? Array(repeating: 0, count: positions.count)
        self.next = Array(repeating: -1, count: positions.count)
        precondition(self.weights.count == positions.count && self.radii.count == positions.count)
        guard !positions.isEmpty else { return }

        var lo = SIMD2<Double>(repeating: .infinity)
        var hi = SIMD2<Double>(repeating: -.infinity)
        for p in positions where p.x.isFinite && p.y.isFinite {
            lo = pointwiseMin(lo, p)
            hi = pointwiseMax(hi, p)
        }
        guard lo.x.isFinite else { return }
        let span = max(hi.x - lo.x, hi.y - lo.y)
        let size = max(span, 1) * 1.0001 + 1e-6
        cells.reserveCapacity(positions.count * 2)
        cells.append(Cell(minX: lo.x, minY: lo.y, size: size))
        for i in positions.indices where positions[i].x.isFinite && positions[i].y.isFinite {
            insert(Int32(i))
        }
        accumulate()
    }

    var isEmpty: Bool { cells.isEmpty || cells[0].count == 0 }
    var root: Cell? { cells.first }

    // MARK: building

    private func quadrant(of p: SIMD2<Double>, in cell: Cell) -> Int {
        let half = cell.size / 2
        return (p.x >= cell.minX + half ? 1 : 0) | (p.y >= cell.minY + half ? 2 : 0)
    }

    private mutating func child(of index: Int, quadrant q: Int) -> Int {
        let existing = cells[index].children[q]
        if existing >= 0 { return Int(existing) }
        let parent = cells[index]
        let half = parent.size / 2
        let cell = Cell(minX: parent.minX + (q & 1 == 0 ? 0 : half),
                        minY: parent.minY + (q & 2 == 0 ? 0 : half),
                        size: half)
        cells.append(cell)
        let new = cells.count - 1
        cells[index].children[q] = Int32(new)
        return new
    }

    private mutating func insert(_ point: Int32) {
        let p = positions[Int(point)]
        var c = 0
        var depth = 0
        while true {
            if !cells[c].isLeaf {
                c = child(of: c, quadrant: quadrant(of: p, in: cells[c]))
                depth += 1
                continue
            }
            let head = cells[c].head
            if head < 0 {
                cells[c].head = point
                next[Int(point)] = -1
                return
            }
            if positions[Int(head)] == p || depth >= Self.maxDepth {
                next[Int(point)] = head
                cells[c].head = point
                return
            }
            // Split: the existing chain (all coincident) moves into one child, then descend again.
            cells[c].isLeaf = false
            cells[c].head = -1
            let target = child(of: c, quadrant: quadrant(of: positions[Int(head)], in: cells[c]))
            cells[target].head = head
        }
    }

    private mutating func accumulate() {
        for c in cells.indices.reversed() {
            var cell = cells[c]
            var mass = 0.0, absMass = 0.0, sx = 0.0, sy = 0.0, maxR = 0.0
            var count: Int32 = 0
            if cell.isLeaf {
                var j = cell.head
                while j >= 0 {
                    let i = Int(j)
                    let w = weights[i]
                    let aw = abs(w) > 0 ? abs(w) : 1e-12
                    mass += w
                    absMass += aw
                    sx += positions[i].x * aw
                    sy += positions[i].y * aw
                    maxR = max(maxR, radii[i])
                    count += 1
                    j = next[i]
                }
            } else {
                for q in 0..<4 {
                    let k = cell.children[q]
                    guard k >= 0 else { continue }
                    let ch = cells[Int(k)]
                    guard ch.count > 0 else { continue }
                    mass += ch.mass
                    absMass += ch.absMass
                    sx += ch.centerX * ch.absMass
                    sy += ch.centerY * ch.absMass
                    maxR = max(maxR, ch.maxRadius)
                    count += ch.count
                }
            }
            cell.mass = mass
            cell.absMass = absMass
            cell.count = count
            cell.maxRadius = maxR
            if absMass > 0 {
                cell.centerX = sx / absMass
                cell.centerY = sy / absMass
            } else {
                cell.centerX = cell.minX + cell.size / 2
                cell.centerY = cell.minY + cell.size / 2
            }
            cells[c] = cell
        }
    }

    // MARK: queries

    /// Calls `body` for every point index whose position lies within `radius` of `center`
    /// (the cell test is widened by each cell's `maxRadius` when `includeRadii` is set).
    func forEachPoint(near center: SIMD2<Double>, within radius: Double, includeRadii: Bool = false,
                      _ body: (Int) -> Void) {
        guard !cells.isEmpty else { return }
        var stack: [Int32] = [0]
        stack.reserveCapacity(64)
        while let top = stack.popLast() {
            let cell = cells[Int(top)]
            guard cell.count > 0 else { continue }
            let reach = radius + (includeRadii ? cell.maxRadius : 0)
            // Distance from center to the cell box.
            let dx = max(cell.minX - center.x, 0, center.x - (cell.minX + cell.size))
            let dy = max(cell.minY - center.y, 0, center.y - (cell.minY + cell.size))
            guard dx * dx + dy * dy <= reach * reach else { continue }
            if cell.isLeaf {
                var j = cell.head
                while j >= 0 {
                    let i = Int(j)
                    let d = positions[i] - center
                    let r = radius + (includeRadii ? radii[i] : 0)
                    if d.x * d.x + d.y * d.y <= r * r { body(i) }
                    j = next[i]
                }
            } else {
                for q in 0..<4 where cell.children[q] >= 0 { stack.append(cell.children[q]) }
            }
        }
    }

    /// Barnes-Hut many-body velocity contribution on point `index`, before multiplying by alpha.
    /// Each source contributes `delta * weight / distance²` (so the magnitude falls off as 1/distance);
    /// negative weights repel. Cells whose `size² / theta² < distance²` are treated as one body.
    func manyBody(on index: Int, theta: Double, distanceMin: Double = 1, distanceMax: Double = .infinity,
                  stack: inout [Int32], jiggle: () -> Double) -> SIMD2<Double> {
        guard !cells.isEmpty else { return .zero }
        let p = positions[index]
        let theta2 = theta * theta
        let dMin2 = distanceMin * distanceMin
        let dMax2 = distanceMax * distanceMax
        var force = SIMD2<Double>.zero
        stack.removeAll(keepingCapacity: true)
        stack.append(0)
        while let top = stack.popLast() {
            let cell = cells[Int(top)]
            guard cell.count > 0, cell.mass != 0 else { continue }
            var dx = cell.centerX - p.x
            var dy = cell.centerY - p.y
            var l = dx * dx + dy * dy
            let w = cell.size
            if !cell.isLeaf {
                if w * w / theta2 < l {
                    if l < dMax2 {
                        if dx == 0 { dx = jiggle(); l += dx * dx }
                        if dy == 0 { dy = jiggle(); l += dy * dy }
                        if l < dMin2 { l = (dMin2 * l).squareRoot() }
                        force.x += dx * cell.mass / l
                        force.y += dy * cell.mass / l
                    }
                } else {
                    for q in 0..<4 where cell.children[q] >= 0 { stack.append(cell.children[q]) }
                }
                continue
            }
            var j = cell.head
            while j >= 0 {
                let i = Int(j)
                if i != index {
                    var ex = positions[i].x - p.x
                    var ey = positions[i].y - p.y
                    var m = ex * ex + ey * ey
                    if m < dMax2 {
                        if ex == 0 { ex = jiggle(); m += ex * ex }
                        if ey == 0 { ey = jiggle(); m += ey * ey }
                        if m < dMin2 { m = (dMin2 * m).squareRoot() }
                        force.x += ex * weights[i] / m
                        force.y += ey * weights[i] / m
                    }
                }
                j = next[i]
            }
        }
        return force
    }
}
