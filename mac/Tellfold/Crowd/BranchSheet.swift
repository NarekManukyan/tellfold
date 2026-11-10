import SwiftUI

/// The rules of "Branch from here…", free of views so they can be tested.
nonisolated enum BranchLogic {
    /// A finished run can be branched from a round it reached that leaves at least one round to run.
    static func canBranch(round: Int, latestRound: Int, totalRounds: Int, isLive: Bool) -> Bool {
        !isLive && round >= 0 && round <= latestRound && round < totalRounds
    }

    /// The source's events after `round`: they still happen in the branch unless the user removes them.
    static func laterEvents(_ events: [GodEvent], after round: Int) -> [GodEvent] {
        events.filter { $0.round > round }.sorted { $0.round < $1.round }
    }

    /// Events to send: non-empty text, trimmed, in a round the branch will run.
    static func sendable(_ events: [GodEvent], after round: Int, totalRounds: Int) -> [GodEvent] {
        events.compactMap { e in
            let text = e.content.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !text.isEmpty, e.round > round, e.round <= totalRounds else { return nil }
            return GodEvent(round: e.round, content: text)
        }
    }

    /// Model calls for the branch when nothing needs a retry: only the rounds after `round`, then the report. It has no
    /// topic map, new people or opening posts (`tellfold.runner.estimate_call_split` for a branched config).
    static func estimatedCalls(agents: Int, batch: Int, round: Int, totalRounds: Int) -> Int {
        let perRound = NewRunRequest.estimatedCallSplit(agents: agents, rounds: 1, batch: batch, chunks: 0).agent
            - NewRunRequest.estimatedCallSplit(agents: agents, rounds: 0, batch: batch, chunks: 0).agent
        return max(0, totalRounds - round) * perRound + 1
    }

    static func summary(round: Int) -> String {
        round == 0
            ? "The opening posts stay exactly as they were. Everything after them runs again with the events below."
            : "Rounds 0 to \(round) stay exactly as they happened. Everything after round \(round) runs again with the events below."
    }
}

/// "Branch from here…": keep the run up to a round, change what happens next, and run it again as a new run that is
/// compared with the original.
struct BranchSheet: View {
    let store: RunStore
    let round: Int
    @Environment(AppModel.self) private var app
    @Environment(\.dismiss) private var dismiss

    private struct Row: Identifiable {
        let id = UUID()
        var round: Int
        var content: String
    }

    @State private var rows: [Row] = []
    @State private var isSending = false
    @State private var errorMessage: String?

    private var events: [GodEvent] {
        BranchLogic.sendable(rows.map { GodEvent(round: $0.round, content: $0.content) }, after: round,
                             totalRounds: store.totalRounds)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: Theme.space4) {
            VStack(alignment: .leading, spacing: 4) {
                Text("Branch from round \(round)")
                    .font(Theme.title)
                    .foregroundStyle(Theme.text)
                    .accessibilityAddTraits(.isHeader)
                Text(BranchLogic.summary(round: round))
                    .font(Theme.small)
                    .foregroundStyle(Theme.muted)
                    .fixedSize(horizontal: false, vertical: true)
            }
            VStack(alignment: .leading, spacing: Theme.space2) {
                ForEach($rows) { $row in
                    HStack(spacing: Theme.space2) {
                        Stepper(value: $row.round, in: (round + 1)...max(round + 1, store.totalRounds)) {
                            Text("Round \(row.round)").font(Theme.mono(12)).frame(width: 70, alignment: .leading)
                        }
                        .fixedSize()
                        TextField("Event", text: $row.content, prompt: Text("What happens? e.g. The owner apologises"))
                            .consoleField()
                        Button {
                            rows.removeAll { $0.id == row.id }
                        } label: {
                            Image(systemName: "minus.circle")
                        }
                        .buttonStyle(.plain)
                        .foregroundStyle(Theme.muted)
                        .accessibilityLabel("Remove this event")
                    }
                }
                Button {
                    rows.append(Row(round: round + 1, content: ""))
                } label: {
                    Label("Add an event", systemImage: "plus")
                }
                .buttonStyle(.console(.ghost, compact: true))
                if rows.isEmpty {
                    Text("With no events, the branch shows how much of the outcome is chance.")
                        .font(Theme.caption).foregroundStyle(Theme.muted)
                }
            }
            if let config = store.config {
                let calls = BranchLogic.estimatedCalls(agents: config.numAgents, batch: config.batchSize,
                                                       round: round, totalRounds: store.totalRounds)
                Text("Uses about \(calls) model calls: the rounds after round \(round) and a report.")
                    .font(Theme.caption)
                    .foregroundStyle(Theme.muted)
            }
            if let errorMessage {
                Text(errorMessage).font(Theme.small).foregroundStyle(Theme.negative)
            }
            HStack {
                Spacer()
                Button("Cancel", role: .cancel) { dismiss() }
                    .buttonStyle(.console)
                    .keyboardShortcut(.cancelAction)
                Button {
                    Task { await run() }
                } label: {
                    if isSending { ProgressView().controlSize(.small) } else { Text("Run the branch") }
                }
                .buttonStyle(.consolePrimary)
                .keyboardShortcut(.defaultAction)
                .disabled(isSending || !app.engine.state.isRunning)
            }
        }
        .padding(20)
        .frame(width: 560)
        .background(Theme.panel)
        .onAppear {
            rows = BranchLogic.laterEvents(store.events, after: round).map { Row(round: $0.round, content: $0.content) }
        }
    }

    private func run() async {
        isSending = true
        defer { isSending = false }
        do {
            try await app.branch(runID: store.runID, round: round, events: events)
            dismiss()
        } catch {
            errorMessage = error.localizedDescription
        }
    }
}
