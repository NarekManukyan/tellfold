import Foundation
import Observation
import os

/// One line of engine output (or an app-side note) for the Diagnostics window.
struct EngineLogLine: Identifiable, Hashable, Sendable {
    enum Source: String, Sendable { case stdout, stderr, app }

    let id: Int
    let date: Date
    let source: Source
    let text: String
}

/// Owns the engine process: `tellfold serve --port 0 --runs <dir> --log-level <level>`, run from the engine binary
/// built into the app, or through `uv run --project <enginePath>` (the Python package, when an engine path is set),
/// with the bearer token in `TELLFOLD_SERVE_TOKEN` (equivalent to `--token`, but hidden from `ps`).
///
/// It reads the `{"port": N}` line from stdout, verifies `/api/health`, then publishes a `TellfoldAPI`. stdin
/// stays open for the life of the process (the engine exits when it closes, so it dies with the app). On a
/// crash it restarts with exponential backoff; stderr goes to the Diagnostics log. Keychain secrets are
/// injected as environment variables.
@MainActor @Observable
final class EngineController {
    enum State: Hashable, Sendable {
        case stopped
        case starting
        case running(port: Int)
        case waitingToRestart(attempt: Int, seconds: Int)
        case failed(String)
        case uvMissing

        var title: String {
            switch self {
            case .stopped: "Stopped"
            case .starting: "Starting"
            case .running(let port): "Running on port \(port)"
            case .waitingToRestart(_, let s): "Restarting in \(s) s"
            case .failed: "Failed"
            case .uvMissing: "uv not found"
            }
        }

        var isRunning: Bool { if case .running = self { true } else { false } }
    }

    private(set) var state: State = .stopped
    /// The API of the running engine; nil until it is ready and after it stops.
    private(set) var api: TellfoldAPI?
    private(set) var health: Health?
    private(set) var log: [EngineLogLine] = []
    private(set) var pid: Int32?
    private(set) var commandDescription = ""
    private(set) var startedAt: Date?

    /// Called whenever `api` changes (engine became ready, restarted on a new port, or stopped).
    @ObservationIgnored var onAPIChange: (@MainActor (TellfoldAPI?) -> Void)?

    @ObservationIgnored private let prefs: AppPreferences
    @ObservationIgnored private let keychain: Keychain
    @ObservationIgnored private var process: Process?
    @ObservationIgnored private var stdinPipe: Pipe?
    @ObservationIgnored private var token = ""
    @ObservationIgnored private var generation = 0
    @ObservationIgnored private var attempt = 0
    @ObservationIgnored private var userStopped = false
    @ObservationIgnored private var restartTask: Task<Void, Never>?
    @ObservationIgnored private var readyTask: Task<Void, Never>?
    @ObservationIgnored private var readTasks: [Task<Void, Never>] = []
    @ObservationIgnored private var nextLogID = 0
    /// When the current engine reached `.running`; nil while it is starting (or never got there).
    @ObservationIgnored private var readyAt: Date?

    static let maxLogLines = 5_000
    /// How long a `uv run` engine gets to print its port: the first run may install dependencies.
    static let readyTimeout: Duration = .seconds(180)
    /// The same for the engine built into the app, which prints its port within milliseconds. The slack is for a
    /// macOS privacy prompt about the runs folder, which blocks the engine until it is answered.
    static let bundledReadyTimeout: Duration = .seconds(60)
    static let maxAttempts = 8
    /// An engine that stayed up this long counts as healthy: its next crash starts the backoff over.
    nonisolated static let stableRunSeconds: TimeInterval = 60
    /// After SIGTERM, how long the engine gets before its process group is sent SIGKILL.
    static let killGrace: Duration = .seconds(5)
    /// Per-request timeout of the startup health probe (the engine answers it in milliseconds).
    static let healthProbeTimeout: TimeInterval = 3
    private static let logger = Logger(subsystem: "dev.tellfold.mac", category: "Engine")

    init(prefs: AppPreferences, keychain: Keychain = .apiKeys) {
        self.prefs = prefs
        self.keychain = keychain
    }

    // MARK: Control

    func start() {
        userStopped = false
        attempt = 0
        restartTask?.cancel()
        launch()
    }

    /// Graceful stop: close stdin, then SIGTERM after 3 s and SIGKILL after 5 s if it is still alive.
    func stop() {
        userStopped = true
        restartTask?.cancel()
        guard let process else {
            state = .stopped
            return
        }
        try? stdinPipe?.fileHandleForWriting.close()
        let pid = process.processIdentifier
        Task {
            try? await Task.sleep(for: .seconds(3))
            if process.isRunning { process.terminate() }
            try? await Task.sleep(for: .seconds(2))
            if process.isRunning { Self.forceKill(pid: pid) }
        }
    }

