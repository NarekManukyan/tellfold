import SwiftUI

/// Settings > Agents & Models: detected runtimes with a Test button, default runtime + model per role,
/// Ollama URL, OpenAI-compatible endpoints, and API keys (Keychain).
struct AgentsSettingsTab: View {
    @Environment(AppModel.self) private var model
    @State private var testResults: [String: TestState] = [:]
    @State private var editingEndpoint: OpenAICompatibleEndpoint?
    @State private var isAddingEndpoint = false
    @State private var keysChanged = false
    @State private var keyError: String?

    enum TestState: Hashable {
        case running
        case finished(RuntimeTestResult)
        case error(String)
    }

    var body: some View {
        ScrollView {
            VStack(spacing: 0) {
                runtimesSection
                defaultsSection
                ollamaSection
                endpointsSection
                keysSection
            }
        }
        .scrollContentBackground(.hidden)
        .background(Theme.panel)
        .frame(minHeight: 560)
        .task {
            if model.runtimes.isEmpty { await model.loadRuntimes() }
            if model.engineSettings == nil { await model.loadEngineSettings() }
        }
        .sheet(item: $editingEndpoint) { endpoint in
            EndpointEditor(endpoint: endpoint, isNew: isAddingEndpoint) { saved, key in
                save(endpoint: saved, key: key, replacing: isAddingEndpoint ? nil : endpoint.id)
            }
        }
    }

    // MARK: Runtimes

    private var runtimesSection: some View {
        ConsoleSection("Detected runtimes", footer: "Test makes one tiny real call through the runtime.") {
            HStack(spacing: Theme.space3) {
                if model.isLoadingRuntimes {
                    ProgressView().controlSize(.small)
                }
                Button("Check sign-in") { Task { await model.loadRuntimes(checkAuth: true) } }
                    .buttonStyle(.consoleLink)
                    .disabled(!model.engine.state.isRunning || model.isLoadingRuntimes)
                Button("Refresh") { Task { await model.loadRuntimes() } }
                    .buttonStyle(.consoleLink)
                    .disabled(!model.engine.state.isRunning || model.isLoadingRuntimes)
            }
        } content: {
            if !model.engine.state.isRunning {
                Text("Start the engine to detect runtimes.")
                    .font(Theme.small)
                    .foregroundStyle(Theme.muted)
            } else if model.runtimesUnsupported {
                Text("This engine version cannot detect runtimes; only Claude Code is available.")
                    .font(Theme.small)
                    .foregroundStyle(Theme.muted)
            } else if let error = model.runtimesError {
                Text(error).font(Theme.small).foregroundStyle(Theme.negative)
            }
            VStack(spacing: 0) {
                RuntimeTableHeader()
                ForEach(model.availableRuntimes) { runtime in
                    Seam()
                    RuntimeRow(runtime: runtime, test: testResults[runtime.id]) {
                        Task { await test(runtime) }
                    }
                }
            }
            .overlay(RoundedRectangle(cornerRadius: Theme.radius).strokeBorder(Theme.seam, lineWidth: 1))
            .clipShape(RoundedRectangle(cornerRadius: Theme.radius))
        }
    }

    private func test(_ runtime: RuntimeInfo) async {
        guard let api = model.api else { return }
        testResults[runtime.id] = .running
        do {
            testResults[runtime.id] = .finished(try await api.testRuntime(id: runtime.id))
        } catch {
            testResults[runtime.id] = .error(error.localizedDescription)
        }
    }

    // MARK: Defaults

    private var defaultsSection: some View {
        ConsoleSection("Defaults", footer: "Agents make per-round decisions, score sentiment and answer interviews. The planner builds the graph, personas and report and answers questions about it.") {
            if let error = model.engineSettingsError {
                Text(error).font(Theme.small).foregroundStyle(Theme.muted)
            }
            roleRow(title: "Agents", runtime: settingsBinding(\.agentRuntime, default: "claude-code"),
                    modelName: settingsBinding(\.agentModel, default: "haiku"), role: "agent")
            roleRow(title: "Planner", runtime: settingsBinding(\.plannerRuntime, default: "claude-code"),
                    modelName: settingsBinding(\.plannerModel, default: "sonnet"), role: "planner")
        }
        .disabled(model.engineSettings == nil)
    }

