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
        let args = EngineCommand.arguments(enginePath: "/eng", runsFolder: "/runs dir", logLevel: "INFO")
        #expect(args == ["run", "--project", "/eng", "tellfold", "serve", "--port", "0", "--runs", "/runs dir", "--log-level", "info"])
        let shown = EngineCommand.displayCommand(executable: "/bin/uv", arguments: args + ["--token", "tok"], token: "tok")
        #expect(!shown.contains("tok "))
        #expect(shown.hasSuffix("--token •••"))
        #expect(shown.contains("'/runs dir'"))
    }

    @Test func bundledCommandLine() {
        let args = EngineCommand.bundledArguments(runsFolder: "/runs", logLevel: "Debug")
        #expect(args == ["serve", "--port", "0", "--runs", "/runs", "--log-level", "debug"])
        let binary = EngineCommand.bundledBinary(in: URL(filePath: "/App.app/Contents/Resources/engine", directoryHint: .isDirectory))
        #expect(binary.path == "/App.app/Contents/Resources/engine/tellfold")
    }

    @Test func examplesFolderIsPassedWhenBundled() {
        let args = EngineCommand.bundledArguments(runsFolder: "/runs", examplesFolder: "/App.app/Contents/Resources/Examples",
                                                  logLevel: "warning")
        #expect(args == ["serve", "--port", "0", "--runs", "/runs",
                         "--examples", "/App.app/Contents/Resources/Examples", "--log-level", "warning"])
        let dev = EngineCommand.arguments(enginePath: "/eng", runsFolder: "/runs", examplesFolder: "/ex", logLevel: "info")
        #expect(dev.suffix(4) == ["--examples", "/ex", "--log-level", "info"])
        #expect(!EngineCommand.arguments(enginePath: "/eng", runsFolder: "/runs", logLevel: "info").contains("--examples"))
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

    @Test func findsUVOnTheSearchPath() {
        let present: Set<String> = ["/opt/homebrew/bin/uv", "/home/me/.local/bin/uv"]
        let found = EngineCommand.findUV(preferred: nil, path: "/usr/bin:/home/me/.local/bin:/opt/homebrew/bin") { present.contains($0) }
        #expect(found?.path == "/home/me/.local/bin/uv")
        #expect(EngineCommand.findUV(preferred: nil, path: "/usr/bin") { present.contains($0) } == nil)
    }

    @Test func explicitUVPathWinsAndIsNotSilentlyReplaced() {
        let present: Set<String> = ["/custom/uv", "/opt/homebrew/bin/uv"]
        #expect(EngineCommand.findUV(preferred: "/custom/uv", path: "/opt/homebrew/bin") { present.contains($0) }?.path == "/custom/uv")
        #expect(EngineCommand.findUV(preferred: "/missing/uv", path: "/opt/homebrew/bin") { present.contains($0) } == nil)
        #expect(EngineCommand.findUV(preferred: "  ", path: "/opt/homebrew/bin") { present.contains($0) }?.path == "/opt/homebrew/bin/uv")
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
        #expect(env["PYTHONUNBUFFERED"] == "1")
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
    /// Reference values from `tellfold.runner.estimate_call_split`; the same table is in tests/test_server.py.
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
        #expect(ruled.agent == plain.agent && ruled.planner == plain.planner + 1 + 6)
        let saved = NewRunRequest.estimatedCallSplit(agents: 20, rounds: 6, batch: 6, chunks: 3, reusesCast: true,
                                                     crowdRules: 3)
        #expect(saved.planner == 1 + 6)
    }

    /// A saved audience brings its people; the same rows are in tests/test_server.py and the engine's golden tests.
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
