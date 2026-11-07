import AppKit
import SwiftUI

@main
struct ShoalApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate

    var body: some Scene {
        WindowGroup("Shoal", id: "main") {
            MainView()
                .environment(delegate.model)
                .frame(minWidth: 960, minHeight: 560)
        }
        .defaultSize(width: 1320, height: 840)
        .commands { ShoalCommands(model: delegate.model) }

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
        }
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    let model = AppModel()

    /// Unit tests run inside the app; they must not spawn the engine.
    static var isRunningTests: Bool {
        ProcessInfo.processInfo.environment["XCTestConfigurationFilePath"] != nil
            || NSClassFromString("XCTestCase") != nil
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

    /// Seed files dropped on the Dock icon or opened with Shoal from Finder.
    func application(_ application: NSApplication, open urls: [URL]) {
        model.openSeedFiles(urls)
    }
}
