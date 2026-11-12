import Foundation
import UniformTypeIdentifiers

/// An error from the engine API: an HTTP status plus the server's `detail` message, or a transport failure
/// (status 0).
struct TellfoldAPIError: Error, LocalizedError, Hashable, Sendable {
    var status: Int
    var message: String

    var errorDescription: String? { message }

    var isConflict: Bool { status == 409 }
    var isNotFound: Bool { status == 404 }
    var isUnauthorized: Bool { status == 401 }

    /// Builds the message from a FastAPI error body: `{"detail": "text"}` or
    /// `{"detail": [{"msg": ..., "loc": [...]}, ...]}`.
    static func from(status: Int, body: Data) -> TellfoldAPIError {
        let fallback = HTTPURLResponse.localizedString(forStatusCode: status).capitalized
        guard let json = try? JSONDecoder().decode([String: JSONValue].self, from: body),
              let detail = json["detail"] else {
            let text = String(decoding: body.prefix(500), as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
            return TellfoldAPIError(status: status, message: text.isEmpty ? fallback : text)
        }
        if let s = detail.stringValue { return TellfoldAPIError(status: status, message: s) }
        if let items = detail.arrayValue {
            let parts = items.compactMap { item -> String? in
                guard let msg = item["msg"]?.stringValue else { return nil }
                let field = item["loc"]?.arrayValue?.last?.stringValue
                return field.map { "\($0): \(msg)" } ?? msg
            }
            if !parts.isEmpty { return TellfoldAPIError(status: status, message: parts.joined(separator: "\n")) }
        }
        return TellfoldAPIError(status: status, message: fallback)
    }
}

/// Everything the New Simulation sheet sends to `POST /api/runs` (multipart).
struct NewRunRequest: Hashable, Sendable {
    var files: [URL]
    var question: String
    var agents: Int = 20
    var rounds: Int = 6
    var batch: Int = 6
    var agentRuntime: String = "claude-code"
    var agentModel: String = "haiku"
    var plannerRuntime: String = "claude-code"
    var plannerModel: String = "sonnet"
    var concurrency: Int = 4
    var language: String = "English"
    var think: Bool = false
    var events: [GodEvent] = []
    /// RNG seed for activity sampling (the engine's default is 7).
    var seed: Int = 7
    /// Documents that exist only in memory, such as the pasted description, sent like uploaded seed files.
    var inlineDocuments: [InlineDocument] = []
    /// Reuse this run's topic map and people ("Try another version…"); the engine then skips both stages.
    var castFrom: String?
    /// The study to join; nil starts one (or joins the cast's own study).
    var studyID: String?
    /// The label in the study, e.g. "Draft B"; nil lets the engine pick the next letter.
    var variant: String?
    /// Pause once the people are ready so they can be checked and edited before the run.
    var reviewCast = false
    /// Give every camp a turn to post and keep strongly held views from being starved.
    var fairTurns = true
    /// Start from a saved audience: its people (and topic map) are used as they are, so casting is skipped.
    var audience: SavedAudience?
    /// The user's own audience material to build the people from (redacted by the engine before any model sees it).
    var material: [MaterialInput] = []
    /// Extra names or words to hide in the material.
    var materialHide: [String] = []

    static let seedExtensions: Set<String> = ["pdf", "md", "markdown", "txt"]
    static let maxFileBytes = 20 * 1024 * 1024
    /// The engine's limits (`MAX_FILES` and `MAX_TOTAL_BYTES` in tellfold/server/app.py).
    static let maxFiles = 100
    static let maxTotalBytes = 100 * 1024 * 1024

    /// Chunks the topic map reads at most (`MAX_EXTRACT_CHUNKS` in tellfold/graph.py).
    static let maxExtractChunks = 64

    /// Share of the crowd expected to act in a round (`ESTIMATED_ACTIVE_SHARE` in tellfold/runner.py).
    static let estimatedActiveShare = 0.4

