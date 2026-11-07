import SwiftUI

/// The machinery, honestly but out of the way: pipeline stages and timings, provenance, counts, failed batches, the
/// actions that change the run, the run log and the knowledge graph.
struct DetailsView: View {
    let store: RunStore

    @Environment(AppModel.self) private var app
    @Environment(\.runNavigate) private var navigate
    @State private var confirmStop = false
    @State private var rebuilding = false

    private var status: RunStatus? { store.status }
    private var state: RunState { status?.state ?? .unknown }

    var body: some View {
        ScrollView {
            VStack(spacing: Theme.space4) {
                pipelineCard
                if !store.stats.isEmpty { roundsCard }
                provenanceCard
                if !store.batchFailures.isEmpty { failuresCard }
                actionsCard
                logCard
                graphCard
            }
            .frame(maxWidth: 980)
            .padding(Theme.space5)
            .frame(maxWidth: .infinity)
        }
        .background(Theme.ground)
        .confirmationDialog("Stop this simulation?", isPresented: $confirmStop) {
            Button("Stop Simulation", role: .destructive) { Task { await store.stop() } }
        } message: {
            Text("The current round finishes, then the run stops. A report is still written if at least one round completed.")
        }
    }

    // MARK: pipeline

    private enum Phase { case done, active, failed, halted, pending }

    private func phase(_ index: Int) -> Phase {
        let current = RunUI.stageIndex((status?.stage ?? .reading).rawValue)
        if index < current { return .done }
        if index > current { return .pending }
        switch state {
        case .running, .queued: return .active
        case .failed: return .failed
        case .done: return .done
        default: return .halted
        }
    }

