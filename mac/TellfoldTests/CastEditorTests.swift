import Foundation
import Synchronization
import Testing
@testable import Tellfold

/// The cast editor, saved audiences and the `personas_ready` pause.
@MainActor
struct CastEditorTests {
    nonisolated private static func person(_ id: String, camp: String? = "lean_support", stance: String = "Likes the plan.") -> Persona {
        var p = Persona(id: id, handle: "user_\(id)", name: "User \(id)")
        p.bio = "Lives nearby."
        p.stance = stance
        p.camp = camp
        p.source = "cast"
        return p
    }

    /// Ids a01..aNN in order, as the engine numbers a cast it takes back.
    nonisolated private static func renumbered(_ cast: [Persona]) -> [Persona] {
        cast.enumerated().map { i, p in
            var p = p
            p.id = String(format: "a%02d", i + 1)
            return p
        }
    }

    nonisolated private static func castJSON(_ people: [Persona], waiting: Bool = true) throws -> Data {
        try JSONEncoder().encode(CastResponse(waiting: waiting, personas: people))
    }

    // MARK: models

    @Test func personaCampAndSourceDecodeAndOlderPeopleHaveNone() throws {
        let new = try JSONDecoder().decode(Persona.self, from: Data(#"""
        {"id": "a01", "handle": "jo", "name": "Jo", "camp": "strong_support", "source": "group"}
        """#.utf8))
        #expect((new.camp, new.source) == ("strong_support", "group"))
        #expect(PersonaSource(new) == .group)
        #expect(CastCamp.label(for: new) == "Strongly for")
        let old = try JSONDecoder().decode(Persona.self, from: Data(#"{"id": "a02", "handle": "x", "name": "X"}"#.utf8))
        #expect(old.camp == nil && old.source == nil && PersonaSource(old) == nil)
        #expect(CastCamp.label(for: old) == "Not planned")
        // nil fields are left out when the app sends people back
        let sent = String(decoding: try TellfoldAPI.encoder.encode(old), as: UTF8.self)
        #expect(!sent.contains("camp") && !sent.contains("source"))
    }

    @Test func aWaitingRunSummaryAwaitsItsCast() throws {
        let json = #"""
        {"id": "r1", "question": "q", "state": "running", "stage": "personas", "round": 0, "total_rounds": 3,
         "agents": 6, "created_at": "", "has_report": false, "pause": "personas_ready"}
        """#
        var run = try JSONDecoder().decode(RunSummary.self, from: Data(json.utf8))
        #expect(run.awaitsCast)
        run.state = .stopped
        #expect(!run.awaitsCast)
        let older = try JSONDecoder().decode(RunSummary.self, from: Data(json.replacingOccurrences(
            of: #", "pause": "personas_ready""#, with: "").utf8))
        #expect(older.pause == nil && !older.awaitsCast)
    }

    @Test func pausedStatusAndConfigFlagsDecode() throws {
        let status = try JSONDecoder().decode(RunStatus.self, from: Data(#"""
        {"state": "running", "stage": "personas", "round": 0, "total_rounds": 3, "detail": "6 people ready",
         "started_at": "2026-11-11T00:00:00Z", "updated_at": "2026-11-11T00:00:00Z", "pause": "personas_ready"}
        """#.utf8))
        #expect(status.awaitsCast)
        var finished = status
        finished.state = .stopped
        #expect(!finished.awaitsCast)
        let config = try JSONDecoder().decode(SimConfig.self, from: Data(#"""
        {"question": "q", "review_cast": true, "fair_turns": true, "audience": "Bus parents"}
        """#.utf8))
        #expect(config.reviewCast && config.fairTurns && config.audience == "Bus parents")
        let older = try JSONDecoder().decode(SimConfig.self, from: Data(#"{"question": "q"}"#.utf8))
        #expect(!older.reviewCast && !older.fairTurns && older.audience == nil)
    }

    @Test func personasReadyEventDecodes() throws {
        let event = ServerEvent.decode(name: "personas_ready",
                                       data: #"{"personas": [{"id": "a01", "handle": "jo", "name": "Jo"}]}"#)
        guard case .personasReady(let people) = event else {
            Issue.record("expected personas_ready, got \(event)")
            return
        }
        #expect(people.map(\.id) == ["a01"])
        #expect(event.name == "personas_ready" && !event.isTerminal)
    }

    @Test func savedAudienceSkipsCastingInTheEstimate() {
        let fresh = NewRunRequest.estimatedCallSplit(agents: 20, rounds: 6, batch: 6, chunks: 8)
        let reused = NewRunRequest.estimatedCallSplit(agents: 20, rounds: 6, batch: 6, chunks: 8, reusesCast: true)
        #expect(reused.agent == fresh.agent)
        #expect(reused.planner == 1, "only the report")
        #expect(fresh.planner > reused.planner)
    }

    // MARK: the pause in the run store

    @Test func theEditorOpensAtThePauseAndClosesWhenTheRunGoesOn() async throws {
        let detail = try Fixtures.decode(RunDetail.self, "run_detail_live.json")
        let store = RunStore(detail: detail, posts: [], actions: [])
        #expect(!store.showsCastEditor)
        var paused = RunStatus(state: .running, stage: .personas, totalRounds: 3, detail: "ready",
                               startedAt: "", updatedAt: "", pause: RunStatus.personasReady)
        await store.handle(.status(paused))
        #expect(store.showsCastEditor && store.awaitsCast)
        // "Later" closes it; another status of the same pause does not reopen it
        store.showsCastEditor = false
        paused.updatedAt = "later"
        await store.handle(.status(paused))
        #expect(!store.showsCastEditor && store.awaitsCast)
        // the personas_ready event a reconnecting client gets brings it back
        await store.handle(.personasReady([Self.person("a01")]))
        #expect(store.showsCastEditor)
        var going = paused
        going.pause = nil
        going.stage = .seeding
        await store.handle(.status(going))
        #expect(!store.showsCastEditor && !store.awaitsCast)
    }

    // MARK: the editor

    @Test func editsStayLocalUntilTheRunStarts() async throws {
        let people = [Self.person("a01"), Self.person("a02", camp: "strong_critical"), Self.person("a03")]
        let calls = Mutex<[String]>([])
        let (session, key) = StubURLProtocol.session { request in
            let line = "\(request.httpMethod ?? "") \(request.url?.path ?? "")"
            calls.withLock { $0.append(line) }
            switch line {
            case "GET /api/runs/r1/personas":
                return .init(body: try! Self.castJSON(people))
            case "PUT /api/runs/r1/personas":
                let sent = try! JSONDecoder().decode(CastResponse.self, from: Data(
                    #"{"waiting": true, "#.utf8 + request.httpBody!.dropFirst()))
                // the engine renumbers
                let tidy = sent.personas.enumerated().map { i, p in
                    var p = p
                    p.id = String(format: "a%02d", i + 1)
                    return p
                }
                return .init(body: try! Self.castJSON(tidy))
            default:
                return .init(body: Data(#"{"ok": true}"#.utf8))
            }
        }
        let model = CastEditorModel(runID: "r1", api: TellfoldAPI(port: 1, token: "t", session: session))
        await model.load()
        #expect(model.people.map(\.id) == ["a01", "a02", "a03"])
        #expect(model.campCounts.map(\.camp) == [.leanSupport, .strongCritical])

        model.remove("a01")
        var changed = model.people[0]
        changed.stance = "Changed my mind."
        #expect(model.update(changed, from: model.people[0]))
        #expect(model.people[0].source == PersonaSource.edited.rawValue)
        #expect(model.hasLocalEdits)
        #expect(calls.withLock { $0 } == ["GET /api/runs/r1/personas"], "nothing is sent while editing")

        #expect(await model.confirm())
        #expect(calls.withLock { $0 } == ["GET /api/runs/r1/personas", "PUT /api/runs/r1/personas",
                                           "POST /api/runs/r1/personas/confirm"])
        #expect(model.people.map(\.id) == ["a01", "a02"])
        #expect(model.people[0].stance == "Changed my mind.")
        #expect(!model.hasLocalEdits)
        let put = try #require(StubURLProtocol.requests(key).first { $0.httpMethod == "PUT" })
        let body = String(decoding: try #require(put.httpBody), as: UTF8.self)
        #expect(body.hasPrefix(#"{"personas":["#))
    }

    @Test func aGroupIsAddedAfterTheLocalEditsAreSent() async throws {
        let people = [Self.person("a01"), Self.person("a02")]
        let calls = Mutex<[String]>([])
        let (session, key) = StubURLProtocol.session { request in
            let line = "\(request.httpMethod ?? "") \(request.url?.path ?? "")"
            calls.withLock { $0.append(line) }
            if line == "POST /api/runs/r1/personas/group" {
                var added = people
                for i in 3...5 { added.append(Self.person(String(format: "a%02d", i), camp: "undecided")) }
                return .init(body: try! Self.castJSON(added))
            }
            return .init(body: try! Self.castJSON(people))
        }
        let model = CastEditorModel(runID: "r1", api: TellfoldAPI(port: 1, token: "t", session: session), people: people)
        model.remove("a02")
        #expect(!(await model.addGroup(description: "   ", count: 3)), "an empty description sends nothing")
        #expect(await model.addGroup(description: "three parents of Year 7 children who take the bus", count: 50))
        #expect(calls.withLock { $0 } == ["PUT /api/runs/r1/personas", "POST /api/runs/r1/personas/group"])
        #expect(model.people.count == 5)
        let group = try #require(StubURLProtocol.requests(key).last)
        let body = try JSONSerialization.jsonObject(with: try #require(group.httpBody)) as? [String: Any]
        #expect(body?["description"] as? String == "three parents of Year 7 children who take the bus")
        #expect(body?["count"] as? Int == CastEditorModel.maxGroup, "clamped to what the engine accepts")
    }

    /// Remove a01, open a03's editor, add a group: the engine renumbers, so a03 is now someone else, and saving the
    /// open edit must not overwrite them.
    @Test func anOpenEditNeverLandsOnARenumberedPerson() async throws {
        let people = (1...4).map { Self.person(String(format: "a%02d", $0), stance: "Stance \($0)") }
        let (session, _) = StubURLProtocol.session { request in
            let line = "\(request.httpMethod ?? "") \(request.url?.path ?? "")"
            if line == "PUT /api/runs/r1/personas" {
                let sent = try! JSONDecoder().decode(CastResponse.self, from: Data(
                    #"{"waiting": true, "#.utf8 + request.httpBody!.dropFirst()))
                return .init(body: try! Self.castJSON(Self.renumbered(sent.personas)))
            }
            // the group comes back after the renumbered three
            var cast = Self.renumbered(Array(people.dropFirst()))
            cast.append(Self.person("a04", camp: "undecided", stance: "New member"))
            return .init(body: try! Self.castJSON(cast))
        }
        let model = CastEditorModel(runID: "r1", api: TellfoldAPI(port: 1, token: "t", session: session), people: people)
        model.remove("a01")
        model.toggleEditing("a03")
        let original = try #require(model.people.first { $0.id == "a03" })
        var draft = original
        draft.stance = "An edit of the old a03"
        #expect(model.editingID == "a03")

        #expect(await model.addGroup(description: "one new member", count: 1))
        #expect(model.editingID == nil, "the cast came back renumbered, so no editor stays open")
        let renumbered = try #require(model.people.first { $0.id == "a03" })
        #expect(renumbered.stance == "Stance 4", "a03 is the old a04 now")
        #expect(!model.update(draft, from: original), "the edit was started on someone else")
        #expect(model.people.first { $0.id == "a03" } == renumbered)
        #expect(!model.people.contains { $0.stance == "An edit of the old a03" })
    }

    /// What the stub engine of `fakeEngine` saw and holds.
    final class EngineState: Sendable {
        let calls = Mutex<[String]>([])
        let held: Mutex<[Persona]>
        init(_ start: [Persona]) { held = Mutex(start) }
    }

    /// A stub engine that holds the waiting cast: PUT replaces it (renumbered), GET serves it, a group adds one person.
    nonisolated private static func fakeEngine(_ start: [Persona]) -> (URLSession, String, EngineState) {
        let state = EngineState(start)
        let (session, key) = StubURLProtocol.session { request in
            let line = "\(request.httpMethod ?? "") \(request.url?.path ?? "")"
            state.calls.withLock { $0.append(line) }
            switch line {
            case "PUT /api/runs/r1/personas":
                let sent = try! JSONDecoder().decode(CastResponse.self, from: Data(
                    #"{"waiting": true, "#.utf8 + request.httpBody!.dropFirst()))
                state.held.withLock { $0 = renumbered(sent.personas) }
            case "POST /api/runs/r1/personas/group":
                state.held.withLock { $0 = renumbered($0 + [person("g1", camp: "undecided", stance: "New member")]) }
            default:
                break
            }
            return .init(body: try! castJSON(state.held.withLock { $0 }))
        }
        return (session, key, state)
    }

    @Test func laterSendsTheEditsSoReopeningShowsThem() async throws {
        let people = [Self.person("a01"), Self.person("a02")]
        let (session, key, engine) = Self.fakeEngine(people)
        let api = TellfoldAPI(port: 1, token: "t", session: session)
        let model = CastEditorModel(runID: "r1", api: api, people: people)
        #expect(await model.saveForLater(), "nothing to send is fine")
        #expect(engine.calls.withLock { $0 }.isEmpty)
        model.remove("a01")
        #expect(await model.saveForLater())
        #expect(engine.calls.withLock { $0 } == ["PUT /api/runs/r1/personas"])
        #expect(!model.hasLocalEdits)
        let put = try #require(StubURLProtocol.requests(key).first { $0.httpMethod == "PUT" })
        let sent = try JSONDecoder().decode(CastResponse.self, from: Data(
            #"{"waiting": true, "#.utf8 + (try #require(put.httpBody)).dropFirst()))
        #expect(sent.personas.map(\.handle) == ["user_a02"])
        // a new editor (the sheet opened again) loads what the engine now holds
        let reopened = CastEditorModel(runID: "r1", api: api)
        await reopened.load()
        #expect(reopened.people.map(\.handle) == ["user_a02"])
        #expect(reopened.people.map(\.id) == ["a01"], "renumbered by the engine")
    }

    @Test func laterWaitsForAnOpenRowEditor() async {
        let people = [Self.person("a01")]
        let (session, _, engine) = Self.fakeEngine(people)
        let model = CastEditorModel(runID: "r1", api: TellfoldAPI(port: 1, token: "t", session: session), people: people)
        model.toggleEditing("a01")
        #expect(!model.canClose)
        #expect(!(await model.saveForLater()), "the open draft would be lost")
        #expect(engine.calls.withLock { $0 }.isEmpty)
        model.toggleEditing("a01")
        #expect(model.canClose)
    }

    /// Removing everyone must not trap the user: the engine keeps an empty cast while the run waits, so Later closes
    /// and a group can be added; only starting the run needs people.
    @Test func anEmptiedCastCanStillCloseOrGrow() async {
        let people = [Self.person("a01"), Self.person("a02")]
        let (session, _, engine) = Self.fakeEngine(people)
        let model = CastEditorModel(runID: "r1", api: TellfoldAPI(port: 1, token: "t", session: session), people: people)
        model.remove("a01")
        model.remove("a02")
        #expect(!model.canContinue)
        #expect(await model.saveForLater())
        #expect(engine.held.withLock { $0 }.isEmpty)
        #expect(await model.addGroup(description: "one new member", count: 1))
        #expect(engine.calls.withLock { $0 } == ["PUT /api/runs/r1/personas", "POST /api/runs/r1/personas/group"])
        #expect(model.people.map(\.stance) == ["New member"])
        #expect(model.canContinue)
    }

    @Test func aFailedGroupKeepsTheCastAndSaysWhy() async throws {
        let (session, _) = StubURLProtocol.session { _ in
            .init(status: 502, body: Data(#"{"detail": "the model call failed: rate limited"}"#.utf8))
        }
        let people = [Self.person("a01")]
        let model = CastEditorModel(runID: "r1", api: TellfoldAPI(port: 1, token: "t", session: session), people: people)
        #expect(!(await model.addGroup(description: "two bus drivers", count: 2)))
        #expect(model.people == people)
        #expect(model.errorMessage?.contains("rate limited") == true)
    }

    @Test func createRunSendsTheReviewFlagFairTurnsAndAudience() async throws {
        let dir = URL.temporaryDirectory.appending(path: "tellfold-tests-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let seed = dir.appending(path: "seed.md")
        try Data("# Seed".utf8).write(to: seed)
        let (session, key) = StubURLProtocol.session { _ in .init(body: Data(#"{"id":"r2"}"#.utf8)) }
        let api = TellfoldAPI(port: 1, token: "t", session: session)
        var request = NewRunRequest(files: [seed], question: "Will it work?")
        request.reviewCast = true
        request.audience = SavedAudience(id: "x", name: "Bus parents", createdAt: .now, personas: [Self.person("a01")],
                                         graph: KnowledgeGraph())
        _ = try await api.createRun(request)
        let body = String(decoding: try #require(StubURLProtocol.requests(key).first?.httpBody), as: UTF8.self)
        #expect(body.contains("name=\"review_cast\"\r\n\r\ntrue\r\n"))
        #expect(body.contains("name=\"fair_turns\"\r\n\r\ntrue\r\n"))
        #expect(body.contains("name=\"audience\"\r\n\r\n{\"graph\":{\"entities\":[],\"relations\":[]},\"name\":\"Bus parents\",\"personas\":[{"))

        let (plain, plainKey) = StubURLProtocol.session { _ in .init(body: Data(#"{"id":"r3"}"#.utf8)) }
        _ = try await TellfoldAPI(port: 1, token: "t", session: plain).createRun(NewRunRequest(files: [seed], question: "q"))
        let plainBody = String(decoding: try #require(StubURLProtocol.requests(plainKey).first?.httpBody), as: UTF8.self)
        #expect(!plainBody.contains("review_cast") && !plainBody.contains("name=\"audience\""))
    }

    // MARK: saved audiences

    @Test func audiencesAreSavedRenamedAndDeleted() async throws {
        let dir = URL.temporaryDirectory.appending(path: "tellfold-audiences-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: dir) }
        let store = AudienceStore(directory: dir)
        await store.reload()
        #expect(store.audiences.isEmpty)
        let older = try store.save(name: "  Bus \n parents ", question: "Will the bus change work?",
                                   personas: [Self.person("a01"), Self.person("a02")], graph: KnowledgeGraph(),
                                   now: Date(timeIntervalSince1970: 1_000))
        #expect(older.name == "Bus parents")
        let newer = try store.save(name: "", question: "", personas: [Self.person("a01")], graph: nil,
                                   now: Date(timeIntervalSince1970: 2_000))
        #expect(newer.name == "Saved audience")
        #expect(store.audiences.map(\.id) == [newer.id, older.id], "newest first")

        // a fresh store reads the same files back
        let reread = AudienceStore(directory: dir)
        await reread.reload()
        #expect(reread.audiences == store.audiences)
        #expect(reread.audience(older.id)?.personas.map(\.camp) == ["lean_support", "lean_support"])

        try store.rename(older.id, to: "Parents of Year 7")
        #expect(store.audience(older.id)?.name == "Parents of Year 7")
        try Data("not json".utf8).write(to: dir.appending(path: "broken.json"))
        await store.reload()
        #expect(store.audiences.count == 2, "an unreadable file is skipped")
        #expect(store.audience(older.id)?.name == "Parents of Year 7", "the rename was written to its file")
        try store.delete(newer.id)
        #expect(store.audiences.map(\.id) == [older.id])
        #expect(!FileManager.default.fileExists(atPath: dir.appending(path: "\(newer.id).json").path))
    }

    /// Rename and delete act on the file an audience was read from, whatever id the file claims.
    @Test func audienceFilesAreFoundByWhereTheyWereRead() async throws {
        let dir = URL.temporaryDirectory.appending(path: "tellfold-audiences-\(UUID().uuidString)")
        let outside = URL.temporaryDirectory.appending(path: "tellfold-outside-\(UUID().uuidString).json")
        defer {
            try? FileManager.default.removeItem(at: dir)
            try? FileManager.default.removeItem(at: outside)
        }
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        try Data("keep me".utf8).write(to: outside)
        let sneaky = "../\(outside.deletingPathExtension().lastPathComponent)"
        let file = dir.appending(path: "copied.json")
        try Data(#"""
        {"id": "\#(sneaky)", "name": "Copied", "created_at": "2026-11-11T00:00:00Z",
         "personas": [{"id": "a01", "handle": "jo", "name": "Jo"}]}
        """#.utf8).write(to: file)
        let store = AudienceStore(directory: dir)
        await store.reload()
        let audience = try #require(store.audiences.first)
        #expect(audience.question == "", "a file written without a question reads as none")
        try store.rename(audience.id, to: "Renamed")
        #expect(String(decoding: try Data(contentsOf: file), as: UTF8.self).contains("Renamed"))
        #expect(String(decoding: try Data(contentsOf: outside), as: UTF8.self) == "keep me")
        try store.delete(audience.id)
        #expect(!FileManager.default.fileExists(atPath: file.path))
        #expect(FileManager.default.fileExists(atPath: outside.path))
        #expect(store.audiences.isEmpty)
    }

    /// A file copied in Finder keeps the id inside it: each copy is still its own audience, and deleting one leaves
    /// the other.
    @Test func aDuplicatedAudienceFileIsItsOwnAudience() async throws {
        let dir = URL.temporaryDirectory.appending(path: "tellfold-audiences-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: dir) }
        let store = AudienceStore(directory: dir)
        let saved = try store.save(name: "Original", question: "", personas: [Self.person("a01")], graph: nil)
        let copy = dir.appending(path: "\(saved.id) copy.json")
        try FileManager.default.copyItem(at: dir.appending(path: "\(saved.id).json"), to: copy)
        await store.reload()
        #expect(store.audiences.count == 2)
        #expect(Set(store.audiences.map(\.id)).count == 2, "list ids stay unique")
        #expect(store.audience(saved.id) != nil, "the first file keeps its id")
        let other = try #require(store.audiences.first { $0.id != saved.id })
        try store.delete(other.id)
        #expect(store.audiences.map(\.id) == [saved.id])
        #expect(FileManager.default.fileExists(atPath: dir.appending(path: "\(saved.id).json").path))
        #expect(!FileManager.default.fileExists(atPath: copy.path))
    }

    @Test func aSaveDuringAReloadIsKept() async throws {
        let dir = URL.temporaryDirectory.appending(path: "tellfold-audiences-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: dir) }
        let store = AudienceStore(directory: dir)
        let reading = Task { await store.reload() }
        await Task.yield()
        let saved = try store.save(name: "Mid-read", question: "", personas: [Self.person("a01")], graph: nil)
        await reading.value
        #expect(store.audiences.map(\.id) == [saved.id])
    }

    @Test func savingFromTheEditorKeepsTheEdits() throws {
        let dir = URL.temporaryDirectory.appending(path: "tellfold-audiences-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: dir) }
        let store = AudienceStore(directory: dir)
        let model = CastEditorModel(runID: "r1", api: nil, people: [Self.person("a01"), Self.person("a02")])
        model.remove("a01")
        let saved = try #require(model.saveAudience(name: "Just one", question: "q", graph: nil, to: store))
        #expect(saved.personas.map(\.id) == ["a02"])
        #expect(store.audiences.map(\.name) == ["Just one"])
        #expect(NewSimulationRules.audienceNote(saved).hasPrefix("1 person from \u{201C}Just one\u{201D}."))
    }
}
