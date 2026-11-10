import Foundation
import Testing
@testable import Tellfold

@Suite("Branch from a round")
struct BranchTests {
    @Test func whenARunCanBeBranched() {
        #expect(BranchLogic.canBranch(round: 2, latestRound: 6, totalRounds: 6, isLive: false))
        #expect(BranchLogic.canBranch(round: 0, latestRound: 6, totalRounds: 6, isLive: false))
        #expect(!BranchLogic.canBranch(round: 6, latestRound: 6, totalRounds: 6, isLive: false))  // nothing left to run
        #expect(!BranchLogic.canBranch(round: 4, latestRound: 3, totalRounds: 6, isLive: false))  // not reached
        #expect(!BranchLogic.canBranch(round: 1, latestRound: 3, totalRounds: 6, isLive: true))
    }

    @Test func laterEventsArePrefilledInOrder() {
        let events = [GodEvent(round: 5, content: "e5"), GodEvent(round: 1, content: "e1"), GodEvent(round: 3, content: "e3")]
        #expect(BranchLogic.laterEvents(events, after: 2).map(\.content) == ["e3", "e5"])
        #expect(BranchLogic.laterEvents(events, after: 5).isEmpty)
    }

    @Test func onlySendableEventsAreSent() {
        let rows = [GodEvent(round: 3, content: "  The owner apologises "), GodEvent(round: 4, content: "  "),
                    GodEvent(round: 2, content: "too early"), GodEvent(round: 9, content: "too late")]
        #expect(BranchLogic.sendable(rows, after: 2, totalRounds: 6) == [GodEvent(round: 3, content: "The owner apologises")])
    }

    @Test func summaryWording() {
        #expect(BranchLogic.summary(round: 2).hasPrefix("Rounds 0 to 2 stay exactly"))
        #expect(BranchLogic.summary(round: 0).hasPrefix("The opening posts stay"))
    }

    @Test func configDecodesBranchFields() throws {
        let c = try TellfoldAPI.decoder.decode(SimConfig.self, from: Data(#"{"question":"q","branched_from":"src","branch_round":2}"#.utf8))
        #expect(c.branchedFrom == "src" && c.branchRound == 2)
        let old = try TellfoldAPI.decoder.decode(SimConfig.self, from: Data(#"{"question":"q"}"#.utf8))
        #expect(old.branchedFrom == nil && old.branchRound == nil)
    }
}
