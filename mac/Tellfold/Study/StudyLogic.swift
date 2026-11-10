import Foundation

/// The sidebar selection of a study: studies share the run list's selection, tagged `study:<id>`.
nonisolated enum StudySelection {
    static let prefix = "study:"

    static func tag(_ studyID: String) -> String { prefix + studyID }

    /// The study id in a sidebar selection, or nil when a run (or nothing) is selected.
    static func studyID(from selection: String?) -> String? {
        guard let selection, selection.hasPrefix(prefix) else { return nil }
        let id = String(selection.dropFirst(prefix.count))
        return id.isEmpty ? nil : id
    }
}

/// One study in the sidebar.
nonisolated struct StudyRow: Identifiable, Hashable, Sendable {
    var id: String
    var question: String
    /// Members in study order, only those still in the run list.
    var members: [StudyMember]
    var isActive: Bool
    var hasCompare: Bool
    var comparing: Bool
    /// A "Check it" study: the same run repeated.
    var isRepeat: Bool = false
    var planned: Int = 0

    var title: String {
        isRepeat ? "Check it: \(max(planned, members.count)) runs" : members.map(\.variant).joined(separator: " vs ")
    }
}

/// Where a study stands, worded for its header.
nonisolated enum StudyPhase: Equatable, Sendable {
    /// At least one version is still running.
    case running
    /// Every version finished; the comparison is being written.
    case comparing
    /// The comparison is there.
    case compared
    /// Every version finished, at least two with a report, and no comparison yet (or the last one failed).
    case readyToCompare
    /// Fewer than two versions have a report.
    case notEnoughRuns
}

