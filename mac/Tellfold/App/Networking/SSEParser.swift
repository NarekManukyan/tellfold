import Foundation

/// One dispatched Server-Sent Event.
struct SSEMessage: Hashable, Sendable {
    var event: String
    var data: String
    var id: String?
}

/// Incremental parser for the `text/event-stream` format (WHATWG HTML, "Server-sent events").
///
/// Feed it raw bytes in any chunking; it buffers partial lines, accepts `\n`, `\r\n` and `\r` line endings,
/// ignores `:` comment lines (the server's `: ping`), joins multi-line `data:` fields with `\n`, and dispatches
/// on a blank line. An event without a name is `message`.
struct SSEParser: Sendable {
    private var lineBuffer: [UInt8] = []
    private var lastWasCR = false
    private var eventName = ""
    private var dataLines: [String] = []
    private var lastEventID: String?
    private var sawData = false

    init() {}

    /// Feed raw bytes; returns every event completed by them.
    mutating func feed<S: Sequence>(_ bytes: S) -> [SSEMessage] where S.Element == UInt8 {
        var out: [SSEMessage] = []
        for byte in bytes {
            if let message = feed(byte: byte) { out.append(message) }
        }
        return out
    }

    /// Feed one byte; returns an event when this byte completes one.
    mutating func feed(byte: UInt8) -> SSEMessage? {
        switch byte {
        case 0x0A: // \n
            if lastWasCR {
                lastWasCR = false
                return nil // second half of \r\n
            }
            return endLine()
        case 0x0D: // \r
            lastWasCR = true
            return endLine()
        default:
            lastWasCR = false
            lineBuffer.append(byte)
            return nil
        }
    }

    /// Feed one complete line (without its terminator).
    mutating func feed(line: String) -> SSEMessage? {
        process(line: line)
    }

    /// Feed a string chunk (convenience for tests).
    mutating func feed(_ text: String) -> [SSEMessage] {
        feed(Array(text.utf8))
    }

    private mutating func endLine() -> SSEMessage? {
        let line = String(decoding: lineBuffer, as: UTF8.self)
        lineBuffer.removeAll(keepingCapacity: true)
        return process(line: line)
    }

    private mutating func process(line: String) -> SSEMessage? {
        if line.isEmpty { return dispatch() }
        if line.hasPrefix(":") { return nil }
        let field: Substring
        var value: Substring
        if let colon = line.firstIndex(of: ":") {
            field = line[..<colon]
            value = line[line.index(after: colon)...]
            if value.hasPrefix(" ") { value = value.dropFirst() }
        } else {
            field = Substring(line)
            value = ""
        }
        switch field {
        case "event": eventName = String(value)
        case "data":
            dataLines.append(String(value))
            sawData = true
        case "id":
            if !value.contains("\0") { lastEventID = String(value) }
        default: break // "retry" and unknown fields are ignored
        }
        return nil
    }

    private mutating func dispatch() -> SSEMessage? {
        defer {
            eventName = ""
            dataLines.removeAll()
            sawData = false
        }
        guard sawData else { return nil }
        return SSEMessage(event: eventName.isEmpty ? "message" : eventName,
                          data: dataLines.joined(separator: "\n"), id: lastEventID)
    }
}
