import Foundation
import Observation

/// The cast of a run waiting at `personas_ready`, as the cast editor edits it. Removing and editing people change
/// the local copy only; adding a group, saving an audience and starting the run send the edits first.
@MainActor @Observable
final class CastEditorModel {
    let runID: String
    @ObservationIgnored private let api: TellfoldAPI?

    private(set) var people: [Persona] = []
    private(set) var isLoading = false
    private(set) var isAddingGroup = false
    private(set) var isContinuing = false
    /// Edits not sent to the engine yet.
    private(set) var hasLocalEdits = false
    var errorMessage: String?

    /// The most a cast may hold and one group may add (`MAX_CAST` and `MAX_GROUP` in the engine).
    static let maxPeople = 200
    static let maxGroup = 20

    init(runID: String, api: TellfoldAPI?, people: [Persona] = []) {
        self.runID = runID
        self.api = api
        self.people = people
    }

    var isBusy: Bool { isLoading || isAddingGroup || isContinuing }
    var canContinue: Bool { !people.isEmpty && !isBusy }

    /// How many people each camp has, in `CastCamp` order; people without a planned camp are left out.
    var campCounts: [(camp: CastCamp, count: Int)] {
        CastCamp.allCases.compactMap { camp in
            let n = people.filter { $0.camp == camp.rawValue }.count
            return n > 0 ? (camp, n) : nil
        }
    }

    func load() async {
        guard let api else { return }
        isLoading = true
        defer { isLoading = false }
        do {
            people = try await api.cast(runID: runID).personas
            hasLocalEdits = false
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func remove(_ id: String) {
        people.removeAll { $0.id == id }
        hasLocalEdits = true
    }

    /// Replaces the person with `edited`'s id; a change to what they say or think marks them "edited".
    func update(_ edited: Persona) {
        guard let i = people.firstIndex(where: { $0.id == edited.id }) else { return }
        var person = edited
        let old = people[i]
        if (old.bio, old.stance, old.background, old.camp, old.name) !=
            (person.bio, person.stance, person.background, person.camp, person.name) {
            person.source = "edited"
        }
        guard person != old else { return }
        people[i] = person
        hasLocalEdits = true
    }

    /// Sends local edits so the engine's cast matches what the editor shows.
    private func push() async throws {
        guard hasLocalEdits, let api else { return }
        people = try await api.replaceCast(runID: runID, personas: people)
        hasLocalEdits = false
    }

    /// The engine writes `count` people for `description` and adds them. Returns false when it failed.
    @discardableResult
    func addGroup(description: String, count: Int) async -> Bool {
        let text = description.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, let api, !isBusy else { return false }
        isAddingGroup = true
        defer { isAddingGroup = false }
        do {
            try await push()
            people = try await api.addGroup(runID: runID, description: text,
                                            count: min(max(count, 1), Self.maxGroup))
            errorMessage = nil
            return true
        } catch {
            errorMessage = error.localizedDescription
            return false
        }
    }

    /// Sends the edits and lets the run go on. Returns false when it failed.
    @discardableResult
    func confirm() async -> Bool {
        guard let api, canContinue else { return false }
        isContinuing = true
        defer { isContinuing = false }
        do {
            try await push()
            try await api.confirmCast(runID: runID)
            return true
        } catch {
            errorMessage = error.localizedDescription
            return false
        }
    }

    /// Saves the cast as shown (edits included) as a reusable audience.
    func saveAudience(name: String, question: String, graph: KnowledgeGraph?, to store: AudienceStore) -> SavedAudience? {
        do {
            return try store.save(name: name, question: question, personas: people, graph: graph)
        } catch {
            errorMessage = "Could not save the audience: \(error.localizedDescription)"
            return nil
        }
    }
}
