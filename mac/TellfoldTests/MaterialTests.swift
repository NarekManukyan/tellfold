import Foundation
import Testing
import UniformTypeIdentifiers
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
                 "files": ["reviews 1", "survey 1"]},
     "files": [{"name": "reviews.txt", "kind": "reviews", "items": 3, "label": "reviews 1"},
               {"name": "survey.csv", "kind": "survey", "items": 1, "columns": ["Name", "Comment"], "column": "Comment",
                "label": "survey 1"}],
     "items": [{"id": "m001", "kind": "reviews", "file": "reviews 1", "text": "Too dear now. — [name]"}],
     "warnings": ["Only 3 reviews and 1 survey answer: a few voices will shape the whole crowd."],
     "thin": true, "estimated_calls": 1}
    """#

    @Test func previewDecodes() throws {
        let preview = try JSONDecoder().decode(MaterialPreview.self, from: Data(Self.previewJSON.utf8))
        #expect(preview.summary.items == 4)
        #expect(preview.files.map(\.label) == ["reviews 1", "survey 1"])
        #expect(preview.thin && preview.estimatedCalls == 1)
        #expect(MaterialStep.summary(preview)
                == "4 items: 3 reviews and 1 survey answer. Hidden: 2 names, 1 phone number, 1 order or account number.")
    }

    @Test func aReplyWithoutLabelsStillDecodes() throws {
        let json = #"{"summary": {}, "files": [{"name": "a.txt", "kind": "reviews", "items": 2}], "items": []}"#
        let preview = try JSONDecoder().decode(MaterialPreview.self, from: Data(json.utf8))
        #expect(preview.files[0].label == nil && preview.files[0].items == 2)
    }

    /// The server renames files ("answers (1).csv" becomes "answers _1_.csv", duplicates get "-2"), so a report belongs
    /// to a row by position, and only while the preview was read for exactly the files now in the sheet.
    @Test func reportsAreMatchedByPositionNotName() throws {
        var preview = try JSONDecoder().decode(MaterialPreview.self, from: Data(Self.previewJSON.utf8))
        let first = MaterialInput(url: URL(filePath: "/x/Survey (1).CSV"))
        let second = MaterialInput(url: URL(filePath: "/z/Survey (1).CSV"))
        let inputs = [first, second]
        preview.inputs = inputs
        #expect(preview.report(at: 0, of: inputs)?.name == "reviews.txt")
        #expect(preview.report(at: 1, of: inputs)?.columns == ["Name", "Comment"])
        #expect(preview.report(at: 2, of: inputs) == nil)
        // a different kind or column, another order, another count: no report (stale)
        #expect(preview.report(at: 1, of: [first, MaterialInput(url: second.url, kind: .reviews)]) == nil)
        #expect(preview.report(at: 1, of: [first, MaterialInput(url: second.url, column: "Name")]) == nil)
        #expect(preview.report(at: 0, of: [second, first]) == nil)
        #expect(preview.report(at: 0, of: [first]) == nil)
        // the column list stays while only the kind or column changed
        let changed = [first, MaterialInput(url: second.url, column: "Name")]
        #expect(preview.columnsReport(at: 1, of: changed)?.columns == ["Name", "Comment"])
        #expect(preview.columnsReport(at: 0, of: [second, first]) == nil)
        // items show the local name of the file behind their label
        #expect(preview.fileName(forLabel: "survey 1") == "Survey (1).CSV")
        #expect(preview.fileName(forLabel: "other 9") == "other 9")
        preview.inputs = []
        #expect(preview.fileName(forLabel: "survey 1") == "survey 1")
        #expect(preview.report(at: 0, of: []) == nil)
    }

    @Test func fewerItemsThanPeopleIsNotedOnTheClient() {
        let note = "12 items for 20 people: some people will be written from the same quotes."
        #expect(MaterialText.fewerItemsNote(items: 12, agents: 20, thin: false) == note)
        #expect(MaterialText.fewerItemsNote(items: 20, agents: 20, thin: false) == nil)
        #expect(MaterialText.fewerItemsNote(items: 12, agents: 20, thin: true) == nil)
        #expect(MaterialText.fewerItemsNote(items: 0, agents: 20, thin: false) == nil)
    }

    @Test func theSheetNamesTheSameFileTypesTheEngineReads() {
        #expect(MaterialKind.extensionHint == ".txt, .md, .markdown, .csv, .tsv or .eml")
        #expect(MaterialKind.extensions == ["txt", "md", "markdown", "csv", "tsv", "eml"])
        // the file importer lets each of them be picked
        for ext in MaterialKind.orderedExtensions {
            let type = UTType(filenameExtension: ext)
            #expect(type.map { type in MaterialStep.contentTypes.contains { type.conforms(to: $0) } } == true, "\(ext)")
        }
    }

    @Test func redactionIsNotClaimedToBeComplete() {
        #expect(MaterialText.redactedCategories == "common first names, names in greetings, sign-offs and introductions, "
                + "@handles, email addresses, phone numbers, street addresses, order or account numbers and links")
        #expect(MaterialText.namesMayRemain.contains("can remain"))
    }

    @Test func aGroundedRunTellsTheExporterAboutItsQuotes() throws {
        func detail(config: String, personas: String) throws -> RunDetail {
            let json = #"{"id": "r1", "config": \#(config), "live": false, "personas": \#(personas), "stats": [], "events": [], "follows": [], "calls": {}}"#
            return try JSONDecoder().decode(RunDetail.self, from: Data(json.utf8))
        }
        func person(source: String, grounding: String = "") -> String {
            #"{"id": "a01", "handle": "h", "name": "N", "bio": "", "background": "", "stance": "", "source": "\#(source)"\#(grounding)}"#
        }
        let quotes = #", "grounding": {"theme": "t", "counts": {"reviews": 1}, "quotes": ["The price doubled."]}"#
        let noQuotes = #", "grounding": {"theme": "t", "counts": {"reviews": 1}, "quotes": []}"#
        let plain = try detail(config: #"{"question": "q"}"#, personas: "[]")
        #expect(!plain.isGrounded)
        // the bundle decides, not the config or the source label
        #expect(try !detail(config: #"{"question": "q", "material": {"items": 3}}"#, personas: "[]").isGrounded)
        #expect(try !detail(config: #"{"question": "q"}"#, personas: "[\(person(source: "material"))]").isGrounded)
        #expect(try !detail(config: #"{"question": "q"}"#, personas: "[\(person(source: "cast", grounding: noQuotes))]").isGrounded)
        #expect(try detail(config: #"{"question": "q"}"#, personas: "[\(person(source: "material", grounding: quotes))]").isGrounded)
        #expect(try detail(config: #"{"question": "q"}"#, personas: "[\(person(source: "edited", grounding: quotes))]").isGrounded)
        #expect(RunBundle.panelMessage(grounded: false) == RunBundle.basePanelMessage)
        #expect(RunBundle.panelMessage(grounded: true).hasSuffix(RunBundle.groundedNote))
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

    @Test func whatThePlannerReadIsKeptAndPreferredOverWhatWasSent() throws {
        let json = #"{"items": 40, "counts": {"reviews": 30, "survey": 10}, "dropped": 1, "redacted": {"names": 2}, "files": ["a.txt"], "read": {"reviews": 1}}"#
        let summary = try JSONDecoder().decode(MaterialSummary.self, from: Data(json.utf8))
        #expect(summary.read == ["reviews": 1] && summary.counts == ["reviews": 30, "survey": 10] && summary.items == 40)
        #expect(summary.basedOn == "Based on 1 review")
        #expect(NewSimulationRules.materialAgainNote(summary).hasPrefix("The last run was built from 1 review."))
        let back = try JSONDecoder().decode(MaterialSummary.self, from: try JSONEncoder().encode(summary))
        #expect(back == summary && back.read == ["reviews": 1])
        // a summary from before `read`, or one where everything was read, still decodes and falls back to what was sent
        let plain = try JSONDecoder().decode(
            MaterialSummary.self, from: Data(#"{"items": 3, "counts": {"notes": 3}}"#.utf8))
        #expect(plain.read == nil && plain.basedOn == "Based on 3 meeting notes")
        let encoded = try #require(try JSONSerialization.jsonObject(with: JSONEncoder().encode(plain)) as? [String: Any])
        #expect(encoded["read"] == nil)
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
            .previewMaterial(material, hide: [])
        #expect(preview.summary.items == 4)
        let sent = try #require(StubURLProtocol.requests(previewKey).first)
        #expect(sent.url?.path() == "/api/material/preview")
        let previewBody = String(decoding: try #require(sent.httpBody), as: UTF8.self)
        #expect(!previewBody.contains("name=\"agents\"") && !previewBody.contains("material_hide"))

        // a run without material sends none of it
        let (plain, plainKey) = StubURLProtocol.session { _ in .init(body: Data(#"{"id":"r3"}"#.utf8)) }
        _ = try await TellfoldAPI(port: 1, token: "t", session: plain).createRun(NewRunRequest(files: [seed], question: "q"))
        let plainBody = String(decoding: try #require(StubURLProtocol.requests(plainKey).first?.httpBody), as: UTF8.self)
        #expect(!plainBody.contains("material"))
    }
}
