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

    /// The sidebar selection: a run id, or a study tagged `study:<id>` (`StudySelection`).
    var selectedRunID: String? {
        didSet { if oldValue != selectedRunID { makeStore() } }
    }
    /// The store for `selectedRunID` when a run is selected.
    private(set) var currentStore: RunStore?
    /// The store for the selected study.
    private(set) var currentStudyStore: StudyStore?
    /// Studies (runs of one crowd compared with each other), from `GET /api/studies`. Empty with an older engine.
    private(set) var studies: [StudySummary] = []
    /// The run "Try another version…" makes a new version of; the main window presents the sheet for it.
    var anotherVersionSource: AnotherVersionSource?
    /// A post to show once its run is open (a citation in a study comparison).
    var pendingPostFocus: PostFocus?

    struct PostFocus: Equatable {
        var runID: String
        var postID: Int
    }

    var selectedStudyID: String? { StudySelection.studyID(from: selectedRunID) }
    var studyRows: [StudyRow] { StudyLogic.rows(studies: studies, runs: runs) }

    var isNewSimulationPresented = false
    /// What the sheet opens with (a question from the empty state, or a run's settings for "Run again…"); the sheet
    /// takes it when it appears.
    var newSimulationPrefill: NewSimulationPrefill?
    /// Seed files waiting for the New Simulation sheet (Dock drop, File > Open).
    var pendingSeedFiles: [URL] = []
    /// The run whose report is being exported as PDF, if any; one export at a time.
    private(set) var exportingPDF: String?

    private(set) var runtimes: [RuntimeInfo] = []
    private(set) var runtimesError: String?
    private(set) var isLoadingRuntimes = false
    /// True when the engine has no `/api/runtimes` (older engine): only Claude Code is offered.
    private(set) var runtimesUnsupported = false
    var engineSettings: EngineSettings?
    private(set) var engineSettingsError: String?
    /// The first-run step that suggests an AI tool (also opened from Settings > Agents).
    var isAgentSetupPresented = false
    /// Local model servers that answer on their usual port but are not among the engine's endpoints yet.
    private(set) var localServers: [LocalModelServer] = []
    /// The last settings the engine confirmed; a failed save reverts to them.
    @ObservationIgnored private var confirmedEngineSettings: EngineSettings?
    /// One save in flight at a time, newest local value wins (see `updateEngineSettings`).
    @ObservationIgnored private lazy var settingsSaver = makeSettingsSaver()

    @ObservationIgnored var openWindowAction: OpenWindowAction?
    @ObservationIgnored private var pollTask: Task<Void, Never>?
    @ObservationIgnored private var knownStates: [String: RunState] = [:]

    var api: TellfoldAPI? { engine.api }

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

    private func apiChanged(_ api: TellfoldAPI?) {
        pollTask?.cancel()
        pollTask = nil
        makeStore()
        guard api != nil else { return }
        Task {
            await refreshRuns()
            await loadRuntimes()
            await loadEngineSettings()
            if !prefs.agentSetupDone && !runtimesUnsupported {
                // Behind the first-run sheet, a finished example: closing the sheet shows a real report.
                if selectedRunID == nil, runs.contains(where: { $0.id == Self.firstRunExampleID }) {
                    selectedRunID = Self.firstRunExampleID
                }
                isAgentSetupPresented = true
            }
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
            await refreshStudies()
            if let id = selectedRunID, selectedStudyID == nil, !list.contains(where: { $0.id == id }) { selectedRunID = nil }
            if let id = selectedStudyID, !studies.contains(where: { $0.id == id }) { selectedRunID = nil }
        } catch is CancellationError {
        } catch {
            runsError = error.localizedDescription
        }
    }

    func refreshStudies() async {
        guard let api else { return }
        do {
            studies = try await api.studies()
        } catch let error as TellfoldAPIError where error.isNotFound {
            studies = []  // an engine from before studies
        } catch {
            // keep the last list; the runs list shows the error
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
        currentStudyStore?.stop()
        currentStudyStore = nil
        // Without an engine a store could never load (it would spin forever) and would hide the engine's own
        // status view; keep the selection and build the store once the engine is back (`apiChanged`).
        guard let id = selectedRunID, let api else {
            currentStore = nil
            return
        }
        if let studyID = StudySelection.studyID(from: id) {
            currentStore = nil
            currentStudyStore = StudyStore(studyID: studyID, api: api)
            return
        }
        let store = RunStore(runID: id, api: api)
        currentStore = store
        Task { await store.load() }
    }

    func summary(_ id: String) -> RunSummary? { runs.first { $0.id == id } }

    /// The example opened behind the first-run sheet (`Resources/Examples/corner-cafe`).
    static let firstRunExampleID = "example-corner-cafe"
    static let examplePrefix = "example-"

    func isExample(_ id: String) -> Bool { summary(id)?.example ?? false }

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
        if isExample(id), id.hasPrefix(Self.examplePrefix), let examples = EngineCommand.bundledExamples() {
            return examples.appending(path: String(id.dropFirst(Self.examplePrefix.count)), directoryHint: .isDirectory)
        }
        return prefs.resolvedRunsFolder.appending(path: id, directoryHint: .isDirectory)
    }

    func revealInFinder(_ id: String) {
        guard !isExample(id) else { return }  // inside the app bundle
        let dir = runDirectory(id)
        if FileManager.default.fileExists(atPath: dir.path) {
            NSWorkspace.shared.activateFileViewerSelecting([dir])
        }
    }

    /// Moves the run folder to the Trash. Live runs must be stopped first.
    func deleteRun(_ id: String) async throws {
        if let run = summary(id), run.state.isActive {
            throw TellfoldAPIError(status: 409, message: "Stop the simulation before deleting it.")
        }
        if isExample(id) {
            throw TellfoldAPIError(status: 403, message: "Examples come with the app and cannot be deleted.")
        }
        let dir = runDirectory(id)
        guard !id.isEmpty, !id.contains("/"), !id.contains(".."),
              dir.standardizedFileURL.deletingLastPathComponent().path == prefs.resolvedRunsFolder.standardizedFileURL.path,
              FileManager.default.fileExists(atPath: dir.appending(path: "config.json").path) else {
            throw TellfoldAPIError(status: 400, message: "That folder is not a run in the runs folder.")
        }
        if selectedRunID == id { selectedRunID = nil }
        try FileManager.default.trashItem(at: dir, resultingItemURL: nil)
        await refreshRuns()
    }

    func rebuildReport(_ id: String) async throws {
        guard let api else { throw TellfoldAPIError(status: 0, message: "The engine is not running.") }
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

    /// Renders the run's report as a paged PDF and asks where to save it (File > Export PDF…, the report toolbar).
    /// `title` names the file. Returns the saved file, or nil when cancelled.
    @discardableResult
    func exportPDF(_ id: String, title: String) async throws -> URL? {
        guard exportingPDF == nil else { return nil }
        exportingPDF = id
        defer { exportingPDF = nil }
        guard let api else { throw TellfoldAPIError(status: 0, message: "The engine is not running.") }
        let html = try await api.reportHTML(runID: id)
        return try await ReportPDFExporter().export(html: html, title: title, in: NSApp.keyWindow ?? NSApp.mainWindow)
    }

    // MARK: New simulation

    func createRun(_ request: NewRunRequest) async throws -> String {
        guard let api else { throw TellfoldAPIError(status: 0, message: "The engine is not running.") }
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
                    let folder = FileManager.default.temporaryDirectory.appending(path: "tellfold-run-again-\(UUID().uuidString)")
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
        newSimulation(prefill: .runAgain(config: config, seedFiles: urls, unavailable: failed, fromExample: isExample(runID)))
    }

    // MARK: Studies

    /// Opens "Try another version…" for a finished run.
    func tryAnotherVersion(runID: String, config: SimConfig, text: String? = nil) {
        anotherVersionSource = AnotherVersionSource(runID: runID, config: config, text: text)
    }

    /// Downloads a run's seed documents into a temporary folder; empty when the engine has none.
    func downloadSeeds(runID: String) async -> [URL] {
        guard let api, let list = try? await api.seeds(runID: runID), !list.isEmpty else { return [] }
        let folder = FileManager.default.temporaryDirectory.appending(path: "tellfold-version-\(UUID().uuidString)")
        guard (try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)) != nil else { return [] }
        var urls: [URL] = []
        for item in list {
            guard let data = try? await api.seedData(runID: runID, name: item.name) else { continue }
            let url = folder.appending(path: item.name)
            if (try? data.write(to: url)) != nil { urls.append(url) }
        }
        return urls
    }

    /// Starts a new version of a run and shows its study, where both runs and then the comparison appear.
    func createVersion(_ request: NewRunRequest) async throws {
        guard let api else { throw TellfoldAPIError(status: 0, message: "The engine is not running.") }
        let id = try await api.createRun(request)
        await refreshRuns()
        let studyID = runs.first { $0.id == id }?.studyID ?? StudyLogic.study(of: id, runs: runs, studies: studies)
        selectedRunID = studyID.map(StudySelection.tag) ?? id
    }

    /// "Test this version": runs a suggested rewrite past the same people right away (the 2A flow, labelled with the
    /// study's next letter), then shows the study.
    func testVersion(runID: String, config: SimConfig, suggestion: RewriteSuggestion) async throws {
        let config = versionConfig(runID: runID, config: config)
        let seeds = await downloadSeeds(runID: runID)
        let name = suggestion.sourceName.isEmpty ? InlineDocument.descriptionName : suggestion.sourceName
        let request = StudyLogic.versionRequest(
            source: runID, config: config, studyID: StudyLogic.study(of: runID, runs: runs, studies: studies),
            variant: "", editedName: name, editedText: suggestion.revisedText,
            otherFiles: seeds.filter { $0.lastPathComponent != name })
        try await createVersion(request)
    }

    /// The settings a new version of a run uses: the run's own, except that a built-in example uses the AI tools set up on
    /// this Mac (the example's may not be installed here).
    func versionConfig(runID: String, config: SimConfig) -> SimConfig {
        guard isExample(runID) else { return config }
        var config = config
        config.agentRuntime = defaultAgentRuntime
        config.plannerRuntime = defaultPlannerRuntime
        config.agentModel = defaultModel(runtime: config.agentRuntime, role: "agent")
        config.plannerModel = defaultModel(runtime: config.plannerRuntime, role: "planner")
        return config
    }

    /// "Branch from here…": a copy of the run up to `round` continues with `events`; shows the study with its source.
    func branch(runID: String, round: Int, events: [GodEvent]) async throws {
        guard let api else { throw TellfoldAPIError(status: 0, message: "The engine is not running.") }
        let created = try await api.branch(runID: runID, round: round, events: events)
        await refreshRuns()
        selectStudy(created.studyID)
    }

    func selectStudy(_ studyID: String) { selectedRunID = StudySelection.tag(studyID) }

    /// "Check it": runs a finished run again with the same people and text, then shows the study.
    func checkIt(runID: String, runs: Int = StudyLogic.checkRuns) async throws {
        guard let api else { throw TellfoldAPIError(status: 0, message: "The engine is not running.") }
        let studyID = try await api.checkRun(runID: runID, runs: runs)
        await refreshRuns()
        selectStudy(studyID)
    }

    /// Opens a run and selects one of its posts once the run has loaded (a citation such as `B#7`).
    func openPost(runID: String, postID: Int) {
        pendingPostFocus = PostFocus(runID: runID, postID: postID)
        selectedRunID = runID
    }

    static let seedContentTypes: [UTType] = [
        UTType(filenameExtension: "md") ?? .plainText, .plainText, .pdf,
    ]

    nonisolated static func isSeedFile(_ url: URL) -> Bool {
        NewRunRequest.seedExtensions.contains(url.pathExtension.lowercased())
    }

    nonisolated static func isFolder(_ url: URL) -> Bool {
        (try? url.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) == true
    }

    /// Folders never worth reading as seeds: version control, dependencies and build output.
    nonisolated static let skippedFolderNames: Set<String> = [
        ".git", ".build", ".venv", "venv", "__pycache__", "node_modules", "Pods", "Carthage", "DerivedData",
        "build", "dist", "out", "target", "coverage", "vendor",
    ]

    /// Non-empty seed documents in `folder` and its subfolders, in path order, stopping once `limit` are found so a
    /// huge tree isn't walked to the end. Hidden files, packages and the folders in `skippedFolderNames` are left out.
    nonisolated static func seedFiles(inFolder folder: URL, limit: Int = .max) -> [URL] {
        let keys: [URLResourceKey] = [.isDirectoryKey, .fileSizeKey]
        guard let walker = FileManager.default.enumerator(at: folder, includingPropertiesForKeys: keys,
                                                          options: [.skipsHiddenFiles, .skipsPackageDescendants])
        else { return [] }
        var found: [URL] = []
        for case let url as URL in walker {
            let values = try? url.resourceValues(forKeys: Set(keys))
            if values?.isDirectory == true {
                if skippedFolderNames.contains(url.lastPathComponent) { walker.skipDescendants() }
            } else if isSeedFile(url), (values?.fileSize ?? 0) > 0 {
                found.append(url)
                if found.count >= limit { break }
            }
        }
        return found.sorted { $0.path.localizedStandardCompare($1.path) == .orderedAscending }
    }

    /// Files dropped on the Dock icon or chosen with File > Open: queue them and open the sheet.
    func openSeedFiles(_ urls: [URL]) {
        let seeds = urls.filter { Self.isSeedFile($0) || Self.isFolder($0) }
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
        panel.canChooseDirectories = true
        panel.allowedContentTypes = Self.seedContentTypes
        panel.message = "Choose documents (.md, .txt or .pdf), or a folder of them, for a new simulation."
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
        } catch let error as TellfoldAPIError where error.isNotFound {
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
        } catch let error as TellfoldAPIError where error.isNotFound {
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
                guard let api = self?.api else { throw TellfoldAPIError(status: 0, message: "The engine is not running.") }
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

    // MARK: Choosing an AI tool

    /// Detects runtimes with a real sign-in check and looks for LM Studio on its usual port.
    func detectAgents() async {
        async let lmStudio = LocalModelServer.probe(.lmStudio)
        await loadRuntimes(checkAuth: true)
        let configured = Set((engineSettings?.openAICompatible ?? []).flatMap { [$0.id, $0.baseURL] })
        localServers = [await lmStudio].compactMap { $0 }.filter {
            !configured.contains($0.endpointID) && !configured.contains($0.baseURL) && runtime($0.runtimeID) == nil
        }
    }

    /// The runtime suggested for this Mac, or nil when nothing can run a simulation yet.
    var recommendedRuntime: RuntimeInfo? { RuntimeRecommendation.recommended(runtimes + localServers.map(\.asRuntime)) }

    /// Detected AI tools and local servers, best first.
    var detectedRuntimes: [RuntimeInfo] { RuntimeRecommendation.detected(runtimes + localServers.map(\.asRuntime)) }

    /// Makes `id` the default for both roles, with its default models. A local server found by `detectAgents` is added
    /// to the engine's endpoints first.
    func useRuntime(_ id: String) async {
        if let server = localServers.first(where: { $0.runtimeID == id }) {
            updateEngineSettings { settings in
                var list = settings.openAICompatible
                list.removeAll { $0.id == server.endpointID }
                list.append(server.endpoint)
                settings.openAICompatible = list
            }
            await waitForEngineSettingsSaves()
            localServers.removeAll { $0 == server }
            await loadRuntimes()
        }
        let info = runtime(id)
        let agentModel = info?.defaultModels["agent"] ?? info?.models.first ?? ""
        let plannerModel = info?.defaultModels["planner"] ?? info?.models.first ?? ""
        updateEngineSettings { settings in
            settings.agentRuntime = id
            settings.plannerRuntime = id
            settings.agentModel = agentModel
            settings.plannerModel = plannerModel
        }
        await waitForEngineSettingsSaves()
        prefs.agentSetupDone = true
    }

    func skipAgentSetup() { prefs.agentSetupDone = true }

    // MARK: Defaults for the New Simulation sheet

    var defaultAgentRuntime: String { preferredRuntime(engineSettings?.agentRuntime) }
    var defaultPlannerRuntime: String { preferredRuntime(engineSettings?.plannerRuntime) }

    /// The configured runtime while it is installed; otherwise the suggested one, so a Mac without Claude Code does
    /// not open the sheet on a tool it does not have.
    private func preferredRuntime(_ configured: String?) -> String {
        if let configured, runtime(configured)?.installed == true { return configured }
        return RuntimeRecommendation.recommended(runtimes)?.id ?? configured ?? "claude-code"
    }

    func defaultModel(runtime: String, role: String) -> String {
        if role == "agent", runtime == engineSettings?.agentRuntime, let m = engineSettings?.agentModel, !m.isEmpty { return m }
        if role == "planner", runtime == engineSettings?.plannerRuntime, let m = engineSettings?.plannerModel, !m.isEmpty {
            return m
        }
        let info = self.runtime(runtime)
        return info?.defaultModels[role] ?? info?.models.first ?? (role == "agent" ? "haiku" : "sonnet")
    }
}
