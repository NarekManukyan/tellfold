import SwiftUI

/// The main window: run list in the sidebar, the selected run in the detail column.
/// The run detail itself (header, Network / Knowledge Graph / Feed / Report, timeline, inspector) is
/// `RunDetailView(store:)` from the run views.
struct MainView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        @Bindable var model = model
        NavigationSplitView {
            RunSidebar()
                .navigationSplitViewColumnWidth(min: 240, ideal: 290, max: 420)
        } detail: {
            detail
                .background(Theme.ground)
        }
        .sheet(isPresented: $model.isNewSimulationPresented) {
            NewSimulationSheet()
        }
        .sheet(isPresented: uvMissingBinding) {
            UVOnboardingView()
        }
        .onAppear { model.openWindowAction = openWindow }
    }

    @ViewBuilder private var detail: some View {
        if let store = model.currentStore {
            RunDetailContainer(store: store)
                .id(store.runID)
        } else if !model.engine.state.isRunning {
            EngineUnavailableView()
        } else if model.hasLoadedRuns && model.runs.isEmpty {
            ContentUnavailableView {
                Label("No Simulations Yet", systemImage: "circle.hexagongrid")
            } description: {
                Text("Start one from a few seed documents and a question about how people will react.")
            } actions: {
                Button("New Simulation…") { model.newSimulation() }
                    .buttonStyle(.consolePrimary)
            }
        } else {
            ContentUnavailableView("No Simulation Selected", systemImage: "sidebar.left",
                                   description: Text("Choose a simulation in the sidebar."))
        }
    }

    private var uvMissingBinding: Binding<Bool> {
        Binding(get: { model.engine.state == .uvMissing }, set: { _ in })
    }
}

/// Hosts `RunDetailView`, publishes the store to the Simulation menu and presents the Inject Event sheet.
private struct RunDetailContainer: View {
    @Bindable var store: RunStore

    var body: some View {
        Group {
            switch store.loadState {
            case .idle, .loading:
                if store.detail == nil {
                    ProgressView("Loading simulation…")
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                } else {
                    RunDetailView(store: store)
                }
            case .failed(let message) where store.detail == nil:
                ContentUnavailableView {
                    Label("Could Not Load Simulation", systemImage: "exclamationmark.triangle")
                } description: {
                    Text(message)
                } actions: {
                    Button("Try Again") { Task { await store.load() } }
                }
            default:
                RunDetailView(store: store)
            }
        }
        .focusedSceneValue(\.runStore, store)
        .sheet(isPresented: $store.showsInjectEvent) {
            InjectEventSheet(store: store)
        }
        .alert("Action Failed", isPresented: Binding(get: { store.actionError != nil },
                                                     set: { if !$0 { store.actionError = nil } })) {
            Button("OK", role: .cancel) { store.actionError = nil }
        } message: {
            Text(store.actionError ?? "")
        }
    }
}

/// Shown in the detail column while the engine is starting, restarting or broken.
struct EngineUnavailableView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        switch model.engine.state {
        case .starting, .running:
            ProgressView("Starting the Shoal engine…")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        case .waitingToRestart(let attempt, let seconds):
            ContentUnavailableView {
                Label("Engine Restarting", systemImage: "arrow.clockwise")
            } description: {
                Text("The engine stopped unexpectedly. Retrying in \(seconds) s (attempt \(attempt)).")
            } actions: {
                Button("Open Diagnostics") { openWindow(id: "diagnostics") }
            }
        case .failed(let message):
            ContentUnavailableView {
                Label("Engine Unavailable", systemImage: "exclamationmark.triangle")
            } description: {
                Text(message)
            } actions: {
                HStack {
                    Button("Try Again") { model.engine.start() }
                    Button("Open Diagnostics") { openWindow(id: "diagnostics") }
                }
            }
        case .uvMissing:
            ContentUnavailableView("uv Is Required", systemImage: "shippingbox",
                                   description: Text("Shoal runs its engine with uv. Install it, then try again."))
        case .stopped:
            ContentUnavailableView {
                Label("Engine Stopped", systemImage: "stop.circle")
            } actions: {
                Button("Start Engine") { model.engine.start() }
            }
        }
    }
}

/// A run state with its symbol, e.g. in the sidebar.
struct RunStateBadge: View {
    let state: RunState

    var body: some View {
        Label(state.title, systemImage: state.systemImage)
            .labelStyle(.iconOnly)
            .foregroundStyle(color)
            .help(state.title)
            .accessibilityLabel(state.title)
    }

    private var color: Color {
        switch state {
        case .running, .queued: Theme.accent
        case .done: Theme.muted
        case .failed: Theme.negative
        case .stopped, .unknown: Theme.muted
        }
    }
}
