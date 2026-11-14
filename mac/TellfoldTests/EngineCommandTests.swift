import Foundation
import Testing
@testable import Tellfold

struct EngineCommandTests {
    @Test(arguments: [
        (#"{"port": 54321}"#, 54321),
        (#"{"port":8796}"#, 8796),
        ("  {\"port\": 1}\n", 1),
        (#"{"port": 65535, "pid": 12}"#, 65535),
    ])
    func parsesPortLine(line: String, port: Int) {
        #expect(EngineCommand.parsePort(fromLine: line) == port)
    }

    @Test(arguments: [
        "", "port 8000", #"{"port": 0}"#, #"{"port": 70000}"#, #"{"port": "8000"}"#, #"{"port": 80.5}"#,
        #"{"host": "127.0.0.1"}"#, "INFO:     Uvicorn running on http://127.0.0.1:8000", "[1, 2]", #"{"port": -1}"#,
    ])
    func rejectsOtherLines(line: String) {
        #expect(EngineCommand.parsePort(fromLine: line) == nil)
    }

    @Test func commandLine() {
        let args = EngineCommand.arguments(runsFolder: "/runs dir", logLevel: "INFO")
        #expect(args == ["serve", "--port", "0", "--runs", "/runs dir", "--log-level", "info"])
        let shown = EngineCommand.displayCommand(executable: "/eng/tellfold", arguments: args + ["--token", "tok"], token: "tok")
        #expect(!shown.contains("tok "))
        #expect(shown.hasSuffix("--token •••"))
        #expect(shown.contains("'/runs dir'"))
        let binary = EngineCommand.bundledBinary(in: URL(filePath: "/App.app/Contents/Resources/engine", directoryHint: .isDirectory))
        #expect(binary.path == "/App.app/Contents/Resources/engine/tellfold")
    }

    @Test func examplesFolderIsPassedWhenGiven() {
        let args = EngineCommand.arguments(runsFolder: "/runs", examplesFolder: "/App.app/Contents/Resources/Examples",
                                           logLevel: "warning")
        #expect(args == ["serve", "--port", "0", "--runs", "/runs",
                         "--examples", "/App.app/Contents/Resources/Examples", "--log-level", "warning"])
        #expect(!EngineCommand.arguments(runsFolder: "/runs", logLevel: "info").contains("--examples"))
    }

    @Test func appBundleShipsTheExamples() throws {
        let examples = try #require(EngineCommand.bundledExamples())
        let names = try FileManager.default.contentsOfDirectory(atPath: examples.path)
        #expect(names.contains("corner-cafe"))
        #expect(FileManager.default.fileExists(atPath: examples.appending(path: "corner-cafe/world.db").path))
    }

    @Test func appBundleShipsTheEngine() throws {
        let engine = try #require(AppPreferences.bundledEngine, "no Resources/engine/tellfold in the app")
        #expect(FileManager.default.isExecutableFile(atPath: EngineCommand.bundledBinary(in: engine).path))
        #expect(try FileManager.default.contentsOfDirectory(atPath: engine.path) == ["tellfold"])
    }

    @Test func tokensAreRandomHex() {
        let a = EngineCommand.makeToken(), b = EngineCommand.makeToken()
        #expect(a.count == 64)
        #expect(a != b)
        #expect(a.allSatisfy { $0.isHexDigit })
    }

    @Test func engineFolderOrRepositoryFindsTheBinary() {
        let present: Set<String> = ["/build/tellfold", "/repo/engine-rs/target/release/tellfold"]
        #expect(EngineCommand.engineBinary(at: "/build") { present.contains($0) }?.path == "/build/tellfold")
        #expect(EngineCommand.engineBinary(at: " /repo ") { present.contains($0) }?.path
                == "/repo/engine-rs/target/release/tellfold")
        #expect(EngineCommand.engineBinary(at: "/elsewhere") { present.contains($0) } == nil)
        #expect(EngineCommand.engineBinary(at: "  ") { _ in true } == nil)
    }

