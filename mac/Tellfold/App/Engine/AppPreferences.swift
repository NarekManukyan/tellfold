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
    /// A `tellfold` engine to run instead of the one built into the app: the binary, a folder holding it, or a
    /// Tellfold repository with a release build (see `EngineCommand.chooseEngine`). Empty means the built-in engine
    /// (and, in a Debug build without one, the repository the app was compiled from). A path that holds no engine,
    /// such as a Python project saved before 0.8, falls back to the built-in one.
    var enginePath: String { didSet { defaults.set(enginePath, forKey: Keys.enginePath) } }
    var logLevel: LogLevel { didSet { defaults.set(logLevel.rawValue, forKey: Keys.logLevel) } }
    var notificationsEnabled: Bool { didSet { defaults.set(notificationsEnabled, forKey: Keys.notifications) } }
    var defaultConcurrency: Int { didSet { defaults.set(defaultConcurrency, forKey: Keys.concurrency) } }
    /// Runs the engine with `TELLFOLD_FAKE_LLM=1` (offline canned model, for trying the app without an LLM).
    var offlineTestModel: Bool { didSet { defaults.set(offlineTestModel, forKey: Keys.offline) } }
    var askedNotificationPermission: Bool { didSet { defaults.set(askedNotificationPermission, forKey: Keys.askedNotifications) } }
    /// The first-run step that picks an AI tool was finished or skipped.
    var agentSetupDone: Bool { didSet { defaults.set(agentSetupDone, forKey: Keys.agentSetupDone) } }
    /// The language of the last simulation started; the New simulation sheet opens with it.
    var lastLanguage: String { didSet { defaults.set(lastLanguage, forKey: Keys.lastLanguage) } }

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        runsFolder = defaults.string(forKey: Keys.runsFolder) ?? Self.defaultRunsFolder.path
        enginePath = defaults.string(forKey: Keys.enginePath) ?? ""
        logLevel = defaults.string(forKey: Keys.logLevel).flatMap(LogLevel.init(rawValue:)) ?? .info
        notificationsEnabled = defaults.object(forKey: Keys.notifications) as? Bool ?? true
        let concurrency = defaults.integer(forKey: Keys.concurrency)
        defaultConcurrency = (1...16).contains(concurrency) ? concurrency : 4
        offlineTestModel = defaults.bool(forKey: Keys.offline)
        askedNotificationPermission = defaults.bool(forKey: Keys.askedNotifications)
        agentSetupDone = defaults.bool(forKey: Keys.agentSetupDone)
        lastLanguage = defaults.string(forKey: Keys.lastLanguage) ?? "English"
    }

    private enum Keys {
        static let runsFolder = "runsFolder"
        static let enginePath = "enginePath"
        static let logLevel = "logLevel"
        static let notifications = "notificationsEnabled"
        static let concurrency = "defaultConcurrency"
        static let offline = "offlineTestModel"
        static let askedNotifications = "askedNotificationPermission"
        static let agentSetupDone = "agentSetupDone"
        static let lastLanguage = "lastLanguage"
    }

    /// `~/Library/Application Support/Tellfold/runs`.
    nonisolated static var defaultRunsFolder: URL {
        URL.applicationSupportDirectory.appending(path: "Tellfold/runs", directoryHint: .isDirectory)
    }

    /// The engine binary every build carries in `Resources/engine` (see `mac/scripts/bundle-engine.sh`). Nil when the
    /// app has none.
    nonisolated static var bundledEngine: URL? {
        guard let engine = Bundle.main.resourceURL?.appending(path: "engine", directoryHint: .isDirectory),
              FileManager.default.isExecutableFile(atPath: EngineCommand.bundledBinary(in: engine).path) else { return nil }
        return engine
    }

    /// The engine a Debug build without a bundled engine runs: the release build in the repository this file was
    /// compiled from (four folders up from `mac/Tellfold/App/Engine`). Release builds never fall back to it.
    nonisolated static let developmentEnginePath: String = {
        var url = URL(filePath: #filePath)
        for _ in 0..<5 { url.deleteLastPathComponent() }
        return (url.path(percentEncoded: false) as NSString).standardizingPath
    }()

    /// The bundled engine, unless an engine path is set in Settings.
    var bundledEngineFolder: URL? {
        enginePath.trimmingCharacters(in: .whitespaces).isEmpty ? Self.bundledEngine : nil
    }

    var resolvedRunsFolder: URL {
        let trimmed = runsFolder.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty else { return Self.defaultRunsFolder }
        return URL(filePath: (trimmed as NSString).expandingTildeInPath, directoryHint: .isDirectory)
    }
}
