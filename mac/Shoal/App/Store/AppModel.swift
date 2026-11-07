import AppKit
import Foundation
import Observation
import SwiftUI
import UniformTypeIdentifiers

/// App-wide state: preferences, the engine, the run list, the selected run's `RunStore`, runtimes and
/// engine settings, and the New Simulation sheet.
@MainActor @Observable
final class AppModel {
    let prefs: AppPreferences
    let engine: EngineController
    @ObservationIgnored let notifier = Notifier()

    private(set) var runs: [RunSummary] = []
    private(set) var runsError: String?
    private(set) var hasLoadedRuns = false
    /// Rounds found in the recorded data of stopped and failed runs, fetched once each: the list's status can lag what the
    /// run actually recorded, and the open view trusts the recorded rounds.
    private(set) var recordedRounds: [String: Int] = [:]
    @ObservationIgnored private var reconciling: Set<String> = []
    var searchText = ""

    var selectedRunID: String? {
        didSet { if oldValue != selectedRunID { makeStore() } }
    }
    /// The store for `selectedRunID`.
    private(set) var currentStore: RunStore?

    var isNewSimulationPresented = false
    /// What the sheet opens with (a question from the empty state, or a run's settings for "Run again…"); the sheet
    /// takes it when it appears.
    var newSimulationPrefill: NewSimulationPrefill?
    /// Seed files waiting for the New Simulation sheet (Dock drop, File > Open).
    var pendingSeedFiles: [URL] = []

    private(set) var runtimes: [RuntimeInfo] = []
    private(set) var runtimesError: String?
    private(set) var isLoadingRuntimes = false
    /// True when the engine has no `/api/runtimes` (older engine): only Claude Code is offered.
    private(set) var runtimesUnsupported = false
    var engineSettings: EngineSettings?
    private(set) var engineSettingsError: String?
    /// The last settings the engine confirmed; a failed save reverts to them.
    @ObservationIgnored private var confirmedEngineSettings: EngineSettings?
    /// One save in flight at a time, newest local value wins (see `updateEngineSettings`).
    @ObservationIgnored private lazy var settingsSaver = makeSettingsSaver()

    @ObservationIgnored var openWindowAction: OpenWindowAction?
    @ObservationIgnored private var pollTask: Task<Void, Never>?
    @ObservationIgnored private var knownStates: [String: RunState] = [:]

    var api: ShoalAPI? { engine.api }

    init(prefs: AppPreferences = AppPreferences()) {
        self.prefs = prefs
        engine = EngineController(prefs: prefs)
    }

    // MARK: Lifecycle

    func start() {
        notifier.activate()
        notifier.onOpenRun = { [weak self] id in
            self?.selectedRunID = id
            NSApp.activate()
        }
        engine.onAPIChange = { [weak self] api in self?.apiChanged(api) }
        engine.start()
        requestNotificationPermissionIfNeeded()
    }

    func shutdown() {
        pollTask?.cancel()
        currentStore?.stopStreaming()
        engine.shutdownNow()
    }

    private func apiChanged(_ api: ShoalAPI?) {
        pollTask?.cancel()
        pollTask = nil
        makeStore()
        guard api != nil else { return }
        Task {
            await refreshRuns()
            await loadRuntimes()
            await loadEngineSettings()
        }
        startPolling()
    }

    func requestNotificationPermissionIfNeeded() {
        guard prefs.notificationsEnabled, !prefs.askedNotificationPermission else { return }
        prefs.askedNotificationPermission = true
        Task { _ = await notifier.requestAuthorization() }
    }

    // MARK: Runs

    func refreshRuns() async {
        guard let api else { return }
        do {
            let list = try await api.runs()
            notifyTransitions(list)
            runs = list
            reconcileRounds(list)
            runsError = nil
            hasLoadedRuns = true
            if let id = selectedRunID, !list.contains(where: { $0.id == id }) { selectedRunID = nil }
        } catch is CancellationError {
        } catch {
            runsError = error.localizedDescription
        }
    }

