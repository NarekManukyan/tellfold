import Charts
import SwiftUI

/// A study: versions of one announcement read by the same crowd. Both sentiment curves on one chart, the camps side by
/// side and the cited comparison. `A#12` citations open post 12 in run A.
struct StudyView: View {
    @Bindable var store: StudyStore
    @Environment(AppModel.self) private var app

    var body: some View {
        Group {
            if let detail = store.detail {
                ScrollView {
                    VStack(alignment: .leading, spacing: Theme.space5) {
                        header(detail)
                        comparison(detail)
                        curves(detail)
                        camps(detail)
                    }
                    .padding(Theme.space5)
                    .frame(maxWidth: 1080, alignment: .leading)
                    .frame(maxWidth: .infinity)
                }
            } else if let error = store.loadError {
                ContentUnavailableView {
                    Label("Could Not Load This Study", systemImage: "exclamationmark.triangle")
                } description: {
                    Text(error)
                } actions: {
                    Button("Try Again") { Task { await store.load() } }
                }
            } else {
                ProgressView("Loading study…").frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .background(Theme.ground)
        .environment(\.openURL, OpenURLAction { url in open(url) })
        .task(id: store.studyID) { await store.load() }
        .onDisappear { store.stop() }
        .alert("Action Failed", isPresented: Binding(get: { store.actionError != nil },
                                                     set: { if !$0 { store.actionError = nil } })) {
            Button("OK", role: .cancel) { store.actionError = nil }
        } message: {
            Text(store.actionError ?? "")
        }
    }

    // MARK: header

    private func header(_ detail: StudyDetail) -> some View {
        VStack(alignment: .leading, spacing: Theme.space3) {
            Text("Study")
                .font(Theme.ui(12, .semibold))
                .foregroundStyle(Theme.muted)
            Text(detail.runs.first?.question ?? "")
                .font(Theme.title)
                .foregroundStyle(Theme.text)
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: Theme.space2) {
                ForEach(Array(detail.runs.enumerated()), id: \.element.id) { index, run in
                    Button {
                        app.selectedRunID = run.id
                    } label: {
                        HStack(spacing: 6) {
                            Circle().fill(Self.color(index)).frame(width: 8, height: 8)
                            Text("\(run.letter)  \(run.variant)")
                            if run.state.isActive { ProgressView().controlSize(.mini) }
                        }
                    }
                    .buttonStyle(.console(.secondary, compact: true))
                    .help("Open \(run.variant)")
                }
                Spacer(minLength: 0)
            }
            Text("The same people read each version. One run per version is a single sample: part of any difference is chance.")
                .font(Theme.small)
                .foregroundStyle(Theme.muted)
        }
    }

    // MARK: comparison

    @ViewBuilder private func comparison(_ detail: StudyDetail) -> some View {
        VStack(alignment: .leading, spacing: Theme.space3) {
            SectionTitle("Comparison") {
                if store.phase == .compared || store.phase == .readyToCompare {
                    Button(store.phase == .compared ? "Compare again" : "Compare") { Task { await store.compare() } }
                        .buttonStyle(.console(.secondary, compact: true))
                        .disabled(!app.engine.state.isRunning)
                }
            }
            switch store.phase {
            case .running?:
                waiting("Running the versions. The comparison is written when every version finishes.")
            case .comparing?:
                waiting("Writing the comparison…")
            case .notEnoughRuns?:
                Text("A comparison needs at least two finished versions with a report.")
                    .font(Theme.body).foregroundStyle(Theme.muted)
            case .readyToCompare?:
                Text(detail.compareState.error.map { "The last comparison failed: \($0)" }
                     ?? "Every version finished. Compare them to see which one the crowd took better.")
                    .font(Theme.body)
                    .foregroundStyle(detail.compareState.error == nil ? Theme.muted : Theme.negative)
            case .compared?, nil:
                if let report = detail.compare?.report { compareReport(report, detail) }
            }
        }
        .card()
    }

    private func waiting(_ text: String) -> some View {
        HStack(spacing: Theme.space2) {
            ProgressView().controlSize(.small)
            Text(text).font(Theme.body).foregroundStyle(Theme.muted)
        }
    }

    private func compareReport(_ report: CompareReport, _ detail: StudyDetail) -> some View {
        VStack(alignment: .leading, spacing: Theme.space4) {
            Text(cited(report.headline, size: 18))
                .font(Theme.ui(18, .semibold))
                .foregroundStyle(Theme.text)
                .fixedSize(horizontal: false, vertical: true)
            Label(report.betterVariant.map { "Better received: \($0)" } ?? "No clear winner",
                  systemImage: report.betterVariant == nil ? "equal.circle" : "checkmark.seal")
                .font(Theme.bodyStrong)
                .foregroundStyle(report.betterVariant == nil ? Theme.secondary : Theme.positive)
            if !report.why.isEmpty {
                Text(cited(report.why)).font(Theme.ui(14)).foregroundStyle(Theme.text).lineSpacing(3)
                    .fixedSize(horizontal: false, vertical: true).textSelection(.enabled)
            }
            HStack(alignment: .top, spacing: Theme.space4) {
                ForEach(report.perVariant) { item in
                    VStack(alignment: .leading, spacing: Theme.space2) {
                        Text(item.variant).font(Theme.bodyStrong).foregroundStyle(Theme.text)
                        Text(cited(item.summary, size: 13)).font(Theme.body).foregroundStyle(Theme.secondary)
                            .fixedSize(horizontal: false, vertical: true).textSelection(.enabled)
                        bullets("Objections", item.objections)
                        if let only = report.objectionsOnlyIn.first(where: { $0.variant == item.variant }) {
                            bullets("Only against this version", only.objections)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            if !report.caveats.isEmpty { bullets("Caveats", report.caveats) }
            if let line = report.citationAudit?.summaryLine {
                Text(line).font(Theme.caption).foregroundStyle(Theme.muted)
            }
        }
    }

    @ViewBuilder private func bullets(_ title: String, _ items: [String]) -> some View {
        if !items.isEmpty {
            VStack(alignment: .leading, spacing: 4) {
                Text(title).font(Theme.ui(12, .semibold)).foregroundStyle(Theme.muted)
                ForEach(Array(items.enumerated()), id: \.offset) { _, item in
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        Text("•").foregroundStyle(Theme.faint)
                        Text(cited(item, size: 13)).font(Theme.body).foregroundStyle(Theme.secondary)
                            .fixedSize(horizontal: false, vertical: true).textSelection(.enabled)
                    }
                }
            }
        }
    }

    private func cited(_ text: String, size: CGFloat = 14) -> AttributedString {
        let options = AttributedString.MarkdownParsingOptions(interpretedSyntax: .inlineOnlyPreservingWhitespace,
                                                              failurePolicy: .returnPartiallyParsedIfPossible)
        var result = (try? AttributedString(markdown: StudyLinks.linkify(text, letters: store.letters), options: options))
            ?? AttributedString(text)
        for run in result.runs where run.link != nil {
            result[run.range].font = Theme.mono(size * 0.92)
            result[run.range].foregroundColor = Theme.link
        }
        return result
    }

    // MARK: sentiment curves

    private func curves(_ detail: StudyDetail) -> some View {
        let series = StudyLogic.curves(detail)
        return VStack(alignment: .leading, spacing: Theme.space3) {
            SectionTitle("Sentiment by round")
            if series.allSatisfy({ $0.points.isEmpty }) {
                Text("No round has been scored yet.").font(Theme.body).foregroundStyle(Theme.muted)
            } else {
                Chart {
                    RuleMark(y: .value("Neutral", 0))
                        .foregroundStyle(Theme.seamStrong)
                        .lineStyle(StrokeStyle(lineWidth: 1))
                    ForEach(series, id: \.run.id) { item in
                        ForEach(item.points, id: \.round) { p in
                            LineMark(x: .value("Round", p.round), y: .value("Sentiment", p.sentiment),
                                     series: .value("Version", item.run.variant))
                                .foregroundStyle(by: .value("Version", "\(item.run.letter)  \(item.run.variant)"))
                                .interpolationMethod(.monotone)
                            PointMark(x: .value("Round", p.round), y: .value("Sentiment", p.sentiment))
                                .foregroundStyle(by: .value("Version", "\(item.run.letter)  \(item.run.variant)"))
                                .symbolSize(36)
                                .accessibilityLabel("\(item.run.variant), round \(p.round)")
                                .accessibilityValue("Sentiment \(RunUI.sentimentText(p.sentiment))")
                        }
                    }
                }
                .chartForegroundStyleScale(domain: series.map { "\($0.run.letter)  \($0.run.variant)" },
                                           range: series.indices.map { Self.color($0) })
                .chartYScale(domain: -1...1)
                .chartXAxisLabel("Round")
                .frame(height: 220)
            }
        }
        .card()
    }

    // MARK: camps

    private func camps(_ detail: StudyDetail) -> some View {
        VStack(alignment: .leading, spacing: Theme.space3) {
            SectionTitle("Camps")
            HStack(alignment: .top, spacing: Theme.space4) {
                ForEach(Array(detail.runs.enumerated()), id: \.element.id) { index, run in
                    VStack(alignment: .leading, spacing: Theme.space2) {
                        HStack(spacing: 6) {
                            Circle().fill(Self.color(index)).frame(width: 8, height: 8)
                            Text(run.variant).font(Theme.bodyStrong).foregroundStyle(Theme.text)
                            Spacer(minLength: 0)
                            SentimentValue(score: run.digest.finalSentiment)
                        }
                        if run.digest.camps.isEmpty {
                            Text(run.state.isActive ? "Still running." : "No camps in this run's report.")
                                .font(Theme.small).foregroundStyle(Theme.muted)
                        }
                        ForEach(run.digest.camps) { camp in
                            HStack(alignment: .firstTextBaseline, spacing: 6) {
                                Text(camp.name).font(Theme.body).foregroundStyle(Theme.secondary).lineLimit(2)
                                Spacer(minLength: 4)
                                Text("\(camp.size)").font(Theme.mono(12)).foregroundStyle(Theme.muted)
                                SentimentValue(score: camp.sentiment)
                            }
                            .help(camp.summary)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
        }
        .card()
    }

    // MARK: links

    static func color(_ index: Int) -> Color { Color(hex: Theme.campHexes[index % Theme.campHexes.count]) }

    private func open(_ url: URL) -> OpenURLAction.Result {
        guard let target = StudyLinks.target(url) else {
            return url.scheme == RunUI.linkScheme ? .handled : .systemAction
        }
        if let detail = store.detail, let run = StudyLogic.run(letter: target.letter, in: detail) {
            app.openPost(runID: run.id, postID: target.postID)
        }
        return .handled
    }
}
