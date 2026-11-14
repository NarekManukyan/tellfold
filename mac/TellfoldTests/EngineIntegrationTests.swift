import Foundation
import Testing
@testable import Tellfold

/// Starts the real engine through `EngineController` (offline test model, temporary runs folder), creates a
/// small run over the API and follows its event stream to the end. Opt-in, because it spawns the engine:
///
///     TEST_RUNNER_TELLFOLD_REAL_ENGINE=1 TEST_RUNNER_TELLFOLD_HOME=/private/tmp/tellfold-mac-home xcodebuild test ...
///
/// Set TELLFOLD_HOME so the engine reads and writes a scratch settings file instead of the user's. The engine
/// built into the app answers by default. TEST_RUNNER_TELLFOLD_ENGINE_PATH runs another build instead: a `tellfold`
/// binary, the folder holding it, or a repository with a release build (`engine-rs/target/release/tellfold`).
@MainActor
@Suite(.enabled(if: ProcessInfo.processInfo.environment["TELLFOLD_REAL_ENGINE"] == "1"), .serialized)
struct EngineIntegrationTests {
    @Test(.timeLimit(.minutes(3)))
    func engineStartsServesAndStops() async throws {
        let suite = "dev.tellfold.mac.tests.\(UUID().uuidString)"
        let defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let prefs = AppPreferences(defaults: defaults)
        let runs = URL.temporaryDirectory.appending(path: "tellfold-engine-test-\(UUID().uuidString)", directoryHint: .isDirectory)
        prefs.runsFolder = runs.path
        prefs.offlineTestModel = true
        prefs.logLevel = .info
        if let path = ProcessInfo.processInfo.environment["TELLFOLD_ENGINE_PATH"] { prefs.enginePath = path }

        let engine = EngineController(prefs: prefs, keychain: Keychain(service: "dev.tellfold.mac.tests.none"))
        engine.start()
        defer { engine.shutdownNow() }

        var api: TellfoldAPI?
        for _ in 0..<1200 where api == nil {
            api = engine.api
            if case .failed(let message) = engine.state { Issue.record("engine failed: \(message)\n\(engine.logText)"); return }
            try await Task.sleep(for: .milliseconds(100))
        }
        let client = try #require(api, "engine never became ready:\n\(engine.logText)")
        #expect(engine.state.isRunning)
        #expect(engine.health?.fakeLLM == true)
        #expect(engine.commandDescription.contains("tellfold serve --port 0"))
        // Without an engine path, the binary built into the app answers.
        #expect(engine.commandDescription.contains("Resources/engine/tellfold serve") == (prefs.bundledEngineFolder != nil))
        // The bundled examples are listed; the new runs folder adds nothing.
        #expect(try await client.runs().allSatisfy(\.example))
        #expect(!(try await client.runtimes()).isEmpty)
        _ = try await client.settings()

        // A wrong token is refused.
        let intruder = TellfoldAPI(baseURL: client.baseURL, token: "wrong-token-wrong-token")
        await #expect(throws: TellfoldAPIError.self) { try await intruder.runs() }

        // Create a small run and follow it live.
        let seed = runs.appending(path: "seed.md")
        try Data("# Pricing\nThe subscription goes from 5 to 8 dollars next month.".utf8).write(to: seed)
        var request = NewRunRequest(files: [seed], question: "Will subscribers accept the new price?")
        request.agents = 6
        request.rounds = 2
        request.batch = 3
        request.concurrency = 2
        let id = try await client.createRun(request)
        let store = RunStore(runID: id, api: client)
        await store.load()
        if store.isLive {
            store.startStreaming()
            for _ in 0..<600 where store.isLive {
                try await Task.sleep(for: .milliseconds(100))
            }
        }
        #expect(!store.isLive)
        #expect(store.status?.state == .done)
        #expect(store.stats.count == 3)
        #expect(!store.posts.isEmpty)
        #expect(store.report != nil)
        #expect(!store.personas.isEmpty)

        // Stopping closes stdin; the engine exits on its own.
        engine.stop()
        for _ in 0..<100 where engine.state != .stopped {
            try await Task.sleep(for: .milliseconds(100))
        }
        #expect(engine.state == .stopped)
        #expect(engine.api == nil)
        try? FileManager.default.removeItem(at: runs)
    }