    private var pipelineCard: some View {
        let calls = store.detail?.calls
        return VStack(alignment: .leading, spacing: Theme.space3) {
            SectionTitle("Pipeline") {
                if let elapsed = elapsed { Text("\(elapsed) elapsed").font(Theme.small).foregroundStyle(Theme.muted) }
            }
            VStack(spacing: 0) {
                ForEach(Array(RunStage.pipeline.enumerated()), id: \.offset) { i, stage in
                    if i > 0 { Seam() }
                    stageRow(stage, phase: phase(i), calls: calls)
                }
            }
            if let error = status?.error, !error.isEmpty, state == .failed {
                Label(error, systemImage: "xmark.octagon.fill")
                    .font(Theme.mono(11))
                    .foregroundStyle(Theme.negative)
                    .textSelection(.enabled)
            } else if store.isLive, !store.stageDetail.isEmpty, store.stageDetail != "no status file (older run)" {
                Text(store.stageDetail.prefix(1).uppercased() + store.stageDetail.dropFirst())
                    .font(Theme.small).foregroundStyle(Theme.muted).lineLimit(2)
            } else if store.stageDetail == "no status file (older run)" {
                Text("Older run: it has no status file, so live progress is unknown.")
                    .font(Theme.small).foregroundStyle(Theme.muted)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .card()
    }

    private func stageRow(_ stage: RunStage, phase: Phase, calls: CallsSummary?) -> some View {
        let symbol: String, tint: Color, word: String
        switch phase {
        case .done: (symbol, tint, word) = ("checkmark.circle.fill", Theme.positive, "Done")
        case .active: (symbol, tint, word) = ("circle.dotted", Theme.accent, "In progress")
        case .failed: (symbol, tint, word) = ("exclamationmark.triangle.fill", Theme.negative, "Failed here")
        case .halted: (symbol, tint, word) = ("stop.circle.fill", Theme.muted, "Stopped here")
        case .pending: (symbol, tint, word) = ("circle", Theme.faint, "Not reached")
        }
        var detail = word
        if stage == .rounds, phase != .pending {
            let seconds = store.stats.reduce(0.0) { $0 + $1.llmSeconds }
            detail = "\(store.latestRound) of \(store.totalRounds) rounds"
                + (seconds > 0 ? ", \(RunProvenance.duration(Int(seconds.rounded()))) of model time" : "")
        } else if stage == .personas, phase != .pending, !store.personas.isEmpty {
            detail = "\(store.personas.count) people"
        } else if stage == .graph, phase != .pending, let g = store.graph, !g.entities.isEmpty {
            detail = "\(g.entities.count) entities, \(g.relations.count) relations"
        }
        return HStack(spacing: Theme.space3) {
            Image(systemName: symbol).font(.system(size: 13)).foregroundStyle(tint).frame(width: 18)
                .symbolEffect(.pulse, options: .repeating, isActive: phase == .active)
            Text(stage.title).font(Theme.ui(13, phase == .active ? .semibold : .regular)).foregroundStyle(Theme.text)
            Spacer()
            Text(detail).font(Theme.small).foregroundStyle(Theme.muted)
        }
        .padding(.vertical, 9)
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(stage.title): \(detail)")
    }

    private var elapsed: String? {
        guard let start = status?.startedDate, let end = status?.updatedDate, end > start else { return nil }
        return RunProvenance.duration(Int(end.timeIntervalSince(start)))
    }

    // MARK: rounds

    private var roundsCard: some View {
        VStack(alignment: .leading, spacing: Theme.space3) {
            SectionTitle("Rounds")
            Grid(alignment: .trailing, horizontalSpacing: Theme.space5, verticalSpacing: 6) {
                GridRow {
                    header("Round", leading: true); header("Posts"); header("Active"); header("Actions")
                    header("Model calls"); header("Model time"); header("Sentiment")
                }
                Divider().gridCellUnsizedAxes(.horizontal)
                ForEach(store.stats.sorted { $0.round < $1.round }) { s in
                    GridRow {
                        Text(s.round == 0 ? "r0" : "r\(s.round)").frame(maxWidth: .infinity, alignment: .leading)
                        Text("\(s.newPosts)")
                        Text("\(s.activeAgents)")
                        Text("\(s.actions.filter { $0.key != "nothing" }.values.reduce(0, +))")
                        Text("\(s.llmCalls)")
                        Text(s.llmSeconds > 0 ? RunProvenance.duration(Int(s.llmSeconds.rounded())) : "–")
                        SentimentValue(score: RunUI.roundSentiment(s.round, in: store), size: 12, weight: .regular)
                    }
                    .font(Theme.mono(12))
                    .foregroundStyle(Theme.text)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .card()
    }

    private func header(_ text: String, leading: Bool = false) -> some View {
        Text(text)
            .font(Theme.ui(12, .medium))
            .foregroundStyle(Theme.muted)
            .frame(maxWidth: leading ? .infinity : nil, alignment: leading ? .leading : .trailing)
    }

    // MARK: provenance and counts

    private var provenanceCard: some View {
        let calls = store.detail?.calls
        return VStack(alignment: .leading, spacing: Theme.space3) {
            SectionTitle("Provenance and counts")
            if let config = store.config {
                Text(RunProvenance.text(config))
                    .font(Theme.mono(12))
                    .foregroundStyle(Theme.secondary)
                    .textSelection(.enabled)
                    .help("Where this run executed, and the runtimes and models it used")
                    .accessibilityLabel("Run provenance: \(RunProvenance.text(config))")
            }
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 130), spacing: Theme.space4, alignment: .leading)],
                      alignment: .leading, spacing: Theme.space3) {
                stat("\(store.personas.count)", "agents")
                stat("\(store.posts.count)", "posts")
                stat("\(store.followEdges.count)", "follows")
                if let calls, calls.count > 0 {
                    stat("\(calls.count)", "model calls")
                    if calls.failed > 0 { stat("\(calls.failed)", "failed calls") }
                    stat(RunProvenance.duration(Int(calls.seconds.rounded())), "model time")
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .card()
    }

    private func stat(_ value: String, _ label: String) -> some View {
        VStack(alignment: .leading, spacing: 1) {
            Text(value).font(Theme.mono(18, .semibold)).foregroundStyle(Theme.text)
            Text(label).font(Theme.small).foregroundStyle(Theme.muted)
        }
        .accessibilityElement(children: .combine)
    }

    // MARK: failures

    private var failuresCard: some View {
        VStack(alignment: .leading, spacing: Theme.space3) {
            SectionTitle("Failed model batches") {
                Text("\(store.batchFailures.count)").font(Theme.small).foregroundStyle(Theme.negative)
            }
            Text("The run went on without these agents' turn in those rounds.")
                .font(Theme.small).foregroundStyle(Theme.muted)
            VStack(alignment: .leading, spacing: 6) {
                ForEach(Array(store.batchFailures.enumerated()), id: \.offset) { _, f in
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Text("r\(f.round)").font(Theme.mono(12, .medium)).foregroundStyle(Theme.text)
                        Text(RunLog.failureText(f)).font(Theme.mono(12)).foregroundStyle(Theme.negative)
                            .textSelection(.enabled)
                    }
                    .accessibilityElement(children: .combine)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .card()
    }

    // MARK: actions

    private var actionsCard: some View {
        VStack(alignment: .leading, spacing: Theme.space3) {
            SectionTitle("Actions")
            RunFlowLayout(spacing: Theme.space2, lineSpacing: Theme.space2) {
                if store.isLive {
                    Button { confirmStop = true } label: { Label("Stop run", systemImage: "stop.fill") }
                        .buttonStyle(.console(.danger))
                    Button { store.showsInjectEvent = true } label: { Label("Inject event", systemImage: "bolt") }
                        .buttonStyle(.console(.event))
                        .disabled(store.latestRound >= store.totalRounds)
                }
                Button {
                    Task { rebuilding = true; await ReportRebuilder.rebuild(store); rebuilding = false }
                } label: {
                    if rebuilding || store.detail?.reportRebuild.running == true {
                        HStack(spacing: 6) { ProgressView().controlSize(.small); Text("Rebuilding…") }
                    } else {
                        Label("Rebuild report", systemImage: "arrow.clockwise")
                    }
                }
                .buttonStyle(.console)
                .disabled(!store.canRebuildReport || rebuilding || store.detail?.reportRebuild.running == true)
                Button {
                    Task {
                        do { try await app.openHTMLReport(store.runID) } catch { store.actionError = error.localizedDescription }
                    }
                } label: { Label("Export report as web page", systemImage: "square.and.arrow.up") }
                    .buttonStyle(.console)
                    .disabled(store.report == nil)
                Button { app.revealInFinder(store.runID) } label: { Label("Reveal in Finder", systemImage: "folder") }
                    .buttonStyle(.console)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .card()
    }

    // MARK: log and graph

    private var logCard: some View {
        VStack(alignment: .leading, spacing: Theme.space3) {
            SectionTitle("Run log")
            RunLogView(store: store) { line in
                if let agent = line.agentID {
                    store.selectedAgentID = agent
                    navigate(.crowd)
                } else if let post = line.postID {
                    store.selectedPostID = post
                    navigate(.crowd)
                }
            }
            .frame(height: 340)
            .clipShape(RoundedRectangle(cornerRadius: Theme.radius))
            .overlay(RoundedRectangle(cornerRadius: Theme.radius).strokeBorder(Theme.seam, lineWidth: 1))
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .card()
    }

    @ViewBuilder
    private var graphCard: some View {
        VStack(alignment: .leading, spacing: Theme.space3) {
            SectionTitle("Knowledge graph") {
                if let g = store.graph, !g.entities.isEmpty {
                    Text("\(g.entities.count) entities, \(g.relations.count) relations").font(Theme.small).foregroundStyle(Theme.muted)
                }
            }
            if store.graph?.entities.isEmpty ?? true {
                Text(store.isLive ? "The graph appears once the seed documents have been read."
                                  : "This run has no knowledge graph.")
                    .font(Theme.body)
                    .foregroundStyle(Theme.muted)
            } else {
                KnowledgeGraphView(store: store)
                    .frame(height: 440)
                    .background(Theme.card)
                    .clipShape(RoundedRectangle(cornerRadius: Theme.radius))
                    .overlay(RoundedRectangle(cornerRadius: Theme.radius).strokeBorder(Theme.seam, lineWidth: 1))
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .card()
    }
}
