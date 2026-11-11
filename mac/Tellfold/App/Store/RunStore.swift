import Foundation
import Observation
import SwiftUI

/// Everything one run's views need: the `RunDetail`, all posts, actions by round, the replay playhead,
/// selection, and live updates from the event stream. Shared by the run views (header, timeline, feed,
/// report, inspector) and the canvases (network, knowledge graph).
@MainActor @Observable
final class RunStore: Identifiable {
    enum LoadState: Hashable, Sendable {
        case idle, loading, loaded
        case failed(String)
    }

    nonisolated let runID: String
    nonisolated var id: String { runID }
    /// nil for preview stores built from fixtures.
    @ObservationIgnored private(set) var api: TellfoldAPI?

    private(set) var loadState: LoadState = .idle
    private(set) var detail: RunDetail?
    /// Every post of the run, ascending by id.
    private(set) var posts: [PostOut] = []
    private(set) var actionsByRound: [Int: [ActionOut]] = [:]

    /// The round the views show (0 = opening posts). Live runs keep it on the latest round while
    /// `followsLive` is true. Prefer `setPlayhead(_:)` / `stepPlayhead(by:)`, which clamp to 0...latestRound.
    var playhead: Int = 0
    /// Replay is playing. The replay controller (run views) advances `playhead` while this is true.
    var isPlaying = false
    /// Replay speed multiplier (0.5, 1, 2).
    var replaySpeed: Double = 1
    /// While live, move the playhead to each new round; turned off when the user scrubs back.
    var followsLive = true

    var selectedAgentID: String?
    /// The camp (a `CampGroup` id) selected in the Crowd view; the agent selection wins over it.
    var selectedCampID: String?
    var selectedPostID: Int?
    /// Shows the Inject Event sheet (presented by the main window).
    var showsInjectEvent = false
    /// The cast editor is up: set when the run starts waiting at `personas_ready`, cleared when it stops waiting.
    /// "Later" closes the sheet; the toolbar's Check the Cast button opens it again.
    var showsCastEditor = false

    /// Latest `RunStatus` (from the stream when live, otherwise from the detail).
    private(set) var status: RunStatus?
    /// The current activity text from `stage` events.
    private(set) var stageDetail = ""
    private(set) var isStreaming = false
    /// The last `round_start` seen on the stream (active agents and events of the round in progress).
    private(set) var lastRoundStart: RoundStartEvent?
    private(set) var batchFailures: [BatchFailedEvent] = []
    /// The most recent stream event and a counter that increases with each one, for views that animate.
    private(set) var lastEvent: ServerEvent?
    private(set) var eventCounter = 0
    /// A user-facing error from the last action (stop, inject, rebuild, ...).
    var actionError: String?
    /// The share card sheet (File > Share Card…, the report toolbar).
    var showsShareCard = false

    @ObservationIgnored private var streamTask: Task<Void, Never>?
    @ObservationIgnored private var sentimentCache: [Int: [String: Double]] = [:]
    @ObservationIgnored private var sentimentCacheKey = -1

    init(runID: String, api: TellfoldAPI?) {
        self.runID = runID
        self.api = api
    }

    /// A store filled with ready data and no API, for previews and tests.
    init(detail: RunDetail, posts: [PostOut], actions: [ActionOut]) {
        runID = detail.id
        api = nil
        apply(detail: detail)
        self.posts = posts.sorted { $0.id < $1.id }
        actionsByRound = Dictionary(grouping: actions, by: \.round)
        playhead = latestRound
        loadState = .loaded
    }

    // MARK: Derived data

