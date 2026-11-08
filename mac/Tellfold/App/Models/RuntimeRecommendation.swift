import Foundation

/// Whether a detected runtime can run a simulation right now, and if not, what is missing.
nonisolated enum RuntimeReadiness: Equatable, Sendable {
    case ready
    case needsSignIn
    /// A local model server that answers but has no model to run.
    case needsModel
    case notInstalled
}

/// Picks the AI tool to suggest from what the engine detected: something that works now before something that needs a
/// login, tools verified against the real CLI before ones built from documentation, then a fixed order of preference
/// (subscription CLIs, then local servers, then pay-per-token APIs). Kept free of views so it can be tested.
nonisolated enum RuntimeRecommendation {
    /// The order among runtimes that are equally ready and equally tested.
    static let preference = [
        "claude-code", "codex", "gemini-cli", "cursor-agent", "opencode", "copilot", "qwen-code", "droid",
        "ollama", "openai-compatible:", "anthropic-api", "openai-api", "grok",
    ]

    static func readiness(_ runtime: RuntimeInfo) -> RuntimeReadiness {
        guard runtime.installed else { return .notInstalled }
        if runtime.auth == .missing { return .needsSignIn }
        if runtime.kind == .http, runtime.models.isEmpty, (runtime.defaultAgentModel ?? "").isEmpty { return .needsModel }
        return .ready
    }

    /// Billed per token through an API key, rather than a tool the user already pays for or runs locally.
    static func isPayPerToken(_ runtime: RuntimeInfo) -> Bool {
        runtime.id == "anthropic-api" || runtime.id == "openai-api"
    }

    private static func preferenceIndex(_ id: String) -> Int {
        preference.firstIndex { $0.hasSuffix(":") ? id.hasPrefix($0) : id == $0 } ?? preference.count
    }

    private static func tier(_ readiness: RuntimeReadiness) -> Int {
        switch readiness {
        case .ready: 0
        case .needsSignIn, .needsModel: 1
        case .notInstalled: 2
        }
    }

    /// Every runtime, best suggestion first.
    static func ranked(_ runtimes: [RuntimeInfo]) -> [RuntimeInfo] {
        runtimes.enumerated().sorted { a, b in
            let ka = (tier(readiness(a.element)), a.element.tested ? 0 : 1, a.element.auth == .ok ? 0 : 1,
                      preferenceIndex(a.element.id), a.offset)
            let kb = (tier(readiness(b.element)), b.element.tested ? 0 : 1, b.element.auth == .ok ? 0 : 1,
                      preferenceIndex(b.element.id), b.offset)
            return ka < kb
        }.map(\.element)
    }

    /// The runtime to suggest, or nil when nothing on this Mac can run a simulation yet.
    static func recommended(_ runtimes: [RuntimeInfo]) -> RuntimeInfo? {
        ranked(runtimes).first { readiness($0) == .ready }
    }

    /// What is on this Mac (installed, signed in or not), best first: the list the setup step offers.
    static func detected(_ runtimes: [RuntimeInfo]) -> [RuntimeInfo] {
        ranked(runtimes).filter { readiness($0) != .notInstalled }
    }

    /// One plain line under a runtime's name in the setup list.
    static func statusLine(_ runtime: RuntimeInfo) -> String {
        switch readiness(runtime) {
        case .notInstalled:
            return "Not installed"
        case .needsSignIn:
            return runtime.authHint.map { "Not signed in. \($0)" } ?? "Not signed in."
        case .needsModel:
            return runtime.id == "ollama" ? "Running, but no models yet. Run `ollama pull llama3.2`."
                : "Running, but it lists no models."
        case .ready:
            var parts: [String] = []
            if runtime.kind == .cli {
                parts.append(runtime.auth == .ok ? "Signed in" : "Installed")
            } else if isPayPerToken(runtime) {
                parts.append("API key saved, billed per token")
            } else {
                let count = runtime.models.count
                parts.append(count == 0 ? "Running on this Mac"
                             : "Running on this Mac, \(count) \(count == 1 ? "model" : "models")")
            }
            if runtime.tested { parts.append("tested with Tellfold") }
            return parts.joined(separator: " · ")
        }
    }
}