    private func roleRow(title: String, runtime: Binding<String>, modelName: Binding<String>, role: String) -> some View {
        LabeledContent(title) {
            HStack {
                Picker("Runtime", selection: runtime) {
                    ForEach(model.availableRuntimes) { rt in
                        Text(rt.installed ? rt.name : "\(rt.name) (not installed)").tag(rt.id).disabled(!rt.installed)
                    }
                    if model.runtime(runtime.wrappedValue) == nil {
                        Text(runtime.wrappedValue).tag(runtime.wrappedValue)
                    }
                }
                .labelsHidden()
                .onChange(of: runtime.wrappedValue) { _, new in
                    let info = model.runtime(new)
                    modelName.wrappedValue = info?.defaultModels[role] ?? info?.models.first ?? modelName.wrappedValue
                }
                let models = model.runtime(runtime.wrappedValue)?.models ?? []
                if models.isEmpty {
                    TextField("Model", text: modelName, prompt: Text("Model"))
                        .labelsHidden()
                        .consoleField()
                        .frame(width: 160)
                } else {
                    Picker("Model", selection: modelName) {
                        ForEach(models, id: \.self) { Text($0).tag($0) }
                        if !models.contains(modelName.wrappedValue) {
                            Text(modelName.wrappedValue).tag(modelName.wrappedValue)
                        }
                    }
                    .labelsHidden()
                    .frame(width: 180)
                }
            }
        }
    }

    private func settingsBinding(_ keyPath: WritableKeyPath<EngineSettings, String?>, default fallback: String) -> Binding<String> {
        Binding(
            get: { model.engineSettings?[keyPath: keyPath] ?? fallback },
            set: { value in model.updateEngineSettings { $0[keyPath: keyPath] = value } })
    }

    // MARK: Ollama

    private var ollamaSection: some View {
        ConsoleSection("Ollama") {
            OllamaURLField(initial: model.engineSettings?.ollamaBaseURL ?? "") { url in
                model.updateEngineSettings { $0.ollamaBaseURL = url.isEmpty ? "http://127.0.0.1:11434" : url }
                Task { await model.loadRuntimes() }
            }
            .disabled(model.engineSettings == nil)
        }
    }

    // MARK: Endpoints

    private var endpointsSection: some View {
        ConsoleSection("OpenAI-compatible endpoints") {
            Button("Add…") {
                isAddingEndpoint = true
                editingEndpoint = OpenAICompatibleEndpoint(id: "", name: "", baseURL: "http://127.0.0.1:1234/v1")
            }
            .buttonStyle(.consoleLink)
            .disabled(model.engineSettings == nil)
        } content: {
            let endpoints = model.engineSettings?.openAICompatible ?? []
            if endpoints.isEmpty {
                Text("None. Add LM Studio, llama.cpp, vLLM, OpenRouter, DeepSeek or any other OpenAI-compatible server.")
                    .font(Theme.small)
                    .foregroundStyle(Theme.muted)
            }
            ForEach(endpoints) { endpoint in
                HStack {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(endpoint.name).font(Theme.body).foregroundStyle(Theme.text)
                        Text(endpoint.baseURL)
                            .font(Theme.mono(11))
                            .foregroundStyle(Theme.muted)
                            .textSelection(.enabled)
                    }
                    Spacer()
                    if let env = endpoint.keyEnv, Keychain.apiKeys.contains(env) {
                        Image(systemName: "key")
                            .foregroundStyle(Theme.muted)
                            .help("API key stored in the Keychain")
                            .accessibilityLabel("Has API key")
                    }
                    Button("Edit…") {
                        isAddingEndpoint = false
                        editingEndpoint = endpoint
                    }
                    .buttonStyle(.console(.secondary, compact: true))
                    Button(role: .destructive) {
                        remove(endpoint)
                    } label: {
                        Image(systemName: "minus")
                            .font(.system(size: 11, weight: .bold))
                            .foregroundStyle(Theme.negative)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Remove \(endpoint.name)")
                }
            }
        }
    }

