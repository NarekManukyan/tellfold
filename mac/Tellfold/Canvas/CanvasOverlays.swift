import SwiftUI

// MARK: - overlays

/// Places a tooltip next to a node, flipping sides to stay inside the canvas.
struct CanvasTooltip<Content: View>: View {
    var anchor: CGPoint
    var nodeRadius: Double
    var container: CGSize
    @ViewBuilder var content: Content
    @State private var size: CGSize = CGSize(width: 180, height: 80)

    var body: some View {
        let gap = nodeRadius + 10
        var x = anchor.x + gap
        if x + size.width > container.width - 6 { x = anchor.x - gap - size.width }
        var y = anchor.y - size.height / 2
        y = min(max(6, y), max(6, container.height - size.height - 6))
        x = min(max(6, x), max(6, container.width - size.width - 6))
        return content
            .fixedSize()
            .padding(.horizontal, 12)
            .padding(.vertical, 10)
            // A tooltip floats above the graph, so it is glass: the avatars and lines behind it refract through.
            .glassEffect(.regular, in: RoundedRectangle(cornerRadius: 12))
            .onGeometryChange(for: CGSize.self, of: { $0.size }, action: { size = $0 })
            .offset(x: x, y: y)
            .allowsHitTesting(false)
            .accessibilityHidden(true)
    }
}

/// A short horizontal sample of an arc style.
struct ArcSample: View {
    var kind: ArcKind
    var palette: CanvasPalette

    var body: some View {
        Canvas { ctx, size in
            let y = size.height / 2
            let style = StrokeStyle(lineWidth: kind.lineWidth, lineCap: .round, dash: kind.dash)
            let color = palette.arc(kind)
            let end = kind.hasArrowhead ? size.width - 5 : size.width
            for offset in kind.isDouble ? [-1.4, 1.4] : [0] {
                var p = Path()
                p.move(to: CGPoint(x: 1, y: y + offset))
                p.addLine(to: CGPoint(x: end, y: y + offset))
                ctx.stroke(p, with: .color(color), style: style)
            }
            if kind.hasArrowhead {
                var a = Path()
                a.move(to: CGPoint(x: size.width, y: y))
                a.addLine(to: CGPoint(x: size.width - 5, y: y - 2.8))
                a.addLine(to: CGPoint(x: size.width - 5, y: y + 2.8))
                a.closeSubpath()
                ctx.fill(a, with: .color(color))
            }
        }
    }
}

// MARK: - knowledge graph overlays

struct EntityTooltipContent: View {
    var entity: EntityVisual
    var palette: CanvasPalette
    var agentNames: [String: String]

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(entity.name).font(.callout.weight(.semibold))
            HStack(spacing: 5) {
                EntityKindSwatch(kind: entity.kind, palette: palette).frame(width: 10, height: 10)
                Text(entity.kind.label).font(.caption).foregroundStyle(.secondary)
                Text("· \(entity.degree) relations").font(.caption.monospacedDigit()).foregroundStyle(.secondary)
            }
            if !entity.summary.isEmpty {
                Text(entity.summary)
                    .font(.caption)
                    .lineLimit(4)
                    .frame(width: 260, alignment: .leading)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if !entity.embodiedBy.isEmpty {
                Label("Agent: \(entity.embodiedBy.map { agentNames[$0] ?? $0 }.joined(separator: ", "))",
                      systemImage: "person.crop.circle")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
    }
}

struct EntityKindSwatch: View {
    var kind: EntityKind
    var palette: CanvasPalette

    var body: some View {
        Canvas { ctx, size in
            let r = min(size.width, size.height) / 2 - 0.5
            let shape = EntityGlyphPath.path(kind.glyph, center: CGPoint(x: size.width / 2, y: size.height / 2), radius: r * 0.85)
            ctx.fill(shape, with: .color(palette.entity(kind)))
        }
        .accessibilityHidden(true)
    }
}

struct KnowledgeGraphLegend: View {
    var scene: KnowledgeGraphScene
    var palette: CanvasPalette
    @Binding var isExpanded: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            Button {
                isExpanded.toggle()
            } label: {
                Label("Legend", systemImage: isExpanded ? "chevron.down" : "chevron.right")
                    .font(Theme.ui(11, .medium))
                    .foregroundStyle(Theme.muted)
            }
            .buttonStyle(.plain)
            .accessibilityLabel(isExpanded ? "Hide legend" : "Show legend")

            if isExpanded {
                ForEach(scene.kindCounts, id: \.kind) { item in
                    HStack(spacing: 6) {
                        EntityKindSwatch(kind: item.kind, palette: palette).frame(width: 11, height: 11)
                        Text(item.kind.label).font(Theme.caption).foregroundStyle(Theme.text)
                        Spacer(minLength: 8)
                        Text("\(item.count)").font(Theme.mono(11)).foregroundStyle(Theme.muted)
                    }
                    .frame(width: 130)
                    .accessibilityElement(children: .ignore)
                    .accessibilityLabel("\(item.kind.label): \(item.count)")
                }
                Text("Outline: an agent represents it.")
                    .font(Theme.caption)
                    .foregroundStyle(Theme.muted)
            }
        }
        .padding(8)
        .glassEffect(.regular, in: RoundedRectangle(cornerRadius: 12))
    }
}
