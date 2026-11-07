import SwiftUI

/// Menu bar: File > Open Seed Files and the Simulation menu.
struct TellfoldCommands: Commands {
    let model: AppModel
    @FocusedValue(\.runStore) private var store

    var body: some Commands {
        CommandGroup(replacing: .newItem) {
            Button("Open Seed Files…") { model.presentOpenPanel() }
                .keyboardShortcut("o")
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
            .disabled(!(store?.isLive ?? false))

            Button("Inject Event…") { store?.showsInjectEvent = true }
                .keyboardShortcut("e")
                .disabled(!(store?.isLive ?? false))

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
            .disabled(store == nil || (store?.isLive ?? true))

            Button("Reveal in Finder") {
                if let id = store?.runID { model.revealInFinder(id) }
            }
            .disabled(store == nil)
        }
        // Window > Diagnostics (⌥⌘D) comes from the Diagnostics `Window` scene itself.
    }
}
