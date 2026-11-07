import SwiftUI

/// Play button, a scrubber with one stop per round (injected events marked in amber) and the speed menu: the replay
/// transport of the Crowd tab. All of it drives the one playhead on the store.
struct RoundScrubber: View {
    let store: RunStore
    let replay: ReplayController

    private var lastRound: Int { max(store.totalRounds, store.latestRound, 1) }

    var body: some View {
        HStack(spacing: Theme.space3) {
            Button { replay.togglePlay() } label: {
                Image(systemName: replay.isPlaying ? "pause.fill" : "play.fill")
                    .font(.system(size: 11, weight: .bold))
                    .foregroundStyle(Theme.accentInk)
                    .frame(width: 28, height: 28)
                    .background(Theme.accentFill, in: Circle())
                    .contentTransition(.symbolEffect(.replace))
            }
            .buttonStyle(.plain)
            .disabled(store.latestRound == 0 && !store.isLive)
            .help(replay.isPlaying ? "Pause replay (Space)" : "Replay round by round (Space)")
            .accessibilityLabel(replay.isPlaying ? "Pause replay" : "Play replay")

            track
                .frame(height: 38)

            Text("Round \(store.playhead) of \(store.totalRounds)")
                .font(Theme.mono(12, .medium))
                .foregroundStyle(Theme.text)
                .fixedSize()
                .accessibilityHidden(true)

            Menu {
                Picker("Speed", selection: Binding(get: { replay.speed }, set: { replay.speed = $0 })) {
                    ForEach(ReplayController.speeds, id: \.self) { Text(speedText($0)).tag($0) }
                }
                .pickerStyle(.inline)
            } label: {
                Text(speedText(replay.speed)).font(Theme.mono(12, .medium))
            }
            .menuStyle(.borderlessButton)
            .menuIndicator(.hidden)
            .fixedSize()
            .help("Replay speed")
            .accessibilityLabel("Replay speed")
            .accessibilityValue(speedText(replay.speed))

            if store.isLive {
                Button {
                    replay.jumpToLatest()
                } label: {
                    Label(replay.followsLive ? "Live" : "Go live", systemImage: "dot.radiowaves.left.and.right")
                }
                .buttonStyle(.console(replay.followsLive ? .ghost : .secondary, compact: true))
                .help(replay.followsLive ? "Following the newest round" : "Jump to the newest round and follow it")
            }
        }
        .padding(.horizontal, Theme.space4)
        .padding(.vertical, Theme.space2)
    }

    private func speedText(_ speed: Double) -> String {
        speed.formatted(.number.precision(.fractionLength(0...1))) + "×"
    }

    // MARK: track

    private var track: some View {
        GeometryReader { geo in
            let inset: CGFloat = 10
            let usable = max(1, geo.size.width - 2 * inset)
            let n = lastRound
            let m = TrackMetrics(inset: inset, usable: usable, n: n)
            let labelEvery = n > 24 ? 5 : (n > 12 ? 2 : 1)
            let events = Set(store.events.map(\.round))
            ZStack(alignment: .topLeading) {
                Capsule().fill(Theme.seam).frame(width: usable, height: 4).offset(x: inset, y: 14)
                Capsule().fill(Theme.accent).frame(width: max(0, m.x(store.playhead) - inset), height: 4).offset(x: inset, y: 14)
                ForEach(0...n, id: \.self) { r in
                    let reached = r <= store.latestRound
                    Circle()
                        .fill(r <= store.playhead ? Theme.accent : (reached ? Theme.card : Theme.seam))
                        .overlay(Circle().strokeBorder(reached ? Theme.accent : Theme.seamStrong, lineWidth: 1))
                        .frame(width: 8, height: 8)
                        .position(x: m.x(r), y: 16)
                    if events.contains(r) {
                        Image(systemName: "bolt.fill")
                            .font(.system(size: 9))
                            .foregroundStyle(Theme.event)
                            .position(x: m.x(r), y: 3)
                            .help("An event was injected in round \(r)")
                    }
                    if r % labelEvery == 0 || r == n {
                        Text("r\(r)")
                            .font(Theme.mono(10, r == store.playhead ? .bold : .regular))
                            .foregroundStyle(r == store.playhead ? Theme.accent : Theme.muted)
                            .position(x: m.x(r), y: 31)
                    }
                }
                Circle()
                    .fill(Theme.card)
                    .overlay(Circle().strokeBorder(Theme.accent, lineWidth: 2.5))
                    .frame(width: 16, height: 16)
                    .shadow(color: Theme.cardShadow, radius: 1, y: 1)
                    .position(x: m.x(store.playhead), y: 16)
            }
            .contentShape(Rectangle())
            .gesture(
                DragGesture(minimumDistance: 0)
                    .onChanged { value in
                        let r = Int((((value.location.x - inset) / usable) * CGFloat(n)).rounded())
                        if replay.isPlaying { replay.pause() }
                        replay.scrub(to: min(max(0, r), n))
                    }
            )
            .accessibilityElement()
            .accessibilityLabel("Round")
            .accessibilityValue("\(store.playhead) of \(store.totalRounds)")
            .accessibilityAdjustableAction { direction in
                switch direction {
                case .increment: replay.step(1)
                case .decrement: replay.step(-1)
                @unknown default: break
                }
            }
        }
    }
}

/// Where each round's stop sits on the track.
private struct TrackMetrics {
    let inset: CGFloat
    let usable: CGFloat
    let n: Int

    func x(_ round: Int) -> CGFloat { inset + usable * CGFloat(round) / CGFloat(max(1, n)) }
}
