import Foundation
import Synchronization

/// Pure helpers for launching the engine: command line, port-line parsing, `uv` discovery and environment.
/// Kept free of process state so they can be unit-tested.
enum EngineCommand {
    /// The environment variable `tellfold serve` reads its bearer token from (same as `--token`, but not visible
    /// to other users in `ps`).
    static let tokenEnvironmentKey = "TELLFOLD_SERVE_TOKEN"

    /// `uv run --project <enginePath> tellfold serve --port 0 --runs <runsFolder> --log-level <level>` (after `uv`).
    /// The token goes in through `TELLFOLD_SERVE_TOKEN` (see `environment`), equivalent to `--token <token>`.
    static func arguments(enginePath: String, runsFolder: String, logLevel: String) -> [String] {
        ["run", "--project", enginePath] + serveArguments(runsFolder: runsFolder, logLevel: logLevel)
    }

    /// The engine built into the app: `<engine>/python/bin/python3 -I -B -m tellfold serve ...` (after the
    /// executable). `-I` keeps the user's PYTHON* variables and site-packages out; `-B` never writes bytecode into
    /// the signed bundle (it is compiled at build time).
    static func bundledArguments(runsFolder: String, logLevel: String) -> [String] {
        ["-I", "-B", "-m"] + serveArguments(runsFolder: runsFolder, logLevel: logLevel)
    }

    /// The Python inside a bundled engine folder (`Resources/engine`).
    static func bundledPython(in engine: URL) -> URL {
        engine.appending(path: "python/bin/python3")
    }

    private static func serveArguments(runsFolder: String, logLevel: String) -> [String] {
        ["tellfold", "serve", "--port", "0", "--runs", runsFolder, "--log-level", logLevel.lowercased()]
    }

    /// The engine prints exactly one JSON line `{"port": N}` to stdout once it is listening. Returns N for that
    /// line (whitespace tolerated, other keys ignored) and nil for anything else.
    static func parsePort(fromLine line: String) -> Int? {
        let trimmed = line.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.hasPrefix("{"), trimmed.hasSuffix("}"),
              let object = try? JSONDecoder().decode([String: JSONValue].self, from: Data(trimmed.utf8)),
              let value = object["port"], case .number(let n) = value,
              n.rounded() == n, (1...65_535).contains(n) else { return nil }
        return Int(n)
    }

    /// A random 256-bit token, hex encoded.
    static func makeToken() -> String {
        var generator = SystemRandomNumberGenerator()
        return (0..<32).map { _ in String(format: "%02x", UInt8.random(in: .min ... .max, using: &generator)) }.joined()
    }

    /// The command line for display, with the token hidden.
    static func displayCommand(executable: String, arguments: [String], token: String) -> String {
        ([executable] + arguments).map { $0 == token ? "•••" : quoteIfNeeded($0) }.joined(separator: " ")
    }

    private static func quoteIfNeeded(_ s: String) -> String {
        s.contains(where: { $0 == " " || $0 == "'" || $0 == "\"" }) ? "'\(s.replacingOccurrences(of: "'", with: "'\\''"))'" : s
    }

    /// Directories GUI apps do not get in their PATH but where developer tools usually live.
    static func extraPathDirectories(home: String) -> [String] {
        ["\(home)/.local/bin", "\(home)/.cargo/bin", "/opt/homebrew/bin", "/opt/homebrew/sbin", "/usr/local/bin",
         "\(home)/.bun/bin", "\(home)/.npm-global/bin", "\(home)/.volta/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin"]
    }

    /// `existing` PATH followed by the extra directories, without duplicates.
    static func searchPath(existing: String?, home: String) -> String {
        var seen = Set<String>()
        var parts: [String] = []
        for dir in (existing ?? "").split(separator: ":").map(String.init) + extraPathDirectories(home: home)
        where !dir.isEmpty && seen.insert(dir).inserted {
            parts.append(dir)
        }
        return parts.joined(separator: ":")
    }

