import SwiftUI

/// The run log, quietly: a line-numbered, monospaced list of every action and event in the Details tab. It scrolls with
/// the playhead (lines of later rounds are dimmed, not hidden), and clicking a line shows that agent or post in the
/// Crowd. "Nothing" actions are hidden unless asked for; injected events are the only amber lines.
struct RunLogView: View {
    let store: RunStore
    /// Selecting a line: the agent and (when the line made a post) the post.
    var select: (RunLogLine) -> Void

    @State private var lines: [RunLogLine] = []
    @AppStorage("runlog.showIdle") private var showIdle = false

    private struct Key: Hashable {
        var posts: Int
        var actions: Int
        var events: Int
        var personas: Int
        var failures: Int
        var state: RunState?
        var scored: Int
    }

    private var key: Key {
        Key(posts: store.posts.count, actions: store.actionsByRound.values.reduce(0) { $0 + $1.count },
            events: store.events.count, personas: store.personas.count, failures: store.batchFailures.count,
            state: store.status?.state, scored: store.posts.reduce(0) { $0 + ($1.sentiment == nil ? 0 : 1) })
    }

    private var shown: [RunLogLine] { showIdle ? lines : lines.filter { !$0.isIdle } }

    var body: some View {
        let shown = shown
        let idleCount = lines.filter(\.isIdle).count
        let godCount = lines.filter(\.isGod).count
        let actionCount = lines.filter { $0.kind == .action }.count
        VStack(spacing: 0) {
            HStack(spacing: Theme.space3) {
                // The same figures the web Studio prints: every line, every agent action (idle included), events, idle.
                Text("\(lines.count) lines, \(actionCount) actions, \(godCount) \(godCount == 1 ? "event" : "events"), \(idleCount) idle")
                    .font(Theme.small)
                    .foregroundStyle(Theme.muted)
                    .lineLimit(1)
                Spacer(minLength: Theme.space2)
                Toggle("Show idle", isOn: $showIdle)
                    .toggleStyle(.checkbox)
                    .controlSize(.small)
                    .font(Theme.small)
                    .help(showIdle ? "Hide the \(idleCount) idle lines" : "Show the \(idleCount) lines where an agent did nothing")
            }
            .padding(.horizontal, Theme.space4)
            .padding(.vertical, Theme.space2)
            Seam()
            if shown.isEmpty {
                Text(store.isLive ? "Actions appear here as agents take them." : "This run recorded no actions.")
                    .font(Theme.small)
                    .foregroundStyle(Theme.muted)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ScrollViewReader { proxy in
                    ScrollView {
                        LazyVStack(alignment: .leading, spacing: 0) {
                            ForEach(shown) { line in
                                RunLogRow(line: line, state: state(of: line)) { select(line) }
                                    .id(line.id)
                            }
                        }
                        .padding(.vertical, 4)
                    }
                    .scrollContentBackground(.hidden)
                    .onAppear { follow(proxy, shown: shown) }
                    .onChange(of: store.playhead) { _, _ in follow(proxy, shown: shown) }
                    .onChange(of: shown.count) { _, _ in follow(proxy, shown: shown) }
                    .onChange(of: showIdle) { _, _ in follow(proxy, shown: shown) }
                }
            }
        }
        .background(Theme.card)
        .task(id: key) { rebuild() }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Run log")
    }

    private func rebuild() {
        lines = RunLog.lines(personas: store.personas, posts: store.posts, actionsByRound: store.actionsByRound,
                             events: store.events, failedBatches: store.batchFailures, status: store.status)
    }

    /// Keeps the last line of the playhead round at the bottom edge.
    private func follow(_ proxy: ScrollViewProxy, shown: [RunLogLine]) {
        guard let last = shown.last(where: { $0.round <= store.playhead }) else { return }
        proxy.scrollTo(last.id, anchor: .bottom)
    }

    private func state(of line: RunLogLine) -> RunLogRow.RowState {
        RunLogRow.RowState(
            future: line.round > store.playhead,
            current: line.round == store.playhead,
            selectedPost: line.postID != nil && line.postID == store.selectedPostID,
            selectedAgent: line.agentID != nil && line.agentID == store.selectedAgentID)
    }
}

private struct RunLogRow: View {
    struct RowState: Equatable {
        var future: Bool
        var current: Bool
        var selectedPost: Bool
        var selectedAgent: Bool
    }

    let line: RunLogLine
    let state: RowState
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: Theme.space3) {
                Text(attributed)
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .frame(maxWidth: .infinity, alignment: .leading)
                sentimentCell
            }
                .padding(.horizontal, Theme.space4)
                .frame(height: 22)
                .background(background)
                .opacity(state.future ? 0.4 : 1)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .accessibilityLabel(line.plainText)
        .accessibilityHint(line.kind == .action ? "Selects this agent" : "Round \(line.round)")
        .accessibilityAddTraits(state.selectedPost || state.selectedAgent ? .isSelected : [])
    }

    private var background: Color {
        if state.selectedPost { return Theme.accentSoft }
        if state.selectedAgent { return Theme.accentSoft.opacity(0.5) }
        if hovering { return Theme.hover }
        return .clear
    }

    /// `  41  r4  #20  @firsttime_tessa      reply  -> #1 @revenuecat` in aligned columns, in ink and greys; only
    /// injected events are tinted.
    private var attributed: AttributedString {
        var out = AttributedString()
        func add(_ text: String, _ color: Color, bold: Bool = false) {
            var run = AttributedString(text)
            run.font = Theme.mono(12, bold ? .semibold : .regular)
            run.foregroundColor = color
            out += run
        }
        func pad(_ s: String, _ width: Int, trailing: Bool = true) -> String {
            let clipped = s.count > width ? String(s.prefix(max(1, width - 1))) + "…" : s
            let spaces = String(repeating: " ", count: max(0, width - clipped.count))
            return trailing ? clipped + spaces : spaces + clipped
        }
        add(pad(String(line.id), 4, trailing: false) + "  ", Theme.faint)
        switch line.kind {
        case .event:
            add(pad("r\(line.round)", 4) + "  ", Theme.event)
            add(pad("event", 24) + " ", Theme.event, bold: true)
            add(pad("", 8) + "  ", Theme.event)
            add(line.text, Theme.event)
            return out
        case .failedBatch, .end:
            // Failures are red; a stopped run ends in plain ink (stopping is not a failure).
            add(pad("r\(line.round)", 4) + "  ", Theme.muted)
            add(pad("–", 6) + " ", Theme.faint)
            add(pad(line.action, 24) + " ", line.action == "failed" ? Theme.negative : Theme.text, bold: true)
            add(line.text, line.action == "failed" ? Theme.negative : Theme.muted)
            return out
        case .action:
            break
        }
        add(pad("r\(line.round)", 4) + "  ", Theme.muted)
        add(pad(line.postID.map { "#\($0)" } ?? "–", 6) + " ", line.postID == nil ? Theme.faint : Theme.accent)
        add(pad("@" + line.handle, 24) + " ", line.isIdle ? Theme.faint : Theme.text)
        add(pad(line.action, 8) + "  ", line.isIdle ? Theme.faint : Theme.secondary, bold: !line.isIdle && line.action != "post")
        add(line.targetText, Theme.muted)
        return out
    }

    /// The created post's signed sentiment, right-aligned (never colour alone: the number is the reading).
    @ViewBuilder private var sentimentCell: some View {
        if line.kind == .action, let score = line.sentiment {
            SentimentValue(score: score, size: 12, weight: .regular)
        }
    }
}
