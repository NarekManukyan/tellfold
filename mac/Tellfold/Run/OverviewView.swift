import Charts
import SwiftUI

/// The default tab: the answer first (verdict, confidence, reasons), then the numbers, the crowd, the sentiment over
/// the rounds, the camps and the key voices. A calm column that scrolls, centred, at most 1120 wide.
struct OverviewView: View {
    let store: RunStore
    let replay: ReplayController
    @Bindable var crowd: CrowdModel

    @Environment(\.runNavigate) private var navigate
    @State private var width: CGFloat = 1000
    /// Width of the crowd canvas, measured so its height can follow it (see `crowdHeight(forWidth:)`).
    @State private var crowdWidth: CGFloat = 443

    private static let sideBySide: CGFloat = 860
    /// Height of the strip the floating view switch covers at the top of the Overview crowd graph.
    static let switchStrip: CGFloat = 22

    /// Height of the part of the canvas the picture is framed in. The camp labels keep their size on screen, so a
    /// narrow canvas needs height for the picture to keep a scale at which the smallest avatar (22 pt at scale 1) is
    /// still about 20 pt across. Wider canvases need less.
    static func crowdHeight(forWidth width: CGFloat) -> CGFloat {
        switch width {
        case ..<540: 580
        case ..<640: 500
        case ..<780: 420
        default: 360
        }
    }

    var body: some View {
        ScrollView {
            VStack(spacing: Theme.space4) {
                SummaryCard(store: store)
                MetricRow(store: store, narrow: width < 700)
                if !store.personas.isEmpty || !store.stats.isEmpty {
                    if width >= Self.sideBySide {
                        HStack(alignment: .top, spacing: Theme.space4) {
                            crowdCard
                            SentimentByRoundCard(store: store, replay: replay)
                        }
                    } else {
                        VStack(spacing: Theme.space4) {
                            crowdCard
                            SentimentByRoundCard(store: store, replay: replay)
                        }
                    }
                }
                if !store.personas.isEmpty { CampsCard(store: store) }
                KeyVoicesCard(store: store)
            }
            .frame(maxWidth: 1120)
            .padding(.horizontal, width < 760 ? Theme.space4 : Theme.space6)
            .padding(.vertical, Theme.space5)
            .frame(maxWidth: .infinity)
        }
        .background(Theme.ground)
        .onGeometryChange(for: CGFloat.self, of: { $0.size.width }) { width = $0 }
    }

    // MARK: crowd card

    private var crowdCard: some View {
        VStack(alignment: .leading, spacing: Theme.space3) {
            SectionTitle("Crowd") {
                Button("Open") { navigate(.crowd) }
                    .buttonStyle(.console(.secondary, compact: true))
                    .fixedSize()
                    .help("Open the crowd in full")
            }
            // The graph runs full-bleed; the view switch floats over it on Liquid Glass and the picture is framed below it.
            CrowdGraphView(store: store, model: crowd, compact: true, insets: CrowdInsets(top: Self.switchStrip, bottom: 0)) { agent, camp in
                if let agent { store.selectedAgentID = agent }
                if let camp { store.selectedAgentID = nil; store.selectedCampID = camp }
                navigate(.crowd)
            }
            .overlay(alignment: .topTrailing) {
                Picker("View", selection: $crowd.mode) {
                    ForEach(CrowdMode.allCases) { Text($0.title).tag($0) }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .controlSize(.small)
                .fixedSize()
                .accessibilityLabel("Crowd view")
                .padding(.horizontal, 4)
                .frame(height: 28)
                .glassSurface(in: .capsule)
                .padding(6)
            }
            .frame(height: Self.crowdHeight(forWidth: crowdWidth) + Self.switchStrip)
            .onGeometryChange(for: CGFloat.self, of: { $0.size.width }) { crowdWidth = $0 }
            Text("Avatars are simulated people and tiles are organisations. Size is influence. Click one to open it.")
                .font(Theme.caption)
                .foregroundStyle(Theme.muted)
        }
        .card()
        .frame(maxWidth: .infinity)
    }
}

// MARK: - summary

/// The verdict, or while the run is going its progress, or what happened when it stopped.
private struct SummaryCard: View {
    let store: RunStore
    @Environment(\.runNavigate) private var navigate
    @State private var confirmStop = false
    @State private var stopping = false
    @State private var rebuilding = false

