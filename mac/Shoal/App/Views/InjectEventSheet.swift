import SwiftUI

/// Inject a god event into a future round of a live run (Simulation > Inject Event, ⌘E).
struct InjectEventSheet: View {
    let store: RunStore
    @Environment(\.dismiss) private var dismiss
    @State private var round = 1
    @State private var content = ""
    @State private var isSending = false
    @State private var errorMessage: String?

    private var firstRound: Int { min(store.latestRound + 1, max(store.totalRounds, 1)) }
    private var lastRound: Int { max(store.totalRounds, firstRound) }
    private var canInject: Bool {
        store.isLive && store.latestRound < store.totalRounds
            && !content.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && !isSending
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Inject event")
                .font(Theme.title)
                .foregroundStyle(Theme.text)
                .accessibilityAddTraits(.isHeader)
            Text("Every agent sees it as breaking news at the start of the round.")
                .font(Theme.small)
                .foregroundStyle(Theme.muted)
            Form {
                Stepper(value: $round, in: firstRound...lastRound) {
                    LabeledContent("Round") { Text("\(round) of \(store.totalRounds)").font(Theme.mono(12)) }
                }
                TextField("Event", text: $content, prompt: Text("What happens?"), axis: .vertical)
                    .lineLimit(3...6)
                    .consoleField()
            }
            .formStyle(.columns)
            if let errorMessage {
                Text(errorMessage)
                    .font(Theme.small)
                    .foregroundStyle(Theme.negative)
            } else if !store.isLive || store.latestRound >= store.totalRounds {
                Text("This simulation has no rounds left to change.")
                    .font(Theme.small)
                    .foregroundStyle(Theme.muted)
            }
            HStack {
                Spacer()
                Button("Cancel", role: .cancel) { dismiss() }
                    .buttonStyle(.console)
                    .keyboardShortcut(.cancelAction)
                Button("Inject") { Task { await inject() } }
                    .keyboardShortcut(.defaultAction)
                    .buttonStyle(ConsoleButtonStyle(kind: .event))
                    .disabled(!canInject)
            }
        }
        .padding(20)
        .frame(width: 440)
        .background(Theme.panel)
        .onAppear { round = firstRound }
    }

    private func inject() async {
        isSending = true
        defer { isSending = false }
        do {
            try await store.injectEvent(round: round, content: content.trimmingCharacters(in: .whitespacesAndNewlines))
            dismiss()
        } catch {
            errorMessage = error.localizedDescription
        }
    }
}