    @Test func chooseEnginePrefersTheEnginePathThenTheBuiltInOne() {
        let bundled = URL(filePath: "/App.app/Contents/Resources/engine", directoryHint: .isDirectory)
        let present: Set<String> = ["/build/tellfold", "/App.app/Contents/Resources/engine/tellfold",
                                    "/repo/engine-rs/target/release/tellfold"]
        func choose(_ path: String, bundled: URL?, development: String? = nil) -> EngineCommand.EngineChoice {
            EngineCommand.chooseEngine(enginePath: path, bundled: bundled, developmentPath: development) { present.contains($0) }
        }
        #expect(choose("/build", bundled: bundled) == .run(URL(filePath: "/build/tellfold"), note: nil))
        #expect(choose("", bundled: bundled, development: "/repo")
                == .run(URL(filePath: "/App.app/Contents/Resources/engine/tellfold"), note: nil))
        // Debug builds without a bundled engine run the repository's release build
        #expect(choose(" ", bundled: nil, development: "/repo")
                == .run(URL(filePath: "/repo/engine-rs/target/release/tellfold"), note: nil))
        // a release build without one has nothing to run
        guard case .missing(let message) = choose("", bundled: nil) else { Issue.record("expected missing"); return }
        #expect(message.contains("Reinstall Tellfold"))
    }

    @Test func anOldPythonEnginePathFallsBackToTheBuiltInEngine() {
        // before 0.8 the engine path named the Python project, run with `uv run --project`
        let bundled = URL(filePath: "/App.app/Contents/Resources/engine", directoryHint: .isDirectory)
        let present: Set<String> = ["/App.app/Contents/Resources/engine/tellfold"]
        let choice = EngineCommand.chooseEngine(enginePath: "/Users/me/tellfold", bundled: bundled,
                                                developmentPath: nil) { present.contains($0) }
        guard case .run(let binary, let note) = choice else { Issue.record("expected the built-in engine"); return }
        #expect(binary.path == "/App.app/Contents/Resources/engine/tellfold")
        #expect(note?.contains("No tellfold engine at /Users/me/tellfold") == true)
        let nothing = EngineCommand.chooseEngine(enginePath: "/Users/me/tellfold", bundled: nil,
                                                 developmentPath: "/repo") { present.contains($0) }
        guard case .missing(let message) = nothing else { Issue.record("expected missing"); return }
        #expect(message.contains("/Users/me/tellfold"))
    }

    @Test func anEngineBinaryIsUsedAsIs() throws {
        let folder = URL.temporaryDirectory.appending(path: "tellfold-engine-\(UUID().uuidString)", directoryHint: .isDirectory)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: folder) }
        let binary = folder.appending(path: "my-tellfold")
        try Data("#!/bin/sh\n".utf8).write(to: binary)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: binary.path)
        #expect(EngineCommand.engineBinary(at: binary.path)?.path == binary.path)
        #expect(EngineCommand.engineBinary(at: folder.path) == nil) // the folder holds no file named tellfold
    }

    @Test func searchPathAddsToolDirectoriesOnce() {
        let path = EngineCommand.searchPath(existing: "/usr/bin:/bin:/usr/bin", home: "/Users/me")
        let parts = path.split(separator: ":").map(String.init)
        #expect(parts.first == "/usr/bin")
        #expect(parts.contains("/Users/me/.local/bin"))
        #expect(parts.contains("/opt/homebrew/bin"))
        #expect(parts.count == Set(parts).count)
    }

    @Test func environmentInjectsSecretsAndSwitches() {
        let base = ["PATH": "/usr/bin", "HOME": "/Users/me", "TELLFOLD_FAKE_LLM": "1",
                    "SHOAL_FAKE_LLM": "1", "SHOAL_SERVE_TOKEN": "stale",
                    "DYLD_INSERT_LIBRARIES": "/x/libXCTestBundleInject.dylib", "XCTestConfigurationFilePath": "/x"]
        let env = EngineCommand.environment(base: base, token: "t0k", secrets: ["ANTHROPIC_API_KEY": "sk-test"],
                                            offline: false, home: "/Users/me")
        #expect(env["ANTHROPIC_API_KEY"] == "sk-test")
        #expect(env["TELLFOLD_SERVE_TOKEN"] == "t0k")
        #expect(env["TELLFOLD_FAKE_LLM"] == nil)
        #expect(env["SHOAL_FAKE_LLM"] == nil)
        #expect(env["SHOAL_SERVE_TOKEN"] == nil)
        #expect(env["HOME"] == "/Users/me")
        #expect(env["DYLD_INSERT_LIBRARIES"] == nil)
        #expect(env["XCTestConfigurationFilePath"] == nil)
        #expect(env["PATH"]?.contains("/Users/me/.local/bin") == true)
        let offline = EngineCommand.environment(base: ["SHOAL_FAKE_LLM": "1"], token: "t", secrets: [:], offline: true, home: "/h")
        #expect(offline["TELLFOLD_FAKE_LLM"] == "1")
        #expect(offline["SHOAL_FAKE_LLM"] == nil)
    }

    @Test func backoffGrowsAndCaps() {
        #expect((1...8).map(EngineCommand.backoffSeconds(attempt:)) == [1, 2, 4, 8, 16, 30, 30, 30])
    }
}

struct KeychainNamingTests {
    @Test func providerEnvNames() {
        #expect(Keychain.envName(forProvider: "lmstudio") == "TELLFOLD_KEY_LMSTUDIO")
        #expect(Keychain.envName(forProvider: "open-router.ai") == "TELLFOLD_KEY_OPEN_ROUTER_AI")
        #expect(Keychain.envName(forProvider: "dé") == "TELLFOLD_KEY_D")
        #expect(Keychain.envName(forProvider: "-a--b_") == "TELLFOLD_KEY_A_B")
    }

    @Test func envNameValidation() {
        #expect(Keychain.isValidEnvName("ANTHROPIC_API_KEY"))
        #expect(Keychain.isValidEnvName("_X1"))
        #expect(!Keychain.isValidEnvName("1X"))
        #expect(!Keychain.isValidEnvName("A-B"))
        #expect(!Keychain.isValidEnvName(""))
        #expect(!Keychain.isValidEnvName("KÉY"))
    }

