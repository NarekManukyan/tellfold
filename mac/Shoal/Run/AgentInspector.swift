import Charts
import SwiftUI

/// Interview transcripts per agent, kept for the lifetime of the run view so switching agents keeps history.
@MainActor
@Observable
final class InterviewSessions {
    struct Turn: Identifiable, Sendable {
        let id = UUID()
        let question: String
        var answer: String?
        var error: String?
    }

    private(set) var turns: [String: [Turn]] = [:]
    private(set) var pending: Set<String> = []

    func history(for agentID: String) -> [Turn] { turns[agentID] ?? [] }
    func isWaiting(_ agentID: String) -> Bool { pending.contains(agentID) }

    func send(_ message: String, to agentID: String, store: RunStore) async {
        let text = message.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, !pending.contains(agentID) else { return }
        let past = (turns[agentID] ?? []).compactMap { t in t.answer.map { ChatTurn(question: t.question, answer: $0) } }
        turns[agentID, default: []].append(Turn(question: text))
        let index = turns[agentID]!.count - 1
        pending.insert(agentID)
        defer { pending.remove(agentID) }
        do {
            let reply = try await store.chat(agentID: agentID, message: text, history: Array(past.suffix(20)))
            turns[agentID]?[index].answer = reply
        } catch {
            turns[agentID]?[index].error = error.localizedDescription
        }
    }

    func clear(_ agentID: String) { turns[agentID] = nil }
}

/// The Crowd tab's side panel for the selected agent: who they are, their camp and sentiment, their posts, and an
/// interview chat.
struct AgentInspector: View {
    let store: RunStore
    var interviews: InterviewSessions
    /// Closes the panel and returns to the camp or the feed.
    var onClose: (() -> Void)?

    @State private var pane: Pane = .profile
    @State private var agent: AgentDetail?
    @State private var loadError: String?
    @State private var loading = false

    enum Pane: String, CaseIterable, Identifiable {
        case profile, interview
        var id: String { rawValue }
        var title: String { self == .profile ? "Profile" : "Interview" }
    }

    init(store: RunStore, interviews: InterviewSessions, onClose: (() -> Void)? = nil) {
        self.store = store
        self.interviews = interviews
        self.onClose = onClose
    }

    private var persona: Persona? {
        guard let id = store.selectedAgentID else { return nil }
        return store.detail?.personas.first { $0.id == id } ?? agent?.persona
    }

    var body: some View {
        Group {
            if store.selectedAgentID != nil, let persona {
                VStack(spacing: 0) {
                    AgentHeader(store: store, persona: persona, onClose: onClose)
                    Picker("Pane", selection: $pane) {
                        ForEach(Pane.allCases) { Text($0.title).tag($0) }
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                    .padding(.horizontal, Theme.space4)
                    .padding(.bottom, Theme.space3)
                    Seam()
                    switch pane {
                    case .profile:
                        AgentProfileView(store: store, persona: persona, agent: agent, loading: loading, loadError: loadError) {
                            if let id = store.selectedAgentID { Task { await load(id) } }
                        }
                    case .interview:
                        InterviewChatView(store: store, persona: persona, interviews: interviews)
                    }
                }
            } else {
                ContentUnavailableView("No Agent Selected", systemImage: "person.crop.circle",
                                       description: Text("Click a person in the crowd, the feed or the report."))
            }
        }
        .background(Theme.card)
        .task(id: TaskKey(agentID: store.selectedAgentID, posts: store.posts.count)) {
            guard let id = store.selectedAgentID else {
                agent = nil
                return
            }
            if agent?.persona.id != id { agent = nil }
            await load(id)
        }
    }

    private struct TaskKey: Hashable {
        let agentID: String?
        let posts: Int
    }

    private func load(_ id: String) async {
        loading = true
        defer { loading = false }
        do {
            let detail = try await store.agentDetail(id)
            guard store.selectedAgentID == id else { return }
            agent = detail
            loadError = nil
        } catch is CancellationError {
        } catch {
            if store.selectedAgentID == id { loadError = error.localizedDescription }
        }
    }
}

// MARK: - profile

/// Avatar, name, handle, camp and current sentiment.
private struct AgentHeader: View {
    let store: RunStore
    let persona: Persona
    let onClose: (() -> Void)?

