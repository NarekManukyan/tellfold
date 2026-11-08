import AppKit
import SwiftUI

/// The full report in a readable column: verdict, prediction with its confidence, the narrative sections, key voices,
/// risks and caveats, with `@handle` and `#id` citations as links; plus an "Ask about this outcome" field, Rebuild and
/// the standalone page.
struct ReportView: View {
    let store: RunStore

    @Environment(AppModel.self) private var app
    @State private var question = ""
    @State private var answers: [QA] = []
    @State private var asking = false
    @State private var rebuilding = false
    @State private var confirmRebuild = false
    @State private var actionError: String?
    @FocusState private var askFocused: Bool

    struct QA: Identifiable {
        let id = UUID()
        let question: String
        var answer: String?
        var error: String?
    }

    private static let column: CGFloat = 700

    var body: some View {
        Group {
            if let report = store.detail?.report {
                ScrollView {
                    VStack(alignment: .leading, spacing: Theme.space5) {
                        header(report)
                        content(report)
                        Seam()
                        askPanel
                    }
                    .frame(maxWidth: Self.column, alignment: .leading)
                    .padding(.horizontal, Theme.space5)
                    .padding(.top, Theme.space6)
                    .padding(.bottom, Theme.space6 + Theme.space4)
                    .frame(maxWidth: .infinity)
                }
                .background(Theme.ground)
            } else {
                emptyState
            }
        }
        .confirmationDialog("Rebuild the report?", isPresented: $confirmRebuild) {
            Button("Rebuild Report") { Task { await rebuild() } }
        } message: {
            Text("The planner model writes a new report from this run. This makes a few model calls.")
        }
        .alert("Report action failed", isPresented: Binding(get: { actionError != nil }, set: { if !$0 { actionError = nil } })) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(actionError ?? "")
        }
    }

    private var linkifier: RunLinkifier {
        RunLinkifier(personas: store.detail?.personas ?? [], postIDs: Set(store.posts.map(\.id)))
    }

    // MARK: sections

    private func header(_ report: Report) -> some View {
        VStack(alignment: .leading, spacing: Theme.space3) {
            HStack(spacing: Theme.space2) {
                StatusChip(state: store.status?.state ?? .unknown)
                Spacer(minLength: Theme.space3)
                actionButtons
            }
            Text(report.title)
                .font(Theme.display)
                .foregroundStyle(Theme.text)
                .lineSpacing(2)
                .fixedSize(horizontal: false, vertical: true)
                .textSelection(.enabled)
                .accessibilityAddTraits(.isHeader)
            Text("A scenario exploration of a simulated crowd, not a poll or measurement of real people.")
                .font(Theme.small)
                .foregroundStyle(Theme.muted)
        }
    }

    private var actionButtons: some View {
        HStack(spacing: 8) {
            if store.controls.rebuildReport {
                Button {
                    confirmRebuild = true
                } label: {
                    if rebuilding || store.detail?.reportRebuild.running == true {
                        HStack(spacing: 6) {
                            ProgressView().controlSize(.small)
                            Text("Rebuilding…")
                        }
                    } else {
                        Label("Rebuild", systemImage: "arrow.clockwise")
                    }
                }
                .buttonStyle(.console(.secondary, compact: true))
                .disabled(rebuilding || store.isLive || store.detail?.reportRebuild.running == true)
                .help(store.isLive ? "Available once the run has finished" : "Write the report again from this run")
            }

            Button {
                Task {
                    do { try await app.openHTMLReport(store.runID) } catch { actionError = error.localizedDescription }
                }
            } label: {
                Label("Web page", systemImage: "arrow.up.right.square")
            }
            .buttonStyle(.console(.secondary, compact: true))
            .help("Open the standalone HTML report")
        }
    }

    @ViewBuilder
    private func content(_ report: Report) -> some View {
        let linkifier = linkifier
        if !report.summary.isEmpty {
            RunMarkdownView(markdown: report.summary, linkifier: linkifier, size: 17, spacing: 12)
                .foregroundStyle(Theme.text)
        }

        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: Theme.space2) {
                Text("Prediction").font(Theme.headline).foregroundStyle(Theme.text).accessibilityAddTraits(.isHeader)
                Spacer()
                ConfidenceBadge(confidence: report.confidence)
            }
            RunMarkdownView(markdown: report.prediction, linkifier: linkifier, size: 15, spacing: 10)
                .foregroundStyle(Theme.text)
        }
        .card(padding: Theme.space4)

        ForEach(Array(report.sections.enumerated()), id: \.offset) { _, s in
            section(s.title, level: .primary) {
                RunMarkdownView(markdown: s.bodyMarkdown, linkifier: linkifier, size: 15, spacing: 10)
                    .foregroundStyle(Theme.text)
            }
        }

        if !report.keyVoices.isEmpty {
            section("Key voices", level: .secondary) {
                RunFlowLayout(spacing: 14, lineSpacing: 8) {
                    ForEach(report.keyVoices, id: \.self) { id in
                        let persona = store.detail?.personas.first { $0.id == id || $0.handle == id }
                        HStack(spacing: 6) {
                            if let persona { AvatarView(persona: persona, size: 22) }
                            RunHandleChip(handle: persona?.handle ?? id, sentiment: nil) {
                                store.selectedAgentID = persona?.id ?? id
                            }
                        }
                    }
                }
            }
        }

        if !report.risks.isEmpty {
            section("Risks", level: .secondary) {
                bulletList(report.risks, linkifier: linkifier)
            }
        }
        if !report.caveats.isEmpty {
            section("Caveats", level: .secondary) {
                bulletList(report.caveats, linkifier: linkifier)
                    .foregroundStyle(Theme.muted)
            }
        }
    }

    private enum HeadingLevel { case primary, secondary }

    private func section<Content: View>(_ title: String, level: HeadingLevel, @ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: level == .primary ? 10 : 8) {
            Text(title)
                .font(level == .primary ? Theme.title : Theme.headline)
                .foregroundStyle(Theme.text)
                .accessibilityAddTraits(.isHeader)
            content()
        }
    }

    private func bulletList(_ items: [String], linkifier: RunLinkifier) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            ForEach(Array(items.enumerated()), id: \.offset) { _, item in
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text("•").foregroundStyle(Theme.faint)
                    Text(linkifier.inline(item, size: 15))
                        .font(Theme.ui(15))
                        .lineSpacing(4)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
    }

    // MARK: ask

    private var askPanel: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Ask about this outcome")
                .font(Theme.headline)
                .foregroundStyle(Theme.text)
                .accessibilityAddTraits(.isHeader)
            ForEach(answers) { qa in
                VStack(alignment: .leading, spacing: 6) {
                    Text(qa.question)
                        .font(Theme.ui(13, .semibold))
                        .foregroundStyle(Theme.text)
                    if let answer = qa.answer {
                        RunMarkdownView(markdown: answer, linkifier: linkifier, size: 14, spacing: 8)
                            .foregroundStyle(Theme.text)
                    } else if let error = qa.error {
                        Label(error, systemImage: "xmark.octagon.fill")
                            .foregroundStyle(Theme.negative)
                            .font(Theme.small)
                    } else {
                        HStack(spacing: 6) {
                            ProgressView().controlSize(.small)
                            Text("Thinking…").foregroundStyle(Theme.muted)
                        }
                        .accessibilityLabel("Waiting for the answer")
                    }
                }
                .padding(12)
                .frame(maxWidth: .infinity, alignment: .leading)
                .card(padding: 0)
            }
            HStack(spacing: 8) {
                TextField("Why did sentiment turn after round 3?", text: $question, axis: .vertical)
                    .lineLimit(1...4)
                    .consoleField()
                    .focused($askFocused)
                    .onSubmit { Task { await ask() } }
                    .disabled(asking)
                    .accessibilityLabel("Question about the outcome")
                Button("Ask") { Task { await ask() } }
                    .buttonStyle(.consolePrimary)
                    .disabled(asking || question.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
        }
    }

    private func ask() async {
        let q = question.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !q.isEmpty, !asking else { return }
        asking = true
        question = ""
        answers.append(QA(question: q))
        let index = answers.count - 1
        do {
            let reply = try await store.ask(q)
            answers[index].answer = reply
        } catch {
            answers[index].error = error.localizedDescription
        }
        asking = false
        askFocused = true
    }

    private func rebuild() async {
        guard !rebuilding else { return }
        rebuilding = true
        defer { rebuilding = false }
        await ReportRebuilder.rebuild(store)  // failures land in store.actionError (shown by the run view)
    }

    // MARK: empty

    @ViewBuilder
    private var emptyState: some View {
        if store.isLive {
            ContentUnavailableView {
                Label("Report in Progress", systemImage: "doc.richtext")
            } description: {
                Text("The report is written after the last round.")
            } actions: {
                ProgressView().controlSize(.small)
            }
        } else if rebuilding || store.detail?.reportRebuild.running == true {
            ContentUnavailableView {
                Label("Writing the Report", systemImage: "doc.richtext")
            } description: {
                Text("The planner model is writing a new report from this run.")
            } actions: {
                ProgressView().controlSize(.small)
            }
        } else {
            ContentUnavailableView {
                Label("No Report", systemImage: "doc.richtext")
            } description: {
                Text("This run has no report. Build one from the rounds that completed.")
            } actions: {
                Button("Build Report") { Task { await rebuild() } }
                    .buttonStyle(.consolePrimary)
                    .disabled(store.latestRound == 0)
            }
        }
    }
}
