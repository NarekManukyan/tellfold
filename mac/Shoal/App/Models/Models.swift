import Foundation

// Codable mirrors of the engine's JSON (shoal/models.py and shoal/server/app.py).
//
// Naming: Swift properties are camelCase with `ID`/`IDs` for identifiers (`agentID`, `targetPostID`,
// `postIDs`); each type maps its keys to the server's snake_case with explicit `CodingKeys`, so use the
// plain decoder in `ShoalAPI.decoder` (no `keyDecodingStrategy`). Decoding is lenient where the server may
// omit a field (older runs, newer engine fields): unknown enum values decode to `.unknown`.

// MARK: - Lenient decoding helpers

extension KeyedDecodingContainer {
    /// The value for `key`, or `fallback` when it is missing, null or of the wrong type.
    func value<T: Decodable>(_ key: Key, default fallback: @autoclosure () -> T) -> T {
        if let v = try? decodeIfPresent(T.self, forKey: key) { return v }
        return fallback()
    }

    /// The value for `key`, or nil when it is missing, null or of the wrong type.
    func optional<T: Decodable>(_ key: Key, as type: T.Type = T.self) -> T? {
        if let v = try? decodeIfPresent(T.self, forKey: key) { return v }
        return nil
    }
}

// MARK: - Enums

enum RunState: String, Codable, Hashable, Sendable, CaseIterable {
    case queued, running, done, failed, stopped, unknown

    init(from decoder: any Decoder) throws {
        let raw = try decoder.singleValueContainer().decode(String.self)
        self = RunState(rawValue: raw) ?? .unknown
    }

    /// Queued or running: the run may still change.
    var isActive: Bool { self == .queued || self == .running }
    var isTerminal: Bool { self == .done || self == .failed || self == .stopped }

    var title: String {
        switch self {
        case .queued: "Queued"
        case .running: "Running"
        case .done: "Done"
        case .failed: "Failed"
        case .stopped: "Stopped"
        case .unknown: "Unknown"
        }
    }

    var systemImage: String {
        switch self {
        case .queued: "clock"
        case .running: "play.circle"
        case .done: "checkmark.circle"
        case .failed: "exclamationmark.triangle"
        case .stopped: "stop.circle"
        case .unknown: "questionmark.circle"
        }
    }
}

enum RunStage: String, Codable, Hashable, Sendable, CaseIterable {
    case reading, graph, personas, seeding, rounds, report, finished, unknown

    init(from decoder: any Decoder) throws {
        let raw = try decoder.singleValueContainer().decode(String.self)
        self = RunStage(rawValue: raw) ?? .unknown
    }

    /// The visible pipeline steps, in order (`finished` and `unknown` are not steps).
    static let pipeline: [RunStage] = [.reading, .graph, .personas, .seeding, .rounds, .report]

    var title: String {
        switch self {
        case .reading: "Reading"
        case .graph: "Graph"
        case .personas: "Personas"
        case .seeding: "Seeding"
        case .rounds: "Rounds"
        case .report: "Report"
        case .finished: "Finished"
        case .unknown: "Unknown"
        }
    }

    /// Position in `pipeline`; `finished` sorts after every step.
    var order: Int {
        if self == .finished { return RunStage.pipeline.count }
        return RunStage.pipeline.firstIndex(of: self) ?? -1
    }
}

enum ActionType: String, Codable, Hashable, Sendable, CaseIterable {
    case post, reply, repost, like, dislike, follow, nothing, unknown

    init(from decoder: any Decoder) throws {
        let raw = try decoder.singleValueContainer().decode(String.self)
        self = ActionType(rawValue: raw) ?? .unknown
    }

    var title: String { rawValue.capitalized }
}

enum ReportConfidence: String, Codable, Hashable, Sendable {
    case low, medium, high, unknown

    init(from decoder: any Decoder) throws {
        let raw = try decoder.singleValueContainer().decode(String.self)
        self = ReportConfidence(rawValue: raw) ?? .unknown
    }

    var title: String { rawValue.capitalized }
}

// MARK: - Knowledge graph

struct Entity: Codable, Hashable, Sendable, Identifiable {
    var id: String
    var name: String
    var kind: String
    var summary: String
    var sourceChunkIDs: [Int] = []

    enum CodingKeys: String, CodingKey {
        case id, name, kind, summary
        case sourceChunkIDs = "source_chunk_ids"
    }
}