    private func save(endpoint: OpenAICompatibleEndpoint, key: String?, replacing oldID: String?) {
        var endpoint = endpoint
        let env = Keychain.envName(forProvider: endpoint.id)
        if let key {
            do {
                try Keychain.apiKeys.set(key, for: env)
                keysChanged = true
            } catch {
                keyError = error.localizedDescription
            }
        }
        endpoint.keyEnv = Keychain.apiKeys.contains(env) ? env : nil
        model.updateEngineSettings { settings in
            var list = settings.openAICompatible
            if let oldID, let i = list.firstIndex(where: { $0.id == oldID }) {
                list[i] = endpoint
            } else {
                list.removeAll { $0.id == endpoint.id }
                list.append(endpoint)
            }
            settings.openAICompatible = list
        }
        Task { await model.loadRuntimes() }
    }

    private func remove(_ endpoint: OpenAICompatibleEndpoint) {
        model.updateEngineSettings { settings in
            settings.openAICompatible = settings.openAICompatible.filter { $0.id != endpoint.id }
        }
        if let env = endpoint.keyEnv { try? Keychain.apiKeys.delete(env) }
        Task { await model.loadRuntimes() }
    }

    // MARK: Keys

    private var keysSection: some View {
        ConsoleSection("API keys", footer: "Keys are stored in your Keychain and given to the engine as environment variables only. They are never written to settings files or logs.") {
            APIKeyField(title: "Anthropic API key", account: Keychain.anthropicAccount) { keysChanged = true }
            APIKeyField(title: "OpenAI API key", account: Keychain.openAIAccount) { keysChanged = true }
            if let keyError {
                Text(keyError).font(Theme.small).foregroundStyle(Theme.negative)
            }
            if keysChanged {
                HStack {
                    Text("The engine picks up new keys after a restart.")
                        .font(Theme.small)
                        .foregroundStyle(Theme.muted)
                    Spacer()
                    Button("Restart engine") {
                        model.engine.restart()
                        keysChanged = false
                    }
                    .buttonStyle(.console)
                }
            }
        }
    }
}

// MARK: - Rows and fields

/// Column widths shared by the runtime table header and its rows.
private enum RuntimeColumns {
    static let id: CGFloat = 104
    static let install: CGFloat = 108
    static let signIn: CGFloat = 92
    static let tested: CGFloat = 80
    static let test: CGFloat = 52
}

