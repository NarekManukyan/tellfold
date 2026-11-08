import Foundation
import Testing
@testable import Tellfold

@Suite("New simulation sheet rules")
struct NewSimulationTests {
    private func runtime(_ id: String = "claude-code", name: String = "Claude Code", kind: RuntimeInfo.Kind = .cli,
                         installed: Bool = true, auth: RuntimeInfo.Auth = .ok, hint: String? = nil) -> RuntimeInfo {
        RuntimeInfo(id: id, kind: kind, name: name, installed: installed, version: nil, path: nil, auth: auth,
                    authHint: hint, models: [], defaultModels: [:], tested: true, notes: nil)
    }

    private let long = String(repeating: "A pricing page for a dog app. ", count: 3)

    // MARK: presets

    @Test func presetsMapToPeopleAndRounds() {
        #expect(SizePreset.quick.agents == 10 && SizePreset.quick.rounds == 3 && SizePreset.quick.minutes == 3)
        #expect(SizePreset.standard.agents == 20 && SizePreset.standard.rounds == 6 && SizePreset.standard.minutes == 8)
        #expect(SizePreset.deep.agents == 40 && SizePreset.deep.rounds == 10 && SizePreset.deep.minutes == 20)
        #expect(SizePreset.default == .standard)
        #expect(SizePreset.quick.title == "Quick look")
        #expect(SizePreset.standard.detail == "20 people · 6 rounds")
        #expect(SizePreset.deep.estimate == "about 20 min")
    }

    @Test func customIsDetectedWhenNumbersLeaveAPreset() {
        #expect(SizePreset.detect(agents: 10, rounds: 3) == .quick)
        #expect(SizePreset.detect(agents: 20, rounds: 6) == .standard)
        #expect(SizePreset.detect(agents: 40, rounds: 10) == .deep)
        // One number off is Custom, and so is a preset's people with another preset's rounds.
        #expect(SizePreset.detect(agents: 21, rounds: 6) == nil)
        #expect(SizePreset.detect(agents: 20, rounds: 7) == nil)
        #expect(SizePreset.detect(agents: 10, rounds: 10) == nil)
    }

    // MARK: disabled reason

    @Test func disabledReasonsAreInPlainWords() {
        func reason(q: String = "How will people react?", d: String = "", files: Int = 0, engine: Bool = true,
                    runtime: Bool = false, models: Bool = true) -> String? {
            NewSimulationRules.disabledReason(question: q, description: d, fileCount: files, engineRunning: engine,
                                              runtimeProblem: runtime, modelsChosen: models)
        }
        #expect(reason(q: "   ", d: long) == "Add a question")
        #expect(reason() == "Describe it or add a file")
        #expect(reason(d: "too short") == "Describe it a little more, or add a file")
        #expect(reason(d: long) == nil)
        #expect(reason(files: 1) == nil)
        #expect(reason(d: "short", files: 2) == nil)
        #expect(reason(d: long, engine: false) == "Waiting for the engine to start")
        #expect(reason(d: long, runtime: true) == "Fix the runtime above")
        #expect(reason(d: long, models: false) == "Choose a model in Advanced")
        // The question comes first, then the material.
        #expect(reason(q: "") == "Add a question")
    }

    @Test func descriptionNeedsAboutFortyCharacters() {
        #expect(!NewSimulationRules.hasMaterial(description: String(repeating: "x", count: 39), fileCount: 0))
        #expect(NewSimulationRules.hasMaterial(description: String(repeating: "x", count: 40), fileCount: 0))
        #expect(!NewSimulationRules.hasMaterial(description: "  " + String(repeating: "x", count: 39) + "  ", fileCount: 0))
        #expect(NewSimulationRules.hasMaterial(description: "", fileCount: 1))
    }

    // MARK: summary