    /// One detail fetch per stopped or failed run, so its row shows the same rounds as the open run would.
    private func reconcileRounds(_ list: [RunSummary]) {
        guard let api else { return }
        for run in list where (run.state == .stopped || run.state == .failed)
            && recordedRounds[run.id] == nil && !reconciling.contains(run.id) {
            reconciling.insert(run.id)
            Task { [weak self] in
                async let detailCall = api.run(run.id)
                async let postsCall = api.posts(runID: run.id, sinceID: 0)
                async let actionsCall = api.actions(runID: run.id)
                let fetched = try? await (detailCall, postsCall, actionsCall)
                guard let self else { return }
                self.reconciling.remove(run.id)
                if let (detail, posts, actions) = fetched {
                    // The same rule as `RunStore.latestRound`, from the fetched data.
                    self.recordedRounds[run.id] = RunRounds.latest(
                        status: run.round, stats: detail.stats.map(\.round), posts: posts.map(\.round),
                        actions: actions.map(\.round))
                }
            }
        }
    }

    /// Rounds a run completed: the open run's loaded data, else the cached recorded rounds, else the list's status.
    func roundsDone(_ run: RunSummary) -> Int {
        if let store = currentStore, store.runID == run.id { return max(run.round, store.roundsCompleted) }
        return max(run.round, recordedRounds[run.id] ?? 0)
    }