private struct RuntimeTableHeader: View {
    var body: some View {
        HStack(spacing: 8) {
            head("ID", width: RuntimeColumns.id)
            head("Runtime", width: nil)
            head("Install", width: RuntimeColumns.install)
            head("Sign-in", width: RuntimeColumns.signIn)
            head("Verified", width: RuntimeColumns.tested)
            Color.clear.frame(width: RuntimeColumns.test, height: 1)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .background(Theme.elevated)
        .accessibilityHidden(true)
    }

    private func head(_ title: String, width: CGFloat?) -> some View {
        Text(title)
            .font(Theme.ui(12, .medium))
            .foregroundStyle(Theme.muted)
            .frame(width: width, alignment: .leading)
            .frame(maxWidth: width == nil ? .infinity : nil, alignment: .leading)
    }
}

/// One detected runtime as a console table row: mono id, name and version, install and sign-in state as symbol plus
/// text, whether it was verified against the real tool, and a Test button. Notes and test results sit under it.
private struct RuntimeRow: View {
    let runtime: RuntimeInfo
    let test: AgentsSettingsTab.TestState?
    let onTest: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(runtime.id)
                    .font(Theme.mono(12))
                    .foregroundStyle(Theme.text)
                    .lineLimit(1)
                    .frame(width: RuntimeColumns.id, alignment: .leading)
                HStack(spacing: 6) {
                    Image(systemName: runtime.kind == .http ? "network" : "terminal")
                        .font(.system(size: 11))
                        .foregroundStyle(Theme.muted)
                        .accessibilityHidden(true)
                    Text(runtime.name).font(Theme.body).foregroundStyle(Theme.text).lineLimit(1)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                state(runtime.installed ? "checkmark" : "xmark", runtime.installed ? "installed" : "not installed",
                      tint: runtime.installed ? Theme.text : Theme.muted)
                    .frame(width: RuntimeColumns.install, alignment: .leading)
                Group {
                    if runtime.installed {
                        state(authSymbol, authTitle, tint: authTint).help(runtime.authHint ?? "")
                    } else {
                        Text("–").font(Theme.mono(12)).foregroundStyle(Theme.faint)
                    }
                }
                .frame(width: RuntimeColumns.signIn, alignment: .leading)
                state(runtime.tested ? "checkmark.seal" : "questionmark", runtime.tested ? "tested" : "untested",
                      tint: runtime.tested ? Theme.text : Theme.muted)
                    .help(runtime.tested ? "Verified against the real tool" : (runtime.notes ?? "Built from the tool's documentation; not verified on a real install"))
                    .frame(width: RuntimeColumns.tested, alignment: .leading)
                Button("Test") { onTest() }
                    .buttonStyle(.console(.secondary, compact: true))
                    .disabled(!runtime.installed || test == .running)
                    .frame(width: RuntimeColumns.test, alignment: .trailing)
            }
            if runtime.path != nil || runtime.version != nil {
                Text([runtime.version, runtime.path].compactMap { $0 }.joined(separator: "  "))
                    .font(Theme.mono(11))
                    .foregroundStyle(Theme.muted)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .textSelection(.enabled)
                    .padding(.leading, RuntimeColumns.id + 8)
            }
            if !runtime.installed || runtime.auth == .missing, let hint = runtime.authHint {
                Text(hint)
                    .font(Theme.caption)
                    .foregroundStyle(Theme.muted)
                    .padding(.leading, RuntimeColumns.id + 8)
            }
            testLine
                .padding(.leading, RuntimeColumns.id + 8)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 7)
        .accessibilityElement(children: .combine)
    }

    private var authTitle: String {
        switch runtime.auth {
        case .ok: "signed in"
        case .missing: "signed out"
        case .unknown: "unknown"
        }
    }

    private var authSymbol: String {
        switch runtime.auth {
        case .ok: "checkmark"
        case .missing: "xmark"
        case .unknown: "questionmark"
        }
    }

    private var authTint: Color {
        switch runtime.auth {
        case .ok: Theme.text
        case .missing: Theme.negative
        case .unknown: Theme.muted
        }
    }

    private func state(_ symbol: String, _ title: String, tint: Color) -> some View {
        HStack(spacing: 5) {
            Image(systemName: symbol).font(.system(size: 9, weight: .bold)).foregroundStyle(tint)
            Text(title).font(Theme.mono(11)).foregroundStyle(Theme.text)
        }
    }

    @ViewBuilder private var testLine: some View {
        switch test {
        case .none:
            EmptyView()
        case .running:
            HStack(spacing: 6) {
                ProgressView().controlSize(.mini)
                Text("Testing…")
            }
            .font(Theme.mono(11))
            .foregroundStyle(Theme.muted)
        case .finished(let result):
            let seconds = result.seconds.map { String(format: "%.1f s", $0) } ?? ""
            HStack(alignment: .firstTextBaseline, spacing: 5) {
                Image(systemName: result.ok ? "checkmark" : "xmark").font(.system(size: 9, weight: .bold))
                Text(result.ok ? "works \(seconds): \(result.reply ?? "")" : "failed: \(result.error ?? "unknown error")")
                    .lineLimit(2)
                    .textSelection(.enabled)
            }
            .font(Theme.mono(11))
            .foregroundStyle(result.ok ? Theme.text : Theme.negative)
        case .error(let message):
            HStack(alignment: .firstTextBaseline, spacing: 5) {
                Image(systemName: "xmark").font(.system(size: 9, weight: .bold))
                Text(message).lineLimit(2)
            }
            .font(Theme.mono(11))
            .foregroundStyle(Theme.negative)
        }
    }
}

private struct OllamaURLField: View {
    @State private var text: String
    let onCommit: (String) -> Void

    init(initial: String, onCommit: @escaping (String) -> Void) {
        _text = State(initialValue: initial)
        self.onCommit = onCommit
    }

    var body: some View {
        HStack {
            TextField("Base URL", text: $text, prompt: Text("http://127.0.0.1:11434"))
                .consoleField()
                .font(Theme.mono(12))
                .onSubmit { onCommit(text.trimmingCharacters(in: .whitespaces)) }
            Button("Apply") { onCommit(text.trimmingCharacters(in: .whitespaces)) }
                .buttonStyle(.console)
        }
    }
}