    private var state: RunState { store.status?.state ?? .unknown }
    private var stage: RunStage { store.status?.stage ?? .unknown }
    private var linkifier: RunLinkifier {
        RunLinkifier(personas: store.personas, postIDs: Set(store.posts.map(\.id)))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: Theme.space3) {
            HStack(spacing: Theme.space2) {
                StatusChip(state: state)
                if let report = store.report, store.isLive == false { ConfidenceBadge(confidence: report.confidence) }
            }
            if !store.question.isEmpty {
                Text(store.question)
                    .font(Theme.body)
                    .foregroundStyle(Theme.muted)
                    .lineLimit(3)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if store.isLive || state == .queued {
                progress
            } else {
                finished
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .card(padding: Theme.space5)
        .confirmationDialog("Stop this simulation?", isPresented: $confirmStop) {
            Button("Stop Simulation", role: .destructive) { Task { stopping = true; await store.stop(); stopping = false } }
        } message: {
            Text("The current round finishes, then the run stops. A report is still written if at least one round completed.")
        }
    }

    // MARK: running

    private var progress: some View {
        let status = store.status
        let round = store.roundsCompleted
        let total = status?.totalRounds ?? store.totalRounds
        let title = state == .queued ? "Waiting to start" : RunProgress.stageTitle(stage: stage, round: round, totalRounds: total)
        return VStack(alignment: .leading, spacing: Theme.space3) {
            Text(title)
                .font(Theme.title)
                .foregroundStyle(Theme.text)
                .accessibilityAddTraits(.isHeader)
            ProgressView(value: RunProgress.fraction(stage: stage, round: round, totalRounds: total))
                .progressViewStyle(.linear)
                .tint(Theme.accent)
                .frame(maxWidth: 560)
                .accessibilityLabel("Run progress")
            if !store.stageDetail.isEmpty {
                Text(store.stageDetail)
                    .font(Theme.small)
                    .foregroundStyle(Theme.muted)
                    .lineLimit(2)
            }
            Text("The answer appears here when the run finishes.")
                .font(Theme.body)
                .foregroundStyle(Theme.secondary)
            HStack(spacing: Theme.space2) {
                Button {
                    confirmStop = true
                } label: {
                    if stopping { ProgressView().controlSize(.small) } else { Label("Stop", systemImage: "stop.fill") }
                }
                .buttonStyle(.console(.secondary))
                .disabled(stopping)
                Button {
                    store.showsInjectEvent = true
                } label: {
                    Label("Inject event", systemImage: "bolt")
                }
                .buttonStyle(.console(.event))
                .disabled(store.latestRound >= store.totalRounds)
            }
        }
    }

    // MARK: finished, stopped, failed

    @ViewBuilder
    private var finished: some View {
        if let report = store.report {
            verdict(report)
        } else {
            noReport
        }
    }

    private func verdict(_ report: Report) -> some View {
        VStack(alignment: .leading, spacing: Theme.space3) {
            if state == .failed || state == .stopped { outcomeSentence }
            Text(report.title)
                .font(Theme.display)
                .foregroundStyle(Theme.text)
                .lineSpacing(2)
                .fixedSize(horizontal: false, vertical: true)
                .textSelection(.enabled)
                .accessibilityAddTraits(.isHeader)
            if report.reasons.isEmpty {
                if !report.summary.isEmpty {
                    Text(linkifier.inline(report.summary, size: 15))
                        .font(Theme.ui(15))
                        .foregroundStyle(Theme.secondary)
                        .lineSpacing(4)
                        .lineLimit(4)
                        .fixedSize(horizontal: false, vertical: true)
                        .frame(maxWidth: 760, alignment: .leading)
                }
            } else {
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(Array(report.reasons.enumerated()), id: \.offset) { _, reason in
                        HStack(alignment: .firstTextBaseline, spacing: 10) {
                            Circle().fill(Theme.accent).frame(width: 5, height: 5).offset(y: -2)
                            Text(linkifier.inline(reason, size: 14))
                                .font(Theme.ui(14))
                                .foregroundStyle(Theme.text)
                                .lineSpacing(3)
                                .fixedSize(horizontal: false, vertical: true)
                                .textSelection(.enabled)
                        }
                    }
                }
                .frame(maxWidth: 820, alignment: .leading)
            }
            HStack(spacing: Theme.space2) {
                Button("Read full report") { navigate(.report) }
                    .buttonStyle(.consolePrimary)
                AnotherVersionButton(store: store)
                RunAgainButton(store: store)
                CheckItButton(store: store)
                Text("A scenario exploration of a simulated crowd, not a poll of real people.")
                    .font(Theme.caption)
                    .foregroundStyle(Theme.muted)
            }
            .padding(.top, 4)
        }
    }

    /// No report: why, in one sentence, and what can be done.
    private var noReport: some View {
        VStack(alignment: .leading, spacing: Theme.space3) {
            outcomeSentence
            if store.report == nil, state == .done || state == .stopped || state == .failed {
                HStack(spacing: Theme.space2) {
                    Button {
                        Task { rebuilding = true; await ReportRebuilder.rebuild(store); rebuilding = false }
                    } label: {
                        if rebuilding || store.detail?.reportRebuild.running == true {
                            HStack(spacing: 6) { ProgressView().controlSize(.small); Text("Writing the report…") }
                        } else {
                            Text("Build report")
                        }
                    }
                    .buttonStyle(.consolePrimary)
                    .disabled(!store.canRebuildReport || rebuilding || store.detail?.reportRebuild.running == true)
                    RunAgainButton(store: store)
                    if store.latestRound == 0 {
                        Text("A report needs at least one finished round.")
                            .font(Theme.small)
                            .foregroundStyle(Theme.muted)
                    }
                }
            }
        }
    }

    private var outcomeSentence: some View {
        let status = store.status
        let total = status?.totalRounds ?? store.totalRounds
        return VStack(alignment: .leading, spacing: 6) {
            switch state {
            case .failed:
                Text(failureSentence(total: total))
                    .font(Theme.title)
                    .foregroundStyle(Theme.text)
                if let error = status?.error, !error.isEmpty {
                    Text(error)
                        .font(Theme.mono(12))
                        .foregroundStyle(Theme.negative)
                        .textSelection(.enabled)
                        .lineLimit(4)
                }
                Text(partialNote)
                    .font(Theme.body)
                    .foregroundStyle(Theme.muted)
            case .stopped:
                Text(stoppedSentence(total: total))
                    .font(Theme.title)
                    .foregroundStyle(Theme.text)
                // The headline already says the report covers every round; otherwise say what the report covers.
                if store.report == nil || store.roundsCompleted < total {
                    Text(store.report == nil ? "No report was written. \(partialNote)" : "The report was written from the rounds that finished.")
                        .font(Theme.body)
                        .foregroundStyle(Theme.muted)
                }
            default:
                Text("This run finished without a report.")
                    .font(Theme.title)
                    .foregroundStyle(Theme.text)
            }
        }
    }

    /// Where the stop landed, from the recorded rounds and whether a report exists.
    private func stoppedSentence(total: Int) -> String {
        let done = store.roundsCompleted
        if total > 0, done >= total {
            return store.report == nil
                ? "Stopped after all \(total) rounds, before the report was written."
                : "Stopped after all \(total) rounds; the report was written from them."
        }
        return "Stopped after round \(done) of \(total)."
    }

    /// One sentence about where it failed, from the rounds actually recorded (the status can lag the data).
    private func failureSentence(total: Int) -> String {
        let done = store.roundsCompleted
        if stage == .rounds || stage == .report {
            if done >= total, total > 0 { return "The run failed after round \(done) of \(total), while writing the report." }
            if done > 0 { return "The run failed after round \(done) of \(total)." }
            return "The run failed before round 1 finished."
        }
        return "The run failed while \(RunProgress.stageTitle(stage: stage, round: done, totalRounds: total).lowercased())."
    }

    private var partialNote: String {
        store.posts.isEmpty && store.personas.isEmpty
            ? "Nothing was recorded before it stopped."
            : "What was recorded before it stopped is shown below."
    }
}

// MARK: - metrics

private struct MetricRow: View {
    let store: RunStore
    let narrow: Bool

