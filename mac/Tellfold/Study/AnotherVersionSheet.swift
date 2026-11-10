import SwiftUI

/// What "Try another version…" opens with: a finished run whose people read the new text.
struct AnotherVersionSource: Identifiable, Hashable {
    var runID: String
    var config: SimConfig
    /// Text to start from instead of the run's own document (a suggested rewrite).
    var text: String?
    /// "Try this change" from the report: the step, announced as breaking news at round 1 (already in `config`).
    var change: String?
    /// The label the new version starts with; nil means the study's next letter ("Draft B").
    var label: String?
    var id: String { runID }

    /// "Try this change" on a report step: this run's settings and documents, its people, and the step announced at
    /// round 1, labelled with the step's title so the study comparison says what changed.
    /// A built-in example's trial still runs on this Mac's AI tools: the sheet sends it through `AppModel.versionConfig`.
    static func trial(runID: String, config: SimConfig, step: Recommendation) -> AnotherVersionSource {
        let trial = config.trying(step)
        let title = step.title.trimmingCharacters(in: .whitespacesAndNewlines)
        return AnotherVersionSource(runID: runID, config: trial,
                                    change: trial.events.last { $0.round == 1 }?.content,
                                    label: title.count > 40 ? String(title.prefix(39)) + "…" : title)
    }
}

/// "Try another version…": the run's text in an editor and a label. The new run reuses this run's people (no topic map
/// or persona calls) and joins a study with it, which compares the two when both are done.
struct AnotherVersionSheet: View {
    let source: AnotherVersionSource
    @Environment(AppModel.self) private var app
    @Environment(\.dismiss) private var dismiss

    @State private var text = ""
    @State private var editedName = InlineDocument.descriptionName
    @State private var otherFiles: [URL] = []
    @State private var variant = ""
    @State private var loading = true
    @State private var isSending = false
    @State private var errorMessage: String?

    private var trimmed: String { text.trimmingCharacters(in: .whitespacesAndNewlines) }
    private var canRun: Bool { !loading && !isSending && !trimmed.isEmpty && app.engine.state.isRunning }

    private var calls: Int {
        NewRunRequest.estimatedCallSplit(agents: source.config.numAgents, rounds: source.config.rounds,
                                         batch: source.config.batchSize, chunks: 1).agent + 1
    }

    var body: some View {
        VStack(alignment: .leading, spacing: Theme.space4) {
            VStack(alignment: .leading, spacing: 4) {
                Text(source.change == nil ? "Try another version" : "Try this change")
                    .font(Theme.title)
                    .foregroundStyle(Theme.text)
                    .accessibilityAddTraits(.isHeader)
                Text(source.change == nil
                     ? "The same \(source.config.numAgents) people read your new text. When both runs finish, Tellfold compares them."
                     : "The same \(source.config.numAgents) people read the same text, and hear about the change in round 1. "
                        + "When both runs finish, Tellfold compares them.")
                    .font(Theme.small)
                    .foregroundStyle(Theme.muted)
            }
            if let change = source.change {
                Label("Breaking news in round 1: \(change)", systemImage: "bolt")
                    .font(Theme.small)
                    .foregroundStyle(Theme.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if loading {
                ProgressView("Loading the text…").frame(maxWidth: .infinity, minHeight: 240)
            } else {
                TextEditor(text: $text)
                    .font(Theme.ui(13))
                    .scrollContentBackground(.hidden)
                    .padding(8)
                    .frame(minHeight: 240)
                    .background(Theme.card, in: RoundedRectangle(cornerRadius: Theme.radius))
                    .overlay(RoundedRectangle(cornerRadius: Theme.radius).strokeBorder(Theme.seamStrong))
                    .accessibilityLabel("Text of the new version")
                if !otherFiles.isEmpty {
                    Text("Also sent unchanged: \(otherFiles.map(\.lastPathComponent).joined(separator: ", "))")
                        .font(Theme.caption)
                        .foregroundStyle(Theme.muted)
                }
            }
            HStack(spacing: Theme.space2) {
                Text("Label").font(Theme.body).foregroundStyle(Theme.secondary)
                TextField("Label", text: $variant, prompt: Text("Draft B"))
                    .consoleField()
                    .frame(width: 200)
                    .accessibilityLabel("Label of the new version")
                Spacer()
            }
            Text("Uses about \(calls) model calls: no topic map or new people, the same rounds and a report.")
                .font(Theme.caption)
                .foregroundStyle(Theme.muted)
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
                    if isSending { ProgressView().controlSize(.small) } else { Text("Run this version") }
                }
                .buttonStyle(.consolePrimary)
                .keyboardShortcut(.defaultAction)
                .disabled(!canRun)
            }
        }
        .padding(20)
        .frame(width: 620)
        .background(Theme.panel)
        .task { await load() }
    }

    private func load() async {
        defer { loading = false }
        variant = source.label ?? StudyLogic.nextVariant(for: source.runID, runs: app.runs, studies: app.studies)
        let seeds = await app.downloadSeeds(runID: source.runID)
        let names = seeds.map(\.lastPathComponent)
        if let name = StudyLogic.editableSeed(names), let url = seeds.first(where: { $0.lastPathComponent == name }) {
            editedName = name
            text = (try? String(contentsOf: url, encoding: .utf8)) ?? ""
            otherFiles = seeds.filter { $0 != url }
        } else {
            otherFiles = seeds
        }
        if let suggested = source.text { text = suggested }
        if seeds.isEmpty && source.text == nil {
            errorMessage = "This run's documents are not available. Paste the text of the new version."
        }
    }

    private func run() async {
        isSending = true
        defer { isSending = false }
        let config = app.versionConfig(runID: source.runID, config: source.config)
        let studyID = StudyLogic.study(of: source.runID, runs: app.runs, studies: app.studies)
        let request = StudyLogic.versionRequest(source: source.runID, config: config, studyID: studyID,
                                                variant: variant, editedName: editedName, editedText: trimmed,
                                                otherFiles: otherFiles)
        do {
            try await app.createVersion(request)
            dismiss()
        } catch {
            errorMessage = error.localizedDescription
        }
    }
}
