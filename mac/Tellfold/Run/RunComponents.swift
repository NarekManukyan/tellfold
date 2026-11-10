import SwiftUI

/// Navigation between the run's tabs, offered to every view of the run: `navigate(.crowd)`.
struct RunNavigate: Equatable {
    var go: @MainActor (RunTab) -> Void = { _ in }

    @MainActor
    func callAsFunction(_ tab: RunTab) { go(tab) }

    /// The handler never changes what the views render, so two navigators always compare equal.
    static func == (lhs: RunNavigate, rhs: RunNavigate) -> Bool { true }
}

extension EnvironmentValues {
    @Entry var runNavigate = RunNavigate()
}

/// The tabs in the centre of a run.
enum RunTab: String, CaseIterable, Identifiable {
    case overview, crowd, report, details

    var id: String { rawValue }
    var title: String {
        switch self {
        case .overview: "Overview"
        case .crowd: "Crowd"
        case .report: "Report"
        case .details: "Details"
        }
    }
    var symbol: String {
        switch self {
        case .overview: "rectangle.grid.1x2"
        case .crowd: "person.2"
        case .report: "doc.richtext"
        case .details: "slider.horizontal.3"
        }
    }
}

extension RunStore {
    /// The headline numbers as of the newest round.
    var metrics: RunMetrics {
        let orgs = personas.filter { $0.entityID != nil }.count
        var final: Double?
        for round in stride(from: latestRound, through: 0, by: -1) {
            if let value = RunUI.roundSentiment(round, in: self) { final = value; break }
        }
        return RunMetrics(people: personas.count, organisations: orgs, roundsDone: roundsCompleted, totalRounds: totalRounds,
                          posts: posts.count, roundPosts: posts.filter { $0.round >= 1 }.count, finalSentiment: final, baselineSentiment: RunUI.roundSentiment(0, in: self))
    }

    /// Rounds that actually completed: the recorded data wins over a stale status.
    var roundsCompleted: Int { latestRound }

    /// The run's report text can be rebuilt (not while it runs, not before any round completed, never for an example).
    var canRebuildReport: Bool { controls.rebuildReport && !isLive && api != nil && (latestRound > 0 || report != nil) }

    /// Which run-changing controls appear for this run.
    var controls: RunControls { RunControls(isLive: isLive, isExample: isExample) }

    /// A finished, stopped or failed run whose settings can seed a new simulation.
    var canRunAgain: Bool { !isLive && config != nil && (status?.state.isTerminal ?? false) }

    /// A finished run with a report and people: its crowd can read another version of the text.
    var canTryAnotherVersion: Bool { canRunAgain && report != nil && !personas.isEmpty }
}

/// The controls that change a run. A built-in example is read-only, so it shows none of them: no Stop, no Inject event
/// and no Rebuild report. "Run again…" stays, since it makes a new run of the user's own.
struct RunControls: Equatable {
    var stop: Bool
    var injectEvent: Bool
    var rebuildReport: Bool

    init(isLive: Bool, isExample: Bool) {
        stop = isLive && !isExample
        injectEvent = !isExample
        rebuildReport = !isExample
    }
}

/// "Run again…": opens the New simulation sheet with this run's question, size and options.
struct RunAgainButton: View {
    let store: RunStore
    @Environment(AppModel.self) private var app

    var body: some View {
        Button("Run again…") {
            if let config = store.config { app.runAgain(runID: store.runID, config: config) }
        }
        .buttonStyle(.console(.secondary))
        .disabled(!store.canRunAgain || !app.engine.state.isRunning)
        .help("Start a new simulation from this run's question and settings")
    }
}

/// "Try another version…": the same people read an edited text, and the two runs are compared.
struct AnotherVersionButton: View {
    let store: RunStore
    @Environment(AppModel.self) private var app

    var body: some View {
        Button("Try another version…") {
            if let config = store.config { app.tryAnotherVersion(runID: store.runID, config: config) }
        }
        .buttonStyle(.console(.secondary))
        .disabled(!store.canTryAnotherVersion || !app.engine.state.isRunning)
        .help("Edit the text and run it past the same people, then compare the two versions")
    }
}

// MARK: - small views

/// A signed sentiment number in its colour: never colour alone.
struct SentimentValue: View {
    let score: Double?
    var size: CGFloat = 12
    var weight: Font.Weight = .semibold

    var body: some View {
        Text(RunUI.sentimentText(score))
            .font(Theme.mono(size, weight))
            .foregroundStyle(score == nil ? Theme.faint : RunUI.sentimentColor(score))
            .accessibilityLabel(score.map { "Sentiment \(RunUI.sentimentText($0)), \(SentimentScale.describe($0))" } ?? "No sentiment")
    }
}

/// A camp's mean sentiment, or "no posts" in muted text when none of its people posted (there is nothing to average).
struct CampSentimentValue: View {
    let score: Double?
    var size: CGFloat = 12
    var weight: Font.Weight = .semibold

    /// What the tooltip says when there is no number.
    static let noPostsHelp = "None of these people posted, so there is no sentiment to average."
    static let noPostsText = "no posts"

    var body: some View {
        if score == nil {
            Text(Self.noPostsText)
                .font(Theme.ui(size - 1))
                .foregroundStyle(Theme.muted)
                .help(Self.noPostsHelp)
                .accessibilityLabel("No posts")
                .accessibilityHint(Self.noPostsHelp)
        } else {
            SentimentValue(score: score, size: size, weight: weight)
        }
    }
}