    @Test func summaryIsOnePlainLine() {
        let cc = runtime()
        #expect(NewSimulationRules.summary(agents: 20, rounds: 6, agentRuntime: cc, agentRuntimeID: "claude-code",
                                           agentModel: "haiku", plannerRuntime: cc, plannerRuntimeID: "claude-code",
                                           plannerModel: "sonnet")
                == "20 people react over 6 rounds. Runs locally with Claude Code (haiku + sonnet).")
        #expect(NewSimulationRules.summary(agents: 1, rounds: 1, agentRuntime: cc, agentRuntimeID: "claude-code",
                                           agentModel: "haiku", plannerRuntime: cc, plannerRuntimeID: "claude-code",
                                           plannerModel: "sonnet")
                .hasPrefix("1 person reacts over 1 round."))
    }

    @Test func summaryNamesBothRuntimesWhenTheyDiffer() {
        let ollama = runtime("ollama", name: "Ollama", kind: .http)
        let text = NewSimulationRules.summary(agents: 40, rounds: 10, agentRuntime: ollama, agentRuntimeID: "ollama",
                                              agentModel: "llama3", plannerRuntime: runtime(), plannerRuntimeID: "claude-code",
                                              plannerModel: "sonnet")
        #expect(text == "40 people react over 10 rounds. Runs with Ollama (llama3) for the crowd and Claude Code (sonnet) for planning.")
    }

    @Test func summaryAsksForAToolWhenNoneIsSetUp() {
        let text = NewSimulationRules.summary(agents: 20, rounds: 6, agentRuntime: nil, agentRuntimeID: "claude-code",
                                              agentModel: "haiku", plannerRuntime: nil, plannerRuntimeID: "claude-code",
                                              plannerModel: "sonnet", noToolYet: true)
        #expect(text == "20 people react over 6 rounds. Choose an AI tool to run it.")
    }

    // MARK: readiness

    @Test func readinessNoteAppearsOnlyWhenSomethingNeedsFixing() {
        let ok = runtime()
        #expect(NewSimulationRules.readinessNote(engineRunning: true, runtimes: [ok, ok], ids: ["claude-code", "claude-code"]) == nil)
        let missing = runtime(installed: false)
        #expect(NewSimulationRules.readinessNote(engineRunning: true, runtimes: [missing, missing], ids: ["claude-code", "claude-code"])?
            .contains("not installed") == true)
        #expect(NewSimulationRules.readinessNote(engineRunning: true, runtimes: [missing], ids: ["claude-code"])
            == "Claude Code is not installed on this Mac. Install it, or pick another runtime under Advanced.")
        // With no AI tool anywhere the note names several, not only Claude Code.
        let none = NewSimulationRules.readinessNote(engineRunning: true, runtimes: [missing], ids: ["claude-code"],
                                                    anyReady: false)
        #expect(none?.contains("Codex") == true && none?.contains("Ollama") == true)
        let missingOllama = runtime("ollama", name: "Ollama", kind: .http, installed: false)
        #expect(NewSimulationRules.readinessNote(engineRunning: true, runtimes: [missingOllama], ids: ["ollama"])
            == "Ollama is not installed on this Mac. Install it, or pick another runtime under Advanced.")
        let signedOut = runtime(auth: .missing, hint: "Run `claude` and use /login")
        let note = NewSimulationRules.readinessNote(engineRunning: true, runtimes: [signedOut, ok], ids: ["claude-code", "other"])
        #expect(note == "Claude Code is not signed in. Run `claude` and use /login.")
        #expect(NewSimulationRules.readinessNote(engineRunning: false, runtimes: [ok, ok], ids: ["a", "b"]) != nil)
        #expect(NewSimulationRules.runtimeBlocks(runtimes: [missing, ok]))
        #expect(!NewSimulationRules.runtimeBlocks(runtimes: [signedOut, ok]))
    }

    // MARK: description document

    @Test func pastedDescriptionTravelsAsDescriptionMarkdown() {
        let doc = InlineDocument.description("  Allby reminds you about doses.  \n")
        #expect(doc.filename == "description.md")
        #expect(String(decoding: doc.data, as: UTF8.self) == "Allby reminds you about doses.")
    }

    @Test func descriptionIsSentAsASeedFileInTheUpload() throws {
        var request = NewRunRequest(files: [], question: "Will it work?")
        request.inlineDocuments = [.description("Allby reminds you about doses, every day, on time.")]
        request.seed = 42
        var form = MultipartForm(boundary: "b")
        for doc in request.inlineDocuments {
            form.addFile("files[]", filename: doc.filename, contentType: "text/markdown", data: doc.data)
        }
        let body = String(decoding: form.finalize(), as: UTF8.self)
        #expect(body.contains("name=\"files[]\"; filename=\"description.md\""))
        #expect(NewRunRequest.estimatedChunks(files: [], inlineDocuments: request.inlineDocuments) == 1)
    }

    // MARK: run again

    private func config() -> SimConfig {
        var config = SimConfig(question: "Will they stay?")
        config.numAgents = 24
        config.rounds = 5
        config.seedFiles = ["/runs/r1/seeds/description.md", "/runs/r1/seeds/notes.pdf"]
        return config
    }

    @Test func runAgainPutsDescriptionBackAsTextAndAttachesTheOtherDocuments() {
        let folder = URL(fileURLWithPath: "/tmp/run-again")
        let files = [folder.appending(path: "description.md"), folder.appending(path: "pricing.pdf"), folder.appending(path: "notes.md")]
        let prefill = NewSimulationPrefill.runAgain(config: config(), seedFiles: files) { url in
            url.lastPathComponent == "description.md" ? "  Allby reminds you about doses.\n" : nil
        }
        #expect(prefill.isRunAgain)
        #expect(prefill.question == "Will they stay?")
        #expect(prefill.config?.numAgents == 24)
        #expect(prefill.descriptionText == "Allby reminds you about doses.")
        #expect(prefill.seedFiles.map(\.lastPathComponent) == ["pricing.pdf", "notes.md"])
        #expect(prefill.missingSeedNames.isEmpty)
        #expect(SizePreset.detect(agents: 24, rounds: 5) == nil)
    }

    @Test func runAgainWithOnlyFilesLeavesTheDescriptionEmpty() {
        let files = [URL(fileURLWithPath: "/tmp/run-again/notes.md")]
        let prefill = NewSimulationPrefill.runAgain(config: config(), seedFiles: files)
        #expect(prefill.descriptionText.isEmpty)
        #expect(prefill.seedFiles == files)
    }

    @Test func anUnreadableOrEmptyDescriptionStaysAnAttachedFile() {
        let url = URL(fileURLWithPath: "/tmp/run-again/description.md")
        #expect(NewSimulationPrefill.runAgain(config: config(), seedFiles: [url]) { _ in nil }.seedFiles == [url])
        #expect(NewSimulationPrefill.runAgain(config: config(), seedFiles: [url]) { _ in "  \n" }.seedFiles == [url])
    }

    @Test func runAgainAsksForTheDocumentsAgainOnlyWhenTheRunHasNone() {
        let none = NewSimulationPrefill.runAgain(config: config(), seedFiles: [])
        #expect(none.seedFiles.isEmpty && none.descriptionText.isEmpty)
        #expect(none.missingSeedNames == ["description.md", "notes.pdf"])
        // Some downloaded, one could not be fetched: only that one is asked for again.
        let partial = NewSimulationPrefill.runAgain(config: config(), seedFiles: [URL(fileURLWithPath: "/tmp/x/notes.pdf")],
                                                    unavailable: ["extra.md"])
        #expect(partial.missingSeedNames == ["extra.md"])
        // A description is not "missing": the Describe it box holds it.
        let onlyDescription = NewSimulationPrefill.runAgain(config: config(), seedFiles: [URL(fileURLWithPath: "/tmp/x/description.md")]) { _ in "Text" }
        #expect(onlyDescription.missingSeedNames.isEmpty && onlyDescription.descriptionText == "Text")
    }

    @Test func aQuestionOnlyPrefillIsNotARunAgain() {
        let prefill = NewSimulationPrefill(question: "How will users react if we remove the free plan?")
        #expect(!prefill.isRunAgain)
        #expect(NewSimulationRules.exampleQuestions.contains(prefill.question))
        #expect(NewSimulationRules.exampleQuestions.count == 3)
    }

    // MARK: language

    @Test func languageMenuRoundTripsTheConfigLanguage() {
        #expect(LanguageChoice.selection(for: "French") == "French")
        #expect(LanguageChoice.selection(for: " french ") == "French")
        #expect(LanguageChoice.selection(for: "") == "English")
        #expect(LanguageChoice.selection(for: "Catalan") == LanguageChoice.other)
        #expect(LanguageChoice.custom(for: "Catalan") == "Catalan")
        #expect(LanguageChoice.custom(for: "French").isEmpty)
        #expect(LanguageChoice.resolve(selection: "Armenian", custom: "ignored") == "Armenian")
        #expect(LanguageChoice.resolve(selection: LanguageChoice.other, custom: "  Catalan ") == "Catalan")
        #expect(LanguageChoice.resolve(selection: LanguageChoice.other, custom: "   ") == "English")
        #expect(LanguageChoice.resolve(selection: LanguageChoice.other, custom: String(repeating: "x", count: 60)).count == 40)
        #expect(LanguageChoice.common.count == 17 && LanguageChoice.common.first == "English")
    }

    @Test func runAgainRestoresTheLanguage() {
        var french = config()
        french.language = "French"
        let prefill = NewSimulationPrefill.runAgain(config: french, seedFiles: [])
        let language = try? #require(prefill.config?.language)
        #expect(LanguageChoice.resolve(selection: LanguageChoice.selection(for: language ?? ""),
                                       custom: LanguageChoice.custom(for: language ?? "")) == "French")
        var other = config()
        other.language = "Catalan"
        #expect(LanguageChoice.resolve(selection: LanguageChoice.selection(for: other.language),
                                       custom: LanguageChoice.custom(for: other.language)) == "Catalan")
    }

    @MainActor @Test func lastLanguageIsRemembered() throws {
        let suite = "tellfold-tests-\(UUID().uuidString)"
        let defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        #expect(AppPreferences(defaults: defaults).lastLanguage == "English")
        AppPreferences(defaults: defaults).lastLanguage = "Japanese"
        #expect(AppPreferences(defaults: defaults).lastLanguage == "Japanese")
    }

    // MARK: templates

    @MainActor @Test func everyTemplateHasItsBundledDocument() throws {
        #expect(StartingTemplate.all.count == 5)
        #expect(Set(StartingTemplate.all.map(\.id)).count == StartingTemplate.all.count)
        for template in StartingTemplate.all {
            let url = try #require(template.seedURL(), "missing \(template.id).md in the app bundle")
            let text = try String(contentsOf: url, encoding: .utf8)
            #expect(text.contains("Fictional document"))
            #expect(!template.question.isEmpty && template.question.hasSuffix("?"))
            #expect(template.events.allSatisfy { (1...SizePreset.quick.rounds).contains($0.round) })
            #expect(AppModel.isSeedFile(url))
        }
    }
}
