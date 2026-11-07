import Foundation
import Observation

/// Advances `RunStore.playhead` while `RunStore.isPlaying` is true, one round every 2.5 s / `replaySpeed`.
/// The store holds the replay state (`isPlaying`, `replaySpeed`, `followsLive`) so the app's Simulation menu
/// and these views drive the same thing; the run view calls `sync()` whenever that state changes.
/// Live runs follow the newest round until the user scrubs back (scrubbing to the newest round resumes it).
@MainActor
@Observable
final class ReplayController {
    /// Seconds one round stays on screen at 1x.
    static let secondsPerRound: Double = 2.5
    static let speeds: [Double] = [0.5, 1, 2]

    @ObservationIgnored private weak var store: RunStore?
    @ObservationIgnored private var timer: Task<Void, Never>?
    @ObservationIgnored private var timerSpeed: Double = 0

    init() {}

    func attach(_ store: RunStore) {
        if self.store !== store { cancelTimer() }
        self.store = store
        sync()
    }

    func detach() {
        store?.isPlaying = false
        cancelTimer()
        store = nil
    }

    // MARK: state passthrough

    var isPlaying: Bool { store?.isPlaying ?? false }
    var followsLive: Bool { store?.followsLive ?? false }
    var speed: Double {
        get { store?.replaySpeed ?? 1 }
        set {
            store?.replaySpeed = newValue
            sync()
        }
    }

    var canPlay: Bool {
        guard let store else { return false }
        return store.latestRound > 0
    }
    var canStepBack: Bool { (store?.playhead ?? 0) > 0 }
    var canStepForward: Bool {
        guard let store else { return false }
        return store.playhead < store.latestRound
    }

    // MARK: user actions

    func togglePlay() {
        guard let store, canPlay || store.isPlaying else { return }
        store.togglePlaying()
        sync()
    }

    func pause() {
        store?.isPlaying = false
        cancelTimer()
    }

    func step(_ delta: Int) {
        guard let store else { return }
        pause()
        store.stepPlayhead(by: delta)
    }

    /// Moves the playhead (clamped by the store). Reaching the newest round of a live run resumes following.
    func scrub(to round: Int) {
        guard let store else { return }
        if store.isPlaying { pause() }
        if store.playhead != round { store.setPlayhead(round) }
    }

    func jumpToLatest() {
        guard let store else { return }
        pause()
        store.setPlayhead(store.latestRound)
        store.followsLive = true
    }

    /// Starts, restarts or stops the round timer to match the store. Call when `isPlaying` or `replaySpeed`
    /// changes (including from the app menu).
    func sync() {
        guard let store, store.isPlaying else {
            cancelTimer()
            return
        }
        if timer == nil || timerSpeed != store.replaySpeed { startTimer(speed: store.replaySpeed) }
    }

    /// Call when the run's newest round or live state changes.
    func runDidAdvance() {
        guard let store else { return }
        if store.playhead > store.latestRound { store.playhead = store.latestRound }
        if store.followsLive, !store.isPlaying, store.playhead != store.latestRound {
            store.playhead = store.latestRound
        }
    }

    // MARK: timer

    private func startTimer(speed: Double) {
        timer?.cancel()
        timerSpeed = speed
        let interval = Self.secondsPerRound / max(0.25, speed)
        timer = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(interval))
                guard !Task.isCancelled, let self else { return }
                if !self.advanceOneRound() { return }
            }
        }
    }

    private func cancelTimer() {
        timer?.cancel()
        timer = nil
        timerSpeed = 0
    }

    /// Returns false when playback ended.
    private func advanceOneRound() -> Bool {
        guard let store, store.isPlaying else {
            cancelTimer()
            return false
        }
        if store.playhead < store.latestRound {
            store.playhead += 1
            return true
        }
        // Reached the newest round: a finished run stops; a live run stops and follows new rounds.
        store.isPlaying = false
        if store.isLive { store.followsLive = true }
        cancelTimer()
        return false
    }
}
