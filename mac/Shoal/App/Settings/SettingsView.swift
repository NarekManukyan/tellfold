import AppKit
import SwiftUI

/// The Settings scene: General, Agents & Models, Advanced.
struct SettingsView: View {
    var body: some View {
        TabView {
            Tab("General", systemImage: "gearshape") {
                GeneralSettingsTab()
            }
            Tab("Agents & Models", systemImage: "cpu") {
                AgentsSettingsTab()
            }
            Tab("Advanced", systemImage: "wrench.and.screwdriver") {
                AdvancedSettingsTab()
            }
        }
        .background(Theme.panel)
        .frame(width: 680)
    }
}

// MARK: - General

struct GeneralSettingsTab: View {
    @Environment(AppModel.self) private var model
    @State private var runsFolderChanged = false

    var body: some View {
        @Bindable var prefs = model.prefs
        Form {
            Section {
                LabeledContent("Runs folder") {
                    HStack {
                        Text(prefs.resolvedRunsFolder.path)
                            .lineLimit(1)
                            .truncationMode(.middle)
                            .textSelection(.enabled)
                            .help(prefs.resolvedRunsFolder.path)
                        Spacer()
                        Button("Choose…") { chooseRunsFolder() }
                        Button {
                            NSWorkspace.shared.activateFileViewerSelecting([prefs.resolvedRunsFolder])
                        } label: {
                            Image(systemName: "arrow.right.circle")
                        }
                        .buttonStyle(.borderless)
                        .help("Show in Finder")
                        .accessibilityLabel("Show runs folder in Finder")
                    }
                }
                if runsFolderChanged {
                    HStack {
                        Text("The engine uses the new folder after a restart.")
                            .font(.callout)
                            .foregroundStyle(.secondary)
                        Spacer()
                        Button("Restart Engine") {
                            model.engine.restart()
                            runsFolderChanged = false
                        }
                    }
                }
            } footer: {
                Text("Every simulation is a folder here with its config, world database and report.")
            }

            Section("New Simulations") {
                Stepper(value: $prefs.defaultConcurrency, in: 1...16) {
                    LabeledContent("Parallel model calls") { Text("\(prefs.defaultConcurrency)").monospacedDigit() }
                }
            }

            Section("Notifications") {
                Toggle("Notify me when a simulation finishes or fails", isOn: $prefs.notificationsEnabled)
                    .onChange(of: prefs.notificationsEnabled) { _, on in
                        if on { Task { _ = await model.notifier.requestAuthorization() } }
                    }
            }
        }
        .formStyle(.grouped)
        .scrollContentBackground(.hidden)
        .background(Theme.panel)
    }

    private func chooseRunsFolder() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.canCreateDirectories = true
        panel.directoryURL = model.prefs.resolvedRunsFolder
        panel.prompt = "Use Folder"
        if panel.runModal() == .OK, let url = panel.url {
            model.prefs.runsFolder = url.path
            runsFolderChanged = true
        }
    }
}

// MARK: - Advanced

struct AdvancedSettingsTab: View {
    @Environment(AppModel.self) private var model
    @Environment(\.openWindow) private var openWindow
    @State private var needsRestart = false

    var body: some View {
        @Bindable var prefs = model.prefs
        Form {
            Section {
                LabeledContent("Engine path") {
                    HStack {
                        TextField("Engine path", text: $prefs.enginePath, prompt: Text(AppPreferences.defaultEnginePath))
                            .labelsHidden()
                            .onSubmit { needsRestart = true }
                        Button("Choose…") { chooseEnginePath() }
                    }
                }
                LabeledContent("uv") {
                    TextField("uv path", text: $prefs.uvPath, prompt: Text("Detect automatically"))
                        .labelsHidden()
                        .onSubmit { needsRestart = true }
                }
                Picker("Log level", selection: $prefs.logLevel) {
                    ForEach(AppPreferences.LogLevel.allCases) { Text($0.title).tag($0) }
                }
                .onChange(of: prefs.logLevel) { needsRestart = true }
                Toggle("Use the offline test model (no real model calls)", isOn: $prefs.offlineTestModel)
                    .onChange(of: prefs.offlineTestModel) { needsRestart = true }
            } header: {
                Text("Engine")
            } footer: {
                Text("The engine runs as `uv run --project <engine path> shoal serve`. Changes apply after a restart.")
            }

            Section {
                HStack {
                    Text(needsRestart ? "Restart the engine to apply changes." : model.engine.state.title)
                        .foregroundStyle(.secondary)
                    Spacer()
                    Button("Open Diagnostics") { openWindow(id: "diagnostics") }
                    Button("Restart Engine") {
                        model.engine.restart()
                        needsRestart = false
                    }
                }
            }
        }
        .formStyle(.grouped)
        .scrollContentBackground(.hidden)
        .background(Theme.panel)
    }

    private func chooseEnginePath() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.message = "Choose the Shoal engine folder (the one with pyproject.toml)."
        panel.prompt = "Use Folder"
        if panel.runModal() == .OK, let url = panel.url {
            model.prefs.enginePath = url.path
            needsRestart = true
        }
    }
}
