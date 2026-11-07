import SwiftUI

/// The Crowd tab: the big graph with its view switch and round scrubber on the left; on the right the selected agent,
/// else the selected camp, else the feed of the round at the playhead.
struct CrowdTabView: View {
    let store: RunStore
    let replay: ReplayController
    @Bindable var model: CrowdModel
    let interviews: InterviewSessions

    @State private var width: CGFloat = 1200
    @State private var height: CGFloat = 800
    /// Stacked layout only: the feed under the graph is folded away until asked for, so the graph keeps the height.
    @State private var panelOpen = false

    private static let panelWidth: CGFloat = 360
    private static let stacks: CGFloat = 840

    var body: some View {
        Group {
            if store.personas.isEmpty {
                pending
            } else if width >= Self.stacks {
                HStack(alignment: .top, spacing: Theme.space4) {
                    graphCard
                    sidePanel.frame(width: Self.panelWidth)
                }
                .padding(Theme.space4)
            } else {
                VStack(spacing: Theme.space4) {
                    graphCard
                    stackedPanel
                }
                .padding(Theme.space4)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.ground)
        .onGeometryChange(for: CGSize.self, of: { $0.size }) { width = $0.width; height = $0.height }
        .onChange(of: store.selectedAgentID) { _, id in if id != nil { panelOpen = true } }
        .onChange(of: store.selectedCampID) { _, id in if id != nil { panelOpen = true } }
        .onChange(of: store.selectedPostID) { _, id in
            // A post chosen elsewhere (a citation, the log) shows in the feed: leave the agent and camp panels.
            if id != nil { store.selectedAgentID = nil; store.selectedCampID = nil }
        }
    }

    // MARK: graph

    private var graphCard: some View {
        let grouping = CampGrouping.groups(report: store.report, personas: store.personas)
        return VStack(spacing: 0) {
            HStack(spacing: Theme.space3) {
                Text("Crowd").font(Theme.sectionTitle).foregroundStyle(Theme.text)
                Text("\(store.personas.count) people, round \(store.playhead)")
                    .font(Theme.small)
                    .foregroundStyle(Theme.muted)
                Spacer(minLength: Theme.space2)
                CanvasZoomControls(controller: model, hasUserCamera: model.userCamera != nil)
                Picker("View", selection: $model.mode) {
                    ForEach(CrowdMode.allCases) { Text($0.title).tag($0) }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .fixedSize()
                .accessibilityLabel("Crowd view")
            }
            .padding(.horizontal, Theme.space4)
            .padding(.vertical, Theme.space3)
            Seam()
            CrowdGraphView(store: store, model: model, showsLegend: false)
                .frame(maxWidth: .infinity, minHeight: 140, maxHeight: .infinity)
            Seam()
            legendRow
            if grouping.isFallback, model.mode == .camps {
                Seam()
                CampsFallbackNote(store: store).padding(.horizontal, Theme.space4).padding(.vertical, Theme.space2)
            }
            Seam()
            RoundScrubber(store: store, replay: replay)
        }
        .card(padding: 0, clips: true)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    /// What the arrows mean (Camps, Influence) or what grey means (Shoal), in a row of its own so it never covers the graph.
    private var legendRow: some View {
        let hasScores = store.posts.contains { $0.sentiment != nil }
        let mode: CrowdMode = model.mode == .shoal && !hasScores ? .camps : model.mode
        return HStack {
            CrowdLegend(mode: mode, framed: false)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, Theme.space4)
        .frame(height: 28)
    }

    // MARK: side panel

    /// Narrow window: a bar that opens the feed, agent or camp under the graph.
    @ViewBuilder
    private var stackedPanel: some View {
        let title: String = {
            if let id = store.selectedAgentID, let p = store.personaByID[id] { return p.name }
            if let g = CampGrouping.groups(report: store.report, personas: store.personas).group(store.selectedCampID) { return g.name }
            return store.playhead == 0 ? "Opening posts" : "Round \(store.playhead) feed"
        }()
        VStack(spacing: 0) {
            Button {
                withAnimation(.snappy(duration: 0.2)) { panelOpen.toggle() }
            } label: {
                HStack(spacing: Theme.space2) {
                    Text(title).font(Theme.sectionTitle).foregroundStyle(Theme.text).lineLimit(1)
                    Spacer(minLength: 0)
                    Image(systemName: panelOpen ? "chevron.down" : "chevron.up")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(Theme.muted)
                }
                .padding(.horizontal, Theme.space4)
                .frame(height: 38)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(panelOpen ? "Hide details" : "Show details")
            .accessibilityValue(title)
            if panelOpen {
                Seam()
                sidePanel.frame(height: max(180, min(300, height * 0.34)))
            }
        }
        .card(padding: 0, clips: true)
    }

    @ViewBuilder
    private var sidePanel: some View {
        Group {
            if let id = store.selectedAgentID, store.personaByID[id] != nil {
                AgentInspector(store: store, interviews: interviews) { store.selectedAgentID = nil }
            } else if let group = CampGrouping.groups(report: store.report, personas: store.personas).group(store.selectedCampID) {
                CampPanel(store: store, group: group) { store.selectedCampID = nil }
            } else {
                FeedView(store: store)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .card(padding: 0, clips: true)
    }

    private var pending: some View {
        ContentUnavailableView {
            Label("No People Yet", systemImage: "person.2")
        } description: {
            Text(store.isLive ? "People appear here once they are cast." : "This run stopped before people were cast.")
        } actions: {
            if store.isLive { ProgressView().controlSize(.small) }
        }
    }
}

/// The selected camp: its summary, its people and what they posted in the round at the playhead.
struct CampPanel: View {
    let store: RunStore
    let group: CampGroup
    let close: () -> Void

    private var linkifier: RunLinkifier {
        RunLinkifier(personas: store.personas, postIDs: Set(store.posts.map(\.id)))
    }

    var body: some View {
        let byID = store.personaByID
        let scores = store.sentimentByAgent(at: store.playhead)
        let members = group.members.compactMap { byID[$0] }
            .sorted { ($0.influence, $1.handle) > ($1.influence, $0.handle) }
        let memberIDs = Set(group.members)
        let roundPosts = store.posts.filter { $0.round == store.playhead && memberIDs.contains($0.authorID) }
        ScrollView {
            VStack(alignment: .leading, spacing: Theme.space4) {
                HStack(alignment: .top, spacing: Theme.space2) {
                    Circle().fill(group.color).frame(width: 12, height: 12).padding(.top, 5)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(group.name).font(Theme.title).foregroundStyle(Theme.text)
                            .fixedSize(horizontal: false, vertical: true)
                            .accessibilityAddTraits(.isHeader)
                        HStack(spacing: 6) {
                            Text("\(members.count) \(members.count == 1 ? "person" : "people")")
                                .font(Theme.small).foregroundStyle(Theme.muted)
                            if let s = store.campSentiment(group, at: store.playhead) {
                                Text("mean").font(Theme.small).foregroundStyle(Theme.muted)
                                SentimentValue(score: s, size: 12)
                            }
                        }
                    }
                    Spacer(minLength: 0)
                    Button(action: close) {
                        Image(systemName: "xmark")
                            .font(.system(size: 10, weight: .bold))
                            .foregroundStyle(Theme.muted)
                            .frame(width: 22, height: 22)
                    }
                    .buttonStyle(HoverRowStyle())
                    .help("Close and show the feed")
                    .accessibilityLabel("Close camp")
                }
                if !group.summary.isEmpty {
                    Text(linkifier.inline(group.summary, size: 13))
                        .font(Theme.body)
                        .foregroundStyle(Theme.secondary)
                        .lineSpacing(3)
                        .fixedSize(horizontal: false, vertical: true)
                        .textSelection(.enabled)
                }
                VStack(alignment: .leading, spacing: 4) {
                    SectionTitle("People")
                    ForEach(members) { p in
                        Button {
                            store.selectedAgentID = p.id
                        } label: {
                            HStack(spacing: Theme.space2) {
                                AvatarView(persona: p, size: 28)
                                VStack(alignment: .leading, spacing: 0) {
                                    Text(p.name).font(Theme.ui(13, .medium)).foregroundStyle(Theme.text).lineLimit(1)
                                    Text("@\(p.handle)").font(Theme.mono(11)).foregroundStyle(Theme.muted).lineLimit(1)
                                }
                                Spacer(minLength: 4)
                                SentimentValue(score: scores[p.id], size: 12)
                            }
                            .padding(.horizontal, 6)
                            .padding(.vertical, 4)
                            .frame(maxWidth: .infinity, alignment: .leading)
                        }
                        .buttonStyle(HoverRowStyle())
                        .accessibilityHint("Shows this person")
                    }
                }
                VStack(alignment: .leading, spacing: Theme.space2) {
                    SectionTitle(store.playhead == 0 ? "Opening posts" : "Posts in round \(store.playhead)")
                    if roundPosts.isEmpty {
                        Text("Nobody in this camp posted in this round.")
                            .font(Theme.small)
                            .foregroundStyle(Theme.muted)
                    } else {
                        ForEach(roundPosts) { post in
                            VStack(alignment: .leading, spacing: 4) {
                                HStack(spacing: 6) {
                                    RunHandleChip(handle: post.authorHandle, sentiment: nil) { store.selectedAgentID = post.authorID }
                                    Spacer()
                                    if post.sentiment != nil { SentimentValue(score: post.sentiment, size: 11) }
                                    Text("#\(post.id)").font(Theme.mono(11)).foregroundStyle(Theme.muted)
                                }
                                Text(post.content)
                                    .font(Theme.body)
                                    .foregroundStyle(Theme.text)
                                    .lineSpacing(2)
                                    .fixedSize(horizontal: false, vertical: true)
                                    .textSelection(.enabled)
                            }
                            .padding(10)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .background(Theme.hover, in: RoundedRectangle(cornerRadius: Theme.radius))
                        }
                    }
                }
            }
            .padding(Theme.space4)
        }
    }
}
