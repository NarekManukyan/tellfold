import Foundation

/// One line of the run log: an agent action, a god event, a failed model batch, or the run's end (stopped or
/// failed). Pure data, built from what the run store holds.
nonisolated struct RunLogLine: Identifiable, Hashable, Sendable {
    enum Kind: Hashable, Sendable {
        case action
        /// An injected god event (the only amber lines).
        case event
        /// A model batch that failed; `text` names the batch and the skipped agents.
        case failedBatch
        /// The run stopped or failed; `action` holds the state word.
        case end
    }

    /// 1-based line number over the whole log (stable whether or not idle lines are shown).
    let id: Int
    let round: Int
    var kind: Kind = .action
    var isGod: Bool { kind == .event }
    let agentID: String?
    let handle: String
    /// The post this action created, when it created one.
    let postID: Int?
    /// `post`, `reply`, `repost`, `like`, `dislike`, `follow`, `nothing`, or `event`.
    let action: String
    let targetPostID: Int?
    let targetAgentID: String?
    let targetHandle: String?
    /// Event text for god lines; the detail for failed-batch and end lines.
    let text: String
    /// The created post's sentiment, when the action made a scored post.
    var sentiment: Double? = nil

    var isIdle: Bool { action == "nothing" }

    /// The part after the action word: `-> #1 @revenuecat`, or the event / failure text.
    var targetText: String {
        if kind != .action { return text }
        var parts: [String] = []
        if let targetPostID { parts.append("#\(targetPostID)") }
        if let targetHandle { parts.append("@\(targetHandle)") }
        return parts.isEmpty ? "" : "-> " + parts.joined(separator: " ")
    }

    /// The plain one-line form, e.g. `r4 #20 @firsttime_tessa reply -> #1 @revenuecat` (VoiceOver and copy).
    var plainText: String {
        switch kind {
        case .event: return "r\(round) god event \(text)"
        case .failedBatch: return "r\(round) failed \(text)"
        case .end: return "r\(round) \(action) \(text)"
        case .action: break
        }
        let post = postID.map { "#\($0)" } ?? "-"
        return "r\(round) \(post) @\(handle) \(action) \(targetText)".trimmingCharacters(in: .whitespaces)
    }
}

nonisolated enum RunLog {
    /// Builds the whole log, ascending by round. Within a round the god event comes first, then the actions in
    /// the order the engine recorded them. A post created by `post` / `reply` / `repost` actions is matched to its
    /// action by author and round; posts with no recorded action (the opening posts) get a `post` line of their own.
    /// Failed batches follow the actions of their round; a stopped or failed run ends with one line saying so.
    static func lines(personas: [Persona], posts: [PostOut], actionsByRound: [Int: [ActionOut]],
                      events: [GodEvent], failedBatches: [BatchFailedEvent] = [],
                      status: RunStatus? = nil) -> [RunLogLine] {
        var handleByAgent: [String: String] = [:]
        for p in personas { handleByAgent[p.id] = p.handle }
        var postByID: [Int: PostOut] = [:]
        for p in posts { postByID[p.id] = p }

        // Posts per (round, author), oldest first, consumed by the matching actions.
        struct Key: Hashable { let round: Int; let author: String }
        var queues: [Key: [PostOut]] = [:]
        for p in posts.sorted(by: { $0.id < $1.id }) { queues[Key(round: p.round, author: p.authorID), default: []].append(p) }

        let rounds = Set(actionsByRound.keys).union(posts.map(\.round)).union(events.map(\.round))
            .union(failedBatches.map(\.round)).sorted()
        var out: [RunLogLine] = []
        func append(round: Int, kind: RunLogLine.Kind = .action, agent: String?, handle: String, post: Int?,
                    action: String, targetPost: Int?, targetAgent: String?, targetHandle: String?, text: String) {
            out.append(RunLogLine(id: out.count + 1, round: round, kind: kind, agentID: agent, handle: handle,
                                  postID: post, action: action, targetPostID: targetPost, targetAgentID: targetAgent,
                                  targetHandle: targetHandle, text: text,
                                  sentiment: post.flatMap { postByID[$0]?.sentiment }))
        }

        for round in rounds {
            for event in events where event.round == round {
                append(round: round, kind: .event, agent: nil, handle: "", post: nil, action: "event", targetPost: nil,
                       targetAgent: nil, targetHandle: nil, text: event.content.replacingOccurrences(of: "\n", with: " "))
            }
            for action in actionsByRound[round] ?? [] {
                let type = "\(action.type)"
                var created: Int?
                if ["post", "reply", "repost"].contains(type) {
                    let key = Key(round: round, author: action.agentID)
                    if var queue = queues[key], !queue.isEmpty {
                        created = queue.removeFirst().id
                        queues[key] = queue
                    }
                }
                let targetPost = action.targetPostID
                let targetAgent = action.targetAgentID ?? targetPost.flatMap { postByID[$0]?.authorID }
                let targetHandle = targetAgent.flatMap { handleByAgent[$0] } ?? targetPost.flatMap { postByID[$0]?.authorHandle }
                append(round: round, agent: action.agentID, handle: handleByAgent[action.agentID] ?? action.agentID,
                       post: created, action: type, targetPost: targetPost, targetAgent: targetAgent,
                       targetHandle: type == "nothing" ? nil : targetHandle, text: "")
            }
            // Posts nobody recorded an action for (opening posts, older runs).
            for (key, queue) in queues.sorted(by: { $0.value.first?.id ?? 0 < $1.value.first?.id ?? 0 })
            where key.round == round && !queue.isEmpty {
                for post in queue {
                    let parentAuthor = post.parentID.flatMap { postByID[$0] }
                    let repostOf = post.repostOf.flatMap { postByID[$0] }
                    let target = parentAuthor ?? repostOf
                    append(round: round, agent: post.authorID, handle: post.authorHandle, post: post.id,
                           action: post.isReply ? "reply" : (post.isRepost ? "repost" : "post"),
                           targetPost: target?.id, targetAgent: target?.authorID, targetHandle: target?.authorHandle, text: "")
                }
                queues[key] = []
            }
            for batch in failedBatches where batch.round == round {
                append(round: round, kind: .failedBatch, agent: nil, handle: "", post: nil, action: "failed",
                       targetPost: nil, targetAgent: nil, targetHandle: nil, text: failureText(batch))
            }
        }
        if let status, status.state == .stopped || status.state == .failed {
            let round = max(status.round, rounds.last ?? 0)
            let failed = status.state == .failed
            append(round: round, kind: .end, agent: nil, handle: "", post: nil, action: failed ? "failed" : "stopped",
                   targetPost: nil, targetAgent: nil, targetHandle: nil,
                   text: failed ? (status.error ?? "the run failed") : "stopped after round \(status.round)")
        }
        return out
    }

    /// `batch 2, 3 agents skipped: rate limited` (the sentiment batch reads `batch sentiment`).
    static func failureText(_ batch: BatchFailedEvent) -> String {
        var text = "batch \(batch.batch.isEmpty ? "?" : batch.batch)"
        if !batch.agents.isEmpty { text += ", \(batch.agents.count) agent\(batch.agents.count == 1 ? "" : "s") skipped" }
        if !batch.error.isEmpty { text += ": " + batch.error.replacingOccurrences(of: "\n", with: " ") }
        return text
    }
}
