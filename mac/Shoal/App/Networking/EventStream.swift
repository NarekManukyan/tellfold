import Foundation
import os

/// A run's live event stream: SSE over `URLSession.bytes`, parsed into `ServerEvent`s, reconnecting with
/// exponential backoff when the connection drops. The stream finishes after a terminal event (`run_done`,
/// `run_failed`, `run_stopped`, or a `status` whose state is terminal), on a 4xx response (unknown run, bad
/// token), or when the consumer stops iterating (the task is cancelled).
struct EventStream: Sendable {
    let url: URL
    let token: String?
    var session: URLSession = .shared
    /// Backoff delays in seconds; the last one repeats.
    var backoff: [Double] = [0.5, 1, 2, 4, 8, 15]
    /// Finish after a terminal event instead of waiting for more.
    var finishOnTerminal = true

    private static let log = Logger(subsystem: "dev.shoal.mac", category: "EventStream")

    func events() -> AsyncStream<ServerEvent> {
        let (stream, continuation) = AsyncStream.makeStream(of: ServerEvent.self, bufferingPolicy: .unbounded)
        let task = Task {
            await run(continuation)
            continuation.finish()
        }
        continuation.onTermination = { _ in task.cancel() }
        return stream
    }

    private enum Outcome { case terminal, retry, giveUp }

    private func run(_ continuation: AsyncStream<ServerEvent>.Continuation) async {
        var attempt = 0
        while !Task.isCancelled {
            let (outcome, receivedAny) = await connectOnce(continuation)
            switch outcome {
            case .terminal, .giveUp: return
            case .retry: break
            }
            if receivedAny { attempt = 0 }
            let delay = backoff[min(attempt, backoff.count - 1)]
            attempt += 1
            do {
                try await Task.sleep(for: .seconds(delay))
            } catch {
                return
            }
        }
    }

    private func connectOnce(_ continuation: AsyncStream<ServerEvent>.Continuation) async -> (Outcome, Bool) {
        var request = URLRequest(url: url)
        request.setValue("text/event-stream", forHTTPHeaderField: "Accept")
        request.setValue("no-cache", forHTTPHeaderField: "Cache-Control")
        if let token { request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization") }
        request.timeoutInterval = 60 // the server writes a ping comment every 15 s
        var receivedAny = false
        do {
            let (bytes, response) = try await session.bytes(for: request)
            if let http = response as? HTTPURLResponse, !(200..<300).contains(http.statusCode) {
                Self.log.error("stream \(url.path, privacy: .public) returned \(http.statusCode)")
                return ((400..<500).contains(http.statusCode) ? .giveUp : .retry, false)
            }
            var parser = SSEParser()
            for try await byte in bytes {
                guard let message = parser.feed(byte: byte) else { continue }
                receivedAny = true
                let event = ServerEvent.decode(name: message.event, data: message.data)
                continuation.yield(event)
                if finishOnTerminal, event.isTerminal { return (.terminal, true) }
            }
            return (.retry, receivedAny) // server closed the stream
        } catch {
            if Task.isCancelled { return (.giveUp, receivedAny) }
            Self.log.debug("stream \(url.path, privacy: .public) dropped: \(error.localizedDescription, privacy: .public)")
            return (.retry, receivedAny)
        }
    }
}
