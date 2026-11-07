import Foundation

/// The three sizes offered on the New simulation sheet. A size is only a pair of numbers (people and rounds); anything
/// else the user sets exactly lives under Advanced.
nonisolated enum SizePreset: String, CaseIterable, Identifiable, Sendable {
    case quick, standard, deep

    var id: String { rawValue }

    var title: String {
        switch self {
        case .quick: "Quick look"
        case .standard: "Standard"
        case .deep: "Deep"
        }
    }

    var agents: Int {
        switch self {
        case .quick: 10
        case .standard: 20
        case .deep: 40
        }
    }

    var rounds: Int {
        switch self {
        case .quick: 3
        case .standard: 6
        case .deep: 10
        }
    }

    /// A rough wall-clock estimate; always printed with "about".
    var minutes: Int {
        switch self {
        case .quick: 3
        case .standard: 8
        case .deep: 20
        }
    }

    var detail: String { "\(agents) people · \(rounds) rounds" }
    var estimate: String { "about \(minutes) min" }

    static let `default`: SizePreset = .standard

    /// The preset that matches these exact numbers, or nil when they are "Custom".
    static func detect(agents: Int, rounds: Int) -> SizePreset? {
        allCases.first { $0.agents == agents && $0.rounds == rounds }
    }
}

/// How the user gives the crowd its material.
nonisolated enum AboutMode: String, CaseIterable, Identifiable, Sendable {
    case describe, files

    var id: String { rawValue }
    var title: String { self == .describe ? "Describe it" : "Add files" }
}

/// A document sent to the engine as a seed file without existing on disk (the pasted description).
nonisolated struct InlineDocument: Hashable, Sendable {
    var filename: String
    var data: Data

    /// The pasted description travels as an ordinary seed document with this name.
    static let descriptionName = "description.md"

    static func description(_ text: String) -> InlineDocument {
        InlineDocument(filename: descriptionName, data: Data(text.trimmingCharacters(in: .whitespacesAndNewlines).utf8))
    }
}

/// What the sheet opens with when it is not a blank start: a question from the empty-state card, or a finished run's
/// configuration for "Run again…".
nonisolated struct NewSimulationPrefill: Equatable, Sendable {
    var question = ""
    /// Text for the "Describe it" box.
    var descriptionText = ""
    var config: SimConfig?
    /// Seed documents of the earlier run that are still on disk.
    var seedFiles: [URL] = []
    /// Names of the earlier run's seed documents the app could not get back: the sheet asks for them again.
    var missingSeedNames: [String] = []

    var isRunAgain: Bool { config != nil }

    /// Builds the prefill for "Run again…" from the run's configuration and the seed documents the engine returned
    /// (already downloaded to `seedFiles`). A `description.md` goes back into the Describe it box as text; every other
    /// document is attached as a file. `unavailable` names documents the engine listed but the app could not fetch.
    /// When the run has no stored documents at all (an older run) the sheet asks for them again.
    static func runAgain(config: SimConfig, seedFiles: [URL], unavailable: [String] = [],
                         readText: (URL) -> String? = { try? String(contentsOf: $0, encoding: .utf8) }) -> NewSimulationPrefill {
        var files: [URL] = []
        var text = ""
        for url in seedFiles {
            if url.lastPathComponent.lowercased() == InlineDocument.descriptionName, text.isEmpty,
               let body = readText(url), !body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                text = body.trimmingCharacters(in: .whitespacesAndNewlines)
            } else {
                files.append(url)
            }
        }
        var missing = unavailable
        if seedFiles.isEmpty, unavailable.isEmpty {
            missing = config.seedFiles.map { ($0 as NSString).lastPathComponent }
        }
        return NewSimulationPrefill(question: config.question, descriptionText: text, config: config, seedFiles: files,
                                    missingSeedNames: missing)
    }
}

/// The wording and the rules of the sheet, kept free of views so they can be tested.
nonisolated enum NewSimulationRules {
    /// A pasted description counts on its own from about this many characters.
    static let minimumDescription = 40

    static let exampleQuestions = [
        "How will customers react if we raise the price by 20%?",
        "Will people switch to our app from the one they use now?",
        "How will users react if we remove the free plan?",
    ]

    static func trimmed(_ text: String) -> String { text.trimmingCharacters(in: .whitespacesAndNewlines) }

    static func hasMaterial(description: String, fileCount: Int) -> Bool {
        fileCount > 0 || trimmed(description).count >= minimumDescription
    }

    /// Why "Start simulation" is disabled, in one short line; nil when it can start.
    static func disabledReason(question: String, description: String, fileCount: Int, engineRunning: Bool,
                               runtimeProblem: Bool = false, modelsChosen: Bool = true) -> String? {
        if !engineRunning { return "Waiting for the engine to start" }
        if trimmed(question).isEmpty { return "Add a question" }
        if !hasMaterial(description: description, fileCount: fileCount) {
            return trimmed(description).isEmpty ? "Describe it or add a file" : "Describe it a little more, or add a file"
        }
        if runtimeProblem { return "Fix the runtime above" }
        if !modelsChosen { return "Choose a model in Advanced" }
        return nil
    }

    /// "20 people react over 6 rounds. Runs locally with Claude Code (haiku + sonnet)."
    static func summary(agents: Int, rounds: Int, agentRuntime: RuntimeInfo?, agentRuntimeID: String, agentModel: String,
                        plannerRuntime: RuntimeInfo?, plannerRuntimeID: String, plannerModel: String) -> String {
        let people = "\(agents) \(agents == 1 ? "person reacts" : "people react") over \(rounds) \(rounds == 1 ? "round" : "rounds")."
        let agentName = agentRuntime?.name ?? agentRuntimeID
        let plannerName = plannerRuntime?.name ?? plannerRuntimeID
        let local = (agentRuntime?.kind ?? .cli) == .cli && (plannerRuntime?.kind ?? .cli) == .cli
        let verb = local ? "Runs locally with" : "Runs with"
        if agentRuntimeID == plannerRuntimeID {
            return "\(people) \(verb) \(agentName) (\(agentModel) + \(plannerModel))."
        }
        return "\(people) \(verb) \(agentName) (\(agentModel)) for the crowd and \(plannerName) (\(plannerModel)) for planning."
    }

    /// One note about the engine or the chosen runtimes, with the fix; nil when everything is ready.
    static func readinessNote(engineRunning: Bool, runtimes: [RuntimeInfo?], ids: [String]) -> String? {
        if !engineRunning { return "The engine is still starting. You can fill this in while you wait." }
        var seen = Set<String>()
        for (info, id) in zip(runtimes, ids) where seen.insert(id).inserted {
            guard let info else { return "\(id) is not one of the runtimes this engine knows. Pick another under Advanced." }
            if !info.installed {
                return "\(info.name) is not installed on this Mac. Install it, or pick another runtime under Advanced."
            }
            if info.auth == .missing {
                return "\(info.name) is not signed in. " + (info.authHint.map { $0.hasSuffix(".") ? $0 : $0 + "." } ?? "Sign in, then start.")
            }
        }
        return nil
    }

    /// True when `readinessNote` describes something that stops a run (not just a still-starting engine).
    static func runtimeBlocks(runtimes: [RuntimeInfo?]) -> Bool {
        runtimes.contains { $0 == nil || $0?.installed == false }
    }
}
