import Foundation
import Observation

/// The cast of a run waiting at `personas_ready`, as the cast editor edits it. Removing and editing people change
/// the local copy only; adding a group, closing the editor and starting the run send the edits first.
///
/// The engine renumbers ids (a01..aNN) whenever it takes the cast back, so an id names a different person afterwards:
/// every time the cast comes back from the engine the open row editor closes, and `update(_:from:)` applies an edit
/// only to the person it was started on.
@MainActor @Observable
final class CastEditorModel {
    let runID: String
    @ObservationIgnored private let api: TellfoldAPI?

    private(set) var people: [Persona] = []
    private(set) var isLoading = false
    private(set) var isAddingGroup = false
    private(set) var isContinuing = false
    private(set) var isSaving = false
    /// The person whose row editor is open.
    private(set) var editingID: String?
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

    var isBusy: Bool { isLoading || isAddingGroup || isContinuing || isSaving }
    var canContinue: Bool { !people.isEmpty && !isBusy }
    /// "Later" waits while a row editor is open: its unsaved draft lives in the row and would be lost.
    var canClose: Bool { !isBusy && editingID == nil }

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
            replace(with: try await api.cast(runID: runID).personas)
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    /// Opens the row editor for `id`, or closes it when it is already open.
    func toggleEditing(_ id: String) {
        editingID = editingID == id ? nil : id
    }

    func remove(_ id: String) {
        if editingID == id { editingID = nil }
        people.removeAll { $0.id == id }
        hasLocalEdits = true
    }

    /// Applies an edit started on `original`. Refused (false) when the person with that id is no longer `original`,
    /// for example after the engine renumbered the cast; a change to what they say or think marks them edited.
    @discardableResult
    func update(_ edited: Persona, from original: Persona) -> Bool {
        defer { if editingID == original.id { editingID = nil } }
        guard edited.id == original.id, let i = people.firstIndex(where: { $0.id == edited.id }),
              people[i] == original else { return false }
        var person = edited
        if (original.bio, original.stance, original.background, original.camp, original.name) !=
            (person.bio, person.stance, person.background, person.camp, person.name) {
            person.source = PersonaSource.edited.rawValue
        }
        guard person != original else { return true }
        people[i] = person
        hasLocalEdits = true
        return true
    }

    /// The cast as the engine has it now: ids may name other people, so no row editor stays open.
    private func replace(with cast: [Persona]) {
        people = cast
        hasLocalEdits = false
        editingID = nil
    }

    /// Sends local edits so the engine's cast matches what the editor shows.
    private func push() async throws {
        guard hasLocalEdits, let api else { return }
        replace(with: try await api.replaceCast(runID: runID, personas: people))
    }

    /// Sends the edits so they are still there when the editor opens again ("Later"); an empty cast is sent too, since
    /// the engine keeps it while the run waits. Returns false when it failed or a row editor is open.
    @discardableResult
    func saveForLater() async -> Bool {
        guard canClose else { return false }
        isSaving = true
        defer { isSaving = false }
        do {
            try await push()
            errorMessage = nil
            return true
        } catch {
            errorMessage = error.localizedDescription
            return false
        }
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
            replace(with: try await api.addGroup(runID: runID, description: text,
                                                 count: min(max(count, 1), Self.maxGroup)))
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
            editingID = nil
            try await api.confirmCast(runID: runID)
            return true
        } catch {
            errorMessage = error.localizedDescription
            return false
        }
    }

    /// Saves the cast as shown (edits included) as a reusable audience.
    func saveAudience(name: String, question: String, graph: KnowledgeGraph?, material: MaterialSummary? = nil,
                      to store: AudienceStore) -> SavedAudience? {
        do {
            // grounded people keep their grounding; the summary lets a run with them name its material
            let grounded = people.contains { $0.grounding != nil } ? material : nil
            return try store.save(name: name, question: question, personas: people, graph: graph, material: grounded)
        } catch {
            errorMessage = "Could not save the audience: \(error.localizedDescription)"
            return nil
        }
    }
}
