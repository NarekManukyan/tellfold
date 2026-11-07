import Foundation

/// One event from `GET /api/runs/{id}/stream` (SSE). Payload shapes come from `shoal/runner.py` and
/// `shoal/engine.py`. Malformed or unknown events decode to `.unknown` rather than failing the stream.
enum ServerEvent: Hashable, Sendable {
    /// A full `RunStatus`; sent first on connect and on every change.
    case status(RunStatus)
    case stage(StageEvent)
    case roundStart(RoundStartEvent)
    case roundDone(RoundDoneEvent)
    case postsAdded(PostsAddedEvent)
    case batchFailed(BatchFailedEvent)
    case eventInjected(GodEvent)
    case runDone
    case runFailed(error: String)
    case runStopped
    case unknown(name: String, data: String)

    var name: String {
        switch self {
        case .status: "status"
        case .stage: "stage"
        case .roundStart: "round_start"
        case .roundDone: "round_done"
        case .postsAdded: "posts_added"
        case .batchFailed: "batch_failed"
        case .eventInjected: "event_injected"
        case .runDone: "run_done"
        case .runFailed: "run_failed"
        case .runStopped: "run_stopped"
        case .unknown(let name, _): name
        }
    }

    /// The run will not produce further events.
    var isTerminal: Bool {
        switch self {
        case .runDone, .runFailed, .runStopped: true
        case .status(let s): s.state.isTerminal
        default: false
        }
    }

    static func decode(name: String, data: String) -> ServerEvent {
        let bytes = Data(data.utf8)
        let decoder = JSONDecoder()
        func parse<T: Decodable>(_ type: T.Type) -> T? { try? decoder.decode(T.self, from: bytes) }
        switch name {
        case "status":
            if let v = parse(RunStatus.self) { return .status(v) }
        case "stage":
            if let v = parse(StageEvent.self) { return .stage(v) }
        case "round_start":
            if let v = parse(RoundStartEvent.self) { return .roundStart(v) }
        case "round_done":
            if let v = parse(RoundDoneEvent.self) { return .roundDone(v) }
        case "posts_added":
            if let v = parse(PostsAddedEvent.self) { return .postsAdded(v) }
        case "batch_failed":
            if let v = parse(BatchFailedEvent.self) { return .batchFailed(v) }
        case "event_injected":
            if let v = parse(GodEvent.self) { return .eventInjected(v) }
        case "run_done":
            return .runDone
        case "run_failed":
            let error = parse([String: JSONValue].self)?["error"]?.stringValue ?? "The run failed."
            return .runFailed(error: error)
        case "run_stopped":
            return .runStopped
        default:
            break
        }
        return .unknown(name: name, data: data)
    }
}

/// `stage`: `{stage, detail, ...extra}`. During seeding `step` carries the engine's own sub-step
/// (`follow_graph`, `opening_posts`, ...).
struct StageEvent: Codable, Hashable, Sendable {
    var stage: RunStage
    var detail: String
    var step: String?

    init(stage: RunStage, detail: String, step: String? = nil) {
        self.stage = stage
        self.detail = detail
        self.step = step
    }

    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        stage = c.value(.stage, default: .unknown)
        detail = c.value(.detail, default: "")
        step = c.optional(.step)
    }
}

/// `round_start`: `{round, active, total_rounds, active_agents, batches, events}`.
struct RoundStartEvent: Codable, Hashable, Sendable {
    var round: Int
    var active: Int
    var totalRounds: Int?
    var activeAgents: [String]
    var batches: Int?
    /// Contents of the god events that fire this round.
    var events: [String]

    enum CodingKeys: String, CodingKey {
        case round, active, batches, events
        case totalRounds = "total_rounds"
        case activeAgents = "active_agents"
    }

    init(round: Int, active: Int, totalRounds: Int? = nil, activeAgents: [String] = [], batches: Int? = nil,
         events: [String] = []) {
        self.round = round
        self.active = active
        self.totalRounds = totalRounds
        self.activeAgents = activeAgents
        self.batches = batches
        self.events = events
    }

    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        round = try c.decode(Int.self, forKey: .round)
        active = c.value(.active, default: 0)
        totalRounds = c.optional(.totalRounds)
        activeAgents = c.value(.activeAgents, default: [])
        batches = c.optional(.batches)
        events = c.value(.events, default: [])
    }
}

/// `round_done`: a `RoundStats` plus `total_rounds`.
struct RoundDoneEvent: Codable, Hashable, Sendable {
    var stats: RoundStats
    var totalRounds: Int?

    var round: Int { stats.round }

    init(stats: RoundStats, totalRounds: Int? = nil) {
        self.stats = stats
        self.totalRounds = totalRounds
    }

    private enum ExtraKeys: String, CodingKey { case totalRounds = "total_rounds" }

    init(from decoder: any Decoder) throws {
        stats = try RoundStats(from: decoder)
        totalRounds = try? decoder.container(keyedBy: ExtraKeys.self).decodeIfPresent(Int.self, forKey: .totalRounds)
    }

    func encode(to encoder: any Encoder) throws {
        try stats.encode(to: encoder)
        var c = encoder.container(keyedBy: ExtraKeys.self)
        try c.encodeIfPresent(totalRounds, forKey: .totalRounds)
    }
}

/// `posts_added`: `{round, post_ids}`.
struct PostsAddedEvent: Codable, Hashable, Sendable {
    var round: Int
    var postIDs: [Int]

    enum CodingKeys: String, CodingKey {
        case round
        case postIDs = "post_ids"
    }
}

/// `batch_failed`: `{round, batch, agents, error}`; `batch` is an index or `"sentiment"`.
struct BatchFailedEvent: Codable, Hashable, Sendable {
    var round: Int
    var batch: String
    var agents: [String]
    var error: String

    init(round: Int, batch: String, agents: [String], error: String) {
        self.round = round
        self.batch = batch
        self.agents = agents
        self.error = error
    }

    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        round = c.value(.round, default: 0)
        if let i = try? c.decode(Int.self, forKey: .batch) {
            batch = String(i)
        } else {
            batch = c.value(.batch, default: "")
        }
        agents = c.value(.agents, default: [])
        error = c.value(.error, default: "")
    }
}
