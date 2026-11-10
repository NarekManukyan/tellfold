import Foundation
import Testing
@testable import Tellfold

@Suite("Studies: grouping, labels, links and the version request")
struct StudyTests {
    private func run(_ id: String, state: RunState = .done, studyID: String? = nil, question: String = "Q?") -> RunSummary {
        var r = RunSummary(id: id, question: question, state: state, stage: .finished, round: 2, totalRounds: 2, agents: 4,
                           createdAt: "", hasReport: true)
        r.studyID = studyID
        return r
    }

    private func study(_ id: String, _ members: [(String, String)], created: String = "", question: String = "") -> StudySummary {
        StudySummary(id: id, createdAt: created, question: question,
                     runs: members.enumerated().map { i, m in
                         StudyMember(id: m.0, variant: m.1, letter: String(Array("ABCDEFGH")[i]))
                     })
    }

    @Test func selectionTags() {
        #expect(StudySelection.tag("s1") == "study:s1")
        #expect(StudySelection.studyID(from: "study:s1") == "s1")
        #expect(StudySelection.studyID(from: "20261110-run") == nil)
        #expect(StudySelection.studyID(from: "study:") == nil)
        #expect(StudySelection.studyID(from: nil) == nil)
    }

    @Test func rowsKeepRunsStillListedNewestStudyFirst() {
        let studies = [
            study("old", [("a", "Draft A"), ("gone", "Draft B")], created: "2026-11-01T00:00:00Z"),
            study("new", [("b", "Draft A"), ("c", "Calm")], created: "2026-11-09T00:00:00Z", question: "Will it?"),
            study("empty", [("gone2", "Draft A")], created: "2026-11-10T00:00:00Z"),
        ]
        let runs = [run("a", question: "First?"), run("b"), run("c", state: .running)]
        let rows = StudyLogic.rows(studies: studies, runs: runs)
        #expect(rows.map(\.id) == ["new", "old"])
        #expect(rows[0].title == "Draft A vs Calm")
        #expect(rows[0].isActive && !rows[1].isActive)
        #expect(rows[0].question == "Will it?")
        #expect(rows[1].question == "First?")  // the study has no question: the first run's
        #expect(rows[1].members.map(\.id) == ["a"])
    }

    @Test func studyOfARunAndTheNextLabel() {
        let studies = [study("s1", [("src", "Draft A"), ("v2", "Draft B")])]
        let runs = [run("src"), run("v2", studyID: "s1"), run("lone")]
        #expect(StudyLogic.study(of: "v2", runs: runs, studies: studies) == "s1")
        #expect(StudyLogic.study(of: "src", runs: runs, studies: studies) == "s1")  // only through the study file
        #expect(StudyLogic.study(of: "lone", runs: runs, studies: studies) == nil)
        #expect(StudyLogic.nextVariant(for: "v2", runs: runs, studies: studies) == "Draft C")
        #expect(StudyLogic.nextVariant(for: "lone", runs: runs, studies: studies) == "Draft B")
        #expect(StudyLogic.defaultVariant(0) == "Draft A")
        #expect(StudyLogic.defaultVariant(30) == "Draft 31")
    }

    @Test func phases() throws {
        var detail = try Fixtures.decode(StudyDetail.self, "study_detail.json")
        #expect(StudyLogic.phase(of: detail) == .compared)
        detail.compareState.running = true
        #expect(StudyLogic.phase(of: detail) == .comparing)
        detail.runs[1].state = .running
        #expect(StudyLogic.phase(of: detail) == .running)
        detail.runs[1].state = .done
        detail.compareState = CompareState()
        detail.compare = nil
        #expect(StudyLogic.phase(of: detail) == .readyToCompare)
        detail.runs[1].hasReport = false
        #expect(StudyLogic.phase(of: detail) == .notEnoughRuns)
    }

    @Test func decodesTheStudyFixture() throws {
        let detail = try Fixtures.decode(StudyDetail.self, "study_detail.json")
        #expect(detail.runs.map(\.letter) == ["A", "B"])
        #expect(detail.runs.map(\.variant) == ["Draft A", "Draft B"])
        #expect(detail.runs.allSatisfy { $0.state == .done && $0.hasReport })
        #expect(detail.runs[0].digest.rounds.map(\.round) == [0, 1, 2])
        let compare = try #require(detail.compare)
        #expect(compare.studyID == detail.id)
        #expect(compare.runs.map(\.letter) == ["A", "B"])
        #expect(compare.report.caveats.first?.hasPrefix("Each version was played once") == true)
        #expect(compare.report.citationAudit != nil)

        let list = try Fixtures.decode([StudySummary].self, "studies.json")
        #expect(list.map(\.id) == [detail.id])
        #expect(list[0].hasCompare && !list[0].comparing)
        #expect(list[0].runs.map(\.variant) == ["Draft A", "Draft B"])
    }