    /// Ends an engine that is hung (no port, failed health check): close stdin, SIGTERM, then SIGKILL its process
    /// group after `killGrace`. An engine blocked in a system call (e.g. an open() waiting on a privacy prompt)
    /// never acts on SIGTERM, and without the SIGKILL `processExited` would never run.
    private func terminateHungEngine(_ reason: String) {
        appendApp(reason)
        guard let process else { return }
        try? stdinPipe?.fileHandleForWriting.close()
        let pid = process.processIdentifier
        if process.isRunning { process.terminate() }
        Task { [weak self] in
            try? await Task.sleep(for: Self.killGrace)
            guard process.isRunning else { return }
            self?.appendApp("The engine did not exit after SIGTERM; sending SIGKILL.")
            Self.forceKill(pid: pid)
        }
    }

    /// SIGKILL the engine's process group, or just the process when it does not lead its own group. The bundled
    /// engine is the direct child; through `uv run` the group is uv and the Python server under it.
    nonisolated static func forceKill(pid: pid_t) {
        if getpgid(pid) == pid {
            kill(-pid, SIGKILL)
        } else {
            kill(pid, SIGKILL)
        }
    }

    /// Whether a crash should reset the restart backoff: only when the engine was actually running, and for
    /// longer than `stableRunSeconds`. Time spent starting (or hung while starting) never counts.
    nonisolated static func shouldResetAttempts(readyAt: Date?, now: Date = .now) -> Bool {
        guard let readyAt else { return false }
        return now.timeIntervalSince(readyAt) > stableRunSeconds
    }

    func restart() {
        appendApp("Restarting the engine.")
        guard process != nil else {
            start()
            return
        }
        stop()
        Task {
            while process != nil { try? await Task.sleep(for: .milliseconds(100)) }
            start()
        }
    }

    /// Synchronous stop for app termination.
    func shutdownNow() {
        userStopped = true
        restartTask?.cancel()
        try? stdinPipe?.fileHandleForWriting.close()
        process?.terminate()
    }

    func clearLog() { log.removeAll() }

    var logText: String {
        let formatter = Date.ISO8601FormatStyle(includingFractionalSeconds: false).timeSeparator(.colon)
        return log.map { "\($0.date.formatted(formatter)) [\($0.source.rawValue)] \($0.text)" }.joined(separator: "\n")
    }

    // MARK: Launch

