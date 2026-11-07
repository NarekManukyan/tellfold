import SwiftUI

/// One opinion group as the views show it: the report's camp, or one of the two fallback groups for runs that were
/// reported before camps existed.
nonisolated struct CampGroup: Identifiable, Hashable, Sendable {
    let id: String
    var name: String
    var summary: String
    /// Persona ids; every agent sits in exactly one group.
    var members: [String]
    /// The engine's mean sentiment of the members' scored posts (final), nil when unknown.
    var sentiment: Double?
    var hex: UInt32

    var color: Color { Color(hex: hex) }

    /// Mean of the members' scores in `scores` (agent id → sentiment); nil when no member has one.
    func meanSentiment(_ scores: [String: Double]) -> Double? {
        let values = members.compactMap { scores[$0] }
        return values.isEmpty ? nil : values.reduce(0, +) / Double(values.count)
    }
}

extension RunStore {
    /// A camp's mean sentiment as of `round`, the way the engine computes it: the mean over the members' scored
    /// posts up to that round (posts, not per-agent means). At the run's last round the report's own figure wins.
    func campSentiment(_ group: CampGroup, at round: Int) -> Double? {
        if round >= latestRound, let s = group.sentiment { return s }
        return CampSentiment.mean(members: group.members, posts: posts, upTo: round)
    }
}

nonisolated enum CampSentiment {
    static func mean(members: [String], posts: [PostOut], upTo round: Int) -> Double? {
        let ids = Set(members)
        var sum = 0.0, n = 0
        for p in posts where p.round <= round && ids.contains(p.authorID) {
            if let s = p.sentiment { sum += s; n += 1 }
        }
        return n == 0 ? nil : sum / Double(n)
    }
}

nonisolated enum CampGrouping {
    struct Result: Equatable, Sendable {
        var groups: [CampGroup]
        /// True when the report has no camps and the crowd is split into "Organisations" and "People".
        var isFallback: Bool

        /// Group id by persona id.
        var groupByAgent: [String: String] {
            var out: [String: String] = [:]
            for g in groups { for m in g.members { out[m] = g.id } }
            return out
        }

        func group(_ id: String?) -> CampGroup? { id.flatMap { id in groups.first { $0.id == id } } }
    }

    static let fallbackNote = "This run was reported before camps existed. Rebuild the report to group the crowd by opinion."

    /// Groups the crowd: the report's camps in order (every persona exactly once; strays and unknown ids are dealt
    /// with), else "Organisations" and "People".
    static func groups(report: Report?, personas: [Persona]) -> Result {
        let known = Set(personas.map(\.id))
        guard let camps = report?.camps, !camps.isEmpty else {
            let orgs = personas.filter { $0.entityID != nil }.map(\.id)
            let people = personas.filter { $0.entityID == nil }.map(\.id)
            var groups: [CampGroup] = []
            if !orgs.isEmpty {
                groups.append(CampGroup(id: "fallback-organisations", name: "Organisations", summary: "", members: orgs,
                                        sentiment: nil, hex: Theme.campSlate))
            }
            if !people.isEmpty {
                groups.append(CampGroup(id: "fallback-people", name: "People", summary: "", members: people,
                                        sentiment: nil, hex: Theme.campSlate))
            }
            return Result(groups: groups, isFallback: true)
        }
        var seen = Set<String>()
        var groups: [CampGroup] = []
        var colorIndex = 0
        for (i, camp) in camps.enumerated() {
            let members = camp.members.filter { known.contains($0) && seen.insert($0).inserted }
            guard !members.isEmpty else { continue }
            let other = camp.name.trimmingCharacters(in: .whitespaces).caseInsensitiveCompare("other") == .orderedSame
            let hex = other ? Theme.campSlate : Theme.campHexes[colorIndex % Theme.campHexes.count]
            if !other { colorIndex += 1 }
            groups.append(CampGroup(id: "camp-\(i)", name: camp.name, summary: camp.summary, members: members,
                                    sentiment: camp.sentiment, hex: hex))
        }
        let strays = personas.map(\.id).filter { !seen.contains($0) }
        if !strays.isEmpty {
            if let i = groups.firstIndex(where: { $0.hex == Theme.campSlate && $0.name.lowercased() == "other" }) {
                groups[i].members += strays
            } else {
                groups.append(CampGroup(id: "camp-other", name: "Other", summary: "", members: strays, sentiment: nil,
                                        hex: Theme.campSlate))
            }
        }
        return Result(groups: groups, isFallback: false)
    }
}