    /// The `uv` binary: the user's explicit choice when it is executable, else the first `uv` on the search path.
    static func findUV(preferred: String?, path: String, isExecutable: (String) -> Bool = FileManager.default.isExecutableFile(atPath:)) -> URL? {
        if let preferred {
            let expanded = (preferred.trimmingCharacters(in: .whitespaces) as NSString).expandingTildeInPath
            if !expanded.isEmpty {
                return isExecutable(expanded) ? URL(filePath: expanded) : nil
            }
        }
        for dir in path.split(separator: ":") {
            let candidate = "\(dir)/uv"
            if isExecutable(candidate) { return URL(filePath: candidate) }
        }
        return nil
    }

    /// The engine's environment: the app's own, a PATH that finds `uv` and the agent CLIs, the bearer token, the
    /// Keychain secrets, and output switches. `TELLFOLD_FAKE_LLM` is set only when `offline` is on.
    static func environment(base: [String: String], token: String, secrets: [String: String], offline: Bool,
                            home: String) -> [String: String] {
        // Debugger and test-runner injection (DYLD_INSERT_LIBRARIES, XCTest bundle paths) must not leak into
        // uv and Python: an injected XCTest loader makes the child wait for a test session forever.
        var env = base.filter { key, _ in !inheritedPrefixesToDrop.contains { key.hasPrefix($0) } }
        env["PATH"] = searchPath(existing: base["PATH"], home: home)
        env["PYTHONUNBUFFERED"] = "1"
        env["NO_COLOR"] = "1"
        env["UV_NO_PROGRESS"] = "1"
        // Names from before the rename to Tellfold: never let a stale inherited value reach the engine.
        for legacy in legacyEnvironmentKeys { env.removeValue(forKey: legacy) }
        env[tokenEnvironmentKey] = token
        if offline { env["TELLFOLD_FAKE_LLM"] = "1" } else { env.removeValue(forKey: "TELLFOLD_FAKE_LLM") }
        for (key, value) in secrets { env[key] = value }
        return env
    }

    static let legacyEnvironmentKeys = ["SHOAL_FAKE_LLM", "SHOAL_SERVE_TOKEN"]

    static let inheritedPrefixesToDrop = ["DYLD_", "__XPC_DYLD_", "XCTest", "XCInject", "__XCODE_"]

    /// Restart delay after `attempt` consecutive failures (1-based): 1, 2, 4, 8, 16, 30, 30 ... seconds.
    static func backoffSeconds(attempt: Int) -> Int {
        guard attempt > 1 else { return 1 }
        return min(30, 1 << min(attempt - 1, 5))
    }
}

/// Splits a byte stream into lines (`\n` or `\r\n`), buffering a partial last line.
struct LineSplitter: Sendable {
    private var buffer: [UInt8] = []

    init() {}

    mutating func append(_ data: some Sequence<UInt8>) -> [String] {
        var lines: [String] = []
        for byte in data {
            if byte == 0x0A {
                if buffer.last == 0x0D { buffer.removeLast() }
                lines.append(String(decoding: buffer, as: UTF8.self))
                buffer.removeAll(keepingCapacity: true)
            } else {
                buffer.append(byte)
            }
        }
        return lines
    }

    /// The unterminated rest, if any.
    mutating func flush() -> String? {
        defer { buffer.removeAll() }
        return buffer.isEmpty ? nil : String(decoding: buffer, as: UTF8.self)
    }
}

/// Lines from a pipe as they arrive. Uses `readabilityHandler`, because `FileHandle.bytes` on a pipe can hold
/// data back until a large buffer fills, which would delay the engine's one-line port announcement.
enum PipeLineReader {
    private final class State: Sendable {
        let splitter = Mutex(LineSplitter())
    }

    static func lines(of handle: FileHandle) -> AsyncStream<String> {
        AsyncStream { continuation in
            let state = State()
            handle.readabilityHandler = { fh in
                let data = fh.availableData
                if data.isEmpty { // EOF
                    fh.readabilityHandler = nil
                    if let rest = state.splitter.withLock({ $0.flush() }) { continuation.yield(rest) }
                    continuation.finish()
                    return
                }
                for line in state.splitter.withLock({ $0.append(data) }) { continuation.yield(line) }
            }
            continuation.onTermination = { _ in handle.readabilityHandler = nil }
        }
    }
}
