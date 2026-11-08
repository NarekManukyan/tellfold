import Foundation
import Testing
@testable import Tellfold

/// Codable models decode the JSON the real engine sends (fixtures captured from `tellfold ui` in offline mode).
struct ModelDecodingTests {
    @Test func health() throws {
        let health = try Fixtures.decode(Health.self, "health.json")
        #expect(health.ok)
        #expect(health.fakeLLM)
        #expect(!health.version.isEmpty)
    }

    @Test func runSummaries() throws {
        let runs = try Fixtures.decode([RunSummary].self, "runs.json")
        #expect(runs.count == 1)
        let run = try #require(runs.first)
        #expect(run.state == .done)
        #expect(run.stage == .finished)
        #expect(run.round == 3)
        #expect(run.totalRounds == 3)
        #expect(run.agents == 6)
        #expect(run.hasReport)
        #expect(run.createdDate != nil)
        #expect(run.progress == 1)
    }

    @Test func finishedRunDetail() throws {
        let detail = try Fixtures.decode(RunDetail.self, "run_detail.json")
        #expect(detail.config.numAgents == 6)
        #expect(detail.config.rounds == 3)
        #expect(detail.config.batchSize == 3)
        #expect(detail.config.agentModel == "haiku")
        #expect(detail.config.agentRuntime == "claude-code") // absent in this engine version: default
        #expect(detail.config.events == [GodEvent(round: 2, content: "A competitor cuts prices by half.")])
        let status = try #require(detail.status)
        #expect(status.state == .done)
        #expect(status.stage == .finished)
        #expect(status.startedDate != nil)
        #expect(!detail.live)
        #expect(detail.personas.count == 6)
        #expect(detail.personas.first?.id == "a01")
        #expect(detail.personas.first?.entityID == nil)
        let graph = try #require(detail.graph)
        #expect(!graph.entities.isEmpty)
        #expect(!graph.relations.isEmpty)
        #expect(detail.stats.map(\.round) == [0, 1, 2, 3])
        #expect(detail.stats[0].count(.post) == 2)
        #expect(detail.stats[1].sentiment == nil)
        #expect(!detail.follows.isEmpty)
        #expect(detail.followEdges.count == detail.follows.count)
        #expect(detail.followEdges.first == FollowEdge(follower: "a01", followee: "a02"))
        let report = try #require(detail.report)
        #expect(report.confidence == .medium)
        #expect(!report.sections.isEmpty)
        #expect(!detail.reportRebuild.running)
        #expect(detail.totalRounds == 3)
        #expect(detail.currentRound == 3)
    }

    @Test func liveRunDetail() throws {
        let detail = try Fixtures.decode(RunDetail.self, "run_detail_live.json")
        #expect(detail.live)
        #expect(detail.status?.state == .running)
        #expect(detail.status?.state.isActive == true)
        #expect(detail.personas.count == 120)
        #expect(detail.report == nil)
    }

    @Test func posts() throws {
        let posts = try Fixtures.decode([PostOut].self, "posts.json")
        #expect(!posts.isEmpty)
        #expect(posts.map(\.id) == posts.map(\.id).sorted())
        let first = try #require(posts.first)
        #expect(first.round == 0)
        #expect(first.authorHandle.hasPrefix("fake_user"))
        #expect(first.sentiment == 0.5)
        #expect(posts.contains { $0.sentiment == nil })
        #expect(posts.contains { $0.isReply })
    }

    @Test func actions() throws {
        let actions = try Fixtures.decode([ActionOut].self, "actions.json")
        #expect(!actions.isEmpty)
        #expect(actions.allSatisfy { $0.type != .unknown })
        #expect(actions.contains { $0.type == .nothing })
        #expect(Set(actions.map(\.round)) == [0, 1, 2, 3])
    }

    @Test func agentDetail() throws {
        let agent = try Fixtures.decode(AgentDetail.self, "agent_detail.json")
        #expect(agent.persona.id == "a01")
        #expect(!agent.memory.isEmpty)
        #expect(!agent.posts.isEmpty)
        #expect(agent.sentimentByRound == [RoundScore(round: 3, score: 0.5)])
    }