extension Entity {
    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        name = c.value(.name, default: id)
        kind = c.value(.kind, default: "concept")
        summary = c.value(.summary, default: "")
        sourceChunkIDs = c.value(.sourceChunkIDs, default: [])
    }
}

struct Relation: Codable, Hashable, Sendable, Identifiable {
    var source: String
    var target: String
    var label: String
    var evidence: String = ""

    var id: String { "\(source)|\(label)|\(target)" }
}

extension Relation {
    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        source = try c.decode(String.self, forKey: .source)
        target = try c.decode(String.self, forKey: .target)
        label = c.value(.label, default: "")
        evidence = c.value(.evidence, default: "")
    }
}

struct KnowledgeGraph: Codable, Hashable, Sendable {
    var entities: [Entity] = []
    var relations: [Relation] = []
}

extension KnowledgeGraph {
    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        entities = c.value(.entities, default: [])
        relations = c.value(.relations, default: [])
    }
}

// MARK: - Agents

struct Persona: Codable, Hashable, Sendable, Identifiable {
    var id: String
    var handle: String
    var name: String
    var bio: String = ""
    /// Private to the simulation.
    var background: String = ""
    /// Private to the simulation.
    var stance: String = ""
    var traits: [String] = []
    var activity: Double = 0.6
    var influence: Double = 0.5
    var entityID: String?

    enum CodingKeys: String, CodingKey {
        case id, handle, name, bio, background, stance, traits, activity, influence
        case entityID = "entity_id"
    }
}

extension Persona {
    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        handle = c.value(.handle, default: id)
        name = c.value(.name, default: handle)
        bio = c.value(.bio, default: "")
        background = c.value(.background, default: "")
        stance = c.value(.stance, default: "")
        traits = c.value(.traits, default: [])
        activity = c.value(.activity, default: 0.6)
        influence = c.value(.influence, default: 0.5)
        entityID = c.optional(.entityID)
    }
}

// MARK: - Social world

struct GodEvent: Codable, Hashable, Sendable, Identifiable {
    var round: Int
    var content: String

    var id: String { "\(round):\(content)" }
}

struct RoundStats: Codable, Hashable, Sendable, Identifiable {
    var round: Int
    /// ActionType raw value -> count.
    var actions: [String: Int] = [:]
    var newPosts: Int = 0
    var activeAgents: Int = 0
    /// Mean sentiment of the round's posts, -1...1; nil when the sentiment pass failed or had nothing to score.
    var sentiment: Double?
    var llmCalls: Int = 0
    var llmSeconds: Double = 0

    var id: Int { round }

    func count(_ type: ActionType) -> Int { actions[type.rawValue] ?? 0 }

    enum CodingKeys: String, CodingKey {
        case round, actions, sentiment
        case newPosts = "new_posts"
        case activeAgents = "active_agents"
        case llmCalls = "llm_calls"
        case llmSeconds = "llm_seconds"
    }
}

extension RoundStats {
    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        round = try c.decode(Int.self, forKey: .round)
        actions = c.value(.actions, default: [:])
        newPosts = c.value(.newPosts, default: 0)
        activeAgents = c.value(.activeAgents, default: 0)
        sentiment = c.optional(.sentiment)
        llmCalls = c.value(.llmCalls, default: 0)
        llmSeconds = c.value(.llmSeconds, default: 0)
    }
}

/// `PostOut` = `Post` + `author_handle` + `sentiment` (per-post score, -1...1, nil when unscored).
struct PostOut: Codable, Hashable, Sendable, Identifiable {
    var id: Int
    var round: Int
    var authorID: String
    var content: String
    /// Set for replies.
    var parentID: Int?
    /// Set for reposts.
    var repostOf: Int?
    var likes: Int = 0
    var dislikes: Int = 0
    var reposts: Int = 0
    var replies: Int = 0
    var authorHandle: String
    var sentiment: Double?

    var isReply: Bool { parentID != nil }
    var isRepost: Bool { repostOf != nil }

    enum CodingKeys: String, CodingKey {
        case id, round, content, likes, dislikes, reposts, replies, sentiment
        case authorID = "author_id"
        case parentID = "parent_id"
        case repostOf = "repost_of"
        case authorHandle = "author_handle"
    }
}

extension PostOut {
    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(Int.self, forKey: .id)
        round = c.value(.round, default: 0)
        authorID = try c.decode(String.self, forKey: .authorID)
        content = c.value(.content, default: "")
        parentID = c.optional(.parentID)
        repostOf = c.optional(.repostOf)
        likes = c.value(.likes, default: 0)
        dislikes = c.value(.dislikes, default: 0)
        reposts = c.value(.reposts, default: 0)
        replies = c.value(.replies, default: 0)
        authorHandle = c.value(.authorHandle, default: authorID)
        sentiment = c.optional(.sentiment)
    }
}

