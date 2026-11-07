import Foundation
import Testing
@testable import Shoal

struct SSEParserTests {
    @Test func parsesNamedEventsAndIgnoresComments() {
        var parser = SSEParser()
        let messages = parser.feed(": connected\n\nevent: status\ndata: {\"a\":1}\n\n: ping\n\nevent: run_done\ndata: {}\n\n")
        #expect(messages == [
            SSEMessage(event: "status", data: #"{"a":1}"#, id: nil),
            SSEMessage(event: "run_done", data: "{}", id: nil),
        ])
    }

    @Test func handlesArbitraryChunkingAndCRLF() {
        let text = "event: stage\r\ndata: {\"stage\":\"graph\",\r\ndata: \"detail\":\"x\"}\r\n\r\nevent: run_stopped\rdata: {}\r\r"
        let bytes = Array(text.utf8)
        for chunkSize in [1, 2, 3, 7, 64] {
            var parser = SSEParser()
            var out: [SSEMessage] = []
            var i = 0
            while i < bytes.count {
                out += parser.feed(bytes[i..<min(i + chunkSize, bytes.count)])
                i += chunkSize
            }
            #expect(out.count == 2, "chunk size \(chunkSize)")
            #expect(out.first?.event == "stage")
            #expect(out.first?.data == "{\"stage\":\"graph\",\n\"detail\":\"x\"}")
            #expect(out.last?.event == "run_stopped")
        }
    }

    @Test func defaultsAndEdgeCases() {
        var parser = SSEParser()
        // no event name -> "message"; "data" without colon -> empty line of data; event without data is dropped
        let out = parser.feed("data: hello\n\nevent: lonely\n\ndata\ndata:  two spaces\nid: 7\n\n")
        #expect(out == [
            SSEMessage(event: "message", data: "hello", id: nil),
            SSEMessage(event: "message", data: "\n two spaces", id: "7"),
        ])
    }

    @Test func multibyteUTF8SplitAcrossChunks() {
        let text = "event: event_injected\ndata: {\"round\":2,\"content\":\"Prix réduit — 50 %\"}\n\n"
        let bytes = Array(text.utf8)
        var parser = SSEParser()
        var out: [SSEMessage] = []
        for b in bytes { out += parser.feed([b]) }
        #expect(out.count == 1)
        guard case .eventInjected(let e) = ServerEvent.decode(name: out[0].event, data: out[0].data) else {
            Issue.record("expected event_injected")
            return
        }
        #expect(e == GodEvent(round: 2, content: "Prix réduit — 50 %"))
    }

    @Test func capturedStreamDecodesEveryEvent() throws {
        var parser = SSEParser()
        let messages = parser.feed(try Fixtures.string("stream_full.sse"))
        let events = messages.map { ServerEvent.decode(name: $0.event, data: $0.data) }
        #expect(events.count > 20)
        let unknown = events.filter { if case .unknown = $0 { true } else { false } }
        #expect(unknown.isEmpty, "undecoded: \(unknown)")
        guard case .status(let first) = events.first else {
            Issue.record("the first event must be a status")
            return
        }
        #expect(first.state == .running)
        let names = Set(events.map(\.name))
        for name in ["status", "stage", "round_start", "round_done", "posts_added", "run_done"] {
            #expect(names.contains(name), "missing \(name)")
        }
        #expect(events.last == .runDone)
        let roundStarts = events.compactMap { if case .roundStart(let e) = $0 { e } else { nil } }
        #expect(roundStarts.map(\.round) == [1, 2, 3, 4])
        #expect(roundStarts.allSatisfy { $0.active == $0.activeAgents.count && $0.totalRounds == 4 })
        let done = events.compactMap { if case .roundDone(let e) = $0 { e } else { nil } }
        #expect(done.map(\.round) == [0, 1, 2, 3, 4])
        #expect(done.allSatisfy { $0.totalRounds == 4 })
        let added = events.compactMap { if case .postsAdded(let e) = $0 { e } else { nil } }
        #expect(added.allSatisfy { !$0.postIDs.isEmpty })
    }

    @Test func tailFixtureEndsWithTerminalEvents() throws {
        var parser = SSEParser()
        let events = parser.feed(try Fixtures.string("stream_tail.sse")).map { ServerEvent.decode(name: $0.event, data: $0.data) }
        #expect(events.last == .runDone)
        #expect(events.contains { if case .status(let s) = $0 { s.state == .done } else { false } })
    }
}

struct ServerEventTests {
    @Test func batchFailedAcceptsIndexOrName() {
        let indexed = ServerEvent.decode(name: "batch_failed", data: #"{"round":2,"batch":3,"agents":["a01"],"error":"timeout"}"#)
        #expect(indexed == .batchFailed(BatchFailedEvent(round: 2, batch: "3", agents: ["a01"], error: "timeout")))
        let named = ServerEvent.decode(name: "batch_failed", data: #"{"round":2,"batch":"sentiment","agents":[],"error":"x"}"#)
        #expect(named == .batchFailed(BatchFailedEvent(round: 2, batch: "sentiment", agents: [], error: "x")))
    }

    @Test func runFailedCarriesError() {
        #expect(ServerEvent.decode(name: "run_failed", data: #"{"error":"boom"}"#) == .runFailed(error: "boom"))
        #expect(ServerEvent.decode(name: "run_failed", data: "{}") == .runFailed(error: "The run failed."))
    }

    @Test func stageWithSeedingStep() {
        let e = ServerEvent.decode(name: "stage", data: #"{"stage":"seeding","detail":"writing opening posts","step":"opening_posts"}"#)
        #expect(e == .stage(StageEvent(stage: .seeding, detail: "writing opening posts", step: "opening_posts")))
    }

    @Test func malformedAndUnknownEvents() {
        #expect(ServerEvent.decode(name: "status", data: "not json") == .unknown(name: "status", data: "not json"))
        #expect(ServerEvent.decode(name: "wat", data: "{}") == .unknown(name: "wat", data: "{}"))
    }

    @Test func terminality() {
        #expect(ServerEvent.runDone.isTerminal)
        #expect(ServerEvent.runStopped.isTerminal)
        #expect(ServerEvent.runFailed(error: "x").isTerminal)
        #expect(!ServerEvent.stage(StageEvent(stage: .rounds, detail: "")).isTerminal)
        let done = RunStatus(state: .done, stage: .finished, totalRounds: 3, startedAt: "", updatedAt: "")
        #expect(ServerEvent.status(done).isTerminal)
    }
}