/// Low / Medium / High confidence: three bars and the words.
struct ConfidenceBadge: View {
    let confidence: ReportConfidence

    private var level: Int {
        switch confidence {
        case .low: 1
        case .medium: 2
        case .high: 3
        case .unknown: 0
        }
    }

    var body: some View {
        HStack(spacing: 6) {
            Text(confidence == .unknown ? "Confidence unknown" : "\(confidence.title) confidence")
                .font(Theme.ui(12, .medium))
                .foregroundStyle(Theme.secondary)
            HStack(spacing: 2) {
                ForEach(0..<3, id: \.self) { i in
                    RoundedRectangle(cornerRadius: 1).fill(i < level ? Theme.accent : Theme.seamStrong)
                        .frame(width: 4, height: 10)
                }
            }
            .accessibilityHidden(true)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 4)
        .overlay(Capsule().strokeBorder(Theme.seamStrong, lineWidth: 1))
        .fixedSize()
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(confidence == .unknown ? "Confidence unknown" : "\(confidence.title) confidence")
    }
}

/// Overlapping avatars, up to `limit` and then "+n".
struct AvatarStack: View {
    let personas: [Persona]
    var limit = 6
    var size: CGFloat = 24

    var body: some View {
        let shown = Array(personas.prefix(limit))
        HStack(spacing: -size * 0.28) {
            ForEach(Array(shown.enumerated()), id: \.element.id) { i, p in
                AvatarView(persona: p, size: size)
                    .padding(1.5)
                    .background(Theme.card, in: Circle())
                    .zIndex(Double(shown.count - i))
            }
            if personas.count > limit {
                Text("+\(personas.count - limit)")
                    .font(Theme.ui(11, .medium))
                    .foregroundStyle(Theme.muted)
                    .padding(.leading, size * 0.4)
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(personas.count) people")
    }
}

/// A row that highlights on hover and press: for lists inside cards.
struct HoverRowStyle: ButtonStyle {
    var selected = false

    func makeBody(configuration: Configuration) -> some View {
        Row(configuration: configuration, selected: selected)
    }

    private struct Row: View {
        let configuration: Configuration
        let selected: Bool
        @State private var hovering = false

        var body: some View {
            configuration.label
                .background(
                    RoundedRectangle(cornerRadius: Theme.radius)
                        .fill(selected ? Theme.accentSoft : (configuration.isPressed ? Theme.seam : (hovering ? Theme.hover : .clear)))
                )
                .contentShape(RoundedRectangle(cornerRadius: Theme.radius))
                .onHover { hovering = $0 }
        }
    }
}

/// The quiet note shown when a run's report has no camps, with the button that writes a fresh report.
struct CampsFallbackNote: View {
    let store: RunStore
    /// Floating over the graph: the note sits on glass and its button is a glass button.
    var glass = false
    @State private var confirm = false
    @State private var rebuilding = false

    var body: some View {
        let row = HStack(alignment: .center, spacing: Theme.space3) {
            Text(CampGrouping.fallbackNote)
                .font(Theme.small)
                .foregroundStyle(glass ? Theme.secondary : Theme.muted)
                // A minimum width keeps the window's minimum height sane: probed at zero width the sentence would wrap per letter.
                .frame(minWidth: 220, alignment: .leading)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: Theme.space2)
            if store.controls.rebuildReport { rebuildButton }
        }
        Group {
            if glass {
                row
                    .padding(.horizontal, 14)
                    .padding(.vertical, 8)
                    .glassSurface(in: RoundedRectangle(cornerRadius: 14))
            } else {
                row
            }
        }
        .confirmationDialog("Rebuild the report?", isPresented: $confirm) {
            Button("Rebuild Report") { Task { await rebuild() } }
        } message: {
            Text("The planner model writes a new report from this run. This makes a few model calls.")
        }
    }

    @ViewBuilder private var rebuildButton: some View {
        let button = Button {
            confirm = true
        } label: {
            if rebuilding || store.detail?.reportRebuild.running == true {
                HStack(spacing: 6) { ProgressView().controlSize(.small); Text("Rebuilding…") }
            } else {
                Text("Rebuild report")
            }
        }
        .disabled(!store.canRebuildReport || rebuilding || store.detail?.reportRebuild.running == true)
        .help(store.isLive ? "Available once the run has finished" : "Write the report again, grouping the crowd by opinion")
        if glass {
            button.glassButtonStyle().controlSize(.small)
        } else {
            button.buttonStyle(.console(.secondary, compact: true))
        }
    }

    private func rebuild() async {
        rebuilding = true
        defer { rebuilding = false }
        await ReportRebuilder.rebuild(store)
    }
}

/// Starts a report rebuild and waits (up to about ten minutes) until the new report is in.
@MainActor
enum ReportRebuilder {
    /// Returns when the engine finished, failed, or the wait ran out. Failures land in `store.actionError`.
    static func rebuild(_ store: RunStore) async {
        let before = store.report.map { "\($0.title)|\($0.summary)|\($0.camps.count)" }
        store.actionError = nil
        await store.rebuildReport()
        guard store.actionError == nil else { return }
        for _ in 0..<200 {
            try? await Task.sleep(for: .seconds(3))
            if Task.isCancelled { return }
            await store.reloadDetail()
            let now = store.report.map { "\($0.title)|\($0.summary)|\($0.camps.count)" }
            let running = store.detail?.reportRebuild.running ?? false
            if let error = store.detail?.reportRebuild.error, !running {
                store.actionError = "The report could not be rebuilt: \(error)"
                return
            }
            if !running, now != nil, now != before { return }
        }
    }
}