    /// The same formula as `tellfold.runner.estimate_call_split`: model calls for the crowd (agent runtime) and for
    /// planning (graph, personas, report) when nothing needs a retry.
    /// `reusesCast` (a saved audience) skips the graph and persona calls; `materialItems` of audience material add
    /// the planner calls that read them (Rust-only, `material_calls` in tellfold-store/src/estimate.rs).
    static func estimatedCallSplit(agents: Int, rounds: Int, batch: Int, chunks: Int,
                                   reusesCast: Bool = false, materialItems: Int = 0) -> (agent: Int, planner: Int) {
        let batch = Double(max(1, batch))
        let graph = reusesCast ? 0 : Int((Double(min(chunks, maxExtractChunks)) / 4).rounded(.up))
        let personas = reusesCast ? 0 : Int((Double(agents) / 10).rounded(.up))
        let openingAuthors = (0.3 * Double(agents)).rounded(.toNearestOrEven)
        let opening = Int((openingAuthors / batch).rounded(.up)) + 1
        let active = max(1, (Double(agents) * estimatedActiveShare).rounded(.up))
        let perRound = Int((active / batch).rounded(.up)) + 1
        let material = reusesCast ? 0 : MaterialText.calls(items: materialItems)
        return (opening + rounds * perRound, graph + personas + material + 1)
    }

    /// The same formula as `tellfold.runner.estimate_calls`.
    static func estimatedCalls(agents: Int, rounds: Int, batch: Int, chunks: Int) -> Int {
        let split = estimatedCallSplit(agents: agents, rounds: rounds, batch: batch, chunks: chunks)
        return split.agent + split.planner
    }

