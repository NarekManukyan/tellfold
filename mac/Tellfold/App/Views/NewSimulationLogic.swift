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
/// A folder added to the New Simulation sheet whole, with the documents found in it.
nonisolated struct SeedFolder: Identifiable, Equatable, Sendable {
    var url: URL
    var files: [URL]
    var bytes: Int = 0
    var id: URL { url }
}

/// What adding files and folders to the New Simulation sheet found. `scan` does all the disk work, so the sheet
/// runs it off the main thread.
nonisolated struct SeedScan: Sendable {
    var files: [URL] = []
    var folders: [SeedFolder] = []
    var bytes: [URL: Int] = [:]
    var rejected: [String] = []
    var leftOut: [String] = []

    var message: String? {
        var parts: [String] = []
        if !rejected.isEmpty {
            parts.append("Skipped: \(rejected.joined(separator: ", ")). Use .md, .txt or .pdf, or a folder of them.")
        }
        if !leftOut.isEmpty { parts.append("Left out: \(leftOut.joined(separator: ", ")).") }
        return parts.isEmpty ? nil : parts.joined(separator: " ")
    }

    /// Adds as many documents as the run limits allow on top of `existing` (already `existingBytes` in size).
    static func scan(_ urls: [URL], existing: Set<URL>, existingBytes: Int, knownFolders: Set<URL>) -> SeedScan {
        var result = SeedScan()
        var taken = existing
        var total = existingBytes
        func size(_ url: URL) -> Int { (try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0 }
        func fit(_ candidates: [URL]) -> [URL] {
            var kept: [URL] = []
            var tooBig = 0
            var overLimit = false
            for url in candidates where !taken.contains(url) {
                let bytes = size(url)
                if bytes > NewRunRequest.maxFileBytes { tooBig += 1; continue }
                guard taken.count < NewRunRequest.maxFiles, total + bytes <= NewRunRequest.maxTotalBytes else {
                    overLimit = true
                    break
                }
                kept.append(url)
                taken.insert(url)
                result.bytes[url] = bytes
                total += bytes
            }
            if tooBig > 0 { result.leftOut.append("\(tooBig) over 20 MB") }
            if overLimit {
                result.leftOut.append("anything past \(NewRunRequest.maxFiles) files or 100 MB in total")
            }
            return kept
        }
        for url in urls {
            if AppModel.isFolder(url) {
                guard !knownFolders.contains(url), !result.folders.contains(where: { $0.url == url }) else { continue }
                // One past the limit is enough to know there is more than fits.
                let found = AppModel.seedFiles(inFolder: url, limit: NewRunRequest.maxFiles + 1)
                if found.isEmpty {
                    result.rejected.append("\(url.lastPathComponent) (no .md, .txt or .pdf inside)")
                    continue
                }
                let kept = fit(found)
                if !kept.isEmpty {
                    result.folders.append(SeedFolder(url: url, files: kept,
                                                     bytes: kept.reduce(0) { $0 + (result.bytes[$1] ?? 0) }))
                }
            } else if AppModel.isSeedFile(url) {
                result.files.append(contentsOf: fit([url]))
            } else {
                result.rejected.append(url.lastPathComponent)
            }
        }
        return result
    }
}

nonisolated struct NewSimulationPrefill: Equatable, Sendable {
    var question = ""
    /// Text for the "Describe it" box.
    var descriptionText = ""
    var config: SimConfig?
    /// Seed documents of the earlier run that are still on disk.
    var seedFiles: [URL] = []
    /// Names of the earlier run's seed documents the app could not get back: the sheet asks for them again.
    var missingSeedNames: [String] = []
    /// Run again from a read-only run (a built-in example or an imported run): keep the AI tools set up on this Mac
    /// instead of the recorded ones.
    var keepsDefaultRuntimes = false

    var isRunAgain: Bool { config != nil }

    /// Builds the prefill for "Run again…" from the run's configuration and the seed documents the engine returned
    /// (already downloaded to `seedFiles`). A `description.md` goes back into the Describe it box as text; every other
    /// document is attached as a file. `unavailable` names documents the engine listed but the app could not fetch.
    /// When the run has no stored documents at all (an older run) the sheet asks for them again.
    static func runAgain(config: SimConfig, seedFiles: [URL], unavailable: [String] = [], fromReadOnly: Bool = false,
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
                                    missingSeedNames: missing, keepsDefaultRuntimes: fromReadOnly)
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
                               runtimeProblem: Bool = false, modelsChosen: Bool = true,
                               materialProblem: Bool = false) -> String? {
        if !engineRunning { return "Waiting for the engine to start" }
        if trimmed(question).isEmpty { return "Add a question" }
        if !hasMaterial(description: description, fileCount: fileCount) {
            return trimmed(description).isEmpty ? "Describe it or add a file" : "Describe it a little more, or add a file"
        }
        if materialProblem { return "Fix the audience material" }
        if runtimeProblem { return "Fix the runtime above" }
        if !modelsChosen { return "Choose a model in Advanced" }
        return nil
    }

    /// On "Run again…" of a run built from audience material: Tellfold keeps only the cleaned text in the run, not the
    /// files, so the user adds them again.
    static func materialAgainNote(_ material: MaterialSummary) -> String {
        "The last run was built from \(MaterialText.counts(material.counts)). Add the material again to build the crowd "
            + "from it."
    }

    /// Under the People picker when a saved audience is chosen.
    static func audienceNote(_ audience: SavedAudience) -> String {
        let n = audience.personas.count
        return "\(n) \(n == 1 ? "person" : "people") from \u{201C}\(audience.name)\u{201D}. Casting is skipped, so the run "
            + "costs fewer model calls; the size above sets only the rounds."
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

    /// "About 24 model calls on Claude Code: 20 for the crowd (haiku), 4 for planning (sonnet)." The estimate assumes
    /// no retries; when the two roles use different tools each part names its own. `materialCalls` of the planning
    /// calls read the user's audience material, and the line says so.
    static func costLine(agentCalls: Int, plannerCalls: Int, materialCalls: Int = 0, agentRuntime: String,
                         agentModel: String, plannerRuntime: String, plannerModel: String) -> String {
        let total = agentCalls + plannerCalls
        let head = "About \(total) model \(total == 1 ? "call" : "calls")"
        let reading = materialCalls > 0 ? ", \(materialCalls) of them to read your material" : ""
        if agentRuntime == plannerRuntime {
            return "\(head) on \(agentRuntime): \(agentCalls) for the crowd (\(agentModel)), "
                + "\(plannerCalls) for planning (\(plannerModel))\(reading)."
        }
        return "\(head): \(agentCalls) on \(agentRuntime) for the crowd (\(agentModel)), "
            + "\(plannerCalls) on \(plannerRuntime) for planning (\(plannerModel))\(reading)."
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
