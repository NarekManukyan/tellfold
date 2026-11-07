import Foundation

/// A detected LLM runtime (`GET /api/runtimes`), mirroring `shoal.runtimes.base.RuntimeInfo`.
struct RuntimeInfo: Codable, Hashable, Sendable, Identifiable {
    enum Kind: String, Codable, Hashable, Sendable {
        case cli, http, unknown

        init(from decoder: any Decoder) throws {
            let raw = try decoder.singleValueContainer().decode(String.self)
            self = Kind(rawValue: raw) ?? .unknown
        }
    }

    enum Auth: String, Codable, Hashable, Sendable {
        case ok, missing, unknown

        init(from decoder: any Decoder) throws {
            let raw = try decoder.singleValueContainer().decode(String.self)
            self = Auth(rawValue: raw) ?? .unknown
        }
    }

    /// "claude-code", "ollama", "openai-compatible:lmstudio", ...
    var id: String
    var kind: Kind
    var name: String
    var installed: Bool
    var version: String?
    /// Binary path or base URL.
    var path: String?
    var auth: Auth
    var authHint: String?
    /// Known or available models, best first.
    var models: [String]
    /// `{"agent": ..., "planner": ...}`.
    var defaultModels: [String: String]
    /// The adapter was verified against the real tool.
    var tested: Bool
    var notes: String?

    var defaultAgentModel: String? { defaultModels["agent"] ?? models.first }
    var defaultPlannerModel: String? { defaultModels["planner"] ?? models.first }

    enum CodingKeys: String, CodingKey {
        case id, kind, name, installed, version, path, auth, models, tested, notes
        case authHint = "auth_hint"
        case defaultModels = "default_models"
    }

    /// Used when the engine predates `/api/runtimes`: the original Claude Code backend.
    static let claudeCodeFallback = RuntimeInfo(
        id: "claude-code", kind: .cli, name: "Claude Code", installed: true, version: nil, path: nil,
        auth: .unknown, authHint: "Run `claude` and use /login", models: ["haiku", "sonnet", "opus"],
        defaultModels: ["agent": "haiku", "planner": "sonnet"], tested: true, notes: nil)
}

extension RuntimeInfo {
    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        kind = c.value(.kind, default: .unknown)
        name = c.value(.name, default: id)
        installed = c.value(.installed, default: false)
        version = c.optional(.version)
        path = c.optional(.path)
        auth = c.value(.auth, default: .unknown)
        authHint = c.optional(.authHint)
        models = c.value(.models, default: [])
        defaultModels = c.value(.defaultModels, default: [:])
        tested = c.value(.tested, default: false)
        notes = c.optional(.notes)
    }
}

/// `POST /api/runtimes/{id}/test` → `{ok, seconds, reply, error}`.
struct RuntimeTestResult: Codable, Hashable, Sendable {
    var ok: Bool
    var seconds: Double?
    var reply: String?
    var error: String?
}

extension RuntimeTestResult {
    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        ok = c.value(.ok, default: false)
        seconds = c.optional(.seconds)
        reply = c.optional(.reply)
        error = c.optional(.error)
    }
}

/// An OpenAI-compatible endpoint the user added (LM Studio, llama.cpp, vLLM, OpenRouter, ...).
/// `keyEnv` names the environment variable that carries its API key; the key itself lives in the Keychain.
struct OpenAICompatibleEndpoint: Codable, Hashable, Sendable, Identifiable {
    var id: String
    var name: String
    var baseURL: String
    var models: [String] = []
    var keyEnv: String?

    enum CodingKeys: String, CodingKey {
        case id, name, models
        case baseURL = "base_url"
        case keyEnv = "key_env"
    }
}

extension OpenAICompatibleEndpoint {
    /// The id as the engine stores it: trimmed and lower-cased (`OpenAIEndpoint._id` in shoal/settings.py).
    static func normalizedID(_ raw: String) -> String {
        raw.trimmingCharacters(in: .whitespaces).lowercased()
    }