    @Test func replies() throws {
        #expect(try Fixtures.decode(ReplyResponse.self, "chat.json").reply.contains("offline"))
        #expect(try Fixtures.decode(ReplyResponse.self, "ask.json").reply.contains("offline"))
    }

    @Test func errorBodies() throws {
        let conflict = TellfoldAPIError.from(status: 409, body: try Fixtures.data("stop_conflict.json"))
        #expect(conflict.isConflict)
        #expect(conflict.message == "only a run that is live in this server can be stopped")
        let validation = TellfoldAPIError.from(status: 422, body: try Fixtures.data("error_validation.json"))
        #expect(validation.message == "agent_id: String should have at least 1 character")
        let plain = TellfoldAPIError.from(status: 500, body: Data("Internal Server Error".utf8))
        #expect(plain.message == "Internal Server Error")
        let unauthorized = TellfoldAPIError.from(status: 401, body: try Fixtures.data("unauthorized.json"))
        #expect(unauthorized.isUnauthorized && unauthorized.message == "unauthorized")
        let empty = TellfoldAPIError.from(status: 401, body: Data())
        #expect(empty.isUnauthorized)
        #expect(!empty.message.isEmpty)
    }

    @Test func unknownEnumValuesAreTolerated() throws {
        let json = #"{"state":"paused","stage":"dreaming","total_rounds":4,"started_at":"2026-11-07T00:00:00Z","updated_at":"2026-11-07T00:00:00Z"}"#
        let status = try TellfoldAPI.decoder.decode(RunStatus.self, from: Data(json.utf8))
        #expect(status.state == .unknown)
        #expect(status.stage == .unknown)
        #expect(status.round == 0)
        #expect(status.detail == "")
    }

    @Test func runtimes() throws {
        let runtimes = try Fixtures.decode([RuntimeInfo].self, "runtimes_handwritten.json")
        #expect(runtimes.map(\.id) == ["claude-code", "ollama", "openai-compatible:lmstudio"])
        #expect(runtimes[0].kind == .cli)
        #expect(runtimes[0].auth == .ok)
        #expect(runtimes[0].defaultAgentModel == "haiku")
        #expect(runtimes[0].defaultPlannerModel == "sonnet")
        #expect(runtimes[1].installed == false)
        #expect(runtimes[1].defaultAgentModel == nil)
        #expect(runtimes[2].auth == .missing)
    }

    @Test func capturedRuntimes() throws {
        let runtimes = try Fixtures.decode([RuntimeInfo].self, "runtimes.json")
        #expect(runtimes.count >= 10)
        let claude = try #require(runtimes.first { $0.id == "claude-code" })
        #expect(claude.kind == .cli && claude.installed && claude.tested)
        #expect(claude.defaultModels == ["agent": "haiku", "planner": "sonnet"])
        #expect(runtimes.contains { $0.kind == .http })
        #expect(runtimes.allSatisfy { $0.kind != .unknown })
        let test = try Fixtures.decode(RuntimeTestResult.self, "runtime_test.json")
        #expect(test.ok && test.reply == "ok (fake LLM)")
    }