    var body: some View {
        let m = store.metrics
        let events = store.events
        LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: Theme.space4), count: narrow ? 2 : 4),
                  spacing: Theme.space4) {
            tile("People", value: { Text("\(m.people)") },
                 note: peopleNote(m))
            tile("Rounds", value: {
                HStack(alignment: .firstTextBaseline, spacing: 4) {
                    Text("\(m.roundsDone)")
                    Text("/ \(m.totalRounds)").font(Theme.ui(16, .medium)).foregroundStyle(Theme.muted)
                }
            }, note: roundsNote(events))
            tile("Posts", value: { Text("\(m.posts)") },
                 note: m.posts == 0 ? "None yet" : "\(m.postsPerRound.formatted(.number.precision(.fractionLength(0...1)))) per round on average")
            tile("Final sentiment", value: {
                Text(RunUI.sentimentText(m.finalSentiment))
                    .foregroundStyle(m.finalSentiment == nil ? Theme.faint : RunUI.sentimentColor(m.finalSentiment))
            }, note: sentimentNote(m))
        }
    }

    private func peopleNote(_ m: RunMetrics) -> String {
        if m.people == 0 { return "None yet" }
        let people = "\(m.peopleOnly) \(m.peopleOnly == 1 ? "person" : "people")"
        if m.organisations == 0 { return people == "\(m.people) people" ? "Simulated people" : people }
        return "\(people), \(m.organisations) \(m.organisations == 1 ? "organisation" : "organisations")"
    }

    private func roundsNote(_ events: [GodEvent]) -> String {
        var parts = ["r0 baseline"]
        if let first = events.first { parts.append("event at r\(first.round)" + (events.count > 1 ? " and \(events.count - 1) more" : "")) }
        return parts.joined(separator: ", ")
    }

    private func sentimentNote(_ m: RunMetrics) -> String {
        guard m.finalSentiment != nil else { return "No scored posts yet" }
        guard let delta = m.delta else { return "No opening score to compare" }
        return "\(RunUI.sentimentText(delta)) since r0"
    }

    private func tile<Value: View>(_ title: String, @ViewBuilder value: () -> Value, note: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title).font(Theme.body).foregroundStyle(Theme.muted)
            value()
                .font(Theme.ui(28, .semibold))
                .monospacedDigit()
                .foregroundStyle(Theme.text)
                .lineLimit(1)
                .minimumScaleFactor(0.7)
            Text(note).font(Theme.small).foregroundStyle(Theme.muted).lineLimit(2)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .card()
        .accessibilityElement(children: .combine)
    }
}

