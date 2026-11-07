import Foundation
import Synchronization
import Testing
@testable import Tellfold

/// A URLProtocol that answers from per-session handlers, recording each request.
final class StubURLProtocol: URLProtocol, @unchecked Sendable {
    struct Reply: Sendable {
        var status: Int = 200
        var headers: [String: String] = ["Content-Type": "application/json"]
        var body: Data
    }

    typealias Handler = @Sendable (URLRequest) -> Reply

    private static let handlers = Mutex<[String: Handler]>([:])
    private static let recorded = Mutex<[String: [URLRequest]]>([:])

    /// A session whose requests go to `handler`. Requests carry an `X-Stub` header naming the handler.
    static func session(_ handler: @escaping Handler) -> (URLSession, String) {
        let key = UUID().uuidString
        handlers.withLock { $0[key] = handler }
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [StubURLProtocol.self]
        config.httpAdditionalHeaders = ["X-Stub": key]
        return (URLSession(configuration: config), key)
    }

    static func requests(_ key: String) -> [URLRequest] { recorded.withLock { $0[key] ?? [] } }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        let key = request.value(forHTTPHeaderField: "X-Stub") ?? ""
        var request = self.request
        if request.httpBody == nil, let stream = request.httpBodyStream {
            request.httpBody = Self.read(stream)
        }
        Self.recorded.withLock { $0[key, default: []].append(request) }
        guard let handler = Self.handlers.withLock({ $0[key] }) else {
            client?.urlProtocol(self, didFailWithError: URLError(.cannotConnectToHost))
            return
        }
        let reply = handler(request)
        let response = HTTPURLResponse(url: request.url!, statusCode: reply.status, httpVersion: "HTTP/1.1", headerFields: reply.headers)!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: reply.body)
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {}

    private static func read(_ stream: InputStream) -> Data {
        stream.open()
        defer { stream.close() }
        var data = Data()
        var buffer = [UInt8](repeating: 0, count: 16_384)
        while stream.hasBytesAvailable {
            let n = stream.read(&buffer, maxLength: buffer.count)
            if n <= 0 { break }
            data.append(buffer, count: n)
        }
        return data
    }
}

private final class CallCounter: Sendable {
    private let count = Mutex(0)
    func next() -> Int { count.withLock { $0 += 1; return $0 } }
}

struct TellfoldAPITests {
    @Test func sendsBearerTokenAndDecodes() async throws {
        let runs = try Fixtures.data("runs.json")
        let (session, key) = StubURLProtocol.session { _ in .init(body: runs) }
        let api = TellfoldAPI(port: 4242, token: "secret", session: session)
        let list = try await api.runs()
        #expect(list.count == 1)
        let request = try #require(StubURLProtocol.requests(key).first)
        #expect(request.url?.absoluteString == "http://127.0.0.1:4242/api/runs")
        #expect(request.value(forHTTPHeaderField: "Authorization") == "Bearer secret")
        #expect(request.httpMethod == "GET")
    }

