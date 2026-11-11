import Foundation
import Testing
@testable import Tellfold

/// Built-in example runs: their own sidebar section, and none of the controls that would change a run.
@MainActor
struct ExampleRunTests {
    private func summary(_ id: String, _ state: RunState, example: Bool = false) -> RunSummary {
        RunSummary(id: id, question: id, state: state, stage: .finished, round: 6, totalRounds: 6, agents: 20,
                   createdAt: "2026-11-08T10:00:00Z", hasReport: true, example: example)
    }

    @Test func sidebarGroupsExamplesLast() {
        let runs = [
            summary("example-corner-cafe", .done, example: true),
            summary("mine", .done),
            summary("live", .running),
            summary("example-school-phones", .done, example: true),
            summary("example-lookalike", .stopped),
        ]
        let sections = RunSidebar.sections(for: runs)
        #expect(sections.map(\.title) == ["Running", "Done", "Stopped", RunSidebar.examplesTitle])
        #expect(sections.last?.runs.map(\.id) == ["example-corner-cafe", "example-school-phones"])
        #expect(sections.first { $0.title == "Done" }?.runs.map(\.id) == ["mine"])
        #expect(sections.first { $0.title == "Stopped" }?.runs.map(\.id) == ["example-lookalike"])
    }

    @Test func sidebarHasNoExamplesSectionWithoutExamples() {
        #expect(!RunSidebar.sections(for: [summary("mine", .done)]).map(\.title).contains(RunSidebar.examplesTitle))
        #expect(RunSidebar.sections(for: [summary("example-corner-cafe", .done, example: true)]).map(\.title)
                == [RunSidebar.examplesTitle])
    }

    @Test func examplesHideTheControlsThatChangeARun() throws {
        #expect(RunControls(isLive: false, isReadOnly: true) == RunControls(isLive: true, isReadOnly: true))
        let example = RunControls(isLive: false, isReadOnly: true)
        #expect(!example.stop && !example.injectEvent && !example.rebuildReport)
        let live = RunControls(isLive: true, isReadOnly: false)
        #expect(live.stop && live.injectEvent && live.rebuildReport)
        #expect(!RunControls(isLive: false, isReadOnly: false).stop)

        var detail = try Fixtures.decode(RunDetail.self, "run_detail.json")
        detail.example = true
        let store = RunStore(detail: detail, posts: try Fixtures.decode([PostOut].self, "posts.json"),
                             actions: try Fixtures.decode([ActionOut].self, "actions.json"))
        #expect(store.isExample)
        #expect(!store.canRebuildReport)
        #expect(store.canRunAgain)
        #expect(store.controls == example)
    }

    @Test func exampleFlagDecodesAndDefaultsToFalse() throws {
        let json = #"[{"id": "example-corner-cafe", "question": "Q", "state": "done", "stage": "finished", "round": 6,"#
            + #" "total_rounds": 6, "agents": 20, "created_at": "2026-11-08T10:00:00Z", "has_report": true, "example": true},"#
            + #" {"id": "older", "question": "Q", "state": "done", "stage": "finished", "round": 1, "total_rounds": 1,"#
            + #" "agents": 2, "created_at": "2026-11-08T10:00:00Z", "has_report": false}]"#
        let runs = try JSONDecoder().decode([RunSummary].self, from: Data(json.utf8))
        #expect(runs.map(\.example) == [true, false])
        let detail = try Fixtures.decode(RunDetail.self, "run_detail.json")
        #expect(!detail.example)
    }

    @Test func runAgainFromAnExampleKeepsTheDefaultRuntimes() {
        let config = SimConfig(question: "Q", seedFiles: ["seeds/cafe.md"])
        #expect(NewSimulationPrefill.runAgain(config: config, seedFiles: [], fromReadOnly: true).keepsDefaultRuntimes)
        #expect(!NewSimulationPrefill.runAgain(config: config, seedFiles: []).keepsDefaultRuntimes)
    }
}