/// A SecureField bound to one Keychain account. Shows whether a key is stored without revealing it.
private struct APIKeyField: View {
    let title: String
    let account: String
    let onChange: () -> Void
    @State private var value = ""
    @State private var stored = false
    @State private var error: String?

    var body: some View {
        LabeledContent(title) {
            HStack {
                SecureField(title, text: $value, prompt: Text(stored ? "Stored in Keychain" : "Not set"))
                    .labelsHidden()
                    .consoleField()
                    .onSubmit(save)
                Button("Save", action: save)
                    .buttonStyle(.console)
                    .disabled(value.trimmingCharacters(in: .whitespaces).isEmpty)
                if stored {
                    Button("Remove", role: .destructive) {
                        do {
                            try Keychain.apiKeys.delete(account)
                            stored = false
                            onChange()
                        } catch let e {
                            error = e.localizedDescription
                        }
                    }
                    .buttonStyle(.consoleDanger)
                }
            }
        }
        .onAppear { stored = Keychain.apiKeys.contains(account) }
        .help(error ?? "Environment variable \(account)")
    }

    private func save() {
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        do {
            try Keychain.apiKeys.set(trimmed, for: account)
            value = ""
            stored = true
            error = nil
            onChange()
        } catch let e {
            error = e.localizedDescription
        }
    }
}

/// Add or edit one OpenAI-compatible endpoint.
private struct EndpointEditor: View {
    @Environment(\.dismiss) private var dismiss
    @State private var endpoint: OpenAICompatibleEndpoint
    @State private var modelsText: String
    @State private var key = ""
    let isNew: Bool
    let onSave: (OpenAICompatibleEndpoint, String?) -> Void

    init(endpoint: OpenAICompatibleEndpoint, isNew: Bool, onSave: @escaping (OpenAICompatibleEndpoint, String?) -> Void) {
        _endpoint = State(initialValue: endpoint)
        _modelsText = State(initialValue: endpoint.models.joined(separator: ", "))
        self.isNew = isNew
        self.onSave = onSave
    }

    private var idIsValid: Bool { OpenAICompatibleEndpoint.isValidID(endpoint.id) }

    private var urlIsValid: Bool {
        guard let url = URL(string: endpoint.baseURL), let scheme = url.scheme else { return false }
        return (scheme == "http" || scheme == "https") && url.host() != nil
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(isNew ? "Add endpoint" : "Edit endpoint")
                .font(Theme.title)
                .foregroundStyle(Theme.text)
            Form {
                TextField("Name", text: $endpoint.name, prompt: Text("LM Studio"))
                TextField("Identifier", text: $endpoint.id, prompt: Text("lmstudio"))
                    .disabled(!isNew)
                TextField("Base URL", text: $endpoint.baseURL, prompt: Text("http://127.0.0.1:1234/v1"))
                TextField("Models", text: $modelsText, prompt: Text("Comma-separated, optional"))
                SecureField("API key", text: $key, prompt: Text(isNew ? "Optional" : "Leave empty to keep the current key"))
            }
            .formStyle(.columns)
            Text("The key is stored in your Keychain as \(Keychain.envName(forProvider: endpoint.id.isEmpty ? "id" : endpoint.id)).")
                .font(Theme.caption)
                .foregroundStyle(Theme.muted)
            HStack {
                Spacer()
                Button("Cancel", role: .cancel) { dismiss() }
                    .buttonStyle(.console)
                    .keyboardShortcut(.cancelAction)
                Button(isNew ? "Add" : "Save") {
                    var saved = endpoint
                    if isNew { saved.id = OpenAICompatibleEndpoint.normalizedID(saved.id) }
                    saved.name = saved.name.trimmingCharacters(in: .whitespaces).isEmpty ? saved.id : saved.name
                    saved.models = modelsText.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
                    let trimmedKey = key.trimmingCharacters(in: .whitespacesAndNewlines)
                    onSave(saved, trimmedKey.isEmpty ? nil : trimmedKey)
                    dismiss()
                }
                .keyboardShortcut(.defaultAction)
                .buttonStyle(.consolePrimary)
                .disabled(!idIsValid || !urlIsValid)
            }
        }
        .padding(20)
        .frame(width: 460)
        .background(Theme.panel)
    }
}
