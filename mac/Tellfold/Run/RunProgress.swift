import Foundation

/// Plain-words progress for a run: what it is doing now and how far along it is.
nonisolated enum RunProgress {
    /// "Reading documents", "Simulating round 3 of 6", "Writing the report".
    static func stageTitle(stage: RunStage, round: Int, totalRounds: Int) -> String {
        switch stage {
        case .reading: "Reading documents"
        case .graph: "Mapping the topic"
        case .personas: "Casting people"
        case .seeding: "Seeding the conversation"
        case .rounds:
            totalRounds > 0 ? "Simulating round \(min(round + 1, totalRounds)) of \(totalRounds)" : "Simulating rounds"
        case .report: "Writing the report"
        case .finished: "Finished"
        case .unknown: "Starting up"
        }
    }

    /// 0...1 over the whole pipeline; the rounds stage takes most of it.
    static func fraction(stage: RunStage, round: Int, totalRounds: Int) -> Double {
        switch stage {
        case .unknown: 0.02
        case .reading: 0.06
        case .graph: 0.14
        case .personas: 0.24
        case .seeding: 0.32
        case .rounds:
            0.34 + 0.56 * (totalRounds > 0 ? min(1, Double(round) / Double(totalRounds)) : 0)
        case .report: 0.94
        case .finished: 1
        }
    }
}

/// The last completed round by the recorded data: the highest of the status, the stats, the posts and the actions.
/// One rule for the open run, the sidebar rows and the headline, so they cannot disagree.
nonisolated enum RunRounds {
    static func latest(status: Int, stats: [Int], posts: [Int], actions: [Int]) -> Int {
        max(status, stats.max() ?? 0, posts.max() ?? 0, actions.max() ?? 0)
    }
}

/// The run's headline numbers.
nonisolated struct RunMetrics: Equatable, Sendable {
    var people: Int
    var organisations: Int
    var roundsDone: Int
    var totalRounds: Int
    var posts: Int
    /// Posts written in rounds 1 and later (the opening posts of round 0 are not part of a round).
    var roundPosts: Int
    /// Mean sentiment of the newest round that has a figure.
    var finalSentiment: Double?
    /// Mean sentiment of round 0 (the opening posts), when it has one.
    var baselineSentiment: Double?

    var peopleOnly: Int { people - organisations }

    /// `final - baseline`, when both exist.
    var delta: Double? {
        guard let finalSentiment, let baselineSentiment else { return nil }
        return finalSentiment - baselineSentiment
    }

    var postsPerRound: Double { roundsDone > 0 ? Double(roundPosts) / Double(roundsDone) : Double(roundPosts) }
}

/// `local / claude code / haiku + sonnet`: where the run executed and what it used.
nonisolated enum RunProvenance {
    /// Runs from before the engine recorded runtimes decode as claude-code, which is what they used.
    static func text(_ c: SimConfig) -> String {
        func name(_ id: String) -> String { (id.isEmpty ? "claude-code" : id).replacingOccurrences(of: "-", with: " ") }
        let agent = name(c.agentRuntime), planner = name(c.plannerRuntime)
        if agent == planner { return "local / \(agent) / \(c.agentModel) + \(c.plannerModel)" }
        return "local / \(agent) \(c.agentModel) + \(planner) \(c.plannerModel)"
    }

    /// `4m 12s`, `1h 03m`.
    static func duration(_ seconds: Int) -> String {
        if seconds >= 3600 { return "\(seconds / 3600)h \(String(format: "%02d", seconds % 3600 / 60))m" }
        if seconds >= 60 { return "\(seconds / 60)m \(String(format: "%02d", seconds % 60))s" }
        return "\(seconds)s"
    }
}