    @Test func buildsPathsAndQueries() async throws {
        let posts = try Fixtures.data("posts.json")
        let actions = try Fixtures.data("actions.json")
        let (session, key) = StubURLProtocol.session { req in
            .init(body: req.url!.path.hasSuffix("/posts") ? posts : actions)
        }
        let api = TellfoldAPI(port: 1, token: nil, session: session)
        _ = try await api.posts(runID: "20261107-040541-x", sinceID: 5)
        _ = try await api.actions(runID: "20261107-040541-x", round: 2)
        _ = try await api.actions(runID: "20261107-040541-x")
        let urls = StubURLProtocol.requests(key).map { $0.url!.absoluteString }
        #expect(urls == [
            "http://127.0.0.1:1/api/runs/20261107-040541-x/posts?since_id=5",
            "http://127.0.0.1:1/api/runs/20261107-040541-x/actions?round=2",
            "http://127.0.0.1:1/api/runs/20261107-040541-x/actions",
        ])
        #expect(StubURLProtocol.requests(key).allSatisfy { $0.value(forHTTPHeaderField: "Authorization") == nil })
    }

    @Test func postsJSONBodies() async throws {
        let (session, key) = StubURLProtocol.session { req in
            switch req.url!.lastPathComponent {
            case "chat": .init(body: Data(#"{"reply":"hi"}"#.utf8))
            default: .init(body: Data(#"{"ok":true}"#.utf8))
            }
        }
        let api = TellfoldAPI(port: 1, token: "t", session: session)
        let reply = try await api.chat(runID: "r", agentID: "a01", message: "Why?", history: [ChatTurn(question: "q", answer: "a")])
        #expect(reply == "hi")
        try await api.injectEvent(runID: "r", round: 3, content: "News")
        try await api.stop(runID: "r")
        let requests = StubURLProtocol.requests(key)
        #expect(requests.map(\.httpMethod) == ["POST", "POST", "POST"])
        let chatBody = try JSONDecoder().decode([String: JSONValue].self, from: try #require(requests[0].httpBody))
        #expect(chatBody["agent_id"] == .string("a01"))
        #expect(chatBody["history"] == .array([.array([.string("q"), .string("a")])]))
        let eventBody = try JSONDecoder().decode([String: JSONValue].self, from: try #require(requests[1].httpBody))
        #expect(eventBody == ["round": .number(3), "content": .string("News")])
    }

    @Test func surfacesServerErrors() async throws {
        let body = try Fixtures.data("stop_conflict.json")
        let (session, _) = StubURLProtocol.session { _ in .init(status: 409, body: body) }
        let api = TellfoldAPI(port: 1, token: "t", session: session)
        await #expect(throws: TellfoldAPIError(status: 409, message: "only a run that is live in this server can be stopped")) {
            try await api.stop(runID: "r")
        }
    }

    @Test func createRunSendsMultipartForm() async throws {
        let dir = URL.temporaryDirectory.appending(path: "tellfold-tests-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let seed = dir.appending(path: "seed notes.md")
        try Data("# Seed\nHello".utf8).write(to: seed)

        let (session, key) = StubURLProtocol.session { _ in .init(body: Data(#"{"id":"20261107-1"}"#.utf8)) }
        let api = TellfoldAPI(port: 1, token: "t", session: session)
        var request = NewRunRequest(files: [seed], question: "Will it work?")
        request.agentRuntime = "cursor-agent"
        request.think = true
        request.events = [GodEvent(round: 2, content: "News"), GodEvent(round: 3, content: "  ")]
        let id = try await api.createRun(request)
        #expect(id == "20261107-1")

        let sent = try #require(StubURLProtocol.requests(key).first)
        let contentType = try #require(sent.value(forHTTPHeaderField: "Content-Type"))
        #expect(contentType.hasPrefix("multipart/form-data; boundary="))
        let body = String(decoding: try #require(sent.httpBody), as: UTF8.self)
        #expect(body.contains("name=\"question\"\r\n\r\nWill it work?\r\n"))
        #expect(body.contains("name=\"agent_runtime\"\r\n\r\ncursor-agent\r\n"))
        #expect(body.contains("name=\"think\"\r\n\r\ntrue\r\n"))
        #expect(body.contains(#"[{"content":"News","round":2}]"#))
        #expect(body.contains("name=\"files[]\"; filename=\"seed notes.md\"\r\nContent-Type: text/markdown\r\n\r\n# Seed\nHello\r\n"))
        #expect(body.hasSuffix("--\r\n"))
    }

    @Test func missingRuntimesEndpointThrowsNotFound() async throws {
        let (session, _) = StubURLProtocol.session { _ in .init(status: 404, body: Data(#"{"detail":"not found"}"#.utf8)) }
        let api = TellfoldAPI(port: 1, token: "t", session: session)
        await #expect(throws: TellfoldAPIError(status: 404, message: "not found")) { try await api.runtimes() }
    }

    @Test func eventStreamParsesAndFinishesOnTerminalEvent() async throws {
        let sse = try Fixtures.data("stream_tail.sse")
        let (session, key) = StubURLProtocol.session { _ in
            .init(headers: ["Content-Type": "text/event-stream"], body: sse)
        }
        let api = TellfoldAPI(port: 9, token: "tok", session: session)
        var names: [String] = []
        for await event in api.eventStream(runID: "r1") { names.append(event.name) }
        // The stream ends at the first terminal event: the `status` that says done.
        #expect(names == ["status", "posts_added", "round_done", "status", "status", "stage", "status"])
        let request = try #require(StubURLProtocol.requests(key).first)
        #expect(request.url?.path == "/api/runs/r1/stream")
        #expect(request.value(forHTTPHeaderField: "Authorization") == "Bearer tok")
        #expect(request.value(forHTTPHeaderField: "Accept") == "text/event-stream")
    }

    @Test func eventStreamGivesUpOnClientErrors() async throws {
        let (session, key) = StubURLProtocol.session { _ in .init(status: 401, body: Data(#"{"detail":"unauthorized"}"#.utf8)) }
        let stream = EventStream(url: URL(string: "http://127.0.0.1:1/api/runs/x/stream")!, token: "bad", session: session).events()
        var count = 0
        for await _ in stream { count += 1 }
        #expect(count == 0)
        #expect(StubURLProtocol.requests(key).count == 1)
    }

    @Test func eventStreamReconnectsAfterServerClose() async throws {
        let calls = CallCounter()
        let (session, key) = StubURLProtocol.session { _ in
            let n = calls.next()
            let body = n == 1 ? "event: stage\ndata: {\"stage\":\"rounds\",\"detail\":\"r1\"}\n\n" : "event: run_stopped\ndata: {}\n\n"
            return .init(headers: ["Content-Type": "text/event-stream"], body: Data(body.utf8))
        }
        var stream = EventStream(url: URL(string: "http://127.0.0.1:1/api/runs/x/stream")!, token: nil, session: session)
        stream.backoff = [0.01]
        var events: [ServerEvent] = []
        for await e in stream.events() { events.append(e) }
        #expect(events == [.stage(StageEvent(stage: .rounds, detail: "r1")), .runStopped])
        #expect(StubURLProtocol.requests(key).count == 2)
    }
}
