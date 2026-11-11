import AppKit
import SwiftUI

@main
struct TellfoldApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate

    var body: some Scene {
        WindowGroup("Tellfold", id: "main") {
            MainView()
                .environment(delegate.model)
                .frame(minWidth: 960, minHeight: 560)
        }
        .defaultSize(width: 1320, height: 840)
        .commands { TellfoldCommands(model: delegate.model, updater: delegate.updater) }

        Window("Diagnostics", id: "diagnostics") {
            DiagnosticsView()
                .environment(delegate.model)
                .frame(minWidth: 620, minHeight: 360)
        }
        .defaultSize(width: 820, height: 520)
        .keyboardShortcut("d", modifiers: [.command, .option])

        Settings {
            SettingsView()
                .environment(delegate.model)
                .environment(delegate.updater)
        }
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    let model = AppDelegate.makeModel()
    let updater = AppUpdater(enabled: !AppDelegate.isRunningTests)

    /// Unit tests run inside the app; they must not spawn the engine.
    static var isRunningTests: Bool {
        ProcessInfo.processInfo.environment["XCTestConfigurationFilePath"] != nil
            || NSClassFromString("XCTestCase") != nil
    }

    /// Carries preferences, the support folder and API keys over from the app's previous name once, before
    /// `AppPreferences` reads anything. Skipped under test so it never touches the real user's data.
    private static func makeModel() -> AppModel {
        if !isRunningTests { LegacyMigration.live().runIfNeeded() }
        return AppModel()
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        guard !Self.isRunningTests else { return }
        model.start()
    }

    func applicationWillTerminate(_ notification: Notification) {
        model.shutdown()
    }

    /// Keep running when the last window closes, so live simulations continue.
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }

    /// Seed files and `.tellfold` run bundles dropped on the Dock icon or opened with Tellfold from Finder.
    func application(_ application: NSApplication, open urls: [URL]) {
        model.open(urls)
    }
}