    @Test func curvesSkipUnscoredRounds() {
        let digest = RunDigest(letter: "A", runID: "a", variant: "Draft A",
                               rounds: [RoundPoint(round: 0, sentiment: nil), RoundPoint(round: 1, sentiment: 0.2),
                                        RoundPoint(round: 2, sentiment: -0.1)])
        let detail = StudyDetail(id: "s", runs: [StudyRun(id: "a", variant: "Draft A", letter: "A", digest: digest)])
        let curves = StudyLogic.curves(detail)
        #expect(curves.count == 1)
        #expect(curves[0].points.map(\.round) == [1, 2])
        #expect(curves[0].points.map(\.sentiment) == [0.2, -0.1])
        #expect(StudyLogic.run(letter: "A", in: detail)?.id == "a")
        #expect(StudyLogic.run(letter: "B", in: detail) == nil)
    }

    @Test func editableSeedPrefersThePastedDescription() {
        #expect(StudyLogic.editableSeed(["brief.pdf", "notes.txt", "description.md"]) == "description.md")
        #expect(StudyLogic.editableSeed(["brief.pdf", "notice.MD", "notes.txt"]) == "notice.MD")
        #expect(StudyLogic.editableSeed(["brief.pdf"]) == nil)
    }

    @Test func versionRequestReusesTheCastAndSettings() {
        var config = SimConfig(question: "Will they stay?")
        config.numAgents = 12
        config.rounds = 4
        config.seed = 9
        config.agentModel = "small"
        config.events = [GodEvent(round: 2, content: "News")]
        let other = URL(fileURLWithPath: "/tmp/brief.pdf")
        let request = StudyLogic.versionRequest(source: "src", config: config, studyID: "s1", variant: "  Calm ",
                                                editedName: "notice.md", editedText: "New text", otherFiles: [other])
        #expect(request.castFrom == "src" && request.studyID == "s1" && request.variant == "Calm")
        #expect(request.question == "Will they stay?" && request.agents == 12 && request.rounds == 4 && request.seed == 9)
        #expect(request.agentModel == "small" && request.events == config.events)
        #expect(request.files == [other])
        #expect(request.inlineDocuments == [InlineDocument(filename: "notice.md", data: Data("New text".utf8))])
        let unlabeled = StudyLogic.versionRequest(source: "src", config: config, studyID: nil, variant: " ",
                                                  editedName: "d.md", editedText: "x", otherFiles: [])
        #expect(unlabeled.variant == nil && unlabeled.studyID == nil)
    }

    @Test func comparisonCitationsLinkIntoTheRightRun() throws {
        let text = "A#12 pushed back, B#7 agreed; C#3 is not a run, #5 has no letter, and Plan-A#2 stays."
        let linked = StudyLinks.linkify(text, letters: ["A", "B"])
        #expect(linked.contains("[A#12](tellfold://study-post/A/12)"))
        #expect(linked.contains("[B#7](tellfold://study-post/B/7)"))
        #expect(linked.contains("C#3 is not"))
        #expect(linked.contains("#5 has"))
        let url = try #require(StudyLinks.url(letter: "B", postID: 7))
        let target = try #require(StudyLinks.target(url))
        #expect(target.letter == "B" && target.postID == 7)
        #expect(StudyLinks.target(try #require(RunUI.postURL(7))) == nil)
    }

    @Test func runSummaryAndConfigDecodeStudyFields() throws {
        let summary = try TellfoldAPI.decoder.decode(RunSummary.self, from: Data(#"""
        {"id":"r","question":"q","state":"done","stage":"finished","round":2,"total_rounds":2,"agents":4,
         "created_at":"","has_report":true,"study_id":"s1","variant":"Draft B"}
        """#.utf8))
        #expect(summary.studyID == "s1" && summary.variant == "Draft B")
        let config = try TellfoldAPI.decoder.decode(SimConfig.self, from: Data(#"{"question":"q","cast_from":"src"}"#.utf8))
        #expect(config.castFrom == "src" && config.studyID == nil && config.variant == nil)
        let older = try TellfoldAPI.decoder.decode(RunSummary.self, from: Data(#"{"id":"r"}"#.utf8))
        #expect(older.studyID == nil)
    }
}
