import SwiftUI

/// Menu bar: Tellfold > Check for Updates, File > Open (seed documents or a `.tellfold` run), Export PDF, Share Card and
/// Export Run Bundle, and the Simulation menu.
struct TellfoldCommands: Commands {
    let model: AppModel
    let updater: AppUpdater
    @FocusedValue(\.runStore) private var store

    var body: some Commands {
        CommandGroup(after: .appInfo) {
            Button("Check for Updates…") { updater.checkForUpdates() }
                .disabled(!updater.canCheckForUpdates)
        }

        CommandGroup(replacing: .newItem) {
            Button("Open…") { model.presentOpenPanel() }
                .keyboardShortcut("o")
        }

        CommandGroup(replacing: .importExport) {
            Button("Export PDF…") {
                guard let store, let report = store.report else { return }
                Task {
                    do { try await model.exportPDF(store.runID, title: report.title) } catch {
                        store.actionError = error.localizedDescription
                    }
                }
            }
            .keyboardShortcut("e", modifiers: [.command, .shift])
            .disabled(store?.report == nil || model.exportingPDF != nil)

            Button("Share Card…") { store?.showsShareCard = true }
                .disabled(store?.report == nil)

            Button("Export Run Bundle…") {
                guard let store else { return }
                Task {
                    do { try await model.exportBundle(store.runID) } catch { store.actionError = error.localizedDescription }
                }
            }
            .disabled(!(store.map { model.canExportBundle($0.runID) } ?? false))
        }

        CommandMenu("Simulation") {
            Button("New Simulation…") { model.newSimulation() }
                .keyboardShortcut("n")
                .disabled(!model.engine.state.isRunning)

            Divider()

            Button("Stop") {
                guard let store else { return }
                Task { await store.stop() }
            }
            .keyboardShortcut(".")
            .disabled(!(store?.controls.stop ?? false))

            Button("Inject Event…") { store?.showsInjectEvent = true }
                .keyboardShortcut("e")
                .disabled(!(store?.controls.stop ?? false))

            Divider()

            Button(store?.isPlaying == true ? "Pause Replay" : "Replay") { store?.togglePlaying() }
                .keyboardShortcut(.space, modifiers: [])
                .disabled(store == nil || (store?.latestRound ?? 0) == 0)

            Button("Next Round") { store?.stepPlayhead(by: 1) }
                .keyboardShortcut("]")
                .disabled(store == nil || (store?.playhead ?? 0) >= (store?.latestRound ?? 0))

            Button("Previous Round") { store?.stepPlayhead(by: -1) }
                .keyboardShortcut("[")
                .disabled(store == nil || (store?.playhead ?? 0) <= 0)

            Divider()

            Button("Rebuild Report") {
                guard let store else { return }
                Task { await store.rebuildReport() }
            }
            .disabled(!(store?.canRebuildReport ?? false))

            Button("Reveal in Finder") {
                if let id = store?.runID { model.revealInFinder(id) }
            }
            .disabled(store == nil || (store?.isExample ?? false))
        }
        // Window > Diagnostics (⌥⌘D) comes from the Diagnostics `Window` scene itself.
    }
}
