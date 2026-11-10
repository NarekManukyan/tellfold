import AppKit
import SwiftUI

/// "Suggest a better version": the run's text rewritten from the report, as a line diff, with each change linked to the
/// posts behind it. "Test this version" runs it past the same people at once (a study with this run).
struct RewriteSheet: View {
    let store: RunStore
    @Environment(AppModel.self) private var app
    @Environment(\.dismiss) private var dismiss
    /// The run view's link router (agent and post selection); links close the sheet first.
    @Environment(\.openURL) private var routeLink

    @State private var suggestion: RewriteSuggestion?
    @State private var original = ""
    /// The diff of `original` and the suggestion, worked out once per suggestion rather than on every redraw.
    @State private var lines: [LineDiff.Line] = []
    @State private var working = false
    @State private var testing = false
    @State private var errorMessage: String?

    private var linkifier: RunLinkifier {
        RunLinkifier(personas: store.personas, postIDs: Set(store.posts.map(\.id)))
    }

    /// What "Test this version" costs, as on the Try another version sheet: the rounds and the report, no new people.
    private var testCalls: Int? {
        guard let config = store.config else { return nil }
        return NewRunRequest.estimatedCallSplit(agents: config.numAgents, rounds: config.rounds,
                                                batch: config.batchSize, chunks: 1).agent + 1
    }

    var body: some View {
        VStack(alignment: .leading, spacing: Theme.space4) {
            VStack(alignment: .leading, spacing: 4) {
                Text("A better version")
                    .font(Theme.title)
                    .foregroundStyle(Theme.text)
                    .accessibilityAddTraits(.isHeader)
                Text("Rewritten from the report and the posts it cites. Test it to see how the same people take it.")
                    .font(Theme.small)
                    .foregroundStyle(Theme.muted)
            }
            if let suggestion {
                content(suggestion)
            } else if working {
                ProgressView("Writing a better version… This takes one model call.")
                    .frame(maxWidth: .infinity, minHeight: 300)
            } else {
                Spacer(minLength: 200)
            }
            if let errorMessage {
                Text(errorMessage).font(Theme.small).foregroundStyle(Theme.negative)
            }
            if suggestion != nil, let calls = testCalls {
                Text("Test this version uses about \(calls) model calls: the same people, the same rounds and a report.")
                    .font(Theme.caption)
                    .foregroundStyle(Theme.muted)
            }
            HStack {
                if suggestion != nil {
                    Button("Suggest again") { Task { await suggest() } }
                        .buttonStyle(.console)
                        .disabled(working || testing)
                    Button("Copy text") {
                        NSPasteboard.general.clearContents()
                        NSPasteboard.general.setString(suggestion?.revisedText ?? "", forType: .string)
                    }
                    .buttonStyle(.console)
                }
                Spacer()
                Button("Close", role: .cancel) { dismiss() }
                    .buttonStyle(.console)
                    .keyboardShortcut(.cancelAction)
                Button {
                    Task { await test() }
                } label: {
                    if testing { ProgressView().controlSize(.small) } else { Text("Test this version") }
                }
                .buttonStyle(.consolePrimary)
                .keyboardShortcut(.defaultAction)
                .disabled(suggestion == nil || working || testing || !store.canTryAnotherVersion
                          || !app.engine.state.isRunning)
                .help("Run this text past the same people and compare it with the original")
            }
        }
        .padding(20)
        .frame(width: 760, height: 640)
        .background(Theme.panel)
        .environment(\.openURL, OpenURLAction { url in
            dismiss()
            routeLink(url)
            return .handled
        })
        .task { await load() }
    }