struct ActionOut: Codable, Hashable, Sendable {
    var round: Int
    var agentID: String
    var type: ActionType
    var targetPostID: Int?
    var targetAgentID: String?
    var content: String = ""
    /// Private one-liner, never shown in the feed.
    var reasoning: String = ""

    enum CodingKeys: String, CodingKey {
        case round, type, content, reasoning
        case agentID = "agent_id"
        case targetPostID = "target_post_id"
        case targetAgentID = "target_agent_id"
    }
}

extension ActionOut {
    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        round = c.value(.round, default: 0)
        agentID = try c.decode(String.self, forKey: .agentID)
        type = c.value(.type, default: .unknown)
        targetPostID = c.optional(.targetPostID)
        targetAgentID = c.optional(.targetAgentID)
        content = c.value(.content, default: "")
        reasoning = c.value(.reasoning, default: "")
    }
}

/// One `[follower, followee]` pair from `RunDetail.follows`.
struct FollowEdge: Hashable, Sendable {
    var follower: String
    var followee: String
}

/// One `[round, mean_score]` pair from `AgentDetail.sentiment_by_round`.
struct RoundScore: Codable, Hashable, Sendable, Identifiable {
    var round: Int
    var score: Double

    var id: Int { round }

    init(round: Int, score: Double) {
        self.round = round
        self.score = score
    }

    init(from decoder: any Decoder) throws {
        var c = try decoder.unkeyedContainer()
        round = try c.decode(Int.self)
        score = try c.decode(Double.self)
    }

    func encode(to encoder: any Encoder) throws {
        var c = encoder.unkeyedContainer()
        try c.encode(round)
        try c.encode(score)
    }
}

// MARK: - Config, report, status

struct SimConfig: Codable, Hashable, Sendable {
    var question: String
    var seedFiles: [String] = []
    var numAgents: Int = 20
    var rounds: Int = 6
    var batchSize: Int = 6
    var feedSize: Int = 12
    var memorySize: Int = 8
    var agentModel: String = "haiku"
    var plannerModel: String = "sonnet"
    var concurrency: Int = 4
    var language: String = "English"
    var events: [GodEvent] = []
    var seed: Int = 7
    var think: Bool = false
    var agentRuntime: String = "claude-code"
    var plannerRuntime: String = "claude-code"

    enum CodingKeys: String, CodingKey {
        case question, rounds, concurrency, language, events, seed, think
        case seedFiles = "seed_files"
        case numAgents = "num_agents"
        case batchSize = "batch_size"
        case feedSize = "feed_size"
        case memorySize = "memory_size"
        case agentModel = "agent_model"
        case plannerModel = "planner_model"
        case agentRuntime = "agent_runtime"
        case plannerRuntime = "planner_runtime"
    }
}

extension SimConfig {
    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        question = c.value(.question, default: "")
        seedFiles = c.value(.seedFiles, default: [])
        numAgents = c.value(.numAgents, default: 20)
        rounds = c.value(.rounds, default: 6)
        batchSize = c.value(.batchSize, default: 6)
        feedSize = c.value(.feedSize, default: 12)
        memorySize = c.value(.memorySize, default: 8)
        agentModel = c.value(.agentModel, default: "haiku")
        plannerModel = c.value(.plannerModel, default: "sonnet")
        concurrency = c.value(.concurrency, default: 4)
        language = c.value(.language, default: "English")
        events = c.value(.events, default: [])
        seed = c.value(.seed, default: 7)
        think = c.value(.think, default: false)
        agentRuntime = c.value(.agentRuntime, default: "claude-code")
        plannerRuntime = c.value(.plannerRuntime, default: "claude-code")
    }
}

struct ReportSection: Codable, Hashable, Sendable, Identifiable {
    var title: String
    var bodyMarkdown: String

    var id: String { title }

    enum CodingKeys: String, CodingKey {
        case title
        case bodyMarkdown = "body_markdown"
    }
}

/// One opinion group in the simulated crowd (`report.camps`). Every agent sits in exactly one camp.
struct Camp: Codable, Hashable, Sendable, Identifiable {
    var name: String
    var summary: String = ""
    /// Persona ids.
    var members: [String] = []
    /// Mean of the members' scored posts, nil when none were scored.
    var sentiment: Double?

