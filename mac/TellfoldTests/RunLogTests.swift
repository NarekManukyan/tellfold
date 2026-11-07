import Foundation
import Testing
@testable import Tellfold

@Suite("Run log")
struct RunLogTests {
    private func persona(_ id: String, _ handle: String) -> Persona {
        Persona(id: id, handle: handle, name: handle)
    }

    private func post(_ id: Int, round: Int, author: String, handle: String, parent: Int? = nil) -> PostOut {
        var p = PostOut(id: id, round: round, authorID: author, content: "x", authorHandle: handle)
        p.parentID = parent
        return p
    }

    private func action(_ round: Int, _ agent: String, _ type: ActionType, post: Int? = nil, agentTarget: String? = nil) -> ActionOut {
        ActionOut(round: round, agentID: agent, type: type, targetPostID: post, targetAgentID: agentTarget)
    }

    private var lines: [RunLogLine] {
        let personas = [persona("a1", "revenuecat"), persona("a2", "tessa")]
        let posts = [post(1, round: 0, author: "a1", handle: "revenuecat"),
                     post(20, round: 4, author: "a2", handle: "tessa", parent: 1)]
        let actions: [Int: [ActionOut]] = [
            4: [action(4, "a2", .reply, post: 1), action(4, "a1", .nothing), action(4, "a1", .like, post: 20)],
        ]
        return RunLog.lines(personas: personas, posts: posts, actionsByRound: actions,
                            events: [GodEvent(round: 4, content: "Breaking news")])
    }

    @Test func numbersLinesAndPutsTheGodEventFirstInItsRound() {
        let lines = lines
        #expect(lines.map(\.id) == Array(1...lines.count))
        #expect(lines.map(\.round) == [0, 4, 4, 4, 4])
        #expect(lines[1].isGod && lines[1].text == "Breaking news")
        #expect(lines.filter(\.isGod).count == 1)
    }

    @Test func repliesCarryTheirPostIDAndTarget() {
        let reply = lines.first { $0.action == "reply" }
        #expect(reply?.postID == 20)
        #expect(reply?.targetPostID == 1)
        #expect(reply?.targetHandle == "revenuecat")
        #expect(reply?.plainText == "r4 #20 @tessa reply -> #1 @revenuecat")
    }

    @Test func openingPostsWithoutActionsGetTheirOwnLine() {
        let opening = lines.first { $0.round == 0 }
        #expect(opening?.action == "post" && opening?.postID == 1)
    }

    @Test func idleLinesAreMarked() {
        #expect(lines.filter(\.isIdle).count == 1)
    }

    @Test func failedBatchesAndTheEndStateGetTheirOwnLines() {
        let status = RunStatus(state: .stopped, stage: .rounds, round: 4, totalRounds: 6, startedAt: "", updatedAt: "")
        let lines = RunLog.lines(personas: [persona("a1", "revenuecat")],
                                 posts: [post(1, round: 0, author: "a1", handle: "revenuecat")],
                                 actionsByRound: [4: [action(4, "a1", .nothing)]], events: [],
                                 failedBatches: [BatchFailedEvent(round: 4, batch: "2", agents: ["a2", "a3"], error: "rate limited")],
                                 status: status)
        #expect(lines.map(\.kind) == [RunLogLine.Kind.action, .action, .failedBatch, .end])
        #expect(lines[2].plainText == "r4 failed batch 2, 2 agents skipped: rate limited")
        #expect(lines[3].action == "stopped" && lines[3].text == "stopped after round 4")
        #expect(lines.map(\.id) == [1, 2, 3, 4])
    }

    @Test func scoredPostsCarryTheirSentiment() {
        var scored = post(20, round: 4, author: "a2", handle: "tessa", parent: 1)
        scored.sentiment = -0.66
        let lines = RunLog.lines(personas: [persona("a2", "tessa")], posts: [scored],
                                 actionsByRound: [4: [action(4, "a2", .reply, post: 1)]], events: [])
        #expect(lines.first?.sentiment == -0.66)
    }
}