// MARK: - sentiment by round

private struct SentimentByRoundCard: View {
    let store: RunStore
    let replay: ReplayController
    @State private var hoverRound: Int?

    private struct Point: Identifiable {
        let round: Int
        let value: Double
        var id: Int { round }
    }

    private var points: [Point] {
        store.stats.sorted { $0.round < $1.round }
            .compactMap { s in RunUI.roundSentiment(s.round, in: store).map { Point(round: s.round, value: $0) } }
    }

    private var maxRound: Int { max(store.totalRounds, store.latestRound, 1) }

    var body: some View {
        let points = points
        VStack(alignment: .leading, spacing: Theme.space3) {
            SectionTitle("Sentiment by round") {
                Text("mean, −1 to +1").font(Theme.small).foregroundStyle(Theme.muted)
            }
            if points.isEmpty {
                VStack(spacing: 8) {
                    if store.isLive { ProgressView().controlSize(.small) }
                    Text(store.isLive ? "The chart fills in as rounds complete." : "This run has no per-round sentiment.")
                        .font(Theme.body)
                        .foregroundStyle(Theme.muted)
                }
                .frame(maxWidth: .infinity, minHeight: 320)
            } else {
                chart(points)
                    .frame(minHeight: 320, maxHeight: .infinity)
            }
            hoverNote
        }
        .card()
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
    }