    var id: String { name }

    enum CodingKeys: String, CodingKey {
        case name, summary, members, sentiment
    }
}

extension Camp {
    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        name = c.value(.name, default: "Camp")
        summary = c.value(.summary, default: "")
        members = c.value(.members, default: [])
        sentiment = c.optional(.sentiment)
    }
}

struct Report: Codable, Hashable, Sendable {
    var title: String
    var summary: String
    var prediction: String
    var confidence: ReportConfidence
    var sections: [ReportSection] = []
    /// Persona ids whose posts drove the outcome.
    var keyVoices: [String] = []
    /// Two to four one-line reasons for the answer, each with citations. Empty in runs reported before reasons existed.
    var reasons: [String] = []
    /// Opinion groups covering every agent. Empty in runs reported before camps existed.
    var camps: [Camp] = []
    var risks: [String] = []
    var caveats: [String] = []

    enum CodingKeys: String, CodingKey {
        case title, summary, prediction, confidence, sections, risks, caveats, reasons, camps
        case keyVoices = "key_voices"
    }
}

extension Report {
    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        title = c.value(.title, default: "Report")
        summary = c.value(.summary, default: "")
        prediction = c.value(.prediction, default: "")
        confidence = c.value(.confidence, default: .unknown)
        sections = c.value(.sections, default: [])
        keyVoices = c.value(.keyVoices, default: [])
        reasons = c.value(.reasons, default: [])
        camps = c.value(.camps, default: [])
        risks = c.value(.risks, default: [])
        caveats = c.value(.caveats, default: [])
    }
}

struct RunStatus: Codable, Hashable, Sendable {
    var state: RunState
    var stage: RunStage
    /// Last completed round (0 = opening posts done or not started).
    var round: Int = 0
    var totalRounds: Int
    var detail: String = ""
    var error: String?
    var pid: Int?
    /// Model batches that failed during the run (the run went on without those agents' turn).
    var failedBatches: [BatchFailedEvent] = []
    /// ISO 8601 UTC.
    var startedAt: String
    /// ISO 8601 UTC.
    var updatedAt: String

    var startedDate: Date? { ShoalDates.parse(startedAt) }
    var updatedDate: Date? { ShoalDates.parse(updatedAt) }

    enum CodingKeys: String, CodingKey {
        case state, stage, round, detail, error, pid
        case totalRounds = "total_rounds"
        case failedBatches = "failed_batches"
        case startedAt = "started_at"
        case updatedAt = "updated_at"
    }
}

extension RunStatus {
    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        state = c.value(.state, default: .unknown)
        stage = c.value(.stage, default: .unknown)
        round = c.value(.round, default: 0)
        totalRounds = c.value(.totalRounds, default: 0)
        detail = c.value(.detail, default: "")
        error = c.optional(.error)
        pid = c.optional(.pid)
        failedBatches = c.value(.failedBatches, default: [])
        startedAt = c.value(.startedAt, default: "")
        updatedAt = c.value(.updatedAt, default: startedAt)
    }
}

// MARK: - API shapes (shoal/server/app.py)

struct Health: Codable, Hashable, Sendable {
    var ok: Bool
    var fakeLLM: Bool
    var version: String

    enum CodingKeys: String, CodingKey {
        case ok, version
        case fakeLLM = "fake_llm"
    }
}

struct RunSummary: Codable, Hashable, Sendable, Identifiable {
    var id: String
    var question: String
    var state: RunState
    var stage: RunStage
    var round: Int
    var totalRounds: Int
    var agents: Int
    /// ISO 8601 UTC.
    var createdAt: String
    var hasReport: Bool

    var createdDate: Date? { ShoalDates.parse(createdAt) }
    /// 0...1 progress through the rounds.
    var progress: Double { totalRounds > 0 ? min(1, Double(round) / Double(totalRounds)) : 0 }

    enum CodingKeys: String, CodingKey {
        case id, question, state, stage, round, agents
        case totalRounds = "total_rounds"
        case createdAt = "created_at"
        case hasReport = "has_report"
    }
}

extension RunSummary {
    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        question = c.value(.question, default: "")
        state = c.value(.state, default: .unknown)
        stage = c.value(.stage, default: .unknown)
        round = c.value(.round, default: 0)
        totalRounds = c.value(.totalRounds, default: 0)
        agents = c.value(.agents, default: 0)
        createdAt = c.value(.createdAt, default: "")
        hasReport = c.value(.hasReport, default: false)
    }
}

