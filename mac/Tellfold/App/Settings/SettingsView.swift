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
    @Environment(AppUpdater.self) private var updater
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

            if updater.isAvailable {
                @Bindable var bindable = updater
                Section {
                    Toggle("Check for updates automatically", isOn: $bindable.automaticallyChecks)
                    HStack {
                        Text(lastCheckText)
                            .foregroundStyle(.secondary)
                        Spacer()
                        Button("Check Now") { updater.checkForUpdates() }
                            .disabled(!updater.canCheckForUpdates)
                    }
                } header: {
                    Text("Updates")
                } footer: {
                    Text("Tellfold \(Self.appVersion). New versions download and install from inside the app.")
                }
            }
        }
        .formStyle(.grouped)
        .scrollContentBackground(.hidden)
        .background(Theme.panel)
    }

    private var lastCheckText: String {
        guard let date = updater.lastCheck else { return "Not checked yet" }
        return "Last checked \(date.formatted(.relative(presentation: .named)))"
    }

    private static var appVersion: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "?"
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
                        TextField("Engine path", text: $prefs.enginePath,
                                  prompt: Text(AppPreferences.bundledEngine == nil ? AppPreferences.developmentEnginePath : "Built-in engine"))
                            .labelsHidden()
                            .onSubmit { needsRestart = true }
                        Button("Choose…") { chooseEnginePath() }
                    }
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
                Text(AppPreferences.bundledEngine == nil
                     ? "The engine path is a tellfold binary, the folder holding it, or a Tellfold repository with a release build. Changes apply after a restart."
                     : "Leave the engine path empty to use the engine built into Tellfold. A custom path is a tellfold binary, the folder holding it, or a Tellfold repository with a release build. Changes apply after a restart.")
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
        panel.canChooseFiles = true
        panel.canChooseDirectories = true
        panel.message = "Choose a tellfold engine binary, the folder holding it, or a Tellfold repository."
        panel.prompt = "Use Engine"
        if panel.runModal() == .OK, let url = panel.url {
            model.prefs.enginePath = url.path
            needsRestart = true
        }
    }
}