    private func content(_ s: RewriteSuggestion) -> some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Theme.space4) {
                if !s.changes.isEmpty {
                    VStack(alignment: .leading, spacing: Theme.space2) {
                        Text("What changed").font(Theme.sectionTitle).foregroundStyle(Theme.text)
                        ForEach(s.changes) { change in
                            VStack(alignment: .leading, spacing: 2) {
                                Text(change.what).font(Theme.bodyStrong).foregroundStyle(Theme.text)
                                Text(linkifier.inline(change.why + citeSuffix(change), size: 13))
                                    .font(Theme.body).foregroundStyle(Theme.secondary)
                                    .fixedSize(horizontal: false, vertical: true).textSelection(.enabled)
                            }
                        }
                    }
                }
                if !s.keptOnPurpose.isEmpty {
                    VStack(alignment: .leading, spacing: 4) {
                        Text("Kept on purpose").font(Theme.ui(12, .semibold)).foregroundStyle(Theme.muted)
                        ForEach(Array(s.keptOnPurpose.enumerated()), id: \.offset) { _, note in
                            Text(linkifier.inline("• " + note, size: 13)).font(Theme.body).foregroundStyle(Theme.secondary)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                }
                if let line = s.citationAudit?.summaryLine {
                    Text(line).font(Theme.caption).foregroundStyle(Theme.muted)
                }
                VStack(alignment: .leading, spacing: 0) {
                    let counts = LineDiff.counts(lines)
                    Text("\(s.sourceName.isEmpty ? "Text" : s.sourceName): \(counts.removed) lines out, \(counts.added) in")
                        .font(Theme.ui(12, .semibold)).foregroundStyle(Theme.muted).padding(.bottom, 6)
                    ForEach(lines) { line in diffRow(line) }
                }
                .padding(Theme.space3)
                .background(Theme.card, in: RoundedRectangle(cornerRadius: Theme.radius))
                .overlay(RoundedRectangle(cornerRadius: Theme.radius).strokeBorder(Theme.seam))
            }
        }
    }

    private func citeSuffix(_ change: RewriteChange) -> String {
        let missing = change.cites.filter { !change.why.contains("#\($0)") }
        return missing.isEmpty ? "" : " (" + missing.map { "#\($0)" }.joined(separator: ", ") + ")"
    }

    private func diffRow(_ line: LineDiff.Line) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            Text(line.kind == .added ? "+" : line.kind == .removed ? "−" : " ")
                .font(Theme.mono(12, .semibold))
                .foregroundStyle(line.kind == .added ? Theme.positive : Theme.negative)
                .frame(width: 10)
            Text(line.text.isEmpty ? " " : line.text)
                .font(Theme.mono(12))
                .foregroundStyle(line.kind == .same ? Theme.secondary : Theme.text)
                .strikethrough(line.kind == .removed, color: Theme.negative.opacity(0.5))
                .fixedSize(horizontal: false, vertical: true)
                .textSelection(.enabled)
            Spacer(minLength: 0)
        }
        .padding(.vertical, 1)
        .background(line.kind == .added ? Theme.positive.opacity(0.08) : line.kind == .removed ? Theme.negativeSoft : .clear)
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(line.kind == .added ? "Added" : line.kind == .removed ? "Removed" : "Unchanged"): \(line.text)")
    }

    private func load() async {
        if let api = app.api, let saved = try? await api.savedRewrite(runID: store.runID) {
            await loadOriginal(saved.sourceName)
            show(saved)
        } else {
            await suggest()
        }
    }

    private func show(_ s: RewriteSuggestion) {
        lines = LineDiff.diff(old: original, new: s.revisedText)
        suggestion = s
    }

    private func loadOriginal(_ name: String) async {
        guard let api = app.api, !name.isEmpty, let data = try? await api.seedData(runID: store.runID, name: name) else { return }
        original = String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private func suggest() async {
        guard let api = app.api else { errorMessage = "The engine is not running."; return }
        working = true
        errorMessage = nil
        defer { working = false }
        do {
            let fresh = try await api.rewrite(runID: store.runID)
            await loadOriginal(fresh.sourceName)
            show(fresh)
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    private func test() async {
        guard let suggestion, let config = store.config else { return }
        testing = true
        defer { testing = false }
        do {
            try await app.testVersion(runID: store.runID, config: config, suggestion: suggestion)
            dismiss()
        } catch {
            errorMessage = error.localizedDescription
        }
    }
}
