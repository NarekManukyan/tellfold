import Foundation
import Testing
@testable import Tellfold

@Suite("Finish review fixes")
struct FinishReviewTests {
    private func post(_ id: Int, _ round: Int, _ author: String, _ s: Double?) -> PostOut {
        PostOut(id: id, round: round, authorID: author, content: "c", parentID: nil, repostOf: nil,
                authorHandle: "h_\(author)", sentiment: s)
    }

    @Test func campSentimentIsTheMeanOverPostsNotOverAgents() {
        // a1 posted twice (1.0, 1.0), a2 once (-1.0): per-post mean is 1/3, per-agent mean would be 0.
        let posts = [post(1, 1, "a1", 1), post(2, 2, "a1", 1), post(3, 2, "a2", -1), post(4, 3, "a3", 0.9)]
        let mean = CampSentiment.mean(members: ["a1", "a2"], posts: posts, upTo: 3)
        #expect(mean != nil && abs(mean! - 1.0 / 3.0) < 1e-9)
        // At an earlier playhead only the posts up to it count.
        #expect(CampSentiment.mean(members: ["a1", "a2"], posts: posts, upTo: 1) == 1)
        #expect(CampSentiment.mean(members: ["a9"], posts: posts, upTo: 3) == nil)
        #expect(CampSentiment.mean(members: ["a1"], posts: [post(5, 1, "a1", nil)], upTo: 3) == nil)
    }

    @Test func reportAgentIDsReadAsHandles() {
        let p = Persona(id: "a08", handle: "pearl_and_biscuit", name: "Pearl", influence: 0.5, entityID: nil)
        let linkifier = RunLinkifier(personas: [p], postIDs: [])
        let text = String(linkifier.inline("Skeptics a08, a09 flipped").characters)
        #expect(text.contains("@pearl_and_biscuit"))
        #expect(!text.contains("a08"))
        #expect(text.contains("a09"))  // unknown ids stay as they are
    }

    @Test func longCampLabelsKeepTheirName() {
        let short = CrowdLabel(name: "Other", count: 7, sentiment: 0.48)
        #expect(!short.twoLines)
        let long = CrowdLabel(name: "Unconvinced subscription skeptics", count: 2, sentiment: -0.5)
        #expect(long.twoLines)
        #expect(long.width >= long.nameWidth + CrowdLabel.sidePadding)
        #expect(long.figures == "2 · \u{2212}0.50")
    }

    @Test func spectrumPutsUnscoredAgentsInTheirOwnLaneBelowTheAxis() {
        let nodes = (0..<6).map { CrowdNode(id: "a\($0)", handle: "h\($0)", name: "N\($0)", influence: 0.4, isOrganisation: false, groupIndex: 0) }
        let layout = CrowdLayout.compute(nodes: nodes, groupCount: 1, sentiment: ["a0": 0.2, "a1": -0.4, "a2": 0.9],
                                         size: CGSize(width: 600, height: 400), mode: .spectrum)
        let lane = try! #require(layout.spectrumLaneY)
        #expect(lane > layout.spectrumAxisY)
        for id in ["a3", "a4", "a5"] { #expect((layout.positions[id]?.y ?? 0) > lane) }
        for id in ["a0", "a1", "a2"] { #expect((layout.positions[id]?.y ?? 99999) < layout.spectrumAxisY) }
        let none = CrowdLayout.compute(nodes: nodes, groupCount: 1, sentiment: Dictionary(uniqueKeysWithValues: nodes.map { ($0.id, 0.1) }),
                                       size: CGSize(width: 600, height: 400), mode: .spectrum)
        #expect(none.spectrumLaneY == nil)
    }

    @Test func postsPerRoundLeavesOutTheOpeningPosts() {
        let m = RunMetrics(people: 20, organisations: 0, roundsDone: 6, totalRounds: 6, posts: 38, roundPosts: 36,
                           finalSentiment: 0.5, baselineSentiment: 0.1)
        #expect(m.postsPerRound == 6)
    }

    @Test func latestRoundTakesTheHighestRecordedRound() {
        // A stopped run whose status says 3 but whose data goes to 6.
        #expect(RunRounds.latest(status: 3, stats: [0, 1, 2, 3, 4, 5, 6], posts: [0, 1, 6], actions: [1, 2]) == 6)
        // Posts or actions ahead of the stats count too.
        #expect(RunRounds.latest(status: 2, stats: [0, 1, 2], posts: [0, 4], actions: [5]) == 5)
        // Nothing recorded: the status stands, and an empty run is round 0.
        #expect(RunRounds.latest(status: 2, stats: [], posts: [], actions: []) == 2)
        #expect(RunRounds.latest(status: 0, stats: [], posts: [], actions: []) == 0)
    }
}
