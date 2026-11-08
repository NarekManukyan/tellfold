import Foundation
import Observation

/// App-side preferences (UserDefaults). Engine-side settings (default runtimes, endpoints, ...) live in the
/// engine's own `settings.json` and are edited through `/api/settings`.
@MainActor @Observable
final class AppPreferences {
    enum LogLevel: String, CaseIterable, Identifiable, Sendable {
        case debug, info, warning, error
        var id: String { rawValue }
        var title: String { rawValue.capitalized }
    }

    @ObservationIgnored private let defaults: UserDefaults

    /// Where runs are stored; passed to the engine as `--runs`.
    var runsFolder: String { didSet { defaults.set(runsFolder, forKey: Keys.runsFolder) } }
    /// The Python project the engine runs from (`uv run --project`). Empty means the built-in default.
    var enginePath: String { didSet { defaults.set(enginePath, forKey: Keys.enginePath) } }
    /// Explicit `uv` binary. Empty means auto-detect.
    var uvPath: String { didSet { defaults.set(uvPath, forKey: Keys.uvPath) } }
    var logLevel: LogLevel { didSet { defaults.set(logLevel.rawValue, forKey: Keys.logLevel) } }
    var notificationsEnabled: Bool { didSet { defaults.set(notificationsEnabled, forKey: Keys.notifications) } }
    var defaultConcurrency: Int { didSet { defaults.set(defaultConcurrency, forKey: Keys.concurrency) } }
    /// Runs the engine with `TELLFOLD_FAKE_LLM=1` (offline canned model, for trying the app without an LLM).
    var offlineTestModel: Bool { didSet { defaults.set(offlineTestModel, forKey: Keys.offline) } }
    var askedNotificationPermission: Bool { didSet { defaults.set(askedNotificationPermission, forKey: Keys.askedNotifications) } }

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        runsFolder = defaults.string(forKey: Keys.runsFolder) ?? Self.defaultRunsFolder.path
        enginePath = defaults.string(forKey: Keys.enginePath) ?? ""
        uvPath = defaults.string(forKey: Keys.uvPath) ?? ""
        logLevel = defaults.string(forKey: Keys.logLevel).flatMap(LogLevel.init(rawValue:)) ?? .info
        notificationsEnabled = defaults.object(forKey: Keys.notifications) as? Bool ?? true
        let concurrency = defaults.integer(forKey: Keys.concurrency)
        defaultConcurrency = (1...16).contains(concurrency) ? concurrency : 4
        offlineTestModel = defaults.bool(forKey: Keys.offline)
        askedNotificationPermission = defaults.bool(forKey: Keys.askedNotifications)
    }

    private enum Keys {
        static let runsFolder = "runsFolder"
        static let enginePath = "enginePath"
        static let uvPath = "uvPath"
        static let logLevel = "logLevel"
        static let notifications = "notificationsEnabled"
        static let concurrency = "defaultConcurrency"
        static let offline = "offlineTestModel"
        static let askedNotifications = "askedNotificationPermission"
    }

    /// `~/Library/Application Support/Tellfold/runs`.
    nonisolated static var defaultRunsFolder: URL {
        URL.applicationSupportDirectory.appending(path: "Tellfold/runs", directoryHint: .isDirectory)
    }

    /// The engine Release builds carry in `Resources/engine`, with its own Python (see `mac/scripts/bundle-engine.sh`).
    /// Nil when the app has none, as in Debug builds.
    nonisolated static var bundledEngine: URL? {
        guard let engine = Bundle.main.resourceURL?.appending(path: "engine", directoryHint: .isDirectory),
              FileManager.default.isExecutableFile(atPath: EngineCommand.bundledPython(in: engine).path) else { return nil }
        return engine
    }

    /// The Python project `uv run` uses when there is no bundled engine: Debug builds run it from the repository this
    /// file was compiled from (four folders up from `mac/Tellfold/App/Engine`).
    nonisolated static let developmentEnginePath: String = {
        var url = URL(filePath: #filePath)
        for _ in 0..<5 { url.deleteLastPathComponent() }
        return (url.path(percentEncoded: false) as NSString).standardizingPath
    }()

    /// The bundled engine, unless an engine path is set in Settings (which always runs through `uv`).
    var bundledEngineFolder: URL? {
        enginePath.trimmingCharacters(in: .whitespaces).isEmpty ? Self.bundledEngine : nil
    }

    var resolvedEnginePath: String {
        let trimmed = enginePath.trimmingCharacters(in: .whitespaces)
        return trimmed.isEmpty ? Self.developmentEnginePath : (trimmed as NSString).expandingTildeInPath
    }

    var resolvedRunsFolder: URL {
        let trimmed = runsFolder.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty else { return Self.defaultRunsFolder }
        return URL(filePath: (trimmed as NSString).expandingTildeInPath, directoryHint: .isDirectory)
    }
}