struct CallsSummary: Codable, Hashable, Sendable {
    var count: Int = 0
    var failed: Int = 0
    var seconds: Double = 0
}

struct ReportRebuild: Codable, Hashable, Sendable {
    var running: Bool = false
    /// Why the last rebuild started in this server failed.
    var error: String?
}

struct RunDetail: Codable, Hashable, Sendable, Identifiable {
    var id: String
    var config: SimConfig
    var status: RunStatus?
    var live: Bool
    var personas: [Persona]
    var graph: KnowledgeGraph?
    var stats: [RoundStats]
    var events: [GodEvent]
    /// `[[follower, followee]]` exactly as sent. `followEdges` is the typed view.
    var follows: [[String]]
    var report: Report?
    var calls: CallsSummary
    var reportRebuild: ReportRebuild = .init()

    var followEdges: [FollowEdge] {
        follows.compactMap { $0.count >= 2 ? FollowEdge(follower: $0[0], followee: $0[1]) : nil }
    }

    var totalRounds: Int { status?.totalRounds ?? config.rounds }
    /// Last completed round.
    var currentRound: Int { status?.round ?? stats.map(\.round).max() ?? 0 }

    enum CodingKeys: String, CodingKey {
        case id, config, status, live, personas, graph, stats, events, follows, report, calls
        case reportRebuild = "report_rebuild"
    }
}

extension RunDetail {
    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        config = try c.decode(SimConfig.self, forKey: .config)
        status = try c.decodeIfPresent(RunStatus.self, forKey: .status)
        live = c.value(.live, default: false)
        personas = try c.decodeIfPresent([Persona].self, forKey: .personas) ?? []
        graph = try c.decodeIfPresent(KnowledgeGraph.self, forKey: .graph)
        stats = try c.decodeIfPresent([RoundStats].self, forKey: .stats) ?? []
        events = c.value(.events, default: [])
        follows = c.value(.follows, default: [])
        report = try c.decodeIfPresent(Report.self, forKey: .report)
        calls = c.value(.calls, default: CallsSummary())
        reportRebuild = c.value(.reportRebuild, default: ReportRebuild())
    }
}

struct AgentDetail: Codable, Hashable, Sendable {
    var persona: Persona
    var memory: [String]
    var posts: [PostOut]
    var followers: [String]
    var following: [String]
    /// Mean of the agent's own post scores, per round.
    var sentimentByRound: [RoundScore]

    enum CodingKeys: String, CodingKey {
        case persona, memory, posts, followers, following
        case sentimentByRound = "sentiment_by_round"
    }
}

extension AgentDetail {
    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        persona = try c.decode(Persona.self, forKey: .persona)
        memory = c.value(.memory, default: [])
        posts = try c.decodeIfPresent([PostOut].self, forKey: .posts) ?? []
        followers = c.value(.followers, default: [])
        following = c.value(.following, default: [])
        sentimentByRound = c.value(.sentimentByRound, default: [])
    }
}

struct CreatedRun: Codable, Hashable, Sendable { var id: String }
struct OkResponse: Codable, Hashable, Sendable { var ok: Bool }
struct ReplyResponse: Codable, Hashable, Sendable { var reply: String }

/// One interview exchange; encodes as the `[question, answer]` pair the chat endpoint expects.
struct ChatTurn: Codable, Hashable, Sendable {
    var question: String
    var answer: String

    init(question: String, answer: String) {
        self.question = question
        self.answer = answer
    }

    init(from decoder: any Decoder) throws {
        var c = try decoder.unkeyedContainer()
        question = try c.decode(String.self)
        answer = try c.decode(String.self)
    }

    func encode(to encoder: any Encoder) throws {
        var c = encoder.unkeyedContainer()
        try c.encode(question)
        try c.encode(answer)
    }
}

// MARK: - Dates

enum ShoalDates {
    /// Parses the engine's ISO 8601 UTC strings (`2026-11-07T00:05:41Z`, with or without fractional seconds).
    static func parse(_ string: String) -> Date? {
        guard !string.isEmpty else { return nil }
        if let d = try? Date(string, strategy: .iso8601) { return d }
        let fractional = Date.ISO8601FormatStyle(includingFractionalSeconds: true)
        if let d = try? fractional.parse(string) { return d }
        // `+00:00` offsets
        return try? Date.ISO8601FormatStyle().year().month().day().time(includingFractionalSeconds: false)
            .timeZone(separator: .colon).parse(string)
    }
}
