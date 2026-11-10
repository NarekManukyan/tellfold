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
                        if detail.isRepeat {
                            agreement(detail)
                            band(detail)
                        } else {
                            comparison(detail)
                            curves(detail)
                        }
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
            Text(detail.isRepeat ? "Check it" : "Study")
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
                if let note = StudyLogic.seriesNote(detail) {
                    Text(note).font(Theme.small).foregroundStyle(Theme.muted)
                }
                Spacer(minLength: 0)
            }
            Text(detail.isRepeat
                 ? "The same people read the same text in every run; only chance differs. What holds in all of them is worth trusting more."
                 : "The same people read each version. One run per version is a single sample: part of any difference is chance.")
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

    // MARK: check it

    @ViewBuilder private func agreement(_ detail: StudyDetail) -> some View {
        VStack(alignment: .leading, spacing: Theme.space3) {
            SectionTitle("Did it hold up?") {
                if store.phase == .compared || store.phase == .readyToCompare {
                    Button(store.phase == .compared ? "Check again" : "Check") { Task { await store.compare() } }
                        .buttonStyle(.console(.secondary, compact: true))
                        .disabled(!app.engine.state.isRunning)
                }
            }
            switch store.phase {
            case .running?:
                waiting("Running \(max(detail.planned, detail.runs.count)) runs one after another. The check is written after the last one.")
            case .comparing?:
                waiting("Checking what held…")
            case .notEnoughRuns?:
                Text(detail.compareState.error.map { "The check stopped: \($0)" }
                     ?? "A check needs at least two finished runs with a report.")
                    .font(Theme.body).foregroundStyle(Theme.muted)
            case .readyToCompare?:
                Text(detail.compareState.error.map { "The check stopped: \($0). Check the runs that finished to see what held." }
                     ?? (detail.planned > detail.runs.count ? "Check the runs that finished to see what held." : "Every run finished."))
                    .font(Theme.body)
                    .foregroundStyle(detail.compareState.error == nil ? Theme.muted : Theme.negative)
            case .compared?, nil:
                if let a = detail.agreement { agreementBody(a) }
            }
        }
        .card()
    }

    private func agreementBody(_ a: Agreement) -> some View {
        VStack(alignment: .leading, spacing: Theme.space4) {
            Label(StudyLogic.verdictTitle(a.verdict, runs: a.runs), systemImage: verdictSymbol(a.verdict))
                .font(Theme.ui(18, .semibold))
                .foregroundStyle(verdictColor(a.verdict))
            if !a.summary.isEmpty {
                Text(a.summary).font(Theme.ui(14)).foregroundStyle(Theme.text)
                    .fixedSize(horizontal: false, vertical: true).textSelection(.enabled)
            }
            if let range = StudyLogic.finalRange(a) {
                Text("Final sentiment across the runs: \(range)").font(Theme.small).foregroundStyle(Theme.muted)
            }
            HStack(alignment: .top, spacing: Theme.space4) {
                bullets("Held in every run", a.held)
                    .frame(maxWidth: .infinity, alignment: .leading)
                bullets("Only in some runs", a.split)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            if !a.camps.isEmpty {
                VStack(alignment: .leading, spacing: 4) {
                    Text("Camps").font(Theme.ui(12, .semibold)).foregroundStyle(Theme.muted)
                    ForEach(a.camps) { camp in
                        HStack(spacing: 6) {
                            Text(camp.label).font(Theme.body).foregroundStyle(Theme.secondary)
                            Spacer(minLength: 4)
                            Text("in \(camp.runs) of \(a.runs) runs").font(Theme.mono(12))
                                .foregroundStyle(camp.runs == a.runs ? Theme.positive : Theme.muted)
                        }
                        .help(camp.names.joined(separator: "\n"))
                    }
                }
            }
        }
    }

    private func verdictSymbol(_ v: AgreementVerdict) -> String {
        switch v {
        case .consistent: "checkmark.seal"
        case .mixed: "circle.lefthalf.filled"
        case .unstable: "exclamationmark.triangle"
        case .unknown: "questionmark.circle"
        }
    }

    private func verdictColor(_ v: AgreementVerdict) -> Color {
        switch v {
        case .consistent: Theme.positive
        case .mixed: Theme.event
        case .unstable: Theme.negative
        case .unknown: Theme.secondary
        }
    }

    /// The band between the lowest and highest run per round, with every run's own line on top.
    private func band(_ detail: StudyDetail) -> some View {
        let series = StudyLogic.curves(detail)
        let bands = StudyLogic.bands(detail)
        return VStack(alignment: .leading, spacing: Theme.space3) {
            SectionTitle("Sentiment by round, all runs")
            if bands.isEmpty {
                Text("No round has been scored yet.").font(Theme.body).foregroundStyle(Theme.muted)
            } else {
                Chart {
                    RuleMark(y: .value("Neutral", 0))
                        .foregroundStyle(Theme.seamStrong)
                        .lineStyle(StrokeStyle(lineWidth: 1))
                    ForEach(bands, id: \.round) { b in
                        AreaMark(x: .value("Round", b.round), yStart: .value("Lowest", b.low),
                                 yEnd: .value("Highest", b.high))
                            .foregroundStyle(Theme.accentSoft)
                            .interpolationMethod(.monotone)
                            .accessibilityLabel("Round \(b.round)")
                            .accessibilityValue("\(RunUI.sentimentText(b.low)) to \(RunUI.sentimentText(b.high))")
                    }
                    ForEach(series, id: \.run.id) { item in
                        ForEach(item.points, id: \.round) { p in
                            LineMark(x: .value("Round", p.round), y: .value("Sentiment", p.sentiment),
                                     series: .value("Run", item.run.variant))
                                .foregroundStyle(by: .value("Run", item.run.variant))
                                .lineStyle(StrokeStyle(lineWidth: 1.2))
                                .interpolationMethod(.monotone)
                        }
                    }
                }
                .chartForegroundStyleScale(domain: series.map(\.run.variant), range: series.indices.map { Self.color($0) })
                .chartYScale(domain: -1...1)
                .chartXAxisLabel("Round")
                .frame(height: 220)
            }
        }
        .card()
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
