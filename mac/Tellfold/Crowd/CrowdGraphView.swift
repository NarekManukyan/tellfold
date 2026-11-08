import SwiftUI

/// The crowd as a graph of avatars, in three views (Camps, Spectrum, Influence) that tween into one another and follow the
/// playhead. Selecting an agent or a camp (shared through the `RunStore`) dims everyone else. `compact` is the
/// Overview card: no zoom or pan, a click hands the selection to the owner.
struct CrowdGraphView: View {
    let store: RunStore
    @Bindable var model: CrowdModel
    var compact = false
    /// The legend pill over the graph; the Crowd tab shows the legend in its own row instead.
    var showsLegend = true
    /// Strips along the top and bottom that floating glass controls cover (owned by the caller). The graph draws
    /// full-bleed underneath them and frames the picture between them.
    var insets = CrowdInsets.zero
    /// Compact only: an avatar or camp region was clicked.
    var onOpen: ((_ agentID: String?, _ campID: String?) -> Void)?

    @Environment(\.colorScheme) private var colorScheme
    @Environment(\.colorSchemeContrast) private var contrast
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private struct Trigger: Equatable {
        var signature: Int
        var width: Int
        var height: Int
        var insets: CrowdInsets
    }

    var body: some View {
        let scene = CrowdScene(store: store)
        let mode = model.effective(for: scene)
        let palette = CanvasPalette(scheme: colorScheme, increasedContrast: contrast == .increased)
        let _ = (model.avatarVersion, model.layoutVersion)
        let images = avatarImages(scene.nodes)

        GeometryReader { geo in
            let trigger = Trigger(signature: scene.layoutSignature(mode: mode), width: Int(geo.size.width / 8),
                                  height: Int(geo.size.height / 8), insets: insets)
            TimelineView(.animation(paused: !model.isAnimating)) { _ in
                let now = ProcessInfo.processInfo.systemUptime
                let camera = model.userCamera ?? model.autoCamera(at: now)
                let positions = model.positions(at: now)
                let renderer = CrowdRenderer(
                    scene: scene, mode: mode, positions: positions, camera: camera, weights: model.weights(at: now),
                    layout: model.layout, palette: palette, images: images, hoveredID: model.hoveredID,
                    hoveredEdge: model.hoveredEdge, compact: compact, topInset: insets.top)
                ZStack(alignment: .topLeading) {
                    Canvas { ctx, size in renderer.draw(in: &ctx, size: size) }
                        .accessibilityHidden(true)
                    tooltip(scene: scene, positions: positions, camera: camera, size: geo.size)
                }
            }
            .onChange(of: trigger, initial: true) { _, _ in
                model.relayout(scene: scene, mode: mode, size: geo.size, reduceMotion: reduceMotion, compact: compact, insets: insets)
            }
            .modifier(CrowdInteractionModifier(model: model, compact: compact, onTap: { point in tap(point, scene: scene, mode: mode) },
                                               onEscape: { clearSelection() }))
            .accessibilityElement(children: .contain)
            .accessibilityLabel("Crowd graph")
            .accessibilityValue("Round \(scene.round), \(scene.nodes.count) people in \(scene.groups.count) groups, view: \(mode.title)")
            .accessibilityHint("Each person is listed inside. Activate one to show them.")
            .accessibilityChildren {
                CrowdAccessibilityList(scene: scene) { id in select(agent: id) }
            }
        }
        .task(id: scene.nodes.map(\.id)) { model.warmAvatars(scene.nodes) }
        .overlay(alignment: .top) {
            if model.mode == .spectrum, !scene.runHasSentiment, scene.isLoaded {
                Text("This run has no per-agent sentiment")
                    .font(Theme.small)
                    .foregroundStyle(Theme.secondary)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 5)
                    .glassSurface(in: .capsule)
                    .padding(.top, insets.top + 6)
            }
        }
        .overlay(alignment: .bottomLeading) {
            if !compact, showsLegend { CrowdLegend(mode: mode).padding(10).padding(.bottom, insets.bottom) }
        }
        .clipped()
    }

    // MARK: avatars

    private func avatarImages(_ nodes: [CrowdNode]) -> [String: Image] {
        var out: [String: Image] = [:]
        out.reserveCapacity(nodes.count)
        for n in nodes {
            if let cg = AvatarImageCache.shared.cached(for: n.identity) {
                out[n.id] = Image(decorative: cg, scale: 1).interpolation(.high)
            }
        }
        return out
    }

    // MARK: selection

    private func select(agent id: String) {
        if store.selectedAgentID != id { store.selectedAgentID = id }
    }

    private func clearSelection() {
        if store.selectedAgentID != nil {
            store.selectedAgentID = nil
        } else {
            store.selectedCampID = nil
        }
    }

    private func tap(_ point: CGPoint, scene: CrowdScene, mode: CrowdMode) {
        if let id = model.hitNode(point) {
            if compact { onOpen?(id, nil) } else { select(agent: id) }
            return
        }
        if mode == .camps, let group = model.hitGroup(point, scene: scene, compact: compact) {
            if compact {
                onOpen?(nil, group.id)
            } else {
                store.selectedAgentID = nil
                store.selectedCampID = group.id
            }
            return
        }
        if !compact {
            store.selectedAgentID = nil
            store.selectedCampID = nil
        }
    }

    // MARK: tooltip

    @ViewBuilder
    private func tooltip(scene: CrowdScene, positions: [String: CGPoint], camera: CanvasCamera, size: CGSize) -> some View {
        if let id = model.hoveredID, let node = scene.node(id), let p = positions[id] {
            let s = camera.toScreen(SIMD2(p.x, p.y), size: size)
            CanvasTooltip(anchor: s, nodeRadius: node.radius * CrowdModel.nodeScale(camera.scale), container: size) {
                CrowdTooltipContent(node: node, group: scene.group(of: node), sentiment: scene.sentiment[node.id])
            }
        } else if let edge = model.hoveredEdge, let at = model.hoverLocation {
            CanvasTooltip(anchor: at, nodeRadius: 2, container: size) {
                CrowdEdgeTooltip(edge: edge, scene: scene)
            }
        }
    }
}