    /// The engine's rule for endpoint ids, `^[a-z0-9][a-z0-9_-]{0,39}$`, applied after `normalizedID`.
    static func isValidID(_ raw: String) -> Bool {
        let id = normalizedID(raw)
        guard let first = id.unicodeScalars.first, id.unicodeScalars.count <= 40 else { return false }
        func slug(_ c: Unicode.Scalar) -> Bool { ("a"..."z").contains(c) || ("0"..."9").contains(c) }
        return slug(first) && id.unicodeScalars.allSatisfy { slug($0) || $0 == "-" || $0 == "_" }
    }

    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        name = c.value(.name, default: id)
        baseURL = c.value(.baseURL, default: "")
        models = c.value(.models, default: [])
        keyEnv = c.optional(.keyEnv)
    }
}

/// The engine's `Settings` (`GET`/`PUT /api/settings`). Kept as a raw JSON object so fields this app does not
/// know about survive a round trip; typed accessors read and write the known ones. Each accessor accepts a few
/// plausible key spellings and writes back to whichever one the engine used (first spelling when absent).
struct EngineSettings: Codable, Hashable, Sendable {
    var raw: [String: JSONValue]

    init(raw: [String: JSONValue] = [:]) { self.raw = raw }

    init(from decoder: any Decoder) throws {
        raw = try decoder.singleValueContainer().decode([String: JSONValue].self)
    }

    func encode(to encoder: any Encoder) throws {
        var c = encoder.singleValueContainer()
        try c.encode(raw)
    }

    enum Key {
        static let runsDir = ["runs_dir", "runs_folder", "runs"]
        static let agentRuntime = ["agent_runtime", "default_agent_runtime"]
        static let agentModel = ["agent_model", "default_agent_model"]
        static let plannerRuntime = ["planner_runtime", "default_planner_runtime"]
        static let plannerModel = ["planner_model", "default_planner_model"]
        static let ollamaURL = ["ollama_base_url", "ollama_url", "ollama"]
        static let endpoints = ["openai_endpoints", "openai_compatible", "endpoints"]
        static let concurrency = ["concurrency", "default_concurrency"]
        static let think = ["think", "default_think"]
    }

    private func first(_ keys: [String]) -> JSONValue? {
        for k in keys { if let v = raw[k], !v.isNull { return v } }
        return nil
    }

    private mutating func set(_ keys: [String], _ value: JSONValue) {
        let key = keys.first { raw[$0] != nil } ?? keys[0]
        raw[key] = value
    }

    private func string(_ keys: [String]) -> String? { first(keys)?.stringValue }
    private mutating func setString(_ keys: [String], _ value: String?) {
        set(keys, value.map(JSONValue.string) ?? .null)
    }

    var runsDir: String? {
        get { string(Key.runsDir) }
        set { setString(Key.runsDir, newValue) }
    }
    var agentRuntime: String? {
        get { string(Key.agentRuntime) }
        set { setString(Key.agentRuntime, newValue) }
    }
    var agentModel: String? {
        get { string(Key.agentModel) }
        set { setString(Key.agentModel, newValue) }
    }
    var plannerRuntime: String? {
        get { string(Key.plannerRuntime) }
        set { setString(Key.plannerRuntime, newValue) }
    }
    var plannerModel: String? {
        get { string(Key.plannerModel) }
        set { setString(Key.plannerModel, newValue) }
    }
    var ollamaBaseURL: String? {
        get { string(Key.ollamaURL) }
        set { setString(Key.ollamaURL, newValue) }
    }
    var concurrency: Int? {
        get { first(Key.concurrency)?.intValue }
        set { set(Key.concurrency, newValue.map { .number(Double($0)) } ?? .null) }
    }
    var think: Bool? {
        get { first(Key.think)?.boolValue }
        set { set(Key.think, newValue.map(JSONValue.bool) ?? .null) }
    }
    var openAICompatible: [OpenAICompatibleEndpoint] {
        get {
            guard let items = first(Key.endpoints)?.arrayValue else { return [] }
            return items.compactMap { try? $0.decode(OpenAICompatibleEndpoint.self) }
        }
        set {
            let values = newValue.compactMap { try? JSONValue.from($0) }
            set(Key.endpoints, .array(values))
        }
    }
}
