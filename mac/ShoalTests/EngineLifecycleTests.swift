import Foundation
import Synchronization
import Testing
@testable import Shoal

/// Restart backoff, hung-engine kill, store creation without an engine, ordered settings saves and endpoint ids.
@MainActor
struct EngineLifecycleTests {
    // MARK: Restart backoff

    @Test func backoffResetsOnlyAfterAStableRunningEngine() {
        let now = Date.now
        // never reached .running (hung at startup, e.g. 180 s ready timeout): attempts keep counting
        #expect(!EngineController.shouldResetAttempts(readyAt: nil, now: now))
        #expect(!EngineController.shouldResetAttempts(readyAt: now.addingTimeInterval(-30), now: now))
        #expect(EngineController.shouldResetAttempts(readyAt: now.addingTimeInterval(-61), now: now))
    }

    @Test(.timeLimit(.minutes(1))) func forceKillEndsAProcessThatIgnoresSIGTERM() async throws {
        let process = Process()
        process.executableURL = URL(filePath: "/bin/sh")
        process.arguments = ["-c", "trap '' TERM; sleep 30"]
        try process.run()
        try await Task.sleep(for: .milliseconds(200))
        process.terminate()
        try await Task.sleep(for: .milliseconds(200))
        #expect(process.isRunning) // SIGTERM alone does nothing
        EngineController.forceKill(pid: process.processIdentifier)
        for _ in 0..<100 where process.isRunning { try await Task.sleep(for: .milliseconds(50)) }
        #expect(!process.isRunning)
    }

    // MARK: Detail pane without an engine

    @Test func selectingARunWithoutAnEngineBuildsNoStore() throws {
        let suite = "dev.shoal.mac.tests.\(UUID().uuidString)"
        let defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let model = AppModel(prefs: AppPreferences(defaults: defaults))
        model.selectedRunID = "run-1"
        #expect(model.currentStore == nil) // MainView falls through to EngineUnavailableView
        #expect(model.selectedRunID == "run-1")
    }

    // MARK: Settings saves

    @Test func savesRunOneAtATimeAndOnlyTheNewestResultIsApplied() async {
        let calls = Mutex<[Int]>([])
        var applied: [(Int, Bool)] = []
        let saver = LatestValueSaver<Int>(
            save: { value in
                calls.withLock { $0.append(value) }
                try await Task.sleep(for: .milliseconds(value == 1 ? 100 : 10))
                return value
            },
            onSaved: { value, isLatest in applied.append((value, isLatest)) },
            onFailed: { _, _ in Issue.record("no save fails here") })
        saver.submit(1)      // e.g. the runtime picker
        while calls.withLock({ $0.isEmpty }) { await Task.yield() } // the first PUT is in flight
        saver.submit(2)      // its onChange sets the model...
        saver.submit(3)      // ...and coalesces with any further edit
        await saver.waitUntilIdle()
        #expect(calls.withLock { $0 } == [1, 3])
        #expect(applied.map(\.0) == [1, 3])
        #expect(applied.map(\.1) == [false, true]) // the stale response is never shown
        #expect(!saver.isBusy)
    }

    @Test func aFailedLatestSaveIsReportedForRollback() async {
        struct Rejected: Error {}
        var failures: [Bool] = []
        let saver = LatestValueSaver<Int>(
            save: { value in if value < 0 { throw Rejected() }; return value },
            onSaved: { _, _ in },
            onFailed: { _, isLatest in failures.append(isLatest) })
        saver.submit(-1)
        await saver.waitUntilIdle()
        #expect(failures == [true])
    }

    // MARK: Endpoint ids

    @Test func endpointIDsFollowTheEngineRule() {
        for good in ["lmstudio", "a", "x_1-2", "LMStudio", " deepseek ", String(repeating: "a", count: 40)] {
            #expect(OpenAICompatibleEndpoint.isValidID(good), "\(good)")
        }
        for bad in ["", "_lead", "-lead", "has space", "é", "dot.ted", String(repeating: "a", count: 41)] {
            #expect(!OpenAICompatibleEndpoint.isValidID(bad), "\(bad)")
        }
        #expect(OpenAICompatibleEndpoint.normalizedID(" LMStudio ") == "lmstudio")
    }
}
