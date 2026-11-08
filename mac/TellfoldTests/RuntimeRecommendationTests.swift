import Foundation
import Testing
@testable import Tellfold

@Suite("Suggesting an AI tool")
struct RuntimeRecommendationTests {
    private func runtime(_ id: String, kind: RuntimeInfo.Kind = .cli, installed: Bool = true,
                         auth: RuntimeInfo.Auth = .ok, models: [String] = ["default"], tested: Bool = false,
                         path: String? = nil) -> RuntimeInfo {
        RuntimeInfo(id: id, kind: kind, name: id, installed: installed, version: nil, path: path, auth: auth,
                    authHint: "Sign in.", models: models, defaultModels: [:], tested: tested, notes: nil)
    }

    @Test func testedSignedInToolWins() {
        let list = [runtime("codex"), runtime("cursor-agent", auth: .missing), runtime("claude-code", tested: true)]
        #expect(RuntimeRecommendation.recommended(list)?.id == "claude-code")
    }

    @Test func readyBeatsTestedButSignedOut() {
        let list = [runtime("claude-code", auth: .missing, tested: true), runtime("gemini-cli", auth: .unknown)]
        #expect(RuntimeRecommendation.recommended(list)?.id == "gemini-cli")
    }

    @Test func subscriptionCLIsBeforeLocalServersBeforePaidAPIs() {
        let list = [runtime("openai-api", kind: .http), runtime("ollama", kind: .http, path: "http://127.0.0.1:11434"),
                    runtime("codex")]
        #expect(RuntimeRecommendation.ranked(list).map(\.id) == ["codex", "ollama", "openai-api"])
        #expect(RuntimeRecommendation.recommended(Array(list.prefix(2)))?.id == "ollama")
    }

    @Test func nothingReadyMeansNoSuggestion() {
        let list = [runtime("claude-code", installed: false), runtime("cursor-agent", auth: .missing),
                    runtime("ollama", kind: .http, models: [])]
        #expect(RuntimeRecommendation.recommended(list) == nil)
        #expect(RuntimeRecommendation.detected(list).map(\.id) == ["ollama", "cursor-agent"])
        #expect(RuntimeRecommendation.readiness(list[2]) == .needsModel)
    }

    @Test func statusLinesSayWhatIsWrong() {
        #expect(RuntimeRecommendation.statusLine(runtime("claude-code", tested: true)) == "Signed in · tested with Tellfold")
        #expect(RuntimeRecommendation.statusLine(runtime("cursor-agent", auth: .missing)) == "Not signed in. Sign in.")
        #expect(RuntimeRecommendation.statusLine(runtime("ollama", kind: .http, models: ["a", "b"],
                                                         path: "http://127.0.0.1:11434"))
            == "Running on this Mac, 2 models")
    }

    @Test func localMeansCLIOrLoopback() {
        #expect(runtime("codex").isLocal)
        #expect(runtime("ollama", kind: .http, path: "http://127.0.0.1:11434").isLocal)
        #expect(!runtime("openai-api", kind: .http, path: "https://api.openai.com/v1").isLocal)
    }

    @Test func lmStudioModelsAreReadFromTheModelsList() {
        let body = Data(#"{"object":"list","data":[{"id":"qwen3-8b","object":"model"},{"id":"gemma-3"}]}"#.utf8)
        #expect(LocalModelServer.modelIDs(from: body) == ["qwen3-8b", "gemma-3"])
        var server = LocalModelServer.lmStudio
        server.models = ["qwen3-8b"]
        #expect(server.asRuntime.id == "openai-compatible:lmstudio")
        #expect(RuntimeRecommendation.readiness(server.asRuntime) == .ready)
    }
}