    /// Rough chunk count for the estimate: ~1,600 characters per chunk, PDFs yield ~30 % text per byte.
    /// `sizes` are file sizes already known, so the estimate doesn't read the disk for them.
    static func estimatedChunks(files: [URL], sizes: [URL: Int] = [:],
                                inlineDocuments: [InlineDocument] = []) -> Int {
        var chars = Double(inlineDocuments.reduce(0) { $0 + $1.data.count })
        for url in files {
            let size = sizes[url] ?? (try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0
            chars += url.pathExtension.lowercased() == "pdf" ? Double(size) * 0.3 : Double(size)
        }
        return max(1, Int((chars / 1600).rounded(.up)))
    }
}

/// One entry of `GET /api/runs/{id}/seeds`.
struct SeedInfo: Codable, Hashable, Sendable {
    var name: String
    var size: Int
}

/// Typed client for the engine's HTTP API (UI_CONTRACT.md plus the runtime/settings endpoints).
/// Every `/api` request carries `Authorization: Bearer <token>` when a token is set.
actor TellfoldAPI {
    nonisolated let baseURL: URL
    nonisolated let token: String?
    private let session: URLSession

    static let decoder = JSONDecoder()
    static let encoder: JSONEncoder = {
        let e = JSONEncoder()
        e.outputFormatting = [.sortedKeys]
        return e
    }()

    init(baseURL: URL, token: String?, session: URLSession = .shared) {
        self.baseURL = baseURL
        self.token = token
        self.session = session
    }

    /// `http://127.0.0.1:<port>`.
    init(port: Int, token: String?, session: URLSession = .shared) {
        self.init(baseURL: URL(string: "http://127.0.0.1:\(port)")!, token: token, session: session)
    }

    // MARK: Runs

    /// `timeout` overrides the usual 30 s GET timeout (the startup probe uses a few seconds).
    func health(timeout: TimeInterval? = nil) async throws -> Health {
        var req = makeRequest(["health"])
        if let timeout { req.timeoutInterval = timeout }
        return try await send(req)
    }
    func runs() async throws -> [RunSummary] { try await get("runs") }
    func run(_ runID: String) async throws -> RunDetail { try await get("runs", runID) }

    func posts(runID: String, sinceID: Int = 0) async throws -> [PostOut] {
        try await get("runs", runID, "posts", query: [URLQueryItem(name: "since_id", value: String(max(0, sinceID)))])
    }

    func actions(runID: String, round: Int? = nil) async throws -> [ActionOut] {
        try await get("runs", runID, "actions", query: round.map { [URLQueryItem(name: "round", value: String($0))] } ?? [])
    }

    func agent(runID: String, agentID: String) async throws -> AgentDetail {
        try await get("runs", runID, "agents", agentID)
    }

    /// Starts a run; returns its id.
    func createRun(_ request: NewRunRequest) async throws -> String {
        var form = MultipartForm()
        form.add("question", request.question)
        form.add("agents", String(request.agents))
        form.add("rounds", String(request.rounds))
        form.add("batch", String(request.batch))
        form.add("agent_model", request.agentModel)
        form.add("planner_model", request.plannerModel)
        form.add("agent_runtime", request.agentRuntime)
        form.add("planner_runtime", request.plannerRuntime)
        form.add("concurrency", String(request.concurrency))
        form.add("language", request.language)
        form.add("think", request.think ? "true" : "false")
        form.add("seed", String(request.seed))
        if let castFrom = request.castFrom { form.add("cast_from", castFrom) }
        if let studyID = request.studyID { form.add("study_id", studyID) }
        if let variant = request.variant { form.add("variant", variant) }
        if request.reviewCast { form.add("review_cast", "true") }
        if request.fairTurns { form.add("fair_turns", "true") }
        if let audience = request.audience { form.add("audience", try audience.formJSON()) }
        let events = request.events.filter { !$0.content.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
        if !events.isEmpty {
            let data = try Self.encoder.encode(events)
            form.add("events", String(decoding: data, as: UTF8.self))
        }
        for url in request.files {
            let data: Data
            do {
                data = try Data(contentsOf: url, options: .mappedIfSafe)
            } catch {
                throw TellfoldAPIError(status: 0, message: "Could not read \(url.lastPathComponent): \(error.localizedDescription)")
            }
            if data.count > NewRunRequest.maxFileBytes {
                throw TellfoldAPIError(status: 413, message: "\(url.lastPathComponent) is larger than 20 MB")
            }
            form.addFile("files[]", filename: url.lastPathComponent, contentType: Self.mimeType(for: url), data: data)
        }
        for doc in request.inlineDocuments {
            form.addFile("files[]", filename: doc.filename, contentType: "text/markdown", data: doc.data)
        }
        try Self.addMaterial(request.material, hide: request.materialHide, to: &form)
        var req = makeRequest(["runs"], method: "POST")
        req.setValue(form.contentType, forHTTPHeaderField: "Content-Type")
        req.httpBody = form.finalize()
        let created: CreatedRun = try await send(req)
        return created.id
    }

    /// What a run with this audience material would send (`POST /api/material/preview`): the items after redaction,
    /// what was hidden, and warnings. The crowd size is not sent: the sheet words the note about too few items itself.
    func previewMaterial(_ material: [MaterialInput], hide: [String]) async throws -> MaterialPreview {
        var form = MultipartForm()
        try Self.addMaterial(material, hide: hide, to: &form)
        var req = makeRequest(["material", "preview"], method: "POST")
        req.setValue(form.contentType, forHTTPHeaderField: "Content-Type")
        req.httpBody = form.finalize()
        return try await send(req)
    }

    /// The `material` file parts, `material_spec` and `material_hide` of a form.
    private static func addMaterial(_ material: [MaterialInput], hide: [String], to form: inout MultipartForm) throws {
        guard !material.isEmpty else { return }
        struct Spec: Encodable { let kind: String; let column: String? }
        for input in material {
            let data: Data
            do {
                data = try Data(contentsOf: input.url, options: .mappedIfSafe)
            } catch {
                throw TellfoldAPIError(status: 0, message: "Could not read \(input.url.lastPathComponent): \(error.localizedDescription)")
            }
            if data.count > MaterialLimits.maxFileBytes {
                throw TellfoldAPIError(status: 413, message: "\(input.url.lastPathComponent) is larger than 10 MB")
            }
            form.addFile("material", filename: input.url.lastPathComponent, contentType: "text/plain", data: data)
        }
        let specs = material.map { Spec(kind: $0.kind.rawValue, column: $0.column) }
        form.add("material_spec", String(decoding: try encoder.encode(specs), as: UTF8.self))
        if !hide.isEmpty { form.add("material_hide", hide.joined(separator: "\n")) }
    }

    /// The seed documents saved with a run (`GET /api/runs/{id}/seeds`); empty for runs made before the engine kept them.
    func seeds(runID: String) async throws -> [SeedInfo] { try await get("runs", runID, "seeds") }

    /// One saved seed document's bytes.
    func seedData(runID: String, name: String) async throws -> Data {
        try await sendRaw(makeRequest(["runs", runID, "seeds", name]))
    }

    func injectEvent(runID: String, round: Int, content: String) async throws {
        let _: OkResponse = try await post(["runs", runID, "events"], body: GodEvent(round: round, content: content))
    }

    func stop(runID: String) async throws {
        let _: OkResponse = try await post(["runs", runID, "stop"], body: Optional<String>.none)
    }

    func chat(runID: String, agentID: String, message: String, history: [ChatTurn]) async throws -> String {
        struct Body: Encodable { let agent_id: String; let message: String; let history: [ChatTurn] }
        let r: ReplyResponse = try await post(["runs", runID, "chat"], body: Body(agent_id: agentID, message: message, history: history))
        return r.reply
    }

    func ask(runID: String, question: String) async throws -> String {
        struct Body: Encodable { let question: String }
        let r: ReplyResponse = try await post(["runs", runID, "ask"], body: Body(question: question))
        return r.reply
    }

    // MARK: The cast of a run waiting at personas_ready

    /// The cast under review while the run waits, else the run's people.
    func cast(runID: String) async throws -> CastResponse { try await get("runs", runID, "personas") }

    /// Replaces the waiting cast; returns it as the engine tidied it (ids renumbered, handles made unique).
    func replaceCast(runID: String, personas: [Persona]) async throws -> [Persona] {
        struct Body: Encodable { let personas: [Persona] }
        var req = makeRequest(["runs", runID, "personas"], method: "PUT")
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.httpBody = try Self.encoder.encode(Body(personas: personas))
        let r: CastResponse = try await send(req)
        return r.personas
    }

    /// Lets the waiting run go on with its cast.
    func confirmCast(runID: String) async throws {
        let _: OkResponse = try await post(["runs", runID, "personas", "confirm"], body: Optional<String>.none)
    }

    /// The engine writes `count` people for a group described in words and adds them; returns the whole cast.
    func addGroup(runID: String, description: String, count: Int) async throws -> [Persona] {
        struct Body: Encodable { let description: String; let count: Int }
        let r: CastResponse = try await post(["runs", runID, "personas", "group"],
                                             body: Body(description: description, count: count))
        return r.personas
    }

    func rebuildReport(runID: String) async throws {
        let _: OkResponse = try await post(["runs", runID, "report"], body: Optional<String>.none)
    }

    /// The generated `report.html` (the endpoint needs the bearer token, so a browser cannot open it directly).
    func reportHTML(runID: String) async throws -> Data {
        try await sendRaw(makeRequest(["runs", runID, "report.html"]))
    }

    /// Downloads the finished run as one `.tellfold` file (`GET /api/runs/{id}/bundle`) and moves it to `destination`,
    /// replacing a file there. The bundle goes to disk as it arrives; it is never held in memory.
    func downloadBundle(runID: String, to destination: URL) async throws {
        var req = makeRequest(["runs", runID, "bundle"])
        req.timeoutInterval = 300
        let (temp, http) = try await transfer { try await session.download(for: req) }
        defer { try? FileManager.default.removeItem(at: temp) }
        guard (200..<300).contains(http.statusCode) else {
            throw TellfoldAPIError.from(status: http.statusCode, body: (try? Data(contentsOf: temp)) ?? Data())
        }
        do {
            _ = try FileManager.default.replaceItemAt(destination, withItemAt: temp)
        } catch {
            throw RunBundle.FileError(message: "Could not save \(destination.lastPathComponent): \(error.localizedDescription)")
        }
    }

    /// Sends the `.tellfold` file at `file` as a new, read-only run (`POST /api/runs/import`), streamed from disk;
    /// returns the new run's id.
    func importBundle(fileAt file: URL) async throws -> String {
        var req = makeRequest(["runs", "import"], method: "POST")
        req.setValue(RunBundle.mediaType, forHTTPHeaderField: "Content-Type")
        req.timeoutInterval = 300
        let (data, http) = try await transfer { try await session.upload(for: req, fromFile: file) }
        guard (200..<300).contains(http.statusCode) else { throw TellfoldAPIError.from(status: http.statusCode, body: data) }
        do {
            return try Self.decoder.decode(CreatedRun.self, from: data).id
        } catch {
            throw TellfoldAPIError(status: 0, message: "Unexpected response from the engine: \(error)")
        }
    }

    // MARK: Studies

    func studies() async throws -> [StudySummary] { try await get("studies") }
    func study(_ studyID: String) async throws -> StudyDetail { try await get("studies", studyID) }

    /// Continues a copy of a run from the end of `round` with `events` after it; returns the new run and its study.
    func branch(runID: String, round: Int, events: [GodEvent]) async throws -> (id: String, studyID: String) {
        struct Body: Encodable { let round: Int; let events: [GodEvent] }
        struct Created: Decodable, Sendable { let id: String; let study_id: String }
        let r: Created = try await post(["runs", runID, "branch"], body: Body(round: round, events: events))
        return (r.id, r.study_id)
    }

    /// Suggests a better version of the run's text (one planner call; saved as rewrite.json).
    func rewrite(runID: String) async throws -> RewriteSuggestion {
        var req = makeRequest(["runs", runID, "rewrite"], method: "POST")
        req.timeoutInterval = 600
        return try await send(req)
    }

    /// The suggestion saved earlier; a 404 error when there is none.
    func savedRewrite(runID: String) async throws -> RewriteSuggestion { try await get("runs", runID, "rewrite") }

    /// "Check it": runs a finished run again `runs - 1` times with the same people and text; returns the study id.
    func checkRun(runID: String, runs: Int, agentModel: String? = nil) async throws -> String {
        struct Body: Encodable { let runs: Int; let agent_model: String? }
        struct Created: Decodable, Sendable { let study_id: String }
        let r: Created = try await post(["runs", runID, "check"], body: Body(runs: runs, agent_model: agentModel))
        return r.study_id
    }

    /// Writes (or rewrites) the study's comparison in the background.
    func compareStudy(_ studyID: String) async throws {
        let _: OkResponse = try await post(["studies", studyID, "compare"], body: Optional<String>.none)
    }

    /// Live events for one run. Iterate it in a task; cancelling the task closes the connection.
    nonisolated func eventStream(runID: String) -> AsyncStream<ServerEvent> {
        EventStream(url: url(["runs", runID, "stream"]), token: token, session: session).events()
    }

    // MARK: Runtimes and settings

    func runtimes(checkAuth: Bool = false) async throws -> [RuntimeInfo] {
        try await get("runtimes", query: [URLQueryItem(name: "check_auth", value: checkAuth ? "1" : "0")])
    }

    func testRuntime(id: String) async throws -> RuntimeTestResult {
        var req = makeRequest(["runtimes", id, "test"], method: "POST")
        req.timeoutInterval = 300
        return try await send(req)
    }

    func settings() async throws -> EngineSettings { try await get("settings") }

    func updateSettings(_ settings: EngineSettings) async throws -> EngineSettings {
        var req = makeRequest(["settings"], method: "PUT")
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.httpBody = try Self.encoder.encode(settings)
        return try await send(req)
    }

    // MARK: Plumbing

    nonisolated func url(_ components: [String], query: [URLQueryItem] = []) -> URL {
        var url = baseURL.appending(path: "api")
        for component in components {
            url.append(path: component, directoryHint: .notDirectory)
        }
        if !query.isEmpty { url.append(queryItems: query) }
        return url
    }

    private func makeRequest(_ components: [String], method: String = "GET", query: [URLQueryItem] = []) -> URLRequest {
        var req = URLRequest(url: url(components, query: query))
        req.httpMethod = method
        req.setValue("application/json", forHTTPHeaderField: "Accept")
        if let token { req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization") }
        req.timeoutInterval = method == "GET" ? 30 : 240
        return req
    }

    private func get<T: Decodable & Sendable>(_ components: String..., query: [URLQueryItem] = []) async throws -> T {
        try await send(makeRequest(components, query: query))
    }

    private func post<T: Decodable & Sendable, B: Encodable>(_ components: [String], body: B?) async throws -> T {
        var req = makeRequest(components, method: "POST")
        if let body {
            req.setValue("application/json", forHTTPHeaderField: "Content-Type")
            req.httpBody = try Self.encoder.encode(body)
        }
        return try await send(req)
    }

    private func send<T: Decodable & Sendable>(_ request: URLRequest) async throws -> T {
        let data = try await sendRaw(request)
        do {
            return try Self.decoder.decode(T.self, from: data)
        } catch {
            throw TellfoldAPIError(status: 0, message: "Unexpected response from the engine: \(error)")
        }
    }

    private func sendRaw(_ request: URLRequest) async throws -> Data {
        let (data, http) = try await transfer { try await session.data(for: request) }
        guard (200..<300).contains(http.statusCode) else {
            throw TellfoldAPIError.from(status: http.statusCode, body: data)
        }
        return data
    }

    /// One URLSession task, with transport failures as `TellfoldAPIError` (cancellation stays a `CancellationError`).
    /// The status is left to the caller.
    private func transfer<R>(_ task: () async throws -> (R, URLResponse)) async throws -> (R, HTTPURLResponse) {
        let result: R
        let response: URLResponse
        do {
            (result, response) = try await task()
        } catch let error as URLError where error.code == .cancelled {
            throw CancellationError()
        } catch {
            throw TellfoldAPIError(status: 0, message: "The engine is not reachable (\(error.localizedDescription)).")
        }
        guard let http = response as? HTTPURLResponse else {
            throw TellfoldAPIError(status: 0, message: "No HTTP response from the engine.")
        }
        return (result, http)
    }

    static func mimeType(for url: URL) -> String {
        switch url.pathExtension.lowercased() {
        case "md", "markdown": return "text/markdown"
        case "txt": return "text/plain"
        case "pdf": return "application/pdf"
        default: return UTType(filenameExtension: url.pathExtension)?.preferredMIMEType ?? "application/octet-stream"
        }
    }
}

/// A minimal `multipart/form-data` body builder.
struct MultipartForm: Sendable {
    let boundary: String
    private var body = Data()

    init(boundary: String = "tellfold-\(UUID().uuidString)") {
        self.boundary = boundary
    }

    var contentType: String { "multipart/form-data; boundary=\(boundary)" }

    mutating func add(_ name: String, _ value: String) {
        body.append(Data("--\(boundary)\r\n".utf8))
        body.append(Data("Content-Disposition: form-data; name=\"\(Self.escape(name))\"\r\n\r\n".utf8))
        body.append(Data(value.utf8))
        body.append(Data("\r\n".utf8))
    }

    mutating func addFile(_ name: String, filename: String, contentType: String, data: Data) {
        body.append(Data("--\(boundary)\r\n".utf8))
        body.append(Data("Content-Disposition: form-data; name=\"\(Self.escape(name))\"; filename=\"\(Self.escape(filename))\"\r\n".utf8))
        body.append(Data("Content-Type: \(contentType)\r\n\r\n".utf8))
        body.append(data)
        body.append(Data("\r\n".utf8))
    }

    func finalize() -> Data {
        var out = body
        out.append(Data("--\(boundary)--\r\n".utf8))
        return out
    }

    private static func escape(_ s: String) -> String {
        s.replacingOccurrences(of: "\r", with: " ").replacingOccurrences(of: "\n", with: " ")
            .replacingOccurrences(of: "\"", with: "%22")
    }
}