/// Grouping and labels for studies, free of views so they can be tested.
nonisolated enum StudyLogic {
    /// Studies with at least one run still in the list, newest first, members in study order. A run can be deleted;
    /// its study then shows the rest.
    static func rows(studies: [StudySummary], runs: [RunSummary]) -> [StudyRow] {
        let byID = Dictionary(runs.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a })
        return studies.compactMap { study in
            let members = study.runs.filter { byID[$0.id] != nil }
            guard !members.isEmpty else { return nil }
            let question = study.question.isEmpty ? (byID[members[0].id]?.question ?? "") : study.question
            return StudyRow(id: study.id, question: question, members: members,
                            isActive: members.contains { byID[$0.id]?.state.isActive ?? false },
                            hasCompare: study.hasCompare, comparing: study.comparing, isRepeat: study.isRepeat,
                            planned: study.planned)
        }
        .sorted { a, b in
            let da = studies.first { $0.id == a.id }?.createdAt ?? ""
            let db = studies.first { $0.id == b.id }?.createdAt ?? ""
            return da == db ? a.id > b.id : da > db
        }
    }

    /// The study a new version of a run joins: its own `studyID`, else the first study that lists it (a source run
    /// joins its study only through the study file). Never a "Check it" study: it holds only repeats of one run, so a
    /// new version of one of its runs starts a study of its own (nil).
    static func study(of runID: String, runs: [RunSummary], studies: [StudySummary]) -> String? {
        let repeats = Set(studies.filter(\.isRepeat).map(\.id))
        if let own = runs.first(where: { $0.id == runID })?.studyID, !repeats.contains(own) { return own }
        return studies.first { !$0.isRepeat && $0.runs.contains { $0.id == runID } }?.id
    }

    /// "Draft A", "Draft B", ... for the `index`-th run of a study (as the engine labels them).
    static func defaultVariant(_ index: Int) -> String {
        let letters = Array("ABCDEFGHIJKLMNOPQRSTUVWXYZ")
        return "Draft \(index >= 0 && index < letters.count ? String(letters[index]) : String(index + 1))"
    }

    /// The label a new version of `runID` gets by default: the next letter of its study, or "Draft B" when the run
    /// starts a study.
    static func nextVariant(for runID: String, runs: [RunSummary], studies: [StudySummary]) -> String {
        guard let studyID = study(of: runID, runs: runs, studies: studies),
              let study = studies.first(where: { $0.id == studyID }) else { return defaultVariant(1) }
        let count = study.runs.contains { $0.id == runID } ? study.runs.count : study.runs.count + 1
        return defaultVariant(count)
    }

    static func phase(of detail: StudyDetail) -> StudyPhase {
        if detail.repeating || detail.runs.contains(where: { $0.state.isActive }) { return .running }
        if detail.compareState.running { return .comparing }
        if detail.isRepeat ? detail.agreement != nil : detail.compare != nil { return .compared }
        return detail.runs.filter(\.hasReport).count >= 2 ? .readyToCompare : .notEnoughRuns
    }

    /// How far a "Check it" series is: "2 more to come" while it runs, "stopped after 2 of 3 runs" when it ended
    /// early without saying so (the engine restarted mid-series); nil when every planned run is there.
    static func seriesNote(_ detail: StudyDetail) -> String? {
        guard detail.isRepeat, detail.planned > detail.runs.count else { return nil }
        return detail.repeating
            ? "\(detail.planned - detail.runs.count) more to come"
            : "stopped after \(detail.runs.count) of \(detail.planned) runs"
    }

    /// Runs in a "Check it" by default, and the most the engine allows.
    static let checkRuns = 3
    static let maxCheckRuns = 5

    /// The usage warning before "Check it": the extra runs make roughly this many calls each.
    static func checkWarning(runs: Int, callsPerRun: Int) -> String {
        let extra = runs - 1
        return "Check it runs this simulation \(extra) more time\(extra == 1 ? "" : "s") with the same people and text, one "
            + "after another. That is about \(runs)× the model calls of one run (about \(extra * callsPerRun) more calls)."
    }

    /// The verdict in plain words.
    static func verdictTitle(_ verdict: AgreementVerdict, runs: Int) -> String {
        switch verdict {
        case .consistent: "It held up in all \(runs) runs"
        case .mixed: "The main answer held, some details did not"
        case .unstable: "The runs disagree: don't rely on this answer"
        case .unknown: "Checked across \(runs) runs"
        }
    }

    /// "+0.10 to +0.35 (spread 0.25)", or nil when no run scored its last round.
    static func finalRange(_ a: Agreement) -> String? {
        guard let low = a.finalLow, let high = a.finalHigh else { return nil }
        return "\(RunUI.sentimentText(low)) to \(RunUI.sentimentText(high)) (spread \(String(format: "%.2f", high - low)))"
    }

    /// The run a comparison citation such as `B#7` points into.
    static func run(letter: String, in detail: StudyDetail) -> StudyRun? {
        detail.runs.first { $0.letter == letter }
    }

    /// Chart series: one (round, sentiment) list per run, rounds without a score left out.
    static func curves(_ detail: StudyDetail) -> [(run: StudyRun, points: [(round: Int, sentiment: Double)])] {
        detail.runs.map { run in
            (run, run.digest.rounds.compactMap { p in p.sentiment.map { (p.round, $0) } })
        }
    }

    /// The low-high band per round for a "Check it" chart: the engine's bands when the check is written, else
    /// computed from the runs so far. Rounds no run scored are left out.
    static func bands(_ detail: StudyDetail) -> [(round: Int, low: Double, high: Double)] {
        if let bands = detail.agreement?.bands, !bands.isEmpty {
            return bands.compactMap { b in
                guard let low = b.low, let high = b.high else { return nil }
                return (b.round, low, high)
            }
        }
        var byRound: [Int: [Double]] = [:]
        for run in detail.runs {
            for p in run.digest.rounds { if let s = p.sentiment { byRound[p.round, default: []].append(s) } }
        }
        return byRound.keys.sorted().map { r in (r, byRound[r]!.min()!, byRound[r]!.max()!) }
    }

    /// Text files are edited in the sheet; the first one (a pasted `description.md` first) is the version's text.
    static func editableSeed(_ names: [String]) -> String? {
        let text = names.filter { ["md", "markdown", "txt"].contains(($0 as NSString).pathExtension.lowercased()) }
        return text.first { $0.lowercased() == InlineDocument.descriptionName } ?? text.first
    }

    /// The request for a new version of a run: the source's settings, the edited text in place of the original
    /// document, the other documents unchanged, and the source's people (`castFrom`).
    static func versionRequest(source runID: String, config: SimConfig, studyID: String?, variant: String,
                               editedName: String, editedText: String, otherFiles: [URL]) -> NewRunRequest {
        var request = NewRunRequest(files: otherFiles, question: config.question)
        request.agents = config.numAgents
        request.rounds = config.rounds
        request.batch = config.batchSize
        request.agentRuntime = config.agentRuntime
        request.agentModel = config.agentModel
        request.plannerRuntime = config.plannerRuntime
        request.plannerModel = config.plannerModel
        request.concurrency = config.concurrency
        request.language = config.language
        request.think = config.think
        request.events = config.events
        request.seed = config.seed
        request.inlineDocuments = [InlineDocument(filename: editedName, data: Data(editedText.utf8))]
        request.castFrom = runID
        request.studyID = studyID
        let label = variant.trimmingCharacters(in: .whitespacesAndNewlines)
        request.variant = label.isEmpty ? nil : label
        return request
    }
}

/// `A#12` and `B#7` in a comparison become `tellfold://study-post/A/12` links; the study view opens the post in run A.
nonisolated enum StudyLinks {
    static let host = "study-post"
    private static let pattern = #"(?<![\w&#\[])([A-Z])#(\d{1,8})\b"#

    static func url(letter: String, postID: Int) -> URL? {
        URL(string: "\(RunUI.linkScheme)://\(host)/\(letter)/\(postID)")
    }

    static func target(_ url: URL) -> (letter: String, postID: Int)? {
        guard url.scheme == RunUI.linkScheme, url.host() == host else { return nil }
        let parts = url.path().split(separator: "/").map(String.init)
        guard parts.count == 2, parts[0].count == 1, let id = Int(parts[1]) else { return nil }
        return (parts[0], id)
    }

    /// Rewrites `A#12` refs to letters in `letters` as markdown links.
    static func linkify(_ text: String, letters: Set<String>) -> String {
        guard let regex = try? NSRegularExpression(pattern: pattern) else { return text }
        var out = text
        for match in regex.matches(in: text, range: NSRange(text.startIndex..., in: text)).reversed() {
            guard let whole = Range(match.range, in: text), let l = Range(match.range(at: 1), in: text),
                  let n = Range(match.range(at: 2), in: text), letters.contains(String(text[l])),
                  let id = Int(text[n]), let url = url(letter: String(text[l]), postID: id) else { continue }
            out.replaceSubrange(whole, with: "[\(text[whole])](\(url.absoluteString))")
        }
        return out
    }
}