/// Pointer handling: the full pan / zoom / keyboard set, or hover and click only for the compact card.
private struct CrowdInteractionModifier: ViewModifier {
    let model: CrowdModel
    let compact: Bool
    let onTap: (CGPoint) -> Void
    let onEscape: () -> Void

    func body(content: Content) -> some View {
        if compact {
            content
                .contentShape(Rectangle())
                .onContinuousHover(coordinateSpace: .local) { model.hover($0) }
                .onTapGesture(coordinateSpace: .local) { onTap($0) }
        } else {
            content.graphCanvasInteraction(model, onTap: onTap, onEscape: onEscape)
        }
    }
}

extension CrowdModel {
    /// The camp whose region (Camps view) is under `point`.
    func hitGroup(_ point: CGPoint, scene: CrowdScene, compact: Bool = false) -> CampGroup? {
        let now = ProcessInfo.processInfo.systemUptime
        let pos = positions(at: now)
        let cam = camera
        let ns = Self.nodeScale(cam.scale)
        func screen(_ id: String) -> CGPoint? { pos[id].map { cam.toScreen(SIMD2($0.x, $0.y), size: viewSize) } }
        for (gi, group) in scene.groups.enumerated().reversed() {
            if let rect = CrowdRenderer.regionRect(group: gi, nodes: scene.nodes, camera: cam, scale: ns, label: scene.label(group, compact: compact), screen: screen),
               rect.contains(point) { return group }
        }
        return nil
    }
}

// MARK: - overlays

