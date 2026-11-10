import Foundation
import Observation
import Sparkle

/// In-app updates through Sparkle: Tellfold > Check for Updates… and the daily automatic check, both fed from the
/// appcast on GitHub Pages (zoel-io.github.io/tellfold) that `mac/scripts/release.sh` writes.
@MainActor @Observable
final class AppUpdater {
    /// False while a check is already running, or when updates are off (under unit tests).
    private(set) var canCheckForUpdates = false

    @ObservationIgnored private let controller: SPUStandardUpdaterController?
    @ObservationIgnored private var observation: NSKeyValueObservation?

    /// `enabled` is false under unit tests, which must never reach the network.
    init(enabled: Bool) {
        guard enabled else {
            controller = nil
            return
        }
        let controller = SPUStandardUpdaterController(startingUpdater: true, updaterDelegate: nil, userDriverDelegate: nil)
        self.controller = controller
        observation = controller.updater.observe(\.canCheckForUpdates, options: [.initial, .new]) { [weak self] updater, _ in
            let value = updater.canCheckForUpdates
            Task { @MainActor in self?.canCheckForUpdates = value }
        }
    }

    var isAvailable: Bool { controller != nil }

    /// Sparkle keeps this in the app's defaults; the Info.plist turns it on for new installs.
    var automaticallyChecks: Bool {
        get {
            access(keyPath: \.automaticallyChecks)
            return controller?.updater.automaticallyChecksForUpdates ?? false
        }
        set {
            withMutation(keyPath: \.automaticallyChecks) {
                controller?.updater.automaticallyChecksForUpdates = newValue
            }
        }
    }

    /// The date of the last check, if there was one.
    var lastCheck: Date? { controller?.updater.lastUpdateCheckDate }

    func checkForUpdates() {
        controller?.checkForUpdates(nil)
    }
}