    private func chart(_ points: [Point]) -> some View {
        let events = store.events
        return Chart {
            RuleMark(y: .value("Neutral", 0))
                .foregroundStyle(Theme.seamStrong)
                .lineStyle(StrokeStyle(lineWidth: 1))
            ForEach(events, id: \.round) { event in
                RuleMark(x: .value("Round", event.round))
                    .foregroundStyle(Theme.event.opacity(0.55))
                    .lineStyle(StrokeStyle(lineWidth: 1, dash: [3, 3]))
                PointMark(x: .value("Round", event.round), y: .value("Event", 1.0))
                    .symbol(.diamond)
                    .symbolSize(46)
                    .foregroundStyle(Theme.event)
                    .accessibilityLabel("Event in round \(event.round)")
                    .accessibilityValue(event.content)
            }
            RuleMark(x: .value("Playhead", store.playhead))
                .foregroundStyle(Theme.accent.opacity(0.35))
                .lineStyle(StrokeStyle(lineWidth: 2))
            ForEach(points) { p in
                LineMark(x: .value("Round", p.round), y: .value("Sentiment", p.value))
                    .foregroundStyle(Theme.secondary)
                    .lineStyle(StrokeStyle(lineWidth: 1.5))
                    .interpolationMethod(.monotone)
            }
            ForEach(points) { p in
                PointMark(x: .value("Round", p.round), y: .value("Sentiment", p.value))
                    .foregroundStyle(RunUI.sentimentColor(p.value))
                    .symbolSize(p.round == store.playhead || p.round == hoverRound ? 110 : 56)
                    .accessibilityLabel("Round \(p.round)")
                    .accessibilityValue("Sentiment \(RunUI.sentimentText(p.value))")
                    .annotation(position: p.value < 0 ? .bottom : .top, spacing: 4) {
                        let emphasised = p.round == hoverRound || p.round == store.playhead
                        SentimentValue(score: p.value, size: emphasised ? 11 : 10, weight: emphasised ? .bold : .regular)
                    }
            }
        }
        .chartXScale(domain: -0.4...(Double(maxRound) + 0.4))
        .chartYScale(domain: -1.12...1.12)
        .chartXAxis {
            AxisMarks(values: Array(0...maxRound)) { value in
                AxisValueLabel {
                    if let r = value.as(Int.self) {
                        Text("r\(r)")
                            .font(Theme.mono(10, r == store.playhead ? .bold : .regular))
                            .foregroundStyle(r == store.playhead ? Theme.accent : Theme.muted)
                    }
                }
            }
        }
        .chartYAxis {
            AxisMarks(position: .leading, values: [-1, -0.5, 0, 0.5, 1]) { value in
                AxisGridLine().foregroundStyle(Theme.seam)
                AxisValueLabel {
                    if let v = value.as(Double.self) {
                        Text(v == 0 ? "0" : (v > 0 ? "+" : "\u{2212}") + (abs(v) == 1 ? "1" : "0.5"))
                            .font(Theme.mono(10))
                            .foregroundStyle(Theme.muted)
                    }
                }
            }
        }
        .chartOverlay { proxy in
            GeometryReader { geo in
                let plot = proxy.plotFrame.map { geo[$0] } ?? CGRect(origin: .zero, size: geo.size)
                Rectangle()
                    .fill(.clear)
                    .contentShape(Rectangle())
                    .onContinuousHover { phase in
                        switch phase {
                        case .active(let location): hoverRound = round(at: location.x - plot.minX, proxy: proxy)
                        case .ended: hoverRound = nil
                        }
                    }
                    .onTapGesture(coordinateSpace: .local) { location in
                        if let r = round(at: location.x - plot.minX, proxy: proxy) { replay.scrub(to: r) }
                    }
            }
        }
        .accessibilityLabel("Mean sentiment by round, from −1 to +1")
    }