    var body: some View {
        let grouping = CampGrouping.groups(report: store.report, personas: store.personas)
        let group = grouping.group(grouping.groupByAgent[persona.id])
        let score = store.sentimentByAgent(at: store.playhead)[persona.id]
        HStack(alignment: .top, spacing: Theme.space3) {
            AvatarView(persona: persona, size: 52)
            VStack(alignment: .leading, spacing: 3) {
                Text(persona.name)
                    .font(Theme.ui(17, .semibold))
                    .foregroundStyle(Theme.text)
                    .fixedSize(horizontal: false, vertical: true)
                    .accessibilityAddTraits(.isHeader)
                Text("@\(persona.handle)")
                    .font(Theme.mono(12))
                    .foregroundStyle(Theme.muted)
                HStack(spacing: 8) {
                    if let group {
                        Button {
                            store.selectedAgentID = nil
                            store.selectedCampID = group.id
                        } label: {
                            HStack(spacing: 5) {
                                Circle().fill(group.color).frame(width: 7, height: 7)
                                Text(group.name).font(Theme.small).foregroundStyle(Theme.secondary).lineLimit(1)
                            }
                            .padding(.horizontal, 8)
                            .padding(.vertical, 2)
                            .background(Theme.hover, in: Capsule())
                        }
                        .buttonStyle(.plain)
                        .help("Show this camp")
                    }
                    if score != nil { SentimentValue(score: score, size: 12) }
                    if persona.entityID != nil {
                        Text("Organisation").font(Theme.small).foregroundStyle(Theme.muted)
                    }
                }
                .padding(.top, 2)
            }
            Spacer(minLength: 0)
            Button { onClose?() } label: {
                Image(systemName: "xmark")
                    .font(.system(size: 10, weight: .bold))
                    .foregroundStyle(Theme.muted)
                    .frame(width: 22, height: 22)
            }
            .buttonStyle(HoverRowStyle())
            .help("Close and go back")
            .accessibilityLabel("Close")
        }
        .textSelection(.enabled)
        .padding(Theme.space4)
        .accessibilityElement(children: .contain)
    }
}

/// A titled block of the profile: a 13 semibold heading and its content.
private struct InspectorSection<Content: View>: View {
    let title: String
    var readout: String?
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text(title)
                    .font(Theme.sectionTitle)
                    .foregroundStyle(Theme.text)
                    .accessibilityAddTraits(.isHeader)
                Spacer()
                if let readout { Text(readout).font(Theme.small).foregroundStyle(Theme.muted) }
            }
            content
        }
        .padding(.horizontal, Theme.space4)
        .padding(.vertical, Theme.space3)
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct AgentProfileView: View {
    let store: RunStore
    let persona: Persona
    let agent: AgentDetail?
    let loading: Bool
    let loadError: String?
    let retry: () -> Void

    private var handles: [String: String] {
        Dictionary((store.detail?.personas ?? []).map { ($0.id, $0.handle) }, uniquingKeysWith: { a, _ in a })
    }

    /// Mean sentiment of the agent's own posts per round (from the loaded detail, else from the store's posts).
    private var sentimentSeries: [(round: Int, value: Double)] {
        let posts = agent?.posts ?? store.posts.filter { $0.authorID == persona.id }
        let grouped = Dictionary(grouping: posts.compactMap { p in p.sentiment.map { (p.round, $0) } }, by: \.0)
        return grouped.keys.sorted().map { r in
            let values = grouped[r]!.map(\.1)
            return (r, values.reduce(0, +) / Double(values.count))
        }
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                if !persona.bio.isEmpty {
                    Text(persona.bio)
                        .font(Theme.body)
                        .foregroundStyle(Theme.text)
                        .lineSpacing(3)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                        .padding(.horizontal, Theme.space4)
                        .padding(.top, Theme.space3)
                        .padding(.bottom, Theme.space1)
                }
                InspectorSection(title: "Sentiment by round") { sparkline }
                agentPosts
                InspectorSection(title: "Private to the simulation") {
                    privateField("Stance", persona.stance)
                    privateField("Background", persona.background)
                }
                if !persona.traits.isEmpty {
                    InspectorSection(title: "Traits") {
                        RunFlowLayout(spacing: 6, lineSpacing: 6) {
                            ForEach(persona.traits, id: \.self) { trait in
                                Text(trait)
                                    .font(Theme.small)
                                    .foregroundStyle(Theme.secondary)
                                    .padding(.horizontal, 8)
                                    .padding(.vertical, 2)
                                    .background(Theme.hover, in: Capsule())
                            }
                        }
                    }
                }
                InspectorSection(title: "Disposition") {
                    meter("Activity", persona.activity, hint: "Chance of acting in a round", step: nil)
                    meter("Influence", persona.influence, hint: "Initial follower weight",
                          step: InfluenceRamp.step(persona.influence) + 1)
                }
                agentSections
            }
            .padding(.bottom, Theme.space4)
        }
        .scrollContentBackground(.hidden)
    }

    private func privateField(_ title: String, _ text: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title)
                .font(Theme.ui(12, .medium))
                .foregroundStyle(Theme.muted)
            Text(text.isEmpty ? "Not set" : text)
                .font(Theme.body)
                .foregroundStyle(text.isEmpty ? Theme.faint : Theme.text)
                .lineSpacing(2)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .combine)
    }

    private func meter(_ title: String, _ value: Double, hint: String, step: Int?) -> some View {
        HStack(spacing: 10) {
            Text(title).font(Theme.body).foregroundStyle(Theme.text).frame(width: 74, alignment: .leading)
            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    Capsule().fill(Theme.seam)
                    Capsule().fill(Theme.muted).frame(width: geo.size.width * min(1, max(0, value)))
                }
            }
            .frame(height: 4)
            Text(value.formatted(.number.precision(.fractionLength(2))))
                .font(Theme.mono(12))
                .foregroundStyle(Theme.text)
            if let step {
                Text("step \(step)/5").font(Theme.mono(10)).foregroundStyle(Theme.faint)
            }
        }
        .help(hint)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(title)
        .accessibilityValue("\(Int((value * 100).rounded())) percent")
    }

    @ViewBuilder
    private var sparkline: some View {
        let series = sentimentSeries
        if series.isEmpty {
            Text("No scored posts yet.")
                .font(Theme.small)
                .foregroundStyle(Theme.muted)
        } else if series.count == 1, let only = series.first {
            // One point is not a trend: say the number.
            HStack(spacing: 6) {
                SentimentValue(score: only.value, size: 13)
                Text("in round \(only.round), the only scored round so far")
                    .font(Theme.small)
                    .foregroundStyle(Theme.muted)
            }
        } else {
            Chart {
                RuleMark(y: .value("Neutral", 0))
                    .foregroundStyle(Theme.seamStrong)
                    .lineStyle(StrokeStyle(lineWidth: 1))
                ForEach(series, id: \.round) { point in
                    LineMark(x: .value("Round", point.round), y: .value("Sentiment", point.value))
                        .foregroundStyle(Theme.secondary)
                        .interpolationMethod(.monotone)
                        .lineStyle(StrokeStyle(lineWidth: 1.5))
                    PointMark(x: .value("Round", point.round), y: .value("Sentiment", point.value))
                        .foregroundStyle(RunUI.sentimentColor(point.value))
                        .symbolSize(30)
                }
                RuleMark(x: .value("Playhead", store.playhead))
                    .foregroundStyle(Theme.accent.opacity(0.5))
                    .lineStyle(StrokeStyle(lineWidth: 1))
            }
            .chartYScale(domain: -1.1...1.1)
            .chartXScale(domain: 0...max(1, store.totalRounds))
            .chartXAxis(.hidden)
            .chartYAxis(.hidden)
            .frame(height: 54)
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("Sentiment by round")
            .accessibilityValue(series.map { "round \($0.round) \(RunUI.sentimentText($0.value))" }.joined(separator: ", "))
            HStack {
                Text("latest").font(Theme.small).foregroundStyle(Theme.muted)
                Spacer()
                SentimentValue(score: series.last?.value, size: 12)
            }
        }
    }

    @ViewBuilder
    private var agentPosts: some View {
        if let agent {
            InspectorSection(title: "Posts", readout: "\(agent.posts.count)") {
                if agent.posts.isEmpty {
                    Text("No posts yet.").font(Theme.small).foregroundStyle(Theme.muted)
                } else {
                    ForEach(agent.posts.sorted { $0.id > $1.id }.prefix(30), id: \.id) { post in
                        Button {
                            store.selectedPostID = post.id
                        } label: {
                            VStack(alignment: .leading, spacing: 3) {
                                HStack {
                                    Text("r\(post.round)  #\(post.id)")
                                        .font(Theme.mono(11))
                                        .foregroundStyle(Theme.muted)
                                    Spacer()
                                    if post.sentiment != nil { SentimentValue(score: post.sentiment, size: 11) }
                                }
                                Text(post.content)
                                    .font(Theme.small)
                                    .foregroundStyle(Theme.text)
                                    .lineLimit(4)
                                    .multilineTextAlignment(.leading)
                            }
                            .padding(8)
                            .frame(maxWidth: .infinity, alignment: .leading)
                        }
                        .buttonStyle(HoverRowStyle())
                        .accessibilityHint("Shows this post in the feed")
                    }
                }
            }
        }
    }

    @ViewBuilder
    private var agentSections: some View {
        if let agent {
            InspectorSection(title: "Memory") {
                if agent.memory.isEmpty {
                    Text("No memory notes yet.").font(Theme.small).foregroundStyle(Theme.muted)
                } else {
                    ForEach(Array(agent.memory.enumerated()).reversed(), id: \.offset) { _, note in
                        Text(note)
                            .font(Theme.small)
                            .foregroundStyle(Theme.text)
                            .textSelection(.enabled)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
            }
            InspectorSection(title: "Followers", readout: "\(agent.followers.count)") {
                handleList(agent.followers)
            }
            InspectorSection(title: "Following", readout: "\(agent.following.count)") {
                handleList(agent.following)
            }
        } else if let loadError {
            InspectorSection(title: "Could not load") {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Image(systemName: "xmark.octagon.fill").foregroundStyle(Theme.negative)
                    Text(loadError).font(Theme.small).foregroundStyle(Theme.negative)
                }
                Button("Try Again", action: retry).buttonStyle(.console)
            }
        } else if loading {
            InspectorSection(title: "Loading") {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Loading memory, posts and followers…").font(Theme.small).foregroundStyle(Theme.muted)
                }
            }
        }
    }

    @ViewBuilder
    private func handleList(_ ids: [String]) -> some View {
        if ids.isEmpty {
            Text("None").font(Theme.small).foregroundStyle(Theme.muted)
        } else {
            RunFlowLayout(spacing: 12, lineSpacing: 5) {
                ForEach(ids, id: \.self) { id in
                    RunHandleChip(handle: handles[id] ?? id, sentiment: nil) {
                        store.selectedAgentID = id
                    }
                }
            }
        }
    }
}

