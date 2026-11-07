import Foundation
import Testing
@testable import Tellfold

@Suite("Report camps and reasons")
struct CampsTests {
    private func report(_ json: String) throws -> Report {
        try TellfoldAPI.decoder.decode(Report.self, from: Data(json.utf8))
    }

    @Test func decodesCampsAndReasons() throws {
        let r = try report(#"""
        {"title":"Free tier stays","summary":"s","prediction":"p","confidence":"medium",
         "reasons":["Light users stayed @carol_and_duke #5","Grape news hurt trust #20"],
         "camps":[{"name":"Stayed on free","summary":"Free was enough.","members":["a01","a02"],"sentiment":-0.2},
                  {"name":"Turned against Plus","summary":"","members":["a03"],"sentiment":null}]}
        """#)
        #expect(r.reasons.count == 2)
        #expect(r.camps.map(\.name) == ["Stayed on free", "Turned against Plus"])
        #expect(r.camps[0].members == ["a01", "a02"])
        #expect(r.camps[0].sentiment == -0.2)
        #expect(r.camps[1].sentiment == nil)
        #expect(r.camps[1].summary == "")
    }

    @Test func olderReportsDecodeWithoutThem() throws {
        let r = try report(#"{"title":"T","summary":"s","prediction":"p","confidence":"high"}"#)
        #expect(r.reasons.isEmpty)
        #expect(r.camps.isEmpty)
        #expect(r.confidence == .high)
    }

    @Test func malformedCampFieldsAreTolerated() throws {
        let r = try report(#"{"title":"T","summary":"s","prediction":"p","confidence":"low","reasons":"oops","camps":[{"members":"nope"},{"name":"B","members":["a1"]}]}"#)
        #expect(r.reasons.isEmpty)  // wrong type: empty, not a failure
        #expect(r.camps.count == 2)
        #expect(r.camps[0].name == "Camp" && r.camps[0].members.isEmpty)
        #expect(r.camps[1].members == ["a1"])
    }

    @Test func roundTripsThroughJSON() throws {
        let r = Report(title: "T", summary: "s", prediction: "p", confidence: .low, reasons: ["r"],
                       camps: [Camp(name: "A", summary: "x", members: ["a1"], sentiment: 0.4)])
        let back = try TellfoldAPI.decoder.decode(Report.self, from: try TellfoldAPI.encoder.encode(r))
        #expect(back == r)
    }

    // MARK: grouping

    private func persona(_ id: String, org: Bool = false, influence: Double = 0.5) -> Persona {
        Persona(id: id, handle: "h_\(id)", name: "Name \(id)", influence: influence, entityID: org ? "e_\(id)" : nil)
    }

    private var cast: [Persona] { [persona("a1", org: true), persona("a2"), persona("a3"), persona("a4"), persona("a5", org: true)] }

    @Test func oldRunsFallBackToOrganisationsAndPeople() {
        let result = CampGrouping.groups(report: nil, personas: cast)
        #expect(result.isFallback)
        #expect(result.groups.map(\.name) == ["Organisations", "People"])
        #expect(result.groups[0].members == ["a1", "a5"])
        #expect(result.groups[1].members == ["a2", "a3", "a4"])
        #expect(result.groups.allSatisfy { $0.hex == Theme.campSlate })
        // A report without camps falls back the same way.
        let r = Report(title: "T", summary: "", prediction: "", confidence: .low)
        #expect(CampGrouping.groups(report: r, personas: cast).isFallback)
    }

    @Test func fallbackOmitsAnEmptyGroup() {
        let result = CampGrouping.groups(report: nil, personas: [persona("a1"), persona("a2")])
        #expect(result.groups.map(\.name) == ["People"])
        #expect(CampGrouping.groups(report: nil, personas: []).groups.isEmpty)
    }

    @Test func campsGetTheCategoricalColoursInOrderAndOtherIsSlate() {
        let camps = [Camp(name: "One", members: ["a1"]), Camp(name: "Other", members: ["a2"]), Camp(name: "Two", members: ["a3"]),
                     Camp(name: "Three", members: ["a4"]), Camp(name: "Four", members: ["a5"])]
        let r = Report(title: "T", summary: "", prediction: "", confidence: .low, camps: camps)
        let result = CampGrouping.groups(report: r, personas: cast)
        #expect(!result.isFallback)
        #expect(result.groups.map(\.hex) == [0x6366F1, Theme.campSlate, 0x0EA5E9, 0x14B8A6, 0xA855F7])
        #expect(result.groupByAgent["a3"] == result.groups[2].id)
    }

    @Test func everyAgentLandsInExactlyOneCamp() {
        // a3 is listed twice, a9 does not exist, a5 is in no camp.
        let camps = [Camp(name: "A", members: ["a1", "a2", "a3", "a9"]), Camp(name: "B", members: ["a3", "a4"])]
        let r = Report(title: "T", summary: "", prediction: "", confidence: .low, camps: camps)
        let result = CampGrouping.groups(report: r, personas: cast)
        let all = result.groups.flatMap(\.members)
        #expect(all.sorted() == ["a1", "a2", "a3", "a4", "a5"])
        #expect(Set(all).count == all.count)
        #expect(result.groups.last?.name == "Other")  // the stray gets an "Other" group
        #expect(result.groups.last?.members == ["a5"])
    }

    @Test func strayAgentsJoinAnExistingOtherCamp() {
        let camps = [Camp(name: "A", members: ["a1"]), Camp(name: "Other", members: ["a2"])]
        let r = Report(title: "T", summary: "", prediction: "", confidence: .low, camps: camps)
        let result = CampGrouping.groups(report: r, personas: cast)
        #expect(result.groups.count == 2)
        #expect(Set(result.groups[1].members) == ["a2", "a3", "a4", "a5"])
    }

    @Test func campSentimentIsTheMeanOfScoredMembers() {
        let group = CampGroup(id: "g", name: "G", summary: "", members: ["a1", "a2", "a3"], sentiment: -0.5, hex: 0)
        #expect(group.meanSentiment(["a1": -1, "a2": 0, "a9": 1]) == -0.5)
        #expect(group.meanSentiment([:]) == nil)
    }

    // MARK: key voices

    @Test func keyVoicePicksTheMostEngagedOriginalPost() {
        func post(_ id: Int, author: String, likes: Int = 0, repost: Int? = nil, sentiment: Double? = nil) -> PostOut {
            PostOut(id: id, round: 1, authorID: author, content: "post \(id)", parentID: nil, repostOf: repost, likes: likes,
                    authorHandle: "h_\(author)", sentiment: sentiment)
        }
        let posts = [post(1, author: "a1", likes: 1), post(2, author: "a1", likes: 5), post(3, author: "a1", likes: 9, repost: 7),
                     post(4, author: "a2")]
        let r = Report(title: "T", summary: "", prediction: "", confidence: .low, keyVoices: ["a1", "h_a2", "ghost", "a1"])
        let voices = KeyVoices.pick(report: r, personas: cast, posts: posts)
        #expect(voices.map(\.persona.id) == ["a1", "a2"])  // a repost never stands for a voice; duplicates and unknown ids drop
        #expect(voices[0].post.id == 2)
        #expect(KeyVoices.pick(report: nil, personas: cast, posts: posts).isEmpty)
    }

    // MARK: progress

    @Test func progressSpeaksPlainWords() {
        #expect(RunProgress.stageTitle(stage: .rounds, round: 2, totalRounds: 6) == "Simulating round 3 of 6")
        #expect(RunProgress.stageTitle(stage: .rounds, round: 6, totalRounds: 6) == "Simulating round 6 of 6")
        #expect(RunProgress.stageTitle(stage: .reading, round: 0, totalRounds: 6) == "Reading documents")
        #expect(RunProgress.stageTitle(stage: .graph, round: 0, totalRounds: 6) == "Mapping the topic")
        #expect(RunProgress.stageTitle(stage: .personas, round: 0, totalRounds: 6) == "Casting people")
        #expect(RunProgress.stageTitle(stage: .seeding, round: 0, totalRounds: 6) == "Seeding the conversation")
        #expect(RunProgress.stageTitle(stage: .report, round: 6, totalRounds: 6) == "Writing the report")
        let fractions = [RunStage.reading, .graph, .personas, .seeding].map { RunProgress.fraction(stage: $0, round: 0, totalRounds: 6) }
        #expect(fractions == fractions.sorted())
        #expect(RunProgress.fraction(stage: .rounds, round: 3, totalRounds: 6) > RunProgress.fraction(stage: .rounds, round: 1, totalRounds: 6))
        #expect(RunProgress.fraction(stage: .finished, round: 6, totalRounds: 6) == 1)
    }

    @Test func provenanceReadsLikeTheEngineConfig() {
        var config = SimConfig(question: "q")
        #expect(RunProvenance.text(config) == "local / claude code / haiku + sonnet")
        config.plannerRuntime = "ollama"
        #expect(RunProvenance.text(config) == "local / claude code haiku + ollama sonnet")
        #expect(RunProvenance.duration(42) == "42s")
        #expect(RunProvenance.duration(252) == "4m 12s")
        #expect(RunProvenance.duration(3780) == "1h 03m")
    }
}