    private func round(at x: CGFloat, proxy: ChartProxy) -> Int? {
        guard let value: Double = proxy.value(atX: x) else { return nil }
        return min(maxRound, max(0, Int(value.rounded())))
    }

    /// What the hovered round says, including the injected event.
    @ViewBuilder
    private var hoverNote: some View {
        let round = hoverRound ?? store.playhead
        let event = store.events.first { $0.round == round }
        if let event {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Image(systemName: "bolt.fill").font(.system(size: 10)).foregroundStyle(Theme.event)
                Text("\(Text("Event at r\(event.round): ").font(Theme.small.weight(.semibold)).foregroundStyle(Theme.text))\(Text(event.content.replacingOccurrences(of: "\n", with: " ")).font(Theme.small).foregroundStyle(Theme.secondary))")
            }
            .lineLimit(3)
            .padding(10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Theme.eventSoft, in: RoundedRectangle(cornerRadius: Theme.radius))
            .accessibilityElement(children: .combine)
        } else {
            Text("Click a round to move the crowd there.")
                .font(Theme.caption)
                .foregroundStyle(Theme.muted)
        }
    }
}

// MARK: - camps

private struct CampsCard: View {
    let store: RunStore
    @Environment(\.runNavigate) private var navigate

    var body: some View {
        let grouping = CampGrouping.groups(report: store.report, personas: store.personas)
        let byID = store.personaByID
        VStack(alignment: .leading, spacing: Theme.space3) {
            SectionTitle("Camps") {
                Text(grouping.isFallback ? "\(grouping.groups.count) groups" : "\(grouping.groups.count) camps by opinion")
                    .font(Theme.small).foregroundStyle(Theme.muted)
            }
            VStack(spacing: 2) {
                ForEach(grouping.groups) { group in
                    CampRow(group: group, members: group.members.compactMap { byID[$0] },
                            sentiment: store.campSentiment(group, at: store.latestRound), showsSummary: !grouping.isFallback) {
                        store.selectedAgentID = nil
                        store.selectedCampID = group.id
                        navigate(.crowd)
                    }
                }
            }
            if grouping.isFallback {
                Seam()
                CampsFallbackNote(store: store)
            }
        }
        .card()
    }
}

private struct CampRow: View {
    let group: CampGroup
    let members: [Persona]
    let sentiment: Double?
    let showsSummary: Bool
    let open: () -> Void

    var body: some View {
        Button(action: open) {
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: Theme.space3) {
                    Circle().fill(group.color).frame(width: 10, height: 10)
                    Text(group.name).font(Theme.ui(14, .semibold)).foregroundStyle(Theme.text).lineLimit(1)
                    AvatarStack(personas: members)
                    Text("\(members.count) \(members.count == 1 ? "person" : "people")")
                        .font(Theme.small)
                        .foregroundStyle(Theme.muted)
                    Spacer(minLength: Theme.space2)
                    CampSentimentValue(score: sentiment, size: 13)
                    Image(systemName: "chevron.right").font(.system(size: 10, weight: .semibold)).foregroundStyle(Theme.faint)
                }
                if showsSummary, !group.summary.isEmpty {
                    Text(group.summary)
                        .font(Theme.body)
                        .foregroundStyle(Theme.secondary)
                        .lineLimit(2)
                        .multilineTextAlignment(.leading)
                        .padding(.leading, 22)
                }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .buttonStyle(HoverRowStyle())
        .accessibilityLabel("\(group.name), \(members.count) people")
        .accessibilityValue(sentiment.map { "mean sentiment \(RunUI.sentimentText($0))" } ?? "no sentiment")
        .accessibilityHint("Shows this camp in the crowd")
    }
}

