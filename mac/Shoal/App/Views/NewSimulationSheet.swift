import SwiftUI
import UniformTypeIdentifiers

/// Seeds, question, crowd size, runtimes and models, planned events, and a live estimate.
struct NewSimulationSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    @State private var files: [URL] = []
    @State private var question = ""
    @State private var agents = 20
    @State private var rounds = 6
    @State private var batch = 6
    @State private var agentRuntime = "claude-code"
    @State private var agentModel = "haiku"
    @State private var plannerRuntime = "claude-code"
    @State private var plannerModel = "sonnet"
    @State private var concurrency = 4
    @State private var language = "English"
    @State private var think = false
    @State private var events: [PlannedEvent] = []
    @State private var isDropTargeted = false
    @State private var showsImporter = false
    @State private var isSubmitting = false
    @State private var errorMessage: String?
    @State private var didSetDefaults = false

    struct PlannedEvent: Identifiable, Hashable {
        let id = UUID()
        var round: Int
        var content: String
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text("New simulation")
                    .font(Theme.title)
                    .foregroundStyle(Theme.text)
                    .accessibilityAddTraits(.isHeader)
                Spacer()
            }
            .padding(.horizontal, Theme.space5)
            .padding(.vertical, Theme.space4)
            .overlay(alignment: .bottom) { Seam() }

            ScrollView {
                VStack(spacing: 0) {
                    seedsSection
                    ConsoleSection("Question") {
                        TextField("Question", text: $question, prompt: Text("What do you want to predict?"), axis: .vertical)
                            .lineLimit(2...5)
                            .labelsHidden()
                            .consoleField()
                            .accessibilityLabel("Question")
                    }
                    crowdSection
                    modelsSection
                    runSection
                    eventsSection
                }
            }
            .scrollContentBackground(.hidden)

            Seam()
            footer
        }
        .background(Theme.panel)
        .frame(minWidth: 620, idealWidth: 660, minHeight: 640, idealHeight: 760)
        .fileImporter(isPresented: $showsImporter, allowedContentTypes: AppModel.seedContentTypes,
                      allowsMultipleSelection: true) { result in
            if case .success(let urls) = result { add(urls) }
        }
        .task { await prepare() }
        .onChange(of: model.pendingSeedFiles) { _, new in
            if !new.isEmpty { add(new); model.pendingSeedFiles = [] }
        }
        .onChange(of: rounds) { _, new in
            for i in events.indices where events[i].round > new { events[i].round = new }
        }
    }

    // MARK: Sections

    private var seedsSection: some View {
        ConsoleSection("Seed documents") {
            Button("Choose files…") { showsImporter = true }
                .buttonStyle(.consoleLink)
        } content: {
            VStack(spacing: 8) {
                if files.isEmpty {
                    VStack(spacing: 6) {
                        Image(systemName: "doc.badge.plus")
                            .font(.system(size: 22))
                            .foregroundStyle(Theme.muted)
                        Text("Drop seed documents here").font(Theme.body).foregroundStyle(Theme.text)
                        Text(".md, .txt or .pdf, up to 20 MB each")
                            .font(Theme.mono(11))
                            .foregroundStyle(Theme.muted)
                    }
                    .frame(maxWidth: .infinity, minHeight: 96)
                } else {
                    ForEach(files, id: \.self) { url in
                        HStack {
                            Image(systemName: url.pathExtension.lowercased() == "pdf" ? "doc.richtext" : "doc.text")
                                .foregroundStyle(Theme.muted)
                            Text(url.lastPathComponent)
                                .font(Theme.mono(12))
                                .foregroundStyle(Theme.text)
                                .lineLimit(1)
                                .truncationMode(.middle)
                            Spacer()
                            Text(sizeText(url))
                                .font(Theme.mono(11))
                                .foregroundStyle(Theme.muted)
                            Button {
                                files.removeAll { $0 == url }
                            } label: {
                                Image(systemName: "xmark")
                                    .font(.system(size: 10, weight: .bold))
                                    .foregroundStyle(Theme.muted)
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel("Remove \(url.lastPathComponent)")
                        }
                    }
                }
            }
            .padding(10)
            .frame(maxWidth: .infinity)
            .background {
                RoundedRectangle(cornerRadius: Theme.radius)
                    .strokeBorder(isDropTargeted ? Theme.accent : Theme.seamStrong,
                                  style: StrokeStyle(lineWidth: 1, dash: files.isEmpty ? [5, 4] : []))
            }
            .dropDestination(for: URL.self) { urls, _ in
                add(urls)
                return true
            } isTargeted: { isDropTargeted = $0 }
            .accessibilityElement(children: .contain)
            .accessibilityLabel("Seed documents drop zone")
        }
    }

    private var crowdSection: some View {
        ConsoleSection("Crowd") {
            numberRow("Agents", value: $agents, range: 2...200, step: 1)
            numberRow("Rounds", value: $rounds, range: 1...50, step: 1)
            Stepper(value: $batch, in: 1...20) {
                LabeledContent("Agents per model call") { Text("\(batch)").font(Theme.mono(12)) }
            }
        }
    }

    private var modelsSection: some View {
        ConsoleSection("Models", footer: "Agents make the per-round decisions and answer interviews. The planner builds the graph, personas and report.") {
            runtimePicker("Agent runtime", selection: $agentRuntime, model: $agentModel, role: "agent")
            modelPicker("Agent model", runtime: agentRuntime, selection: $agentModel)
            runtimePicker("Planner runtime", selection: $plannerRuntime, model: $plannerModel, role: "planner")
            modelPicker("Planner model", runtime: plannerRuntime, selection: $plannerModel)
        }
    }

    private var runSection: some View {
        ConsoleSection("Run") {
            Stepper(value: $concurrency, in: 1...16) {
                LabeledContent("Parallel model calls") { Text("\(concurrency)").font(Theme.mono(12)) }
            }
            LabeledContent("Language") {
                TextField("Language", text: $language)
                    .labelsHidden()
                    .consoleField()
                    .frame(width: 180)
            }
            Toggle("Think harder for graph, personas and report", isOn: $think)
        }
    }

    private var eventsSection: some View {
        ConsoleSection("Planned events") {
            Button("Add event") {
                events.append(PlannedEvent(round: min(rounds, max(1, rounds / 2)), content: ""))
            }
            .buttonStyle(ConsoleLinkStyle(tint: Theme.event))
        } content: {
            if events.isEmpty {
                Text("No planned events. Add breaking news that every agent sees at the start of a round.")
                    .font(Theme.small)
                    .foregroundStyle(Theme.muted)
            }
            ForEach($events) { $event in
                HStack(alignment: .firstTextBaseline) {
                    Stepper(value: $event.round, in: 1...rounds) {
                        Text("r\(event.round)").font(Theme.mono(12, .semibold)).foregroundStyle(Theme.event)
                            .frame(width: 40, alignment: .leading)
                    }
                    .fixedSize()
                    TextField("Event", text: $event.content, prompt: Text("What happens?"), axis: .vertical)
                        .labelsHidden()
                        .consoleField()
                    Button {
                        events.removeAll { $0.id == event.id }
                    } label: {
                        Image(systemName: "minus")
                            .font(.system(size: 11, weight: .bold))
                            .foregroundStyle(Theme.muted)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Remove event")
                }
            }
        }
    }

    private var footer: some View {
        HStack(alignment: .center, spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text(estimateTitle)
                    .font(Theme.mono(12, .semibold))
                    .foregroundStyle(Theme.text)
                Text(errorMessage ?? estimateSubtitle)
                    .font(Theme.caption)
                    .foregroundStyle(errorMessage == nil ? Theme.muted : Theme.negative)
                    .lineLimit(2)
            }
            Spacer()
            Button("Cancel", role: .cancel) { dismiss() }
                .buttonStyle(.console)
                .keyboardShortcut(.cancelAction)
            Button {
                Task { await submit() }
            } label: {
                if isSubmitting {
                    ProgressView().controlSize(.small)
                } else {
                    Text("Start Simulation")
                }
            }
            .keyboardShortcut(.defaultAction)
            .buttonStyle(.consolePrimary)
            .disabled(!canSubmit)
        }
        .padding(16)
    }

    // MARK: Pickers

    private func runtimePicker(_ title: String, selection: Binding<String>, model modelBinding: Binding<String>, role: String) -> some View {
        Picker(title, selection: selection) {
            ForEach(model.availableRuntimes) { rt in
                Text(rt.installed ? rt.name : "\(rt.name) (not installed)")
                    .tag(rt.id)
                    .disabled(!rt.installed)
            }
        }
        .onChange(of: selection.wrappedValue) { _, new in
            modelBinding.wrappedValue = model.defaultModel(runtime: new, role: role)
        }
        .help(model.runtime(selection.wrappedValue)?.authHint ?? "")
    }

    @ViewBuilder
    private func modelPicker(_ title: String, runtime: String, selection: Binding<String>) -> some View {
        let models = model.runtime(runtime)?.models ?? []
        if models.isEmpty {
            LabeledContent(title) {
                TextField(title, text: selection, prompt: Text("Model name"))
                    .labelsHidden()
                    .consoleField()
                    .frame(width: 220)
            }
        } else {
            Picker(title, selection: selection) {
                ForEach(models, id: \.self) { Text($0).tag($0) }
                if !models.contains(selection.wrappedValue) && !selection.wrappedValue.isEmpty {
                    Text(selection.wrappedValue).tag(selection.wrappedValue)
                }
            }
        }
    }

    private func numberRow(_ title: String, value: Binding<Int>, range: ClosedRange<Int>, step: Int) -> some View {
        LabeledContent(title) {
            HStack(spacing: 10) {
                Slider(value: Binding(get: { Double(value.wrappedValue) },
                                      set: { value.wrappedValue = Int($0.rounded()) }),
                       in: Double(range.lowerBound)...Double(range.upperBound))
                    .frame(minWidth: 180)
                    .accessibilityLabel(title)
                Stepper(value: value, in: range, step: step) {
                    Text("\(value.wrappedValue)")
                        .font(Theme.mono(12))
                        .frame(minWidth: 30, alignment: .trailing)
                }
                .fixedSize()
            }
        }
    }

    // MARK: Logic

    private var trimmedQuestion: String { question.trimmingCharacters(in: .whitespacesAndNewlines) }

    private var canSubmit: Bool {
        !files.isEmpty && !trimmedQuestion.isEmpty && !isSubmitting && model.engine.state.isRunning
            && !agentModel.isEmpty && !plannerModel.isEmpty
    }

    private var chunks: Int { NewRunRequest.estimatedChunks(files: files) }

    private var estimateTitle: String {
        let calls = NewRunRequest.estimatedCalls(agents: agents, rounds: rounds, batch: batch, chunks: chunks)
        return "About \(calls.formatted()) model calls"
    }

    private var estimateSubtitle: String {
        if !model.engine.state.isRunning { return "Waiting for the engine to start." }
        if files.isEmpty { return "Add seed documents for a firmer estimate." }
        if trimmedQuestion.isEmpty { return "Ask a question to start." }
        return "\(files.count) seed \(files.count == 1 ? "file" : "files"), about \(chunks) chunks · \(agents) agents × \(rounds) rounds"
    }

    private func sizeText(_ url: URL) -> String {
        let size = (try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0
        return ByteCountFormatter.string(fromByteCount: Int64(size), countStyle: .file)
    }

    private func add(_ urls: [URL]) {
        var rejected: [String] = []
        for url in urls {
            guard AppModel.isSeedFile(url) else { rejected.append(url.lastPathComponent); continue }
            let size = (try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0
            guard size <= NewRunRequest.maxFileBytes else { rejected.append("\(url.lastPathComponent) (over 20 MB)"); continue }
            if !files.contains(url), files.count < NewRunRequest.maxFiles { files.append(url) }
        }
        errorMessage = rejected.isEmpty ? nil : "Skipped: \(rejected.joined(separator: ", ")). Use .md, .txt or .pdf."
    }

    private func prepare() async {
        if !model.pendingSeedFiles.isEmpty {
            add(model.pendingSeedFiles)
            model.pendingSeedFiles = []
        }
        guard !didSetDefaults else { return }
        didSetDefaults = true
        concurrency = model.engineSettings?.concurrency ?? model.prefs.defaultConcurrency
        think = model.engineSettings?.think ?? false
        if model.runtimes.isEmpty { await model.loadRuntimes() }
        agentRuntime = model.defaultAgentRuntime
        plannerRuntime = model.defaultPlannerRuntime
        agentModel = model.defaultModel(runtime: agentRuntime, role: "agent")
        plannerModel = model.defaultModel(runtime: plannerRuntime, role: "planner")
    }

    private func submit() async {
        guard canSubmit else { return }
        isSubmitting = true
        errorMessage = nil
        defer { isSubmitting = false }
        let request = NewRunRequest(
            files: files, question: trimmedQuestion, agents: agents, rounds: rounds, batch: batch,
            agentRuntime: agentRuntime, agentModel: agentModel, plannerRuntime: plannerRuntime,
            plannerModel: plannerModel, concurrency: concurrency,
            language: language.trimmingCharacters(in: .whitespaces).isEmpty ? "English" : language,
            think: think,
            events: events.map { GodEvent(round: $0.round, content: $0.content.trimmingCharacters(in: .whitespacesAndNewlines)) }
                .filter { !$0.content.isEmpty })
        do {
            _ = try await model.createRun(request)
            dismiss()
        } catch {
            errorMessage = error.localizedDescription
        }
    }
}