struct CrowdTooltipContent: View {
    let node: CrowdNode
    let group: CampGroup?
    let sentiment: Double?

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            AvatarView(identity: node.identity, size: 32)
            VStack(alignment: .leading, spacing: 3) {
                Text(node.name).font(Theme.bodyStrong).foregroundStyle(Theme.text)
                Text("@\(node.handle)").font(Theme.mono(11)).foregroundStyle(Theme.muted)
                if let group {
                    HStack(spacing: 5) {
                        Circle().fill(group.color).frame(width: 7, height: 7)
                        Text(group.name).font(Theme.small).foregroundStyle(Theme.secondary)
                    }
                }
                HStack(spacing: 5) {
                    if let sentiment {
                        Text(RunUI.sentimentText(sentiment))
                            .font(Theme.mono(11, .semibold))
                            .foregroundStyle(RunUI.sentimentColor(sentiment))
                    }
                    Text(SentimentScale.describe(sentiment)).font(Theme.small).foregroundStyle(Theme.muted)
                }
            }
        }
    }
}

struct CrowdEdgeTooltip: View {
    let edge: CrowdEdge
    let scene: CrowdScene

    var body: some View {
        let from = scene.node(edge.from)?.handle ?? edge.from, to = scene.node(edge.to)?.handle ?? edge.to
        VStack(alignment: .leading, spacing: 3) {
            if edge.replies > 0 {
                Text("@\(from) replied to @\(to) \(edge.replies)×").font(Theme.small).foregroundStyle(Theme.text)
            }
            if edge.reposts > 0 {
                Text("@\(from) reposted @\(to) \(edge.reposts)×").font(Theme.small).foregroundStyle(Theme.text)
            }
        }
    }
}

/// What the curves and arrows mean in the current view.
struct CrowdLegend: View {
    let mode: CrowdMode
    /// A glass capsule (floating over the graph) or bare text (in a row of its own).
    var framed = true
    @Environment(\.colorScheme) private var colorScheme

    var body: some View {
        let palette = CanvasPalette(scheme: colorScheme)
        HStack(spacing: 12) {
            switch mode {
            case .camps:
                ForEach([ArcKind.reply, .like, .dislike, .repost], id: \.self) { kind in
                    HStack(spacing: 5) {
                        ArcSample(kind: kind, palette: palette).frame(width: 20, height: 10)
                        Text(kind.label.lowercased()).fixedSize()
                    }
                }
                Text("this round").foregroundStyle(framed ? Theme.muted : Theme.faint)
            case .influence:
                Text("Arrows: who replied to or reposted whom. Thicker means more.")
                    .fixedSize()
            case .spectrum:
                Text("Grey: no score yet").fixedSize()
            }
        }
        .font(Theme.caption)
        .foregroundStyle(framed ? Theme.secondary : Theme.muted)
        .padding(.horizontal, framed ? 12 : 0)
        .padding(.vertical, framed ? 6 : 0)
        .modifier(GlassCapsule(active: framed))
        .accessibilityHidden(true)
    }
}

/// VoiceOver representation of the graph: one button per person, sorted by name.
struct CrowdAccessibilityList: View {
    let scene: CrowdScene
    var select: (String) -> Void

    var body: some View {
        let sorted = scene.nodes.sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
        ForEach(sorted, id: \.id) { n in
            Button {
                select(n.id)
            } label: {
                Text("\(n.name), @\(n.handle)")
            }
            .accessibilityValue(value(n))
            .accessibilityAddTraits(n.id == scene.selectedAgentID ? .isSelected : [])
        }
    }

    private func value(_ n: CrowdNode) -> String {
        var parts: [String] = []
        if let g = scene.group(of: n) { parts.append("in \(g.name)") }
        if let s = scene.sentiment[n.id] {
            parts.append("sentiment \(RunUI.sentimentText(s)), \(SentimentScale.describe(s))")
        } else {
            parts.append("no scored posts yet")
        }
        parts.append(n.isOrganisation ? "organisation" : "person")
        return parts.joined(separator: "; ")
    }
}

/// Wraps a floating control in a Liquid Glass capsule, or leaves it bare when `active` is false.
struct GlassCapsule: ViewModifier {
    var active = true

    func body(content: Content) -> some View {
        if active {
            content.glassSurface(in: .capsule)
        } else {
            content
        }
    }
}
