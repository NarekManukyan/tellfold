import Foundation
import Testing
@testable import Shoal

/// Starts the real engine through `EngineController` (offline test model, temporary runs folder), creates a
/// small run over the API and follows its event stream to the end. Opt-in, because it spawns `uv`:
///
///     TEST_RUNNER_SHOAL_REAL_ENGINE=1 TEST_RUNNER_SHOAL_HOME=/private/tmp/shoal-mac-home xcodebuild test ...
///
/// Set SHOAL_HOME so the engine reads and writes a scratch settings file instead of the user's.
@MainActor
@Suite(.enabled(if: ProcessInfo.processInfo.environment["SHOAL_REAL_ENGINE"] == "1"), .serialized)
struct EngineIntegrationTests {
    @Test(.timeLimit(.minutes(3)))
    func engineStartsServesAndStops() async throws {
        let suite = "dev.shoal.mac.tests.\(UUID().uuidString)"
        let defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let prefs = AppPreferences(defaults: defaults)
        let runs = URL.temporaryDirectory.appending(path: "shoal-engine-test-\(UUID().uuidString)", directoryHint: .isDirectory)
        prefs.runsFolder = runs.path
        prefs.offlineTestModel = true
        prefs.logLevel = .info
        if let path = ProcessInfo.processInfo.environment["SHOAL_ENGINE_PATH"] { prefs.enginePath = path }

        let engine = EngineController(prefs: prefs, keychain: Keychain(service: "dev.shoal.mac.tests.none"))
        engine.start()
        defer { engine.shutdownNow() }

        var api: ShoalAPI?
        for _ in 0..<1200 where api == nil {
            api = engine.api
            if case .failed(let message) = engine.state { Issue.record("engine failed: \(message)\n\(engine.logText)"); return }
            if engine.state == .uvMissing { Issue.record("uv missing"); return }
            try await Task.sleep(for: .milliseconds(100))
        }
        let client = try #require(api, "engine never became ready:\n\(engine.logText)")
        #expect(engine.state.isRunning)
        #expect(engine.health?.fakeLLM == true)
        #expect(engine.commandDescription.contains("shoal serve --port 0"))
        #expect(try await client.runs().isEmpty)
        #expect(!(try await client.runtimes()).isEmpty)
        _ = try await client.settings()

        // A wrong token is refused.
        let intruder = ShoalAPI(baseURL: client.baseURL, token: "wrong-token-wrong-token")
        await #expect(throws: ShoalAPIError.self) { try await intruder.runs() }

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
}
