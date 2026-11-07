import SwiftUI

/// Detail pane for one run: Overview, Crowd, Report and Details tabs in the toolbar. The Overview answers the
/// question first; the Crowd tab is the big avatar graph with a round scrubber and a side panel for the selected
/// agent, camp or the feed; the Report tab is the full report; Details holds the machinery.
///
/// Replay state lives on the store (`isPlaying`, `replaySpeed`, `followsLive`), so the app's Simulation menu can drive it
/// through `FocusedValues.runStore`. The hosting container (app-core) presents the Inject Event sheet for
/// `store.showsInjectEvent` and the alert for `store.actionError`.
struct RunDetailView: View {
    @Bindable var store: RunStore

    @State private var tab: RunTab = .overview
    @State private var replay = ReplayController()
    @State private var interviews = InterviewSessions()
    @State private var overviewCrowd = CrowdModel()
    @State private var crowd = CrowdModel()
    @State private var confirmStop = false
    @State private var stopping = false
    @Environment(AppModel.self) private var app

    init(store: RunStore, initialTab: RunTab = .overview) {
        self.store = store
        _tab = State(initialValue: initialTab)
    }

    var body: some View {
        content
            .background(Theme.ground)
            .toolbar { toolbar }
            .toolbar(removing: .title)
            .confirmationDialog("Stop this simulation?", isPresented: $confirmStop) {
                Button("Stop Simulation", role: .destructive) { Task { await stop() } }
            } message: {
                Text("The current round finishes, then the run stops. A report is still written if at least one round completed.")
            }
            .environment(\.openURL, OpenURLAction { url in route(url) })
            .environment(\.runNavigate, RunNavigate { tab = $0 })
            .focusedSceneValue(\.runStore, store)
            .task(id: store.runID) {
                replay.attach(store)
                if store.loadState == .idle { await store.load() }
            }
            .onChange(of: store.latestRound) { _, _ in replay.runDidAdvance() }
            .onChange(of: store.isLive) { _, _ in replay.runDidAdvance() }
            .onChange(of: store.isPlaying) { _, _ in replay.sync() }
            .onChange(of: store.replaySpeed) { _, _ in replay.sync() }
            .onChange(of: store.detail?.id) { _, _ in replay.attach(store) }
            .onDisappear { replay.detach() }
    }

    // MARK: content

    @ViewBuilder
    private var content: some View {
        if store.detail != nil {
            switch tab {
            case .overview:
                OverviewView(store: store, replay: replay, crowd: overviewCrowd)
            case .crowd:
                CrowdTabView(store: store, replay: replay, model: crowd, interviews: interviews)
            case .report:
                ReportView(store: store)
            case .details:
                DetailsView(store: store)
            }
        } else if case .failed(let message) = store.loadState {
            ContentUnavailableView {
                Label("Could Not Load This Run", systemImage: "exclamationmark.triangle")
            } description: {
                Text(message)
            } actions: {
                Button("Try Again") { Task { await store.load() } }
            }
        } else {
            ProgressView("Loading run…")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    // MARK: toolbar

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItem(placement: .principal) {
            Picker("View", selection: $tab) {
                ForEach(RunTab.allCases) { tab in
                    Text(tab.title).tag(tab)
                }
            }
            .pickerStyle(.segmented)
            .accessibilityLabel("Run view")
        }
        if store.isLive {
            ToolbarItem(placement: .primaryAction) {
                Button {
                    confirmStop = true
                } label: {
                    if stopping {
                        ProgressView().controlSize(.small)
                    } else {
                        Label("Stop", systemImage: "stop.fill")
                    }
                }
                .help("Stop after the current round (⌘.)")
                .disabled(stopping)
            }
        }
        ToolbarItem(placement: .primaryAction) {
            Menu {
                Button {
                    if let config = store.config { app.runAgain(runID: store.runID, config: config) }
                } label: {
                    Label("Run Again…", systemImage: "arrow.counterclockwise")
                }
                .disabled(!store.canRunAgain)

                Button {
                    store.showsInjectEvent = true
                } label: {
                    Label("Inject Event…", systemImage: "bolt")
                }
                .disabled(!store.isLive || store.latestRound >= store.totalRounds)

                Button {
                    Task { await ReportRebuilder.rebuild(store) }
                } label: {
                    Label("Rebuild Report", systemImage: "arrow.clockwise")
                }
                .disabled(!store.canRebuildReport)

                Divider()

                Button {
                    Task {
                        do { try await app.openHTMLReport(store.runID) } catch { store.actionError = error.localizedDescription }
                    }
                } label: {
                    Label("Export Report as Web Page", systemImage: "square.and.arrow.up")
                }
                .disabled(store.report == nil)

                Button {
                    app.revealInFinder(store.runID)
                } label: {
                    Label("Reveal in Finder", systemImage: "folder")
                }
            } label: {
                Label("More", systemImage: "ellipsis.circle")
            }
            .help("Run again, inject an event, rebuild or export the report")
        }
    }

    // MARK: actions

    private func stop() async {
        stopping = true
        defer { stopping = false }
        await store.stop()  // errors land in store.actionError
    }

    /// Routes `tellfold://agent/<id>` and `tellfold://post/<id>` links from rendered text; other URLs open normally.
    private func route(_ url: URL) -> OpenURLAction.Result {
        switch RunUI.linkTarget(url) {
        case .agent(let id)?:
            let personas = store.personas
            let resolved = personas.first { $0.id == id }?.id
                ?? personas.first { $0.handle.caseInsensitiveCompare(id) == .orderedSame }?.id
                ?? id
            store.selectedAgentID = resolved
            tab = .crowd
            return .handled
        case .post(let id)?:
            if let post = store.posts.first(where: { $0.id == id }) {
                if post.round > store.playhead { replay.scrub(to: post.round) }
                store.selectedAgentID = nil
                store.selectedCampID = nil
                store.selectedPostID = id
                tab = .crowd
            }
            return .handled
        case nil:
            return url.scheme == RunUI.linkScheme ? .handled : .systemAction
        }
    }
}
