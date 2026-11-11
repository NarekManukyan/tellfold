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

    nonisolated private static func castJSON(_ people: [Persona], waiting: Bool = true) throws -> Data {
        try JSONEncoder().encode(CastResponse(waiting: waiting, personas: people))
    }

    // MARK: models

    @Test func personaCampSourceAndTierDecodeAndOlderPeopleHaveNone() throws {
        let new = try JSONDecoder().decode(Persona.self, from: Data(#"""
        {"id": "a01", "handle": "jo", "name": "Jo", "camp": "strong_support", "source": "group", "tier": "core"}
        """#.utf8))
        #expect((new.camp, new.source, new.tier) == ("strong_support", "group", "core"))
        #expect(CastCamp.label(for: new) == "Strongly for")
        let old = try JSONDecoder().decode(Persona.self, from: Data(#"{"id": "a02", "handle": "x", "name": "X"}"#.utf8))
        #expect(old.camp == nil && old.source == nil && old.tier == nil)
        #expect(CastCamp.label(for: old) == "Not planned")
        // nil fields are left out when the app sends people back
        let sent = String(decoding: try TellfoldAPI.encoder.encode(old), as: UTF8.self)
        #expect(!sent.contains("camp") && !sent.contains("source") && !sent.contains("tier"))
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
        model.update(changed)
        #expect(model.people[0].source == "edited")
        #expect(model.hasLocalEdits)
        #expect(calls.withLock { $0 } == ["GET /api/runs/r1/personas"], "nothing is sent while editing")

        #expect(await model.confirm())
        #expect(calls.withLock { $0 } == ["GET /api/runs/r1/personas", "PUT /api/runs/r1/personas",
                                           "POST /api/runs/r1/personas"])
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

    @Test func audiencesAreSavedRenamedAndDeleted() throws {
        let dir = URL.temporaryDirectory.appending(path: "tellfold-audiences-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: dir) }
        let store = AudienceStore(directory: dir)
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
        #expect(reread.audiences == store.audiences)
        #expect(reread.audience(older.id)?.personas.map(\.camp) == ["lean_support", "lean_support"])

        try store.rename(older.id, to: "Parents of Year 7")
        #expect(store.audience(older.id)?.name == "Parents of Year 7")
        try Data("not json".utf8).write(to: dir.appending(path: "broken.json"))
        store.reload()
        #expect(store.audiences.count == 2, "an unreadable file is skipped")
        try store.delete(newer.id)
        #expect(store.audiences.map(\.id) == [older.id])
        #expect(!FileManager.default.fileExists(atPath: dir.appending(path: "\(newer.id).json").path))
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