extension RuntimeInfo {
    /// Runs on this Mac: a CLI, or an HTTP server on the loopback address (Ollama, LM Studio).
    nonisolated var isLocal: Bool {
        if kind == .cli { return true }
        guard let path, let host = URL(string: path)?.host() else { return false }
        return host == "127.0.0.1" || host == "localhost" || host == "::1" || host == "[::1]"
    }
}

/// A local OpenAI-compatible server the engine does not know about yet (LM Studio on its default port). Choosing it
/// adds it to the engine's endpoints.
nonisolated struct LocalModelServer: Hashable, Sendable {
    var endpointID: String
    var name: String
    var baseURL: String
    var models: [String]

    var runtimeID: String { "openai-compatible:\(endpointID)" }

    static let lmStudio = LocalModelServer(endpointID: "lmstudio", name: "LM Studio",
                                           baseURL: "http://127.0.0.1:1234/v1", models: [])

    /// How the setup list shows it before it is added.
    var asRuntime: RuntimeInfo {
        RuntimeInfo(id: runtimeID, kind: .http, name: name, installed: true, version: nil, path: baseURL, auth: .ok,
                    authHint: nil, models: models,
                    defaultModels: models.first.map { ["agent": $0, "planner": $0] } ?? [:], tested: false,
                    notes: "Found running on this Mac.")
    }

    var endpoint: OpenAICompatibleEndpoint {
        OpenAICompatibleEndpoint(id: endpointID, name: name, baseURL: baseURL, models: models)
    }

    /// Asks the server for its models (`GET /models`); nil when nothing answers within the timeout.
    static func probe(_ server: LocalModelServer, timeout: TimeInterval = 1.5) async -> LocalModelServer? {
        guard let url = URL(string: "\(server.baseURL)/models") else { return nil }
        var request = URLRequest(url: url, timeoutInterval: timeout)
        request.cachePolicy = .reloadIgnoringLocalCacheData
        guard let (data, response) = try? await URLSession.shared.data(for: request),
              (response as? HTTPURLResponse)?.statusCode == 200 else { return nil }
        var found = server
        found.models = modelIDs(from: data)
        return found
    }

    /// The `data[].id` list of an OpenAI-style `/models` response.
    static func modelIDs(from data: Data) -> [String] {
        struct List: Decodable {
            struct Model: Decodable { var id: String }
            var data: [Model]
        }
        return ((try? JSONDecoder().decode(List.self, from: data))?.data ?? []).map(\.id)
    }
}

/// Where to get an AI tool when this Mac has none.
nonisolated struct RuntimeInstallOption: Identifiable, Hashable, Sendable {
    var id: String
    var name: String
    var blurb: String
    var command: String?
    var url: URL

    static let suggestions: [RuntimeInstallOption] = [
        RuntimeInstallOption(
            id: "claude-code", name: "Claude Code",
            blurb: "Anthropic's assistant. Works with a Claude Pro or Max plan. Tested with Tellfold.",
            command: "curl -fsSL https://claude.ai/install.sh | bash",
            url: URL(string: "https://claude.com/claude-code")!),
        RuntimeInstallOption(
            id: "codex", name: "Codex",
            blurb: "OpenAI's assistant. Works with a ChatGPT plan.",
            command: "npm install -g @openai/codex",
            url: URL(string: "https://developers.openai.com/codex/cli")!),
        RuntimeInstallOption(
            id: "ollama", name: "Ollama",
            blurb: "Free models that run on this Mac, no account needed. Slower, and best on Apple silicon with 16 GB or more.",
            command: nil,
            url: URL(string: "https://ollama.com/download")!),
    ]
}
