import Foundation
import Testing
@testable import Tellfold

/// The user's own audience material: the wording, the preview the engine returns, the estimate, and the grounding
/// people carry into the cast editor and saved audiences.
struct MaterialTests {
    @Test func countsReadInKindOrder() {
        #expect(MaterialText.counts(["survey": 3, "reviews": 14]) == "14 reviews and 3 survey answers")
        #expect(MaterialText.counts(["notes": 1, "emails": 2, "reviews": 1, "comments": 0])
                == "1 review, 2 support emails and 1 meeting note")
        #expect(MaterialText.counts(["survey": 1]) == "1 survey answer")
        #expect(MaterialText.counts([:]) == "no material")
    }

    @Test func hiddenDetailsInWords() {
        #expect(MaterialText.hidden(["names": 4, "emails": 1, "phones": 0, "numbers": 2])
                == "4 names, 1 email address, 2 order or account numbers")
        #expect(MaterialText.hidden(["names": 0]) == nil)
    }

    @Test func hideWordsAndCalls() {
        #expect(MaterialText.hideWords(" Tamsin, Oakfield Primary\n\n,Mr Okafor ") == ["Tamsin", "Oakfield Primary", "Mr Okafor"])
        #expect(MaterialText.calls(items: 0) == 0)
        #expect(MaterialText.calls(items: 40) == 1)
        #expect(MaterialText.calls(items: 41) == 2)
        #expect(MaterialText.calls(items: 300) == 8)
    }

    @Test func kindsAreGuessedFromTheFileType() {
        #expect(MaterialKind.guess(for: URL(filePath: "/x/inbox.eml")) == .emails)
        #expect(MaterialKind.guess(for: URL(filePath: "/x/answers.CSV")) == .survey)
        #expect(MaterialKind.guess(for: URL(filePath: "/x/reviews.txt")) == .reviews)
        #expect(MaterialInput(url: URL(filePath: "/x/notes.md"), kind: .notes).kind == .notes)
    }

    @Test func placeholdersAreMarked() {
        let pieces = RedactedText.pieces("Hi [name], order #[number] [sic] late [phone]")
        #expect(pieces.map(\.0) == ["Hi ", "[name]", ", order #", "[number]", " [sic] late ", "[phone]"])
        #expect(pieces.map(\.1) == [false, true, false, true, false, true])
    }

    /// A reply of `POST /api/material/preview` as the engine writes it.
    static let previewJSON = #"""
    {"summary": {"items": 4, "counts": {"reviews": 3, "survey": 1}, "dropped": 0,
                 "redacted": {"names": 2, "emails": 0, "phones": 1, "addresses": 0, "numbers": 1, "links": 0},
                 "files": ["reviews.txt", "survey.csv"]},
     "files": [{"name": "reviews.txt", "kind": "reviews", "items": 3},
               {"name": "survey.csv", "kind": "survey", "items": 1, "columns": ["Name", "Comment"], "column": "Comment"}],
     "items": [{"id": "m001", "kind": "reviews", "file": "reviews.txt", "text": "Too dear now. — [name]"}],
     "warnings": ["Only 3 reviews and 1 survey answer: a few voices will shape the whole crowd."],
     "thin": true, "estimated_calls": 1}
    """#

    @Test func previewDecodes() throws {
        let preview = try JSONDecoder().decode(MaterialPreview.self, from: Data(Self.previewJSON.utf8))
        #expect(preview.summary.items == 4)
        #expect(preview.file(named: "survey.csv")?.columns == ["Name", "Comment"])
        #expect(preview.file(named: "survey.csv")?.column == "Comment")
        #expect(preview.file(named: "reviews.txt")?.columns.isEmpty == true)
        #expect(preview.thin && preview.estimatedCalls == 1)
        #expect(MaterialStep.summary(preview)
                == "4 items: 3 reviews and 1 survey answer. Hidden: 2 names, 1 phone number, 1 order or account number.")
    }

    @Test func groundingSurvivesDecodingEditingAndSaving() throws {
        let json = #"""
        {"id": "a03", "handle": "fern_mum", "name": "Fern", "bio": "", "background": "", "stance": "",
         "camp": "lean_critical", "source": "material",
         "grounding": {"theme": "price keeps going up", "counts": {"reviews": 14, "survey": 3},
                       "quotes": ["The price doubled. — [name]"]}}
        """#
        var person = try JSONDecoder().decode(Persona.self, from: Data(json.utf8))
        #expect(PersonaSource(person) == .material)
        let grounding = try #require(person.grounding)
        #expect(GroundingText.line(grounding) == "Based on 14 reviews and 3 survey answers · price keeps going up")
        // an edit changes the source, not what the person was built from
        person.source = PersonaSource.edited.rawValue
        let summary = MaterialSummary(items: 17, counts: ["reviews": 14, "survey": 3])
        let audience = SavedAudience(id: "x", name: "Fern people", createdAt: .now, personas: [person], graph: nil,
                                     material: summary)
        let field = try JSONSerialization.jsonObject(with: Data(try audience.formJSON().utf8)) as? [String: Any]
        let people = try #require(field?["personas"] as? [[String: Any]])
        #expect((people[0]["grounding"] as? [String: Any])?["theme"] as? String == "price keeps going up")
        #expect((field?["material"] as? [String: Any])?["items"] as? Int == 17)
        let encoded = try JSONEncoder().encode(audience)
        let back = try JSONDecoder().decode(SavedAudience.self, from: encoded)
        #expect(back.material == summary && back.personas[0].grounding == grounding)
    }

    @Test func aRunWithoutMaterialHasNone() throws {
        let config = try JSONDecoder().decode(SimConfig.self, from: Data(#"{"question": "q"}"#.utf8))
        #expect(config.material == nil)
        let grounded = try JSONDecoder().decode(
            SimConfig.self, from: Data(#"{"question": "q", "material": {"items": 3, "counts": {"notes": 3}}}"#.utf8))
        #expect(grounded.material?.basedOn == "Based on 3 meeting notes")
        #expect(NewSimulationRules.materialAgainNote(try #require(grounded.material))
                == "The last run was built from 3 meeting notes. Add the material again to build the crowd from it.")
    }

    /// The same rows as `reading_material_costs_one_planner_call_per_forty_items` in tellfold-store/src/estimate.rs.
    @Test(arguments: [(1, 1), (40, 1), (41, 2), (300, 8)])
    func readingMaterialAddsPlannerCalls(items: Int, calls: Int) {
        let plain = NewRunRequest.estimatedCallSplit(agents: 20, rounds: 6, batch: 6, chunks: 10)
        let grounded = NewRunRequest.estimatedCallSplit(agents: 20, rounds: 6, batch: 6, chunks: 10, materialItems: items)
        #expect(grounded.agent == plain.agent && grounded.planner == plain.planner + calls)
        let reused = NewRunRequest.estimatedCallSplit(agents: 20, rounds: 6, batch: 6, chunks: 10, reusesCast: true,
                                                      materialItems: items)
        #expect(reused == NewRunRequest.estimatedCallSplit(agents: 20, rounds: 6, batch: 6, chunks: 10, reusesCast: true))
    }

    @Test func theCostLineSaysWhatReadingTheMaterialCosts() {
        let line = NewSimulationRules.costLine(agentCalls: 20, plannerCalls: 6, materialCalls: 2, agentRuntime: "Claude Code",
                                               agentModel: "haiku", plannerRuntime: "Claude Code", plannerModel: "sonnet")
        #expect(line == "About 26 model calls on Claude Code: 20 for the crowd (haiku), 6 for planning (sonnet), "
                + "2 of them to read your material.")
        #expect(NewSimulationRules.disabledReason(question: "q", description: "", fileCount: 1, engineRunning: true,
                                                  materialProblem: true) == "Fix the audience material")
    }

    @MainActor @Test func runsAndPreviewsSendTheMaterialFilesKindsAndHiddenWords() async throws {
        let dir = URL.temporaryDirectory.appending(path: "tellfold-material-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let seed = dir.appending(path: "seed.md")
        try Data("# Seed".utf8).write(to: seed)
        let reviews = dir.appending(path: "reviews.txt")
        try Data("Too dear now.".utf8).write(to: reviews)
        let survey = dir.appending(path: "answers.csv")
        try Data("Name,Comment\nAnn,Fine\n".utf8).write(to: survey)
        let material = [MaterialInput(url: reviews), MaterialInput(url: survey, column: "Comment")]

        let (session, key) = StubURLProtocol.session { _ in .init(body: Data(#"{"id":"r9"}"#.utf8)) }
        var request = NewRunRequest(files: [seed], question: "Will it work?")
        request.material = material
        request.materialHide = ["Tamsin", "Oakfield"]
        _ = try await TellfoldAPI(port: 1, token: "t", session: session).createRun(request)
        let body = String(decoding: try #require(StubURLProtocol.requests(key).first?.httpBody), as: UTF8.self)
        #expect(body.contains("name=\"material\"; filename=\"reviews.txt\""))
        #expect(body.contains("name=\"material\"; filename=\"answers.csv\""))
        #expect(body.contains("name=\"material_spec\"\r\n\r\n[{\"kind\":\"reviews\"},{\"column\":\"Comment\",\"kind\":\"survey\"}]")
                || body.contains("name=\"material_spec\"\r\n\r\n[{\"kind\":\"reviews\"},{\"kind\":\"survey\",\"column\":\"Comment\"}]"))
        #expect(body.contains("name=\"material_hide\"\r\n\r\nTamsin\nOakfield\r\n"))

        let (previewSession, previewKey) = StubURLProtocol.session { _ in .init(body: Data(Self.previewJSON.utf8)) }
        let preview = try await TellfoldAPI(port: 1, token: "t", session: previewSession)
            .previewMaterial(material, hide: [], agents: 12)
        #expect(preview.summary.items == 4)
        let sent = try #require(StubURLProtocol.requests(previewKey).first)
        #expect(sent.url?.path() == "/api/material/preview")
        let previewBody = String(decoding: try #require(sent.httpBody), as: UTF8.self)
        #expect(previewBody.contains("name=\"agents\"\r\n\r\n12\r\n") && !previewBody.contains("material_hide"))

        // a run without material sends none of it
        let (plain, plainKey) = StubURLProtocol.session { _ in .init(body: Data(#"{"id":"r3"}"#.utf8)) }
        _ = try await TellfoldAPI(port: 1, token: "t", session: plain).createRun(NewRunRequest(files: [seed], question: "q"))
        let plainBody = String(decoding: try #require(StubURLProtocol.requests(plainKey).first?.httpBody), as: UTF8.self)
        #expect(!plainBody.contains("material"))
    }
}