// MARK: - key voices

/// A voice the report names, with the post that best stands for it.
struct KeyVoice: Identifiable {
    let persona: Persona
    let post: PostOut
    var id: String { persona.id }
}

enum KeyVoices {
    /// For each persona the report names (up to `limit`), their most engaging original post.
    static func pick(report: Report?, personas: [Persona], posts: [PostOut], limit: Int = 5) -> [KeyVoice] {
        guard let report else { return [] }
        var out: [KeyVoice] = []
        for ref in report.keyVoices {
            guard out.count < limit else { break }
            guard let persona = personas.first(where: { $0.id == ref })
                ?? personas.first(where: { $0.handle.caseInsensitiveCompare(ref) == .orderedSame }),
                !out.contains(where: { $0.persona.id == persona.id }) else { continue }
            let mine = posts.filter { $0.authorID == persona.id && !$0.content.isEmpty }
            let original = mine.filter { !$0.isRepost }
            let best = (original.isEmpty ? mine : original).max { a, b in
                let ea = a.likes + a.reposts + a.replies, eb = b.likes + b.reposts + b.replies
                if ea != eb { return ea < eb }
                let sa = abs(a.sentiment ?? 0), sb = abs(b.sentiment ?? 0)
                if sa != sb { return sa < sb }
                return a.id > b.id
            }
            if let best { out.append(KeyVoice(persona: persona, post: best)) }
        }
        return out
    }
}

private struct KeyVoicesCard: View {
    let store: RunStore
    @Environment(\.runNavigate) private var navigate

    var body: some View {
        let voices = KeyVoices.pick(report: store.report, personas: store.personas, posts: store.posts)
        if !voices.isEmpty {
            VStack(alignment: .leading, spacing: Theme.space3) {
                SectionTitle("Key voices") {
                    Text("\(voices.count) of \(store.posts.count) posts").font(Theme.small).foregroundStyle(Theme.muted)
                }
                VStack(spacing: 0) {
                    ForEach(Array(voices.enumerated()), id: \.element.id) { i, voice in
                        if i > 0 { Seam().padding(.vertical, Theme.space3) }
                        row(voice)
                    }
                }
            }
            .card()
        }
    }

    private func row(_ voice: KeyVoice) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: Theme.space3) {
                Button {
                    store.selectedAgentID = voice.persona.id
                    navigate(.crowd)
                } label: {
                    HStack(spacing: Theme.space2) {
                        AvatarView(persona: voice.persona, size: 32)
                        VStack(alignment: .leading, spacing: 1) {
                            Text(voice.persona.name).font(Theme.ui(13, .semibold)).foregroundStyle(Theme.text)
                            Text("@\(voice.persona.handle)").font(Theme.mono(11)).foregroundStyle(Theme.muted)
                        }
                    }
                }
                .buttonStyle(.plain)
                .help("Show \(voice.persona.name) in the crowd")
                Spacer(minLength: Theme.space2)
                HStack(spacing: 8) {
                    if let url = RunUI.postURL(voice.post.id) {
                        Link("#\(voice.post.id)", destination: url)
                            .font(Theme.mono(11))
                            .foregroundStyle(Theme.link)
                    }
                    Text("r\(voice.post.round)").font(Theme.mono(11)).foregroundStyle(Theme.muted)
                    if voice.post.sentiment != nil { SentimentValue(score: voice.post.sentiment, size: 11) }
                }
            }
            Text("“\(voice.post.content)”")
                .font(Theme.ui(14))
                .foregroundStyle(Theme.text)
                .lineSpacing(3)
                .fixedSize(horizontal: false, vertical: true)
                .textSelection(.enabled)
                .frame(maxWidth: 760, alignment: .leading)
        }
        .accessibilityElement(children: .contain)
    }
}