    @Test func keychainRoundTrip() throws {
        let keychain = Keychain(service: "dev.tellfold.mac.tests.\(UUID().uuidString)")
        do {
            try keychain.set("secret-1", for: "TEST_KEY")
        } catch {
            // CI machines without an unlocked login keychain cannot store items; nothing else to check there.
            return
        }
        defer { try? keychain.delete("TEST_KEY") }
        #expect(keychain.string(for: "TEST_KEY") == "secret-1")
        try keychain.set("secret-2", for: "TEST_KEY")
        #expect(keychain.environment() == ["TEST_KEY": "secret-2"])
        try keychain.set("", for: "TEST_KEY")
        #expect(!keychain.contains("TEST_KEY"))
        #expect(keychain.accounts().isEmpty)
    }
}

struct EstimateTests {
    /// Reference values from the engine's `estimate_call_split`; the same table is in tellfold-store's golden tests.
    @Test(arguments: [
        (20, 6, 6, 3, 20, 4), (6, 3, 3, 1, 8, 3), (5, 4, 2, 7, 10, 4), (200, 50, 20, 40, 254, 31), (25, 10, 4, 9, 43, 7),
        (20, 6, 6, 640, 20, 19),
    ])
    func matchesEngineFormula(agents: Int, rounds: Int, batch: Int, chunks: Int, agentCalls: Int, plannerCalls: Int) {
        let split = NewRunRequest.estimatedCallSplit(agents: agents, rounds: rounds, batch: batch, chunks: chunks)
        #expect(split.agent == agentCalls && split.planner == plannerCalls)
        #expect(NewRunRequest.estimatedCalls(agents: agents, rounds: rounds, batch: batch, chunks: chunks)
                == agentCalls + plannerCalls)
    }

    /// Crowd rules 2 and later add a stakes plan and a fact check a round on the planner side
    /// (`crowd_rules_calls` in tellfold-store's estimate).
    @Test func crowdRulesAddPlannerCalls() {
        let plain = NewRunRequest.estimatedCallSplit(agents: 20, rounds: 6, batch: 6, chunks: 3)
        let ruled = NewRunRequest.estimatedCallSplit(agents: 20, rounds: 6, batch: 6, chunks: 3, crowdRules: 3)
        #expect(ruled.agent == plain.agent && ruled.planner == plain.planner + 1 + 7)
        let saved = NewRunRequest.estimatedCallSplit(agents: 20, rounds: 6, batch: 6, chunks: 3, reusesCast: true,
                                                     crowdRules: 3)
        #expect(saved.planner == 1 + 7)
    }

    /// The app's copy of the engine's crowd rules version, read from the engine source next to it.
    @Test func crowdRulesMatchTheEngine() throws {
        let lib = URL(filePath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().appending(path: "engine-rs/crates/tellfold-models/src/lib.rs")
        let text = try String(contentsOf: lib, encoding: .utf8)
        let line = try #require(text.split(separator: "\n").first { $0.hasPrefix("pub const CROWD_RULES: i64 = ") })
        #expect(Int(line.dropFirst("pub const CROWD_RULES: i64 = ".count).dropLast()) == NewRunRequest.crowdRules)
    }

    /// A saved audience brings its people; the same rows are in the engine's golden tests.
    @Test(arguments: [(20, 6, 6, 3, 20, 1), (25, 10, 4, 9, 43, 1), (5, 4, 2, 7, 10, 1)])
    func savedAudienceSkipsCasting(agents: Int, rounds: Int, batch: Int, chunks: Int, agentCalls: Int,
                                   plannerCalls: Int) {
        let split = NewRunRequest.estimatedCallSplit(agents: agents, rounds: rounds, batch: batch, chunks: chunks,
                                                     reusesCast: true)
        #expect(split.agent == agentCalls && split.planner == plannerCalls)
    }
}

struct LineSplitterTests {
    @Test func splitsAcrossChunksAndCRLF() {
        var splitter = LineSplitter()
        #expect(splitter.append(Array("{\"po".utf8)).isEmpty)
        #expect(splitter.append(Array("rt\": 5}\r\nINFO a\nINFO".utf8)) == [#"{"port": 5}"#, "INFO a"])
        #expect(splitter.append(Array(" b\n\n".utf8)) == ["INFO b", ""])
        #expect(splitter.append(Array("tail".utf8)).isEmpty)
        #expect(splitter.flush() == "tail")
        #expect(splitter.flush() == nil)
    }

    @Test func readsLinesFromAPipe() async throws {
        let pipe = Pipe()
        let stream = PipeLineReader.lines(of: pipe.fileHandleForReading)
        try pipe.fileHandleForWriting.write(contentsOf: Data("{\"port\": 4321}\nsecond".utf8))
        try pipe.fileHandleForWriting.close()
        var lines: [String] = []
        for await line in stream { lines.append(line) }
        #expect(lines == [#"{"port": 4321}"#, "second"])
        #expect(EngineCommand.parsePort(fromLine: lines[0]) == 4321)
    }
}
