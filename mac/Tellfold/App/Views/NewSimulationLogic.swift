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
    /// Run again from a built-in example: keep the AI tools set up on this Mac instead of the ones the example used.
    var keepsDefaultRuntimes = false

    var isRunAgain: Bool { config != nil }

    /// Builds the prefill for "Run again…" from the run's configuration and the seed documents the engine returned
    /// (already downloaded to `seedFiles`). A `description.md` goes back into the Describe it box as text; every other
    /// document is attached as a file. `unavailable` names documents the engine listed but the app could not fetch.
    /// When the run has no stored documents at all (an older run) the sheet asks for them again.
    static func runAgain(config: SimConfig, seedFiles: [URL], unavailable: [String] = [], fromExample: Bool = false,
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
                                    missingSeedNames: missing, keepsDefaultRuntimes: fromExample)
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
    /// With `noToolYet` (nothing on this Mac can run it) the second sentence asks for a tool instead of naming one.
    static func summary(agents: Int, rounds: Int, agentRuntime: RuntimeInfo?, agentRuntimeID: String, agentModel: String,
                        plannerRuntime: RuntimeInfo?, plannerRuntimeID: String, plannerModel: String,
                        noToolYet: Bool = false) -> String {
        let people = "\(agents) \(agents == 1 ? "person reacts" : "people react") over \(rounds) \(rounds == 1 ? "round" : "rounds")."
        if noToolYet { return "\(people) Choose an AI tool to run it." }
        let agentName = agentRuntime?.name ?? agentRuntimeID
        let plannerName = plannerRuntime?.name ?? plannerRuntimeID
        let local = (agentRuntime?.isLocal ?? true) && (plannerRuntime?.isLocal ?? true)
        let verb = local ? "Runs locally with" : "Runs with"
        if agentRuntimeID == plannerRuntimeID {
            return "\(people) \(verb) \(agentName) (\(agentModel) + \(plannerModel))."
        }
        return "\(people) \(verb) \(agentName) (\(agentModel)) for the crowd and \(plannerName) (\(plannerModel)) for planning."
    }

    /// One note about the engine or the chosen runtimes, with the fix; nil when everything is ready. `anyReady` says
    /// whether some other AI tool on this Mac could run the simulation.
    static func readinessNote(engineRunning: Bool, runtimes: [RuntimeInfo?], ids: [String],
                              anyReady: Bool = true) -> String? {
        if !engineRunning { return "The engine is still starting. You can fill this in while you wait." }
        var seen = Set<String>()
        for (info, id) in zip(runtimes, ids) where seen.insert(id).inserted {
            guard let info else { return "\(id) is not one of the runtimes this engine knows. Pick another under Advanced." }
            if !info.installed {
                if !anyReady {
                    // The usual first-run case on a Mac with no AI tool yet: name a few and point at the setup step.
                    return "Tellfold plays the crowd with an AI tool on this Mac, such as Claude Code, Codex, Gemini CLI "
                        + "or Ollama. None is set up yet."
                }
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

/// The language the agents post in and the report is written in: a short menu of common languages, or any other
/// name typed under "Other…". The engine takes the name as written (up to 40 characters).
nonisolated enum LanguageChoice {
    static let common = [
        "English", "Spanish", "French", "German", "Italian", "Portuguese", "Dutch", "Polish", "Turkish", "Russian",
        "Ukrainian", "Armenian", "Arabic", "Hindi", "Chinese", "Japanese", "Korean",
    ]
    /// The menu row that shows the text field.
    static let other = "Other…"
    /// The engine's limit for the `language` form field.
    static let maxLength = 40

    /// The menu row for a language: the language itself when it is in the menu (any case), otherwise "Other…".
    static func selection(for language: String) -> String {
        let trimmed = language.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty { return "English" }
        return common.first { $0.caseInsensitiveCompare(trimmed) == .orderedSame } ?? other
    }

    /// What the text field under "Other…" starts with for a language: empty for one that is in the menu.
    static func custom(for language: String) -> String {
        selection(for: language) == other ? String(language.trimmingCharacters(in: .whitespacesAndNewlines).prefix(maxLength)) : ""
    }

    /// The language to send: the menu choice, or the typed one under "Other…" (trimmed, at most 40 characters,
    /// English when nothing was typed).
    static func resolve(selection: String, custom: String) -> String {
        guard selection == other else { return selection }
        let typed = String(custom.trimmingCharacters(in: .whitespacesAndNewlines).prefix(maxLength))
        return typed.isEmpty ? "English" : typed
    }
}

/// A ready-made start for the New simulation sheet: a question, a fictional seed document bundled with the app
/// (the `examples/` folder of the repository) and the breaking news to inject partway through.
nonisolated struct StartingTemplate: Identifiable, Hashable, Sendable {
    /// The seed document's file name without `.md`.
    var id: String
    var title: String
    var question: String
    var events: [GodEvent] = []

    func seedURL(in bundle: Bundle = .main) -> URL? { bundle.url(forResource: id, withExtension: "md") }

    static let all: [StartingTemplate] = [
        StartingTemplate(
            id: "corner_cafe_prices", title: "A café raises its prices",
            question: "How will the café's regulars and neighbours react to the new prices and the neighbour card "
                + "over the first weeks, and will the café lose customers?",
            events: [GodEvent(round: 3, content: "A local food blog calls the new prices greedy.")]),
        StartingTemplate(
            id: "school_phone_policy", title: "A school bans phones during the day",
            question: "How will parents, students and teachers react to the phone-free school day, and will the "
                + "school have to change the policy?",
            events: [GodEvent(round: 3, content: "A school bus breaks down on the motorway in the evening, and "
                + "students with locked phones cannot call their parents for over an hour.")]),
        StartingTemplate(
            id: "high_street_car_free", title: "A town closes its main street to cars",
            question: "How will residents, shop owners and commuters react to making Market Street car-free, and "
                + "will the council go ahead with it?"),
        StartingTemplate(
            id: "team_office_return", title: "A company brings staff back to the office",
            question: "How will Halden's staff react to the three office days and the new mixed teams, and how "
                + "many are likely to push back or leave?",
            events: [GodEvent(round: 3, content: "A post on a popular tech forum claims Halden is using office days "
                + "to make people quit so it doesn't have to pay redundancy.")]),
        StartingTemplate(
            id: "mossbook_pricing", title: "An app switches to a subscription",
            question: "How will existing Mossbook buyers and the wider houseplant community react to the new "
                + "subscription pricing during its first two weeks, and will Mossbook have to change the plan?",
            events: [GodEvent(round: 3, content: "A popular plant YouTuber with 800k subscribers posts a video "
                + "titled 'Mossbook just betrayed its users'.")]),
    ]
}
