import AppKit
import SwiftUI

/// The full report in a readable column: verdict, prediction with its confidence, the narrative sections, what to do
/// next, key voices, risks and caveats, with `@handle` and `#id` citations as links; plus an "Ask about this outcome" field, Rebuild, the
/// standalone page, PDF export and the run folder.
struct ReportView: View {
    let store: RunStore

    @Environment(AppModel.self) private var app
    @State private var question = ""
    @State private var answers: [QA] = []
    @State private var asking = false
    @State private var rebuilding = false
    @State private var confirmRebuild = false
    @State private var actionError: String?
    @State private var showsRewrite = false
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
        .sheet(isPresented: $showsRewrite) {
            RewriteSheet(store: store)
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
            Button {
                showsRewrite = true
            } label: {
                Label("Suggest a better version", systemImage: "wand.and.stars")
            }
            .buttonStyle(.console(.secondary, compact: true))
            .disabled(store.isLive || !app.engine.state.isRunning)
            .help("Rewrite the text from what the crowd objected to, then test it against the same people")

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

            Button {
                Task { await exportPDF() }
            } label: {
                if app.exportingPDF == store.runID {
                    HStack(spacing: 6) {
                        ProgressView().controlSize(.small)
                        Text("Exporting…")
                    }
                } else {
                    Label("Export PDF…", systemImage: "doc.richtext")
                }
            }
            .buttonStyle(.console(.secondary, compact: true))
            .disabled(app.exportingPDF != nil)
            .help("Save the report as a PDF to share or print")

            Button {
                app.revealInFinder(store.runID)
            } label: {
                Label("Reveal in Finder", systemImage: "folder")
                    .labelStyle(.iconOnly)
            }
            .buttonStyle(.console(.secondary, compact: true))
            .help("Show the run folder in Finder")
        }
    }

    private func exportPDF() async {
        let title = store.detail?.report?.title ?? store.runID
        do { try await app.exportPDF(store.runID, title: title) } catch { actionError = error.localizedDescription }
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
            if let line = report.citationAudit?.summaryLine {
                Label(line, systemImage: report.citationAudit?.marked == 0 ? "checkmark.seal" : "exclamationmark.triangle")
                    .font(Theme.small)
                    .foregroundStyle(Theme.muted)
                    .help("Every #post, @agent and quote in this report was checked against the simulation. "
                          + "Citations that did not match were replaced with [unverified].")
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

        let steps = report.visibleRecommendations
        if !steps.isEmpty {
            let groups = CampGrouping.groups(report: report, personas: store.personas).groups
            section("What to do next", level: .primary) {
                Text(Recommendation.note)
                    .font(Theme.small)
                    .foregroundStyle(Theme.muted)
                    .fixedSize(horizontal: false, vertical: true)
                VStack(alignment: .leading, spacing: Theme.space3) {
                    ForEach(Array(steps.enumerated()), id: \.offset) { _, step in
                        RecommendationCard(store: store, step: step, linkifier: linkifier,
                                           groups: groups,
                                           tryChange: tryChange(step))
                    }
                }
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

    /// Opens "Try this change": a new version of this run with the same people and text and the step announced at
    /// round 1, compared with this run in a study. Nil when the step is not a change to the plan or the run is still
    /// going.
    private func tryChange(_ step: Recommendation) -> (() -> Void)? {
        guard step.action.canTry, !store.isLive, let config = store.config else { return nil }
        return { app.tryChange(runID: store.runID, config: config, step: step) }
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

// MARK: - what to do next

/// One "What to do next" step: the kind of step with its icon, the title, its confidence, why, the likely effect, who
/// it is for and the posts and people behind it.
private struct RecommendationCard: View {
    let store: RunStore
    let step: Recommendation
    let linkifier: RunLinkifier
    let groups: [CampGroup]
    let tryChange: (() -> Void)?

    @Environment(\.runNavigate) private var navigate

    static func tint(_ action: RecommendationAction) -> Color {
        switch action {
        case .keep: Theme.positive
        case .change: Theme.event
        case .add: Theme.accent
        case .drop: Theme.negative
        case .test: Theme.repost
        case .unknown: Theme.muted
        }
    }

    var body: some View {
        let tint = Self.tint(step.action)
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .firstTextBaseline, spacing: Theme.space2) {
                // The label is drawn in text ink for contrast; the tint is on the icon, the chip and the edge bar.
                Label {
                    Text(step.action.title).foregroundStyle(Theme.text)
                } icon: {
                    Image(systemName: step.action.symbol).foregroundStyle(tint)
                }
                .font(Theme.ui(12, .semibold))
                .padding(.horizontal, 8)
                .padding(.vertical, 3)
                .background(tint.opacity(0.12), in: Capsule())
                .fixedSize()
                .help(step.action.meaning)
                .accessibilityLabel("\(step.action.title): \(step.action.meaning)")
                Text(linkifier.inline(step.title, size: 15))
                    .font(Theme.ui(15, .semibold))
                    .foregroundStyle(Theme.text)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
                    .accessibilityAddTraits(.isHeader)
                Spacer(minLength: Theme.space2)
                if step.confidence != .unknown {
                    ConfidenceBadge(confidence: step.confidence)
                }
            }
            if !step.why.isEmpty {
                Text(linkifier.inline(step.why, size: 14))
                    .font(Theme.ui(14))
                    .foregroundStyle(Theme.text)
                    .lineSpacing(3)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
            }
            if !step.expectedEffect.isEmpty {
                (Text("Likely effect: ").font(Theme.ui(14, .semibold)) + Text(linkifier.inline(step.expectedEffect, size: 14)))
                    .font(Theme.ui(14))
                    .foregroundStyle(Theme.secondary)
                    .lineSpacing(3)
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
            }
            VStack(alignment: .leading, spacing: 6) {
                if !step.addresses.isEmpty {
                    row("For") { ForEach(step.addresses, id: \.self) { addressee($0) } }
                }
                row("Based on") {
                    ForEach(step.evidence.filter { !$0.trimmingCharacters(in: .whitespaces).isEmpty }, id: \.self) { ref in
                        Text(linkifier.inline(ref, size: 12)).font(Theme.mono(12))
                    }
                }
            }
            if let tryChange {
                Button(action: tryChange) {
                    Label("Try this change", systemImage: "arrow.triangle.branch")
                }
                .buttonStyle(.console(.secondary, compact: true))
                .help("Run this crowd again with this change announced in round 1, then compare the two runs")
            }
        }
        .padding(Theme.space4)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.card, in: RoundedRectangle(cornerRadius: Theme.cardRadius))
        .overlay(alignment: .leading) {
            UnevenRoundedRectangle(topLeadingRadius: Theme.cardRadius, bottomLeadingRadius: Theme.cardRadius)
                .fill(tint)
                .frame(width: 3)
                .accessibilityHidden(true)
        }
        .overlay(RoundedRectangle(cornerRadius: Theme.cardRadius).strokeBorder(Theme.seam, lineWidth: 1))
        .accessibilityElement(children: .contain)
    }

    private func row<Content: View>(_ label: String, @ViewBuilder content: () -> Content) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: Theme.space2) {
            Text(label)
                .font(Theme.small)
                .foregroundStyle(Theme.muted)
                .frame(width: 62, alignment: .leading)
            RunFlowLayout(spacing: 10, lineSpacing: 6) { content() }
        }
    }

    @ViewBuilder
    private func addressee(_ id: String) -> some View {
        if let persona = store.detail?.personas.first(where: { $0.id == id }) {
            HStack(spacing: 5) {
                AvatarView(persona: persona, size: 18)
                RunHandleChip(handle: persona.handle, sentiment: nil) { store.selectedAgentID = persona.id }
            }
        } else if let group = groups.first(where: { $0.name == id }) {
            Button {
                store.selectedAgentID = nil
                store.selectedCampID = group.id
                navigate(.crowd)
            } label: {
                HStack(spacing: 5) {
                    Circle().fill(group.color).frame(width: 8, height: 8)
                    Text(group.name).font(Theme.ui(12, .medium)).foregroundStyle(Theme.text)
                }
            }
            .buttonStyle(.plain)
            .help("Show this camp in the crowd")
        } else {
            Text(id).font(Theme.ui(12)).foregroundStyle(Theme.secondary)
        }
    }
}