    var isLive: Bool {
        (detail?.live ?? false) && (status?.state.isActive ?? false)
    }
    var config: SimConfig? { detail?.config }
    /// A built-in example run, which the engine never changes.
    var isExample: Bool { detail?.example ?? false }
    /// Someone else's run opened from a `.tellfold` file, which the engine never changes either.
    var isImported: Bool { detail?.imported ?? false }
    /// An example or an imported run.
    var isReadOnly: Bool { isExample || isImported }
    var question: String { detail?.config.question ?? "" }
    var personas: [Persona] { detail?.personas ?? [] }
    var personaByID: [String: Persona] { Dictionary(personas.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a }) }
    var graph: KnowledgeGraph? { detail?.graph }
    var stats: [RoundStats] { detail?.stats ?? [] }
    var events: [GodEvent] { detail?.events ?? [] }
    var followEdges: [FollowEdge] { detail?.followEdges ?? [] }
    var report: Report? { detail?.report }
    var totalRounds: Int { status?.totalRounds ?? detail?.totalRounds ?? 0 }
    /// The last completed round.
    var latestRound: Int {
        RunRounds.latest(status: status?.round ?? 0, stats: stats.map(\.round), posts: posts.map(\.round),
                         actions: Array(actionsByRound.keys))
    }
    var postByID: [Int: PostOut] { Dictionary(posts.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a }) }
    var postsByRound: [Int: [PostOut]] { Dictionary(grouping: posts, by: \.round) }
    var selectedPersona: Persona? { selectedAgentID.flatMap { personaByID[$0] } }

    func persona(_ id: String) -> Persona? { personas.first { $0.id == id } }
    func persona(handle: String) -> Persona? {
        let wanted = handle.trimmingCharacters(in: CharacterSet(charactersIn: "@")).lowercased()
        return personas.first { $0.handle.lowercased() == wanted }
    }
    func posts(in round: Int) -> [PostOut] { posts.filter { $0.round == round } }
    func actions(in round: Int) -> [ActionOut] { actionsByRound[round] ?? [] }
    func stats(for round: Int) -> RoundStats? { stats.first { $0.round == round } }
    func events(in round: Int) -> [GodEvent] { events.filter { $0.round == round } }

    /// Mean of each agent's post sentiment scores up to and including `round`. Agents that have not posted
    /// a scored post by then are absent (treat as nil / neutral).
    func sentimentByAgent(at round: Int) -> [String: Double] {
        if sentimentCacheKey != posts.count {
            sentimentCache.removeAll()
            sentimentCacheKey = posts.count
        }
        if let cached = sentimentCache[round] { return cached }
        var sums: [String: (Double, Int)] = [:]
        for post in posts where post.round <= round {
            guard let score = post.sentiment else { continue }
            let (s, n) = sums[post.authorID] ?? (0, 0)
            sums[post.authorID] = (s + score, n + 1)
        }
        let result = sums.mapValues { $0.0 / Double($0.1) }
        sentimentCache[round] = result
        return result
    }

    // MARK: Loading

    /// Loads the detail, all posts and all actions. Starts streaming when the run is live.
    func load() async {
        guard let api else { return }
        if detail == nil { loadState = .loading }
        do {
            async let detailCall = api.run(runID)
            async let postsCall = api.posts(runID: runID, sinceID: 0)
            async let actionsCall = api.actions(runID: runID)
            let (detail, posts, actions) = try await (detailCall, postsCall, actionsCall)
            apply(detail: detail)
            self.posts = posts.sorted { $0.id < $1.id }
            actionsByRound = Dictionary(grouping: actions, by: \.round)
            if followsLive || !isLive { playhead = latestRound }
            loadState = .loaded
            if isLive { startStreaming() }
        } catch is CancellationError {
            return
        } catch {
            loadState = .failed(error.localizedDescription)
        }
    }

    /// Re-fetches the detail only (personas, graph, stats, report, status).
    func reloadDetail() async {
        guard let api else { return }
        do {
            apply(detail: try await api.run(runID))
        } catch {
            // keep what we have; the next event or reload retries
        }
    }

    private func apply(detail: RunDetail) {
        let wasWaiting = status?.awaitsCast ?? false
        self.detail = detail
        if let s = detail.status {
            status = s
            mergeBatchFailures(s.failedBatches)
            noteCastPause(wasWaiting: wasWaiting, status: s)
        }
        if stageDetail.isEmpty || !(detail.status?.state.isActive ?? false) { stageDetail = detail.status?.detail ?? "" }
    }

    /// Adds failures not seen yet: the persisted status and the live stream both report them.
    private func mergeBatchFailures(_ failures: [BatchFailedEvent]) {
        for f in failures where !batchFailures.contains(where: { $0.round == f.round && $0.batch == f.batch }) {
            batchFailures.append(f)
        }
    }

    // MARK: Streaming

    func startStreaming() {
        guard let api, streamTask == nil else { return }
        isStreaming = true
        let stream = api.eventStream(runID: runID)
        streamTask = Task { [weak self] in
            for await event in stream {
                guard let self else { return }
                await self.handle(event)
            }
            self?.streamEnded()
        }
    }

    func stopStreaming() {
        streamTask?.cancel()
        streamTask = nil
        isStreaming = false
    }

    private func streamEnded() {
        streamTask = nil
        isStreaming = false
    }

    /// Applies one stream event. Internal so tests can drive the store without a server.
    func handle(_ event: ServerEvent) async {
        lastEvent = event
        eventCounter += 1
        switch event {
        case .status(let s):
            let stageChanged = s.stage != status?.stage
            noteCastPause(wasWaiting: status?.awaitsCast ?? false, status: s)
            status = s
            detail?.status = s
            mergeBatchFailures(s.failedBatches)
            if !s.detail.isEmpty { stageDetail = s.detail }
            if s.state.isTerminal {
                detail?.live = false
                await finishRun()
            } else if stageChanged, [.seeding, .rounds, .report].contains(s.stage) {
                await reloadDetail() // personas and graph appear as stages complete
            }
        case .stage(let e):
            stageDetail = e.detail
        case .roundStart(let e):
            lastRoundStart = e
        case .postsAdded:
            await fetchNewPosts()
        case .roundDone(let e):
            upsert(stats: e.stats)
            await fetchActions(round: e.round)
            if followsLive { playhead = latestRound }
        case .batchFailed(let e):
            mergeBatchFailures([e])
        case .eventInjected(let e):
            if !(detail?.events.contains(e) ?? true) { detail?.events.append(e) }
        case .personasReady:
            if status?.awaitsCast ?? true { showsCastEditor = true }
        case .runDone, .runStopped:
            detail?.live = false
            await finishRun()
        case .runFailed(let error):
            detail?.live = false
            if var s = status, s.error == nil { s.error = error; status = s }
            await finishRun()
        case .unknown:
            break
        }
    }

    /// Opens the cast editor when the run starts waiting for its cast, and closes it once the run goes on or ends.
    private func noteCastPause(wasWaiting: Bool, status: RunStatus) {
        if status.awaitsCast, !wasWaiting {
            showsCastEditor = true
        } else if !status.awaitsCast {
            showsCastEditor = false
        }
    }

    var awaitsCast: Bool { isLive && (status?.awaitsCast ?? false) }

    private func finishRun() async {
        await fetchNewPosts()
        await reloadDetail()
        if let api, let all = try? await api.actions(runID: runID) {
            actionsByRound = Dictionary(grouping: all, by: \.round)
        }
        if followsLive { playhead = latestRound }
    }

    private func upsert(stats: RoundStats) {
        guard detail != nil else { return }
        if let i = detail!.stats.firstIndex(where: { $0.round == stats.round }) {
            detail!.stats[i] = stats
        } else {
            detail!.stats.append(stats)
            detail!.stats.sort { $0.round < $1.round }
        }
    }

    /// Fetches posts newer than the newest one held.
    func fetchNewPosts() async {
        guard let api else { return }
        let since = posts.last?.id ?? 0
        guard let new = try? await api.posts(runID: runID, sinceID: since), !new.isEmpty else { return }
        let known = Set(posts.map(\.id))
        posts.append(contentsOf: new.filter { !known.contains($0.id) })
        posts.sort { $0.id < $1.id }
    }

    func fetchActions(round: Int) async {
        guard let api, let actions = try? await api.actions(runID: runID, round: round) else { return }
        actionsByRound[round] = actions
    }

    // MARK: Playhead

    func stepPlayhead(by delta: Int) {
        setPlayhead(playhead + delta)
    }

    func setPlayhead(_ round: Int) {
        playhead = min(max(round, 0), max(latestRound, 0))
        followsLive = !isLive || playhead >= latestRound
    }

    func togglePlaying() {
        if !isPlaying, playhead >= latestRound { playhead = 0 }
        isPlaying.toggle()
        if isPlaying { followsLive = false }
    }

    // MARK: Actions

    func stop() async {
        guard let api else { return }
        do {
            try await api.stop(runID: runID)
        } catch {
            actionError = error.localizedDescription
        }
    }

    /// Injects a god event for a future round of a live run.
    func injectEvent(round: Int, content: String) async throws {
        guard let api else { throw TellfoldAPIError(status: 0, message: "The engine is not running.") }
        try await api.injectEvent(runID: runID, round: round, content: content)
        let event = GodEvent(round: round, content: content)
        if !(detail?.events.contains(event) ?? true) { detail?.events.append(event) }
    }

    func rebuildReport() async {
        guard let api else { return }
        do {
            try await api.rebuildReport(runID: runID)
            await reloadDetail()
        } catch {
            actionError = error.localizedDescription
        }
    }

    func agentDetail(_ agentID: String) async throws -> AgentDetail {
        guard let api else { throw TellfoldAPIError(status: 0, message: "The engine is not running.") }
        return try await api.agent(runID: runID, agentID: agentID)
    }

    func chat(agentID: String, message: String, history: [ChatTurn]) async throws -> String {
        guard let api else { throw TellfoldAPIError(status: 0, message: "The engine is not running.") }
        return try await api.chat(runID: runID, agentID: agentID, message: message, history: history)
    }

    func ask(_ question: String) async throws -> String {
        guard let api else { throw TellfoldAPIError(status: 0, message: "The engine is not running.") }
        return try await api.ask(runID: runID, question: question)
    }

    /// Fetches `report.html` and writes it to a temporary file, for opening in the browser.
    func reportHTMLFile() async throws -> URL {
        guard let api else { throw TellfoldAPIError(status: 0, message: "The engine is not running.") }
        let data = try await api.reportHTML(runID: runID)
        let dir = URL.temporaryDirectory.appending(path: "Tellfold Reports", directoryHint: .isDirectory)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let file = dir.appending(path: "\(runID).html")
        try data.write(to: file, options: .atomic)
        return file
    }
}

extension FocusedValues {
    /// The run shown in the focused main window; drives the Simulation menu.
    @Entry var runStore: RunStore?
}
