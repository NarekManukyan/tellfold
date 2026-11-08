import SwiftUI
import UniformTypeIdentifiers

/// Starts a simulation by answering three plain questions: what to find out, what it is about, how big. Everything
/// else (exact numbers, runtimes and models, language, seed, planned events) sits under Advanced with today's defaults.
struct NewSimulationSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    @State private var question = ""
    @State private var aboutMode: AboutMode = .describe
    @State private var descriptionText = ""
    @State private var files: [URL] = []
    /// Names of an earlier run's seed documents that could not be loaded again.
    @State private var missingSeedNames: [String] = []
    @State private var agents = SizePreset.default.agents
    @State private var rounds = SizePreset.default.rounds
    @State private var batch = 6
    @State private var agentRuntime = "claude-code"
    @State private var agentModel = "haiku"
    @State private var plannerRuntime = "claude-code"
    @State private var plannerModel = "sonnet"
    @State private var concurrency = 4
    @State private var language = "English"
    @State private var think = false
    @State private var seed = 7
    @State private var events: [PlannedEvent] = []
    @State private var isDropTargeted = false
    @State private var showsImporter = false
    @State private var isSubmitting = false
    @State private var errorMessage: String?
    @State private var didSetDefaults = false
    @State private var showsAgentSetup = false
    /// The user (or "Run again…") picked the runtimes; later detection results no longer replace them.
    @State private var runtimesChosen = false
    @AppStorage("newSimulation.advancedOpen") private var advancedOpen = false
    @FocusState private var focus: Field?

    private enum Field { case question, description }

    struct PlannedEvent: Identifiable, Hashable {
        let id = UUID()
        var round: Int
        var content: String
    }

    var body: some View {
        VStack(spacing: 0) {
            Text("New simulation")
                .font(Theme.title)
                .foregroundStyle(Theme.text)
                .accessibilityAddTraits(.isHeader)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, Theme.space5)
                .padding(.top, Theme.space5)
                .padding(.bottom, Theme.space3)

            ScrollViewReader { proxy in
                ScrollView {
                    VStack(alignment: .leading, spacing: Theme.space5) {
                        if let note = readinessNote {
                            ReadinessNote(text: note, actionTitle: needsAgentSetup ? "Choose AI Tool…" : nil) {
                                showsAgentSetup = true
                            }
                        }
                        questionStep
                        aboutStep
                        sizeStep
                        advancedSection.id("advanced")
                    }
                    .padding(.horizontal, Theme.space5)
                    .padding(.top, Theme.space2)
                    .padding(.bottom, Theme.space5)
                }
                .onChange(of: advancedOpen) { _, open in
                    // Opening Advanced brings its options into view instead of leaving them below the fold.
                    guard open else { return }
                    Task {
                        try? await Task.sleep(for: .milliseconds(200))
                        withAnimation(.snappy(duration: 0.25)) { proxy.scrollTo("advanced", anchor: .top) }
                    }
                }
            }

            Seam()
            footer
        }
        .background(Theme.ground)
        .frame(minWidth: 620, idealWidth: 680, minHeight: 600, idealHeight: 780)
        .fileImporter(isPresented: $showsImporter, allowedContentTypes: AppModel.seedContentTypes,
                      allowsMultipleSelection: true) { result in
            if case .success(let urls) = result { add(urls) }
        }
        .task { await prepare() }
        .onChange(of: model.pendingSeedFiles) { _, new in
            if !new.isEmpty { add(new); model.pendingSeedFiles = [] }
        }
        .sheet(isPresented: $showsAgentSetup, onDismiss: {
            runtimesChosen = false
            useDefaultRuntimes()
        }) {
            AgentSetupView()
        }
        .onChange(of: model.runtimes) { _, _ in if !runtimesChosen { useDefaultRuntimes() } }
        .onChange(of: model.engineSettings?.agentRuntime) { _, _ in if !runtimesChosen { useDefaultRuntimes() } }
        .onChange(of: rounds) { _, new in
            for i in events.indices where events[i].round > new { events[i].round = new }
        }
    }

    // MARK: Step 1: the question

    private var questionStep: some View {
        FormStep(title: "What do you want to find out?") {
            PlainTextArea(text: $question, placeholder: "How will people react to …?", minHeight: 76, label: "Question")
                .focused($focus, equals: .question)
            ExampleChips { chosen in
                question = chosen
                focus = .question
            }
        }
    }

    // MARK: Step 2: what it is about

    private var aboutStep: some View {
        FormStep(title: "What is it about?", helper: "The simulated crowd only knows what you give it here.") {
            Picker("Material", selection: $aboutMode) {
                ForEach(AboutMode.allCases) { Text($0.title).tag($0) }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .fixedSize()
            .accessibilityLabel("How to give it material")

            if !missingSeedNames.isEmpty && files.isEmpty && descriptionText.isEmpty {
                Text("Add the documents again. The last run used \(missingSeedNames.joined(separator: ", ")).")
                    .font(Theme.small)
                    .foregroundStyle(Theme.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }

            switch aboutMode {
            case .describe:
                PlainTextArea(text: $descriptionText,
                              placeholder: "Paste a product description, a pricing page, an announcement or your notes.",
                              minHeight: 132, fontSize: 13, label: "Description")
                    .focused($focus, equals: .description)
                if !files.isEmpty {
                    sideNote("\(files.count) \(files.count == 1 ? "file is" : "files are") sent too.")
                } else if !NewSimulationRules.trimmed(descriptionText).isEmpty,
                          !NewSimulationRules.hasMaterial(description: descriptionText, fileCount: 0) {
                    sideNote("A little more detail helps. Or add a file instead.")
                }
            case .files:
                dropZone
                if !NewSimulationRules.trimmed(descriptionText).isEmpty {
                    sideNote("Your description is sent too.")
                }
            }
        }
    }

    private func sideNote(_ text: String) -> some View {
        Text(text).font(Theme.small).foregroundStyle(Theme.muted)
    }

    private var dropZone: some View {
        VStack(spacing: Theme.space2) {
            if files.isEmpty {
                VStack(spacing: 6) {
                    Image(systemName: "doc.badge.plus")
                        .font(.system(size: 22))
                        .foregroundStyle(Theme.muted)
                    Text("Drop files here").font(Theme.body).foregroundStyle(Theme.text)
                    Text(".md, .txt or .pdf, up to 20 MB each")
                        .font(Theme.small)
                        .foregroundStyle(Theme.muted)
                }
                .frame(maxWidth: .infinity, minHeight: 88)
            } else {
                ForEach(files, id: \.self) { url in
                    HStack(spacing: Theme.space2) {
                        Image(systemName: url.pathExtension.lowercased() == "pdf" ? "doc.richtext" : "doc.text")
                            .foregroundStyle(Theme.muted)
                        Text(url.lastPathComponent)
                            .font(Theme.small)
                            .foregroundStyle(Theme.text)
                            .lineLimit(1)
                            .truncationMode(.middle)
                        Spacer(minLength: Theme.space2)
                        Text(sizeText(url)).font(Theme.mono(11)).foregroundStyle(Theme.muted)
                        Button {
                            files.removeAll { $0 == url }
                        } label: {
                            Image(systemName: "xmark")
                                .font(.system(size: 10, weight: .bold))
                                .foregroundStyle(Theme.muted)
                                .frame(width: 22, height: 22)
                                .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel("Remove \(url.lastPathComponent)")
                    }
                    .frame(minHeight: 26)
                }
            }
            Button("Choose files…") { showsImporter = true }
                .buttonStyle(.console(.secondary, compact: true))
        }
        .padding(Theme.space3)
        .frame(maxWidth: .infinity)
        .background(isDropTargeted ? Theme.accentSoft : Theme.card, in: RoundedRectangle(cornerRadius: Theme.radius))
        .overlay {
            RoundedRectangle(cornerRadius: Theme.radius)
                .strokeBorder(isDropTargeted ? Theme.accent : Theme.seamStrong,
                              style: StrokeStyle(lineWidth: 1, dash: files.isEmpty ? [5, 4] : []))
        }
        .dropDestination(for: URL.self) { urls, _ in
            add(urls)
            return true
        } isTargeted: { isDropTargeted = $0 }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Documents drop zone")
    }

    // MARK: Step 3: how big

    private var sizeStep: some View {
        FormStep(title: "How big?") {
            SizePresetPicker(selection: SizePreset.detect(agents: agents, rounds: rounds),
                             choose: { preset in agents = preset.agents; rounds = preset.rounds },
                             customDetail: SizePreset.detect(agents: agents, rounds: rounds) == nil
                                 ? "\(agents) people · \(rounds) rounds" : nil)
        }
    }

    // MARK: Advanced

    private var advancedSection: some View {
        DisclosureGroup(isExpanded: $advancedOpen) {
            VStack(alignment: .leading, spacing: 0) {
                advancedGroup {
                    row("People") { NumberField(value: $agents, range: 2...200, label: "People") }
                    row("Rounds") { NumberField(value: $rounds, range: 1...50, label: "Rounds") }
                    row("Agents per model call") { NumberField(value: $batch, range: 1...20, label: "Agents per model call") }
                    row("Parallel model calls") { NumberField(value: $concurrency, range: 1...16, label: "Parallel model calls") }
                }
                Seam()
                advancedGroup {
                    row("Agent runtime", hint: "Agents make the per-round decisions and answer interviews.") {
                        runtimePicker("Agent runtime", runtime: runtimeBinding(role: "agent"))
                    }
                    row("Agent model") { modelPicker("Agent model", runtime: agentRuntime, selection: $agentModel) }
                    row("Planner runtime", hint: "The planner builds the graph, personas and report.") {
                        runtimePicker("Planner runtime", runtime: runtimeBinding(role: "planner"))
                    }
                    row("Planner model") { modelPicker("Planner model", runtime: plannerRuntime, selection: $plannerModel) }
                }
                Seam()
                advancedGroup {
                    row("Language") {
                        TextField("Language", text: $language)
                            .labelsHidden()
                            .consoleField()
                            .frame(width: 160)
                    }
                    row("Random seed", hint: "The same seed repeats the same sampling.") {
                        NumberField(value: $seed, range: 0...Int(Int32.max), label: "Random seed", width: 96)
                    }
                    row("Think harder", hint: "Slower and more thorough for the graph, personas and report.") {
                        Toggle("Think harder", isOn: $think).labelsHidden().toggleStyle(.switch).controlSize(.small)
                    }
                }
                Seam()
                eventsGroup
            }
            .frame(maxWidth: .infinity)
            .background(Theme.card, in: RoundedRectangle(cornerRadius: Theme.cardRadius))
            .overlay(RoundedRectangle(cornerRadius: Theme.cardRadius).strokeBorder(Theme.seam, lineWidth: 1))
            .padding(.top, Theme.space2)
        } label: {
            Text("Advanced").font(Theme.sectionTitle).foregroundStyle(Theme.text)
        }
        .accessibilityLabel("Advanced options")
    }

    private func advancedGroup<Content: View>(@ViewBuilder _ content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: Theme.space2) { content() }
            .padding(Theme.space4)
    }

    private func row<Control: View>(_ title: String, hint: String? = nil, @ViewBuilder control: () -> Control) -> some View {
        HStack(alignment: .center, spacing: Theme.space3) {
            VStack(alignment: .leading, spacing: 1) {
                Text(title).font(Theme.body).foregroundStyle(Theme.text)
                if let hint { Text(hint).font(Theme.caption).foregroundStyle(Theme.muted) }
            }
            Spacer(minLength: Theme.space3)
            control()
        }
        .frame(minHeight: 28)
    }

    private var eventsGroup: some View {
        VStack(alignment: .leading, spacing: Theme.space2) {
            HStack {
                VStack(alignment: .leading, spacing: 1) {
                    Text("Breaking news").font(Theme.body).foregroundStyle(Theme.text)
                    Text("Every agent sees it at the start of the round you pick.")
                        .font(Theme.caption)
                        .foregroundStyle(Theme.muted)
                }
                Spacer()
                Button {
                    events.append(PlannedEvent(round: min(rounds, max(1, rounds / 2)), content: ""))
                } label: {
                    Label("Add event", systemImage: "plus")
                }
                .buttonStyle(.console(.event, compact: true))
            }
            ForEach($events) { $event in
                HStack(alignment: .center, spacing: Theme.space2) {
                    Stepper(value: $event.round, in: 1...max(1, rounds)) {
                        Text("r\(event.round)")
                            .font(Theme.mono(12, .semibold))
                            .foregroundStyle(Theme.event)
                            .frame(width: 32, alignment: .leading)
                    }
                    .fixedSize()
                    .accessibilityLabel("Round")
                    TextField("Event", text: $event.content, prompt: Text("What happens?"), axis: .vertical)
                        .labelsHidden()
                        .consoleField()
                    Button {
                        events.removeAll { $0.id == event.id }
                    } label: {
                        Image(systemName: "minus")
                            .font(.system(size: 11, weight: .bold))
                            .foregroundStyle(Theme.muted)
                            .frame(width: 24, height: 24)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Remove event")
                }
            }
        }
        .padding(Theme.space4)
    }

    // MARK: Footer

    private var footer: some View {
        HStack(alignment: .bottom, spacing: Theme.space4) {
            VStack(alignment: .leading, spacing: 3) {
                Text(summary)
                    .font(Theme.body)
                    .foregroundStyle(Theme.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                if let errorMessage {
                    Text(errorMessage).font(Theme.small).foregroundStyle(Theme.negative)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            VStack(alignment: .trailing, spacing: 6) {
                if let reason = disabledReason {
                    Text(reason).font(Theme.small).foregroundStyle(Theme.muted)
                }
                HStack(spacing: Theme.space2) {
                    Button("Cancel", role: .cancel) { dismiss() }
                        .buttonStyle(.console)
                        .keyboardShortcut(.cancelAction)
                    Button {
                        Task { await submit() }
                    } label: {
                        if isSubmitting {
                            ProgressView().controlSize(.small)
                        } else {
                            Text("Start simulation")
                        }
                    }
                    .keyboardShortcut(.return, modifiers: .command)
                    .buttonStyle(.consolePrimary)
                    .disabled(disabledReason != nil || isSubmitting)
                    .help(disabledReason ?? "Start the simulation (⌘↩)")
                }
            }
        }
        .padding(.horizontal, Theme.space5)
        .padding(.vertical, Theme.space4)
    }

    // MARK: Pickers

    /// Choosing a runtime also picks that runtime's default model for the role.
    private func runtimeBinding(role: String) -> Binding<String> {
        Binding(get: { role == "agent" ? agentRuntime : plannerRuntime }, set: { new in
            runtimesChosen = true
            if role == "agent" {
                agentRuntime = new
                agentModel = model.defaultModel(runtime: new, role: "agent")
            } else {
                plannerRuntime = new
                plannerModel = model.defaultModel(runtime: new, role: "planner")
            }
        })
    }

    private func runtimePicker(_ title: String, runtime: Binding<String>) -> some View {
        Picker(title, selection: runtime) {
            ForEach(model.availableRuntimes) { rt in
                Text(rt.installed ? rt.name : "\(rt.name) (not installed)")
                    .tag(rt.id)
                    .disabled(!rt.installed)
            }
            if model.runtime(runtime.wrappedValue) == nil {
                Text(runtime.wrappedValue).tag(runtime.wrappedValue)
            }
        }
        .labelsHidden()
        .frame(width: 220)
        .help(model.runtime(runtime.wrappedValue)?.authHint ?? "")
    }

    @ViewBuilder
    private func modelPicker(_ title: String, runtime: String, selection: Binding<String>) -> some View {
        let models = model.runtime(runtime)?.models ?? []
        if models.isEmpty {
            TextField(title, text: selection, prompt: Text("Model name"))
                .labelsHidden()
                .consoleField()
                .frame(width: 220)
        } else {
            Picker(title, selection: selection) {
                ForEach(models, id: \.self) { Text($0).tag($0) }
                if !models.contains(selection.wrappedValue) && !selection.wrappedValue.isEmpty {
                    Text(selection.wrappedValue).tag(selection.wrappedValue)
                }
            }
            .labelsHidden()
            .frame(width: 220)
        }
    }

    // MARK: Logic

    private var runtimeInfos: [RuntimeInfo?] { [model.runtime(agentRuntime), model.runtime(plannerRuntime)] }

    private var readinessNote: String? {
        NewSimulationRules.readinessNote(engineRunning: model.engine.state.isRunning, runtimes: runtimeInfos,
                                         ids: [agentRuntime, plannerRuntime], anyReady: model.recommendedRuntime != nil)
    }

    /// The chosen tool cannot run (missing or signed out): offer the setup step next to the note.
    private var needsAgentSetup: Bool {
        model.engine.state.isRunning && runtimeInfos.contains {
            $0.map { RuntimeRecommendation.readiness($0) != .ready } ?? true
        }
    }

    private var disabledReason: String? {
        NewSimulationRules.disabledReason(
            question: question, description: descriptionText, fileCount: files.count,
            engineRunning: model.engine.state.isRunning,
            runtimeProblem: NewSimulationRules.runtimeBlocks(runtimes: runtimeInfos),
            modelsChosen: !agentModel.isEmpty && !plannerModel.isEmpty)
    }

    private var summary: String {
        NewSimulationRules.summary(
            agents: agents, rounds: rounds, agentRuntime: model.runtime(agentRuntime), agentRuntimeID: agentRuntime,
            agentModel: agentModel, plannerRuntime: model.runtime(plannerRuntime), plannerRuntimeID: plannerRuntime,
            plannerModel: plannerModel,
            noToolYet: model.engine.state.isRunning && !model.runtimes.isEmpty && model.recommendedRuntime == nil
                && NewSimulationRules.runtimeBlocks(runtimes: runtimeInfos))
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
        if !files.isEmpty { aboutMode = .files }
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
        useDefaultRuntimes()
        if let prefill = model.newSimulationPrefill {
            model.newSimulationPrefill = nil
            apply(prefill)
        }
        focus = NewSimulationRules.trimmed(question).isEmpty ? .question : (aboutMode == .describe ? .description : nil)
    }

    /// The runtimes and models from Settings > Agents (or the suggested tool when that one is not installed).
    private func useDefaultRuntimes() {
        agentRuntime = model.defaultAgentRuntime
        plannerRuntime = model.defaultPlannerRuntime
        agentModel = model.defaultModel(runtime: agentRuntime, role: "agent")
        plannerModel = model.defaultModel(runtime: plannerRuntime, role: "planner")
    }

    /// Fills the form from a question typed in the empty state, or from an earlier run's configuration.
    private func apply(_ prefill: NewSimulationPrefill) {
        question = prefill.question
        descriptionText = prefill.descriptionText
        guard let c = prefill.config else { return }
        agents = c.numAgents
        rounds = c.rounds
        batch = c.batchSize
        concurrency = c.concurrency
        language = c.language
        think = c.think
        seed = c.seed
        events = c.events.map { PlannedEvent(round: min(max(1, $0.round), c.rounds), content: $0.content) }
        if model.runtime(c.agentRuntime) != nil { agentRuntime = c.agentRuntime; agentModel = c.agentModel }
        if model.runtime(c.plannerRuntime) != nil { plannerRuntime = c.plannerRuntime; plannerModel = c.plannerModel }
        runtimesChosen = true
        files = prefill.seedFiles
        missingSeedNames = prefill.missingSeedNames
        if !files.isEmpty || !missingSeedNames.isEmpty { aboutMode = .files }
    }

    private func submit() async {
        guard disabledReason == nil, !isSubmitting else { return }
        isSubmitting = true
        errorMessage = nil
        defer { isSubmitting = false }
        let text = NewSimulationRules.trimmed(descriptionText)
        let request = NewRunRequest(
            files: files, question: NewSimulationRules.trimmed(question), agents: agents, rounds: rounds, batch: batch,
            agentRuntime: agentRuntime, agentModel: agentModel, plannerRuntime: plannerRuntime,
            plannerModel: plannerModel, concurrency: concurrency,
            language: language.trimmingCharacters(in: .whitespaces).isEmpty ? "English" : language,
            think: think,
            events: events.map { GodEvent(round: $0.round, content: $0.content.trimmingCharacters(in: .whitespacesAndNewlines)) }
                .filter { !$0.content.isEmpty },
            seed: seed,
            inlineDocuments: text.isEmpty ? [] : [.description(text)])
        do {
            _ = try await model.createRun(request)
            dismiss()
        } catch {
            errorMessage = error.localizedDescription
        }
    }
}

/// One inline note about the engine or the chosen runtime, with the fix and an optional button for it.
private struct ReadinessNote: View {
    let text: String
    var actionTitle: String?
    var action: () -> Void = {}

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: Theme.space2) {
            Image(systemName: "info.circle")
                .foregroundStyle(Theme.muted)
                .accessibilityHidden(true)
            Text(text)
                .font(Theme.body)
                .foregroundStyle(Theme.secondary)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
            if let actionTitle {
                Button(actionTitle, action: action)
                    .buttonStyle(.console(.secondary, compact: true))
                    .fixedSize()
            }
        }
        .padding(Theme.space3)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.hover, in: RoundedRectangle(cornerRadius: Theme.radius))
        .accessibilityElement(children: .combine)
    }
}

/// A number you can type or step, kept inside `range`.
private struct NumberField: View {
    @Binding var value: Int
    let range: ClosedRange<Int>
    let label: String
    var width: CGFloat = 64

    var body: some View {
        let clamped = Binding<Int>(get: { value }, set: { value = min(max($0, range.lowerBound), range.upperBound) })
        HStack(spacing: 6) {
            TextField(label, value: clamped, format: .number.grouping(.never))
                .labelsHidden()
                .multilineTextAlignment(.trailing)
                .font(Theme.mono(12))
                .consoleField()
                .frame(width: width)
            Stepper(label, value: clamped, in: range)
                .labelsHidden()
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(label)
    }
}