    /// Every route the app calls, through the app's own client and models: a run, its reads, chat, ask, the report
    /// rebuild, rewrite, a branch, Check it and a comparison, runtimes and settings. Whichever engine answers (the
    /// bundled binary, or the one an engine path names), the app must decode every answer.
    @Test(.timeLimit(.minutes(5)))
    func everyRouteTheAppUsesAnswers() async throws {
        let suite = "dev.tellfold.mac.tests.\(UUID().uuidString)"
        let defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let prefs = AppPreferences(defaults: defaults)
        let runs = URL.temporaryDirectory.appending(path: "tellfold-engine-test-\(UUID().uuidString)", directoryHint: .isDirectory)
        defer { try? FileManager.default.removeItem(at: runs) }
        prefs.runsFolder = runs.path
        prefs.offlineTestModel = true
        if let path = ProcessInfo.processInfo.environment["TELLFOLD_ENGINE_PATH"] { prefs.enginePath = path }
        let engine = EngineController(prefs: prefs, keychain: Keychain(service: "dev.tellfold.mac.tests.none"))
        engine.start()
        defer { engine.shutdownNow() }
        var api: TellfoldAPI?
        for _ in 0..<1200 where api == nil {
            api = engine.api
            if case .failed(let message) = engine.state { Issue.record("engine failed: \(message)\n\(engine.logText)"); return }
            try await Task.sleep(for: .milliseconds(100))
        }
        let client = try #require(api, "engine never became ready:\n\(engine.logText)")

        func finished(_ id: String) async throws -> RunDetail {
            for _ in 0..<600 {
                let detail = try await client.run(id)
                if !detail.live, let state = detail.status?.state, state != .running, state != .queued { return detail }
                try await Task.sleep(for: .milliseconds(100))
            }
            throw TellfoldAPIError(status: 0, message: "\(id) did not finish")
        }
        func settled(_ studyID: String) async throws -> StudyDetail {
            for _ in 0..<600 {
                let study = try await client.study(studyID)
                if !study.repeating, !study.compareState.running,
                   study.compare != nil || study.agreement != nil || study.compareState.error != nil { return study }
                try await Task.sleep(for: .milliseconds(100))
            }
            throw TellfoldAPIError(status: 0, message: "study \(studyID) did not settle")
        }

        try FileManager.default.createDirectory(at: runs, withIntermediateDirectories: true)
        let seed = runs.appending(path: "seed.md")
        try Data("# Pricing\nThe subscription goes from 5 to 8 dollars next month. Long-time buyers are upset.".utf8).write(to: seed)
        var request = NewRunRequest(files: [seed], question: "Will subscribers accept the new price?")
        request.agents = 4
        request.rounds = 2
        request.batch = 2
        request.events = [GodEvent(round: 2, content: "A rival keeps its one-time price")]
        let id = try await client.createRun(request)
        let detail = try await finished(id)
        #expect(detail.status?.state == .done && detail.report != nil && detail.events.count == 1)
        #expect(try await client.runs().contains { $0.id == id })
        #expect(!(try await client.posts(runID: id)).isEmpty)
        #expect(!(try await client.actions(runID: id, round: 1)).isEmpty)
        let person = try #require(detail.personas.first)
        #expect(try await client.agent(runID: id, agentID: person.id).persona.id == person.id)
        #expect(try await client.agent(runID: id, agentID: "@" + person.handle).persona.id == person.id)
        let seeds = try await client.seeds(runID: id)
        #expect(seeds.map(\.name) == ["seed.md"])
        #expect(try await client.seedData(runID: id, name: "seed.md") == (try Data(contentsOf: seed)))
        #expect(String(decoding: try await client.reportHTML(runID: id), as: UTF8.self).contains("<html"))
        #expect(!(try await client.chat(runID: id, agentID: person.id, message: "Why?",
                                        history: [ChatTurn(question: "Hi", answer: "Hello")])).isEmpty)
        #expect(!(try await client.ask(runID: id, question: "Who changed their mind?")).isEmpty)
        do {
            try await client.injectEvent(runID: id, round: 2, content: "Too late")
            Issue.record("an event was accepted by a finished run")
        } catch let error as TellfoldAPIError {
            #expect(error.isConflict)
        }
        do {
            try await client.stop(runID: id)
            Issue.record("a finished run was stopped")
        } catch let error as TellfoldAPIError {
            #expect(error.isConflict)
        }

        try await client.rebuildReport(runID: id)
        var rebuilding = true
        for _ in 0..<300 where rebuilding {
            rebuilding = try await client.run(id).reportRebuild.running
            if rebuilding { try await Task.sleep(for: .milliseconds(100)) }
        }
        #expect(try await client.run(id).reportRebuild.error == nil)
        let suggestion = try await client.rewrite(runID: id)
        #expect(try await client.savedRewrite(runID: id) == suggestion)

        // A run bundle opens as someone else's read-only run, next to the original. It goes through files both ways.
        let folder = URL.temporaryDirectory.appending(path: "tellfold-bundle-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        let file = folder.appending(path: "run.tellfold")
        try await client.downloadBundle(runID: id, to: file)
        #expect(try Data(contentsOf: file).starts(with: Data("PK".utf8)))
        let imported = try await client.importBundle(fileAt: file)
        #expect(imported != id)
        #expect(try await client.runs().first { $0.id == imported }?.imported == true)
        let copy = try await client.run(imported)
        #expect(copy.imported && !copy.example && copy.status?.state == .done)
        #expect(copy.report == (try await client.run(id)).report)
        #expect(copy.config.seedFiles == ["seeds/seed.md"])
        do {
            try await client.rebuildReport(runID: imported)
            Issue.record("an imported run's report was rebuilt")
        } catch let error as TellfoldAPIError {
            #expect(error.status == 403)
        }
        let junk = folder.appending(path: "junk.tellfold")
        try Data("not a bundle".utf8).write(to: junk)
        await #expect(throws: TellfoldAPIError.self) { try await client.importBundle(fileAt: junk) }

        let branch = try await client.branch(runID: id, round: 1, events: [GodEvent(round: 2, content: "The owner apologises")])
        #expect(try await finished(branch.id).config.branchedFrom == id)
        let compared = try await settled(branch.studyID)
        #expect(compared.compare != nil && compared.runs.count == 2)
        try await client.compareStudy(branch.studyID)
        #expect(try await settled(branch.studyID).compare != nil)

        let check = try await client.checkRun(runID: id, runs: 2)
        let checked = try await settled(check)
        #expect(checked.kind == "repeat" && checked.agreement != nil && checked.runs.count == 2)
        #expect(try await client.studies().contains { $0.id == check })

        #expect(!(try await client.runtimes()).isEmpty)
        #expect(try await client.testRuntime(id: "claude-code").ok)
        let settings = try await client.settings()
        #expect(try await client.updateSettings(settings) == settings)
        engine.stop()
    }
}
