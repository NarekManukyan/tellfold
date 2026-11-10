import Foundation
import Testing
@testable import Tellfold

@Suite("Check it: repeat studies, bands and wording")
struct CheckItTests {
    @Test func decodesTheCheckFixture() throws {
        let detail = try Fixtures.decode(StudyDetail.self, "study_check.json")
        #expect(detail.isRepeat && detail.planned == 3 && !detail.repeating)
        #expect(detail.runs.map(\.variant) == ["Run 1", "Run 2", "Run 3"])
        let a = try #require(detail.agreement)
        #expect(a.runs == 3 && !a.held.isEmpty)
        #expect(a.verdict != .unknown)
        #expect(a.bands.map(\.round) == [0, 1, 2])
        #expect(StudyLogic.phase(of: detail) == .compared)
    }

    @Test func phaseOfARepeatStudy() throws {
        var detail = try Fixtures.decode(StudyDetail.self, "study_check.json")
        detail.repeating = true
        #expect(StudyLogic.phase(of: detail) == .running)  // between runs, nothing is live but the series goes on
        detail.repeating = false
        detail.agreement = nil
        detail.compare = CompareResult(studyID: detail.id, report: CompareReport(headline: "h"))
        #expect(StudyLogic.phase(of: detail) == .readyToCompare)  // a repeat study is done when its agreement is
    }

    @Test func seriesNoteSaysWhatIsLeftOrThatItStopped() throws {
        var detail = try Fixtures.decode(StudyDetail.self, "study_check.json")
        #expect(StudyLogic.seriesNote(detail) == nil)  // all three runs are there
        detail.planned = 5
        detail.repeating = true
        #expect(StudyLogic.seriesNote(detail) == "2 more to come")
        detail.repeating = false  // the engine restarted mid-series: nothing more is coming
        #expect(StudyLogic.seriesNote(detail) == "stopped after 3 of 5 runs")
        detail.agreement = nil
        #expect(StudyLogic.phase(of: detail) == .readyToCompare)  // the runs that exist can still be checked
    }

    @Test func aNewVersionNeverJoinsACheckItStudy() {
        func member(_ id: String, _ variant: String) -> StudyMember { StudyMember(id: id, variant: variant, letter: "A") }
        let check = StudySummary(id: "chk", kind: "repeat", runs: [member("src", "Run 1"), member("r2", "Run 2")])
        let drafts = StudySummary(id: "s1", runs: [member("src", "Draft A"), member("v2", "Draft B")])
        func run(_ id: String, _ studyID: String?) -> RunSummary {
            var r = RunSummary(id: id, question: "Q?", state: .done, stage: .finished, round: 2, totalRounds: 2,
                               agents: 4, createdAt: "", hasReport: true)
            r.studyID = studyID
            return r
        }
        let runs = [run("src", nil), run("r2", "chk"), run("v2", "s1")]
        #expect(StudyLogic.study(of: "r2", runs: runs, studies: [check, drafts]) == nil)  // starts a study of its own
        #expect(StudyLogic.nextVariant(for: "r2", runs: runs, studies: [check, drafts]) == "Draft B")
        #expect(StudyLogic.study(of: "src", runs: runs, studies: [check, drafts]) == "s1")  // skips the check
        #expect(StudyLogic.study(of: "src", runs: runs, studies: [check]) == nil)
    }

    @Test func unknownVerdictDecodes() throws {
        let a = try TellfoldAPI.decoder.decode(Agreement.self, from: Data(#"{"verdict":"wobbly","runs":2}"#.utf8))
        #expect(a.verdict == .unknown && a.runs == 2 && a.held.isEmpty)
    }

    @Test func bandsComeFromTheAgreementOrTheRunsSoFar() {
        func run(_ id: String, _ scores: [Double?]) -> StudyRun {
            StudyRun(id: id, variant: id, letter: id,
                     digest: RunDigest(letter: id, runID: id, variant: id,
                                       rounds: scores.enumerated().map { RoundPoint(round: $0.offset, sentiment: $0.element) }))
        }
        var detail = StudyDetail(id: "s", kind: "repeat", runs: [run("A", [nil, 0.2, -0.1]), run("B", [nil, 0.5, 0.3])])
        let live = StudyLogic.bands(detail)
        #expect(live.map(\.round) == [1, 2])
        #expect(live.map(\.low) == [0.2, -0.1] && live.map(\.high) == [0.5, 0.3])
        detail.agreement = Agreement(runs: 2, bands: [RoundBand(round: 0), RoundBand(round: 1, low: 0.1, high: 0.4, runs: 2)])
        let written = StudyLogic.bands(detail)
        #expect(written.count == 1 && written[0].round == 1 && written[0].low == 0.1 && written[0].high == 0.4)
    }

    @Test func wording() {
        #expect(StudyLogic.verdictTitle(.consistent, runs: 3) == "It held up in all 3 runs")
        #expect(StudyLogic.verdictTitle(.unstable, runs: 3).contains("disagree"))
        let warning = StudyLogic.checkWarning(runs: 3, callsPerRun: 12)
        #expect(warning.contains("2 more times") && warning.contains("3×") && warning.contains("about 24 more calls"))
        #expect(StudyLogic.checkWarning(runs: 2, callsPerRun: 5).contains("1 more time with"))
        let range = StudyLogic.finalRange(Agreement(finalLow: -0.1, finalHigh: 0.25))
        #expect(range == "\(RunUI.sentimentText(-0.1)) to \(RunUI.sentimentText(0.25)) (spread 0.35)")
        #expect(StudyLogic.finalRange(Agreement()) == nil)
    }

    @Test func sidebarTitleOfACheck() {
        let study = StudySummary(id: "s", kind: "repeat", runs: [StudyMember(id: "a", variant: "Run 1", letter: "A")],
                                 planned: 3)
        let run = RunSummary(id: "a", question: "Q?", state: .done, stage: .finished, round: 2, totalRounds: 2, agents: 4,
                             createdAt: "", hasReport: true)
        let rows = StudyLogic.rows(studies: [study], runs: [run])
        #expect(rows.first?.title == "Check it: 3 runs")
    }
}