    private func launch() {
        guard process == nil else { return }
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        let base = ProcessInfo.processInfo.environment
        let runs = prefs.resolvedRunsFolder
        let examples = EngineCommand.bundledExamples()?.path
        let executable: URL, args: [String], workingDirectory: URL, readyTimeout: Duration, slowStartNote: String
        if let engine = prefs.bundledEngineFolder {
            // The engine built into the app: one binary, nothing to install.
            executable = EngineCommand.bundledBinary(in: engine)
            args = EngineCommand.bundledArguments(runsFolder: runs.path, examplesFolder: examples,
                                                  logLevel: prefs.logLevel.rawValue)
            workingDirectory = runs
            readyTimeout = Self.bundledReadyTimeout
            slowStartNote = ""
        } else {
            #if !DEBUG
            // Without an engine path, a release build has nothing to fall back to: the repository it was built from
            // is not on this Mac.
            if prefs.enginePath.trimmingCharacters(in: .whitespaces).isEmpty {
                state = .failed("The engine built into Tellfold is missing (Contents/Resources/engine/tellfold). "
                                + "Reinstall Tellfold, or set an engine path in Settings > Advanced.")
                appendApp("No executable engine in the app bundle.")
                return
            }
            #endif
            let path = EngineCommand.searchPath(existing: base["PATH"], home: home)
            let preferredUV = prefs.uvPath.trimmingCharacters(in: .whitespaces)
            guard let uv = EngineCommand.findUV(preferred: preferredUV.isEmpty ? nil : preferredUV, path: path) else {
                state = .uvMissing
                appendApp(preferredUV.isEmpty ? "uv was not found on PATH (\(path))." : "uv is not executable at \(preferredUV).")
                return
            }
            let enginePath = prefs.resolvedEnginePath
            guard FileManager.default.fileExists(atPath: enginePath + "/pyproject.toml") else {
                state = .failed("No engine at \(enginePath) (pyproject.toml missing). Set the engine path in Settings > Advanced.")
                appendApp("No pyproject.toml in \(enginePath).")
                return
            }
            executable = uv
            args = EngineCommand.arguments(enginePath: enginePath, runsFolder: runs.path, examplesFolder: examples,
                                           logLevel: prefs.logLevel.rawValue)
            workingDirectory = URL(filePath: enginePath, directoryHint: .isDirectory)
            readyTimeout = Self.readyTimeout
            slowStartNote = " The first start can also take a while when uv installs the engine's dependencies."
        }
        do {
            try FileManager.default.createDirectory(at: runs, withIntermediateDirectories: true)
        } catch {
            state = .failed("Cannot create the runs folder \(runs.path): \(error.localizedDescription)")
            return
        }

        token = EngineCommand.makeToken()
        generation += 1
        let gen = generation
        commandDescription = "TELLFOLD_SERVE_TOKEN=••• " + EngineCommand.displayCommand(executable: executable.path, arguments: args, token: token)

        let process = Process()
        process.executableURL = executable
        process.arguments = args
        process.currentDirectoryURL = workingDirectory
        process.environment = EngineCommand.environment(
            base: base, token: token, secrets: keychain.environment(), offline: prefs.offlineTestModel, home: home)
        let input = Pipe(), output = Pipe(), errors = Pipe()
        process.standardInput = input
        process.standardOutput = output
        process.standardError = errors
        process.terminationHandler = { [weak self] proc in
            let status = proc.terminationStatus
            let signaled = proc.terminationReason == .uncaughtSignal
            Task { @MainActor [weak self] in
                self?.processExited(generation: gen, status: status, signaled: signaled)
            }
        }

        appendApp("Starting: \(commandDescription)")
        state = .starting
        do {
            try process.run()
        } catch {
            appendApp("Could not start the engine: \(error.localizedDescription)")
            scheduleRestart(reason: error.localizedDescription)
            return
        }
        self.process = process
        stdinPipe = input
        pid = process.processIdentifier
        startedAt = .now
        readyAt = nil
        readTasks = [
            readLines(output.fileHandleForReading, source: .stdout, generation: gen),
            readLines(errors.fileHandleForReading, source: .stderr, generation: gen),
        ]
        readyTask?.cancel()
        readyTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(15))
            if let self, self.generation == gen, self.state == .starting, !Task.isCancelled {
                self.appendApp("Still waiting for the engine. If macOS asked whether Tellfold may access files on a "
                               + "removable volume or in a protected folder, allow it." + slowStartNote)
            }
            try? await Task.sleep(for: readyTimeout - .seconds(15))
            guard !Task.isCancelled, let self, self.generation == gen, self.state == .starting else { return }
            self.terminateHungEngine("The engine did not report a port within \(readyTimeout); stopping it.")
        }
    }

    private func readLines(_ handle: FileHandle, source: EngineLogLine.Source, generation gen: Int) -> Task<Void, Never> {
        let lines = PipeLineReader.lines(of: handle)
        return Task { [weak self] in
            for await line in lines {
                guard let self else { return }
                self.received(line: line, source: source, generation: gen)
            }
        }
    }

    private func received(line: String, source: EngineLogLine.Source, generation gen: Int) {
        guard gen == generation else { return }
        if source == .stdout, api == nil, state == .starting, let port = EngineCommand.parsePort(fromLine: line) {
            appendApp("Engine listening on 127.0.0.1:\(port).")
            becomeReady(port: port, generation: gen)
            return
        }
        append(line, source: source)
    }

    private func becomeReady(port: Int, generation gen: Int) {
        let api = TellfoldAPI(port: port, token: token)
        readyTask?.cancel()
        readyTask = Task { [weak self] in
            var lastError: (any Error)?
            for _ in 0..<20 {
                do {
                    let health = try await api.health(timeout: Self.healthProbeTimeout)
                    guard let self, self.generation == gen else { return }
                    self.health = health
                    self.api = api
                    self.state = .running(port: port)
                    self.readyAt = .now
                    self.onAPIChange?(api)
                    return
                } catch {
                    lastError = error
                    try? await Task.sleep(for: .milliseconds(250))
                }
            }
            guard let self, self.generation == gen else { return }
            self.terminateHungEngine("Health check failed: \(lastError?.localizedDescription ?? "unknown error").")
        }
    }

    private func processExited(generation gen: Int, status: Int32, signaled: Bool) {
        guard gen == generation else { return }
        readyTask?.cancel()
        readTasks.forEach { $0.cancel() }
        readTasks = []
        process = nil
        stdinPipe = nil
        pid = nil
        let hadAPI = api != nil
        api = nil
        health = nil
        if hadAPI { onAPIChange?(nil) }
        appendApp(signaled ? "Engine stopped by signal \(status)." : "Engine exited with status \(status).")
        if userStopped {
            state = .stopped
        } else {
            scheduleRestart(reason: "exit status \(status)")
        }
    }

    private func scheduleRestart(reason: String) {
        if Self.shouldResetAttempts(readyAt: readyAt) { attempt = 0 }
        readyAt = nil
        attempt += 1
        guard attempt <= Self.maxAttempts else {
            state = .failed("The engine keeps exiting (\(reason)). Open Diagnostics for its log.")
            return
        }
        let seconds = EngineCommand.backoffSeconds(attempt: attempt)
        state = .waitingToRestart(attempt: attempt, seconds: seconds)
        appendApp("Restarting in \(seconds) s (attempt \(attempt)).")
        restartTask?.cancel()
        restartTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(seconds))
            guard !Task.isCancelled, let self, !self.userStopped else { return }
            self.launch()
        }
    }

    // MARK: Log

    private func appendApp(_ text: String) {
        Self.logger.info("\(text, privacy: .public)")
        append(text, source: .app)
    }

    private func append(_ text: String, source: EngineLogLine.Source) {
        let safe = token.isEmpty ? text : text.replacingOccurrences(of: token, with: "•••")
        log.append(EngineLogLine(id: nextLogID, date: .now, source: source, text: safe))
        nextLogID += 1
        if log.count > Self.maxLogLines { log.removeFirst(log.count - Self.maxLogLines + 500) }
    }
}