    @Test func capturedSettings() throws {
        var settings = try Fixtures.decode(EngineSettings.self, "settings.json")
        #expect(settings.runsDir == nil)
        #expect(settings.agentRuntime == "claude-code")
        #expect(settings.agentModel == "haiku")
        #expect(settings.plannerModel == "sonnet")
        #expect(settings.ollamaBaseURL == "http://127.0.0.1:11434")
        #expect(settings.concurrency == 4)
        #expect(settings.think == false)
        #expect(settings.openAICompatible.isEmpty)
        settings.openAICompatible = [OpenAICompatibleEndpoint(id: "lmstudio", name: "LM Studio", baseURL: "http://127.0.0.1:1234/v1")]
        #expect(settings.raw["openai_endpoints"]?.arrayValue?.count == 1)
        #expect(settings.raw.keys.sorted() == ["agent_model", "agent_runtime", "concurrency", "ollama_base_url",
                                               "openai_endpoints", "planner_model", "planner_runtime", "runs_dir", "think"])
    }

    @Test func runtimeTestResult() throws {
        let ok = try TellfoldAPI.decoder.decode(RuntimeTestResult.self, from: Data(#"{"ok":true,"seconds":1.25,"reply":"pong","error":null}"#.utf8))
        #expect(ok.ok && ok.seconds == 1.25 && ok.reply == "pong" && ok.error == nil)
        let failed = try TellfoldAPI.decoder.decode(RuntimeTestResult.self, from: Data(#"{"ok":false,"error":"not signed in"}"#.utf8))
        #expect(!failed.ok && failed.error == "not signed in")
    }

    @Test func engineSettingsRoundTripKeepsUnknownFields() throws {
        let json = #"""
        {"runs_dir":"/tmp/runs","agent_runtime":"claude-code","agent_model":"haiku","planner_runtime":"ollama",
         "planner_model":"llama3.2","ollama_base_url":"http://127.0.0.1:11434","concurrency":4,"think":false,
         "openai_endpoints":[{"id":"lmstudio","name":"LM Studio","base_url":"http://127.0.0.1:1234/v1","models":["a"],"key_env":null}],
         "future":{"x":1}}
        """#
        var settings = try TellfoldAPI.decoder.decode(EngineSettings.self, from: Data(json.utf8))
        #expect(settings.plannerRuntime == "ollama")
        #expect(settings.concurrency == 4)
        #expect(settings.think == false)
        #expect(settings.openAICompatible.first?.baseURL == "http://127.0.0.1:1234/v1")
        settings.plannerModel = "qwen3"
        settings.openAICompatible.append(OpenAICompatibleEndpoint(id: "or", name: "OpenRouter", baseURL: "https://openrouter.ai/api/v1",
                                                                  models: [], keyEnv: "TELLFOLD_KEY_OR"))
        let encoded = try TellfoldAPI.encoder.encode(settings)
        let back = try TellfoldAPI.decoder.decode([String: JSONValue].self, from: encoded)
        #expect(back["planner_model"] == .string("qwen3"))
        #expect(back["future"] == .object(["x": .number(1)]))
        #expect(back["openai_endpoints"]?.arrayValue?.count == 2)
        #expect(back["concurrency"] == .number(4))
    }

    @Test func engineSettingsWriteNewKeysWhenAbsent() {
        var settings = EngineSettings()
        settings.agentRuntime = "cursor-agent"
        settings.ollamaBaseURL = "http://localhost:11434"
        #expect(settings.raw["agent_runtime"] == .string("cursor-agent"))
        #expect(settings.raw["ollama_base_url"] == .string("http://localhost:11434"))
    }

    @Test func dates() {
        #expect(TellfoldDates.parse("2026-11-07T00:05:41Z") != nil)
        #expect(TellfoldDates.parse("2026-11-07T00:05:41.123Z") != nil)
        #expect(TellfoldDates.parse("") == nil)
        #expect(TellfoldDates.parse("yesterday") == nil)
    }

    @Test func chatTurnEncodesAsPair() throws {
        let data = try TellfoldAPI.encoder.encode([ChatTurn(question: "q", answer: "a")])
        #expect(String(decoding: data, as: UTF8.self) == #"[["q","a"]]"#)
    }

    @Test func callsSummarySplitsByRoleWhenTheEngineDoes() throws {
        let split = try TellfoldAPI.decoder.decode(CallsSummary.self, from: Data(#"""
        {"count":6,"failed":1,"seconds":36.75,"agent":{"count":3,"failed":0,"seconds":4.25},
         "planner":{"count":3,"failed":1,"seconds":32.5}}
        """#.utf8))
        #expect(split.agent == CallTotals(count: 3, failed: 0, seconds: 4.25))
        #expect(split.planner?.failed == 1)
        let old = try TellfoldAPI.decoder.decode(CallsSummary.self, from: Data(#"{"count":2,"failed":0,"seconds":1}"#.utf8))
        #expect(old.count == 2 && old.agent == nil && old.planner == nil)
    }
}