    private func startPolling() {
        pollTask = Task { [weak self] in
            // Tick every 2 s and refresh while a run is active, otherwise every 10 s. Deciding before a long sleep
            // left the sidebar up to 10 s behind a run started right after an idle refresh.
            var idleTicks = 0
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(2))
                guard !Task.isCancelled, let self else { return }
                let active = self.runs.contains { $0.state.isActive }
                idleTicks += 1
                guard active || idleTicks >= 5 else { continue }
                idleTicks = 0
                await self.refreshRuns()
            }
        }
    }

    private func notifyTransitions(_ list: [RunSummary]) {
        defer { knownStates = Dictionary(list.map { ($0.id, $0.state) }, uniquingKeysWith: { a, _ in a }) }
        guard hasLoadedRuns, prefs.notificationsEnabled else { return }
        for run in list where run.state.isTerminal {
            if let previous = knownStates[run.id], previous.isActive {
                notifier.notifyFinished(run: run)
            }
        }
    }

    private func makeStore() {
        currentStore?.stopStreaming()
        // Without an engine a store could never load (it would spin forever) and would hide the engine's own
        // status view; keep the selection and build the store once the engine is back (`apiChanged`).
        guard let id = selectedRunID, let api else {
            currentStore = nil
            return
        }
        let store = RunStore(runID: id, api: api)
        currentStore = store
        Task { await store.load() }
    }

    func summary(_ id: String) -> RunSummary? { runs.first { $0.id == id } }

    enum RunGroup: String, CaseIterable, Identifiable {
        case running = "Running", recent = "Recent", older = "Older"
        var id: String { rawValue }
    }

    /// Runs filtered by `searchText` and grouped Running / Recent (last 7 days) / Older.
    var groupedRuns: [(group: RunGroup, runs: [RunSummary])] {
        let query = searchText.trimmingCharacters(in: .whitespaces).lowercased()
        let filtered = query.isEmpty ? runs : runs.filter {
            $0.question.lowercased().contains(query) || $0.id.lowercased().contains(query)
        }
        let cutoff = Date.now.addingTimeInterval(-7 * 24 * 3600)
        var groups: [RunGroup: [RunSummary]] = [:]
        for run in filtered {
            let group: RunGroup = run.state.isActive ? .running : ((run.createdDate ?? .distantPast) >= cutoff ? .recent : .older)
            groups[group, default: []].append(run)
        }
        return RunGroup.allCases.compactMap { g in groups[g].map { (g, $0) } }
    }

    func runDirectory(_ id: String) -> URL {
        prefs.resolvedRunsFolder.appending(path: id, directoryHint: .isDirectory)
    }

    func revealInFinder(_ id: String) {
        let dir = runDirectory(id)
        if FileManager.default.fileExists(atPath: dir.path) {
            NSWorkspace.shared.activateFileViewerSelecting([dir])
        }
    }

    /// Moves the run folder to the Trash. Live runs must be stopped first.
    func deleteRun(_ id: String) async throws {
        if let run = summary(id), run.state.isActive {
            throw ShoalAPIError(status: 409, message: "Stop the simulation before deleting it.")
        }
        let dir = runDirectory(id)
        guard !id.isEmpty, !id.contains("/"), !id.contains(".."),
              dir.standardizedFileURL.deletingLastPathComponent().path == prefs.resolvedRunsFolder.standardizedFileURL.path,
              FileManager.default.fileExists(atPath: dir.appending(path: "config.json").path) else {
            throw ShoalAPIError(status: 400, message: "That folder is not a run in the runs folder.")
        }
        if selectedRunID == id { selectedRunID = nil }
        try FileManager.default.trashItem(at: dir, resultingItemURL: nil)
        await refreshRuns()
    }

    func rebuildReport(_ id: String) async throws {
        guard let api else { throw ShoalAPIError(status: 0, message: "The engine is not running.") }
        try await api.rebuildReport(runID: id)
        await refreshRuns()
    }

    /// Opens `report.html` in the default browser (from the run folder, or fetched through the API).
    func openHTMLReport(_ id: String) async throws {
        let local = runDirectory(id).appending(path: "report.html")
        if FileManager.default.fileExists(atPath: local.path) {
            NSWorkspace.shared.open(local)
            return
        }
        let store = currentStore?.runID == id ? currentStore! : RunStore(runID: id, api: api)
        NSWorkspace.shared.open(try await store.reportHTMLFile())
    }

    // MARK: New simulation

    func createRun(_ request: NewRunRequest) async throws -> String {
        guard let api else { throw ShoalAPIError(status: 0, message: "The engine is not running.") }
        let id = try await api.createRun(request)
        await refreshRuns()
        selectedRunID = id
        return id
    }

    func newSimulation(prefill: NewSimulationPrefill? = nil) {
        newSimulationPrefill = prefill
        isNewSimulationPresented = true
    }

    /// Opens the sheet with a finished, stopped or failed run's question, size and options, and its seed documents
    /// downloaded from the engine into a temporary folder.
    func runAgain(runID: String, config: SimConfig) {
        Task { await openRunAgain(runID: runID, config: config) }
    }

    func openRunAgain(runID: String, config: SimConfig) async {
        var urls: [URL] = []
        var failed: [String] = []
        if let api {
            do {
                let list = try await api.seeds(runID: runID)
                if !list.isEmpty {
                    let folder = FileManager.default.temporaryDirectory.appending(path: "shoal-run-again-\(UUID().uuidString)")
                    try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
                    for item in list {
                        do {
                            let data = try await api.seedData(runID: runID, name: item.name)
                            let url = folder.appending(path: item.name)
                            try data.write(to: url)
                            urls.append(url)
                        } catch {
                            failed.append(item.name)
                        }
                    }
                }
            } catch {
                // An engine without the endpoint is treated like an older run: the sheet asks for the documents again.
            }
        }
        newSimulation(prefill: .runAgain(config: config, seedFiles: urls, unavailable: failed))
    }

    static let seedContentTypes: [UTType] = [
        UTType(filenameExtension: "md") ?? .plainText, .plainText, .pdf,
    ]

    static func isSeedFile(_ url: URL) -> Bool {
        NewRunRequest.seedExtensions.contains(url.pathExtension.lowercased())
    }

    /// Files dropped on the Dock icon or chosen with File > Open: queue them and open the sheet.
    func openSeedFiles(_ urls: [URL]) {
        let seeds = urls.filter(Self.isSeedFile)
        guard !seeds.isEmpty else { return }
        for url in seeds where !pendingSeedFiles.contains(url) { pendingSeedFiles.append(url) }
        NSApp.activate()
        if !NSApp.windows.contains(where: { $0.isVisible && $0.identifier?.rawValue.hasPrefix("main") == true }) {
            openWindowAction?(id: "main")
        }
        isNewSimulationPresented = true
    }

    func presentOpenPanel() {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = true
        panel.canChooseDirectories = false
        panel.allowedContentTypes = Self.seedContentTypes
        panel.message = "Choose seed documents (.md, .txt or .pdf) for a new simulation."
        panel.prompt = "Use as Seeds"
        if panel.runModal() == .OK { openSeedFiles(panel.urls) }
    }

    // MARK: Runtimes and engine settings

    func loadRuntimes(checkAuth: Bool = false) async {
        guard let api else { return }
        isLoadingRuntimes = true
        defer { isLoadingRuntimes = false }
        do {
            runtimes = try await api.runtimes(checkAuth: checkAuth)
            runtimesError = nil
            runtimesUnsupported = false
        } catch let error as ShoalAPIError where error.isNotFound {
            runtimes = [.claudeCodeFallback]
            runtimesUnsupported = true
            runtimesError = nil
        } catch is CancellationError {
        } catch {
            runtimesError = error.localizedDescription
            if runtimes.isEmpty { runtimes = [.claudeCodeFallback] }
        }
    }

    /// Runtimes for pickers, never empty.
    var availableRuntimes: [RuntimeInfo] { runtimes.isEmpty ? [.claudeCodeFallback] : runtimes }

    func runtime(_ id: String) -> RuntimeInfo? { availableRuntimes.first { $0.id == id } }

    func loadEngineSettings() async {
        guard let api else { return }
        do {
            let settings = try await api.settings()
            confirmedEngineSettings = settings
            if !settingsSaver.isBusy { engineSettings = settings }
            engineSettingsError = nil
        } catch let error as ShoalAPIError where error.isNotFound {
            engineSettings = nil
            engineSettingsError = "This engine version has no settings endpoint."
        } catch is CancellationError {
        } catch {
            engineSettingsError = error.localizedDescription
        }
    }

    /// Updates one engine setting with `change` (shown at once) and saves it. Saves run one at a time and
    /// coalesce: a picker that sends two changes back to back (runtime, then its default model) ends with the
    /// server holding both, never an older PUT landing last. A response is applied only when no newer local change
    /// is waiting; a failed save reverts to the last settings the engine confirmed.
    func updateEngineSettings(_ change: (inout EngineSettings) -> Void) {
        var settings = engineSettings ?? EngineSettings()
        change(&settings)
        engineSettings = settings
        settingsSaver.submit(settings)
    }

    private func makeSettingsSaver() -> LatestValueSaver<EngineSettings> {
        LatestValueSaver(
            save: { [weak self] settings in
                guard let api = self?.api else { throw ShoalAPIError(status: 0, message: "The engine is not running.") }
                return try await api.updateSettings(settings)
            },
            onSaved: { [weak self] saved, isLatest in
                guard let self else { return }
                self.confirmedEngineSettings = saved
                self.engineSettingsError = nil
                if isLatest { self.engineSettings = saved }
            },
            onFailed: { [weak self] error, isLatest in
                guard let self else { return }
                self.engineSettingsError = error.localizedDescription
                if isLatest { self.engineSettings = self.confirmedEngineSettings }
            })
    }

    /// Waits until every queued settings save has finished.
    func waitForEngineSettingsSaves() async { await settingsSaver.waitUntilIdle() }

    // MARK: Defaults for the New Simulation sheet

    var defaultAgentRuntime: String { engineSettings?.agentRuntime ?? "claude-code" }
    var defaultPlannerRuntime: String { engineSettings?.plannerRuntime ?? "claude-code" }

    func defaultModel(runtime: String, role: String) -> String {
        if role == "agent", runtime == defaultAgentRuntime, let m = engineSettings?.agentModel { return m }
        if role == "planner", runtime == defaultPlannerRuntime, let m = engineSettings?.plannerModel { return m }
        let info = self.runtime(runtime)
        return info?.defaultModels[role] ?? info?.models.first ?? (role == "agent" ? "haiku" : "sonnet")
    }
}