// MARK: - interview

private struct InterviewChatView: View {
    let store: RunStore
    let persona: Persona
    let interviews: InterviewSessions
    @State private var draft = ""
    @FocusState private var focused: Bool

    var body: some View {
        let turns = interviews.history(for: persona.id)
        let waiting = interviews.isWaiting(persona.id)
        VStack(spacing: 0) {
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 10) {
                        Text("Interview @\(persona.handle). They answer in character from their persona, memory and posts.")
                            .font(Theme.small)
                            .foregroundStyle(Theme.muted)
                            .padding(.bottom, 4)
                        ForEach(turns) { turn in
                            bubble(turn.question, fromUser: true)
                            if let answer = turn.answer {
                                bubble(answer, fromUser: false)
                            } else if let error = turn.error {
                                HStack(alignment: .firstTextBaseline, spacing: 6) {
                                    Image(systemName: "xmark.octagon.fill").foregroundStyle(Theme.negative)
                                    Text(error).font(Theme.small).foregroundStyle(Theme.negative)
                                }
                            }
                        }
                        if waiting {
                            TypingIndicator()
                                .accessibilityLabel("@\(persona.handle) is typing")
                        }
                        Color.clear.frame(height: 1).id("bottom")
                    }
                    .padding(12)
                }
                .scrollContentBackground(.hidden)
                .onChange(of: turns.count) { _, _ in withAnimation { proxy.scrollTo("bottom") } }
                .onChange(of: waiting) { _, _ in withAnimation { proxy.scrollTo("bottom") } }
            }
            Seam()
            HStack(alignment: .bottom, spacing: 8) {
                TextField("Ask @\(persona.handle)…", text: $draft, axis: .vertical)
                    .lineLimit(1...5)
                    .consoleField()
                    .focused($focused)
                    .onSubmit(send)
                    .accessibilityLabel("Message to @\(persona.handle)")
                Button(action: send) {
                    Image(systemName: "arrow.up")
                        .font(.system(size: 12, weight: .bold))
                        .frame(width: 18)
                }
                .buttonStyle(.consolePrimary)
                .disabled(waiting || draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                .accessibilityLabel("Send")
                .help("Send (Return)")
            }
            .padding(10)
            .background(Theme.card)
        }
        .onAppear { focused = true }
    }

    private func send() {
        let text = draft
        guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty, !interviews.isWaiting(persona.id) else { return }
        draft = ""
        Task { await interviews.send(text, to: persona.id, store: store) }
    }

    /// Your questions are blue-bordered (replies are blue); the agent's answers sit on the elevated ground.
    private func bubble(_ text: String, fromUser: Bool) -> some View {
        HStack {
            if fromUser { Spacer(minLength: 30) }
            VStack(alignment: .leading, spacing: 3) {
                Text(fromUser ? "You" : "@\(persona.handle)")
                    .font(Theme.ui(11, .medium))
                    .foregroundStyle(fromUser ? Theme.link : Theme.muted)
                Text(text)
                    .font(Theme.body)
                    .foregroundStyle(Theme.text)
                    .lineSpacing(2)
                    .textSelection(.enabled)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 7)
            .background(fromUser ? Theme.accentSoft : Theme.hover, in: RoundedRectangle(cornerRadius: Theme.cardRadius))
            .fixedSize(horizontal: false, vertical: true)
            .accessibilityElement(children: .combine)
            .accessibilityLabel(fromUser ? "You: \(text)" : "@\(persona.handle): \(text)")
            if !fromUser { Spacer(minLength: 30) }
        }
    }
}

private struct TypingIndicator: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        TimelineView(.periodic(from: .now, by: 0.35)) { context in
            let phase = Int(context.date.timeIntervalSinceReferenceDate / 0.35) % 3
            HStack(spacing: 4) {
                ForEach(0..<3, id: \.self) { i in
                    Circle()
                        .frame(width: 5, height: 5)
                        .opacity(reduceMotion ? 0.6 : (i == phase ? 0.9 : 0.3))
                }
            }
            .foregroundStyle(Theme.muted)
            .padding(.horizontal, 12)
            .padding(.vertical, 10)
            .background(Theme.hover, in: RoundedRectangle(cornerRadius: Theme.cardRadius))
        }
    }
}
