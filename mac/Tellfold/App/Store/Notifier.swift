import Foundation
@preconcurrency import UserNotifications

/// Local notifications when a run finishes or fails, or waits for its cast to be checked. Clicking one selects the run.
@MainActor
final class Notifier: NSObject {
    /// Called with the run id when the user clicks a notification.
    var onOpenRun: (@MainActor (String) -> Void)?

    private var center: UNUserNotificationCenter { .current() }

    /// Installs the delegate (so banners also show while the app is frontmost).
    func activate() {
        center.delegate = self
    }

    /// Asks for permission once; later calls are no-ops.
    func requestAuthorization() async -> Bool {
        do {
            return try await center.requestAuthorization(options: [.alert, .sound])
        } catch {
            return false
        }
    }

    func notifyFinished(run: RunSummary) {
        let content = UNMutableNotificationContent()
        switch run.state {
        case .done:
            content.title = "Simulation finished"
        case .failed:
            content.title = "Simulation failed"
        case .stopped:
            content.title = "Simulation stopped"
        default:
            return
        }
        content.body = run.question
        content.sound = run.state == .failed ? .defaultCritical : .default
        content.userInfo = ["runID": run.id]
        content.threadIdentifier = "runs"
        let request = UNNotificationRequest(identifier: "run-\(run.id)-\(run.state.rawValue)", content: content, trigger: nil)
        center.add(request) { _ in }
    }

    /// A "Review the cast before the run" simulation has its people and waits.
    func notifyCastReady(run: RunSummary) {
        let content = UNMutableNotificationContent()
        content.title = "The cast is ready to check"
        content.body = run.question
        content.sound = .default
        content.userInfo = ["runID": run.id]
        content.threadIdentifier = "runs"
        center.add(UNNotificationRequest(identifier: "run-\(run.id)-cast", content: content, trigger: nil)) { _ in }
    }
}

extension Notifier: UNUserNotificationCenterDelegate {
    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification)
        async -> UNNotificationPresentationOptions {
        [.banner, .sound, .list]
    }

    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse) async {
        guard let runID = response.notification.request.content.userInfo["runID"] as? String else { return }
        await MainActor.run {
            self.onOpenRun?(runID)
        }
    }
}
