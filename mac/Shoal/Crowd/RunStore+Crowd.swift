import Foundation

// The only file in Crowd/ that reads RunStore and the Codable API models.

extension CrowdScene {
    @MainActor
    init(store: RunStore) {
        self.init()
        guard let detail = store.detail else { return }
        isLoaded = true
        round = store.playhead
        selectedAgentID = store.selectedAgentID
        selectedGroupID = store.selectedCampID

        let grouping = CampGrouping.groups(report: detail.report, personas: detail.personas)
        groups = grouping.groups
        isFallbackGrouping = grouping.isFallback
        var groupIndex: [String: Int] = [:]
        for (i, g) in groups.enumerated() { for m in g.members { groupIndex[m] = i } }
        nodes = detail.personas.map {
            CrowdNode(id: $0.id, handle: $0.handle, name: $0.name, influence: min(1, max(0, $0.influence)),
                      isOrganisation: $0.entityID != nil, groupIndex: groupIndex[$0.id] ?? 0)
        }
        for (i, n) in nodes.enumerated() { index[n.id] = i }
        sentiment = store.sentimentByAgent(at: round)
        runHasSentiment = store.posts.contains { $0.sentiment != nil }
        for g in groups { if let s = store.campSentiment(g, at: round) { groupSentiments[g.id] = s } }

        // Follow edges still to be created by later follow actions are hidden at this playhead.
        var future = Set<CanvasFollow>()
        for (r, list) in store.actionsByRound where r > round {
            for a in list where a.type == .follow {
                if let target = a.targetAgentID { future.insert(CanvasFollow(follower: a.agentID, followee: target)) }
            }
        }
        var seen = Set<CanvasFollow>()
        follows = detail.follows.compactMap { pair -> CanvasFollow? in
            guard pair.count >= 2, pair[0] != pair[1], index[pair[0]] != nil, index[pair[1]] != nil else { return nil }
            let f = CanvasFollow(follower: pair[0], followee: pair[1])
            return future.contains(f) || !seen.insert(f).inserted ? nil : f
        }

        // Replies and reposts up to the playhead, from the posts themselves.
        var authors: [Int: String] = [:]
        authors.reserveCapacity(store.posts.count)
        for p in store.posts { authors[p.id] = p.authorID }
        struct Pair: Hashable { var from: String; var to: String }
        var counts: [Pair: (replies: Int, reposts: Int)] = [:]
        for p in store.posts where p.round <= round {
            if let parent = p.parentID, let to = authors[parent], to != p.authorID, index[to] != nil, index[p.authorID] != nil {
                counts[Pair(from: p.authorID, to: to), default: (0, 0)].replies += 1
            } else if let original = p.repostOf, let to = authors[original], to != p.authorID,
                      index[to] != nil, index[p.authorID] != nil {
                counts[Pair(from: p.authorID, to: to), default: (0, 0)].reposts += 1
            }
        }
        edges = counts.map { CrowdEdge(from: $0.key.from, to: $0.key.to, replies: $0.value.replies, reposts: $0.value.reposts) }
            .sorted { ($0.count, $0.from, $0.to) < ($1.count, $1.from, $1.to) }

        // The playhead round's interactions.
        for action in store.actionsByRound[round] ?? [] {
            guard let kind = ArcKind(actionType: action.type.rawValue), kind != .follow else { continue }
            let target = action.targetPostID.flatMap { authors[$0] } ?? action.targetAgentID
            guard let target, target != action.agentID, index[target] != nil, index[action.agentID] != nil else { continue }
            arcs.append(CrowdArc(from: action.agentID, to: target, kind: kind))
        }
    }
}
