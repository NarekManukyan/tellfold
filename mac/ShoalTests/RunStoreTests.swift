import Foundation
import Testing
@testable import Shoal

@MainActor
struct RunStoreTests {
    private func makeStore() throws -> RunStore {
        RunStore(detail: try Fixtures.decode(RunDetail.self, "run_detail.json"),
                 posts: try Fixtures.decode([PostOut].self, "posts.json"),
                 actions: try Fixtures.decode([ActionOut].self, "actions.json"))
    }

    @Test func previewStoreIsReady() throws {
        let store = try makeStore()
        #expect(store.loadState == .loaded)
        #expect(store.runID == store.detail?.id)
        #expect(store.latestRound == 3)
        #expect(store.playhead == 3)
        #expect(store.totalRounds == 3)
        #expect(!store.isLive)
        #expect(store.personas.count == 6)
        #expect(Set(store.actionsByRound.keys) == [0, 1, 2, 3])
        #expect(store.posts.map(\.id) == store.posts.map(\.id).sorted())
        #expect(store.persona(handle: "@" + (store.personas.first?.handle ?? "")) == store.personas.first)
    }

    @Test func sentimentByAgentIsMeanOfScoredPostsUpToRound() throws {
        let store = try makeStore()
        let round0 = store.sentimentByAgent(at: 0)
        let scored0 = store.posts.filter { $0.round == 0 && $0.sentiment != nil }
        #expect(Set(round0.keys) == Set(scored0.map(\.authorID)))
        let all = store.sentimentByAgent(at: 3)
        for (agent, value) in all {
            let scores = store.posts.filter { $0.authorID == agent }.compactMap(\.sentiment)
            #expect(abs(value - scores.reduce(0, +) / Double(scores.count)) < 1e-9)
        }
        // agents without a scored post are absent
        let silent = store.personas.map(\.id).filter { id in !store.posts.contains { $0.authorID == id && $0.sentiment != nil } }
        #expect(silent.allSatisfy { all[$0] == nil })
        #expect(store.sentimentByAgent(at: -1).isEmpty)
    }

    @Test func playheadClampsAndTracksLive() throws {
        let store = try makeStore()
        store.setPlayhead(99)
        #expect(store.playhead == 3)
        store.stepPlayhead(by: -5)
        #expect(store.playhead == 0)
        store.stepPlayhead(by: 1)
        #expect(store.playhead == 1)
        #expect(store.followsLive) // not live: follow flag stays harmlessly on
        store.togglePlaying()
        #expect(store.isPlaying)
        store.togglePlaying()
        #expect(!store.isPlaying)
    }

    @Test func togglePlayingAtEndRestartsFromZero() throws {
        let store = try makeStore()
        store.setPlayhead(3)
        store.togglePlaying()
        #expect(store.playhead == 0)
        #expect(store.isPlaying)
    }

    @Test func streamEventsUpdateTheStore() async throws {
        let store = try makeStore()
        let before = store.eventCounter

        let stats = RoundStats(round: 4, actions: ["post": 2], newPosts: 2, activeAgents: 3, sentiment: -0.25)
        await store.handle(.roundDone(RoundDoneEvent(stats: stats, totalRounds: 4)))
        #expect(store.stats.last == stats)
        #expect(store.stats.map(\.round) == [0, 1, 2, 3, 4])

        let replaced = RoundStats(round: 4, actions: ["post": 5], newPosts: 5, activeAgents: 3, sentiment: 0.1)
        await store.handle(.roundDone(RoundDoneEvent(stats: replaced)))
        #expect(store.stats.filter { $0.round == 4 } == [replaced])

        await store.handle(.eventInjected(GodEvent(round: 5, content: "Breaking")))
        await store.handle(.eventInjected(GodEvent(round: 5, content: "Breaking")))
        #expect(store.events(in: 5) == [GodEvent(round: 5, content: "Breaking")])

        await store.handle(.stage(StageEvent(stage: .rounds, detail: "round 5 of 6: 2 batches deciding")))
        #expect(store.stageDetail == "round 5 of 6: 2 batches deciding")

        let start = RoundStartEvent(round: 5, active: 2, totalRounds: 6, activeAgents: ["a01", "a02"], batches: 1)
        await store.handle(.roundStart(start))
        #expect(store.lastRoundStart == start)

        let failure = BatchFailedEvent(round: 5, batch: "0", agents: ["a01"], error: "timeout")
        await store.handle(.batchFailed(failure))
        #expect(store.batchFailures == [failure])

        let running = RunStatus(state: .running, stage: .rounds, round: 4, totalRounds: 6, detail: "round 4 of 6 done",
                                startedAt: "2026-11-07T00:00:00Z", updatedAt: "2026-11-07T00:01:00Z")
        await store.handle(.status(running))
        #expect(store.status == running)
        #expect(store.totalRounds == 6)
        #expect(store.latestRound == 4)
        #expect(store.eventCounter == before + 8)
        #expect(store.lastEvent == .status(running))
    }

    @Test func liveStatusMakesTheStoreLive() async throws {
        var detail = try Fixtures.decode(RunDetail.self, "run_detail_live.json")
        detail.stats = []
        let store = RunStore(detail: detail, posts: [], actions: [])
        #expect(store.isLive)
        await store.handle(.runStopped)
        #expect(!store.isLive)
    }
}
