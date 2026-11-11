import Foundation

/// The opinion groups the engine plans a cast around (`CAMPS` in engine-rs/crates/tellfold-engine/src/personas.rs),
/// in the order the cast editor lists them.
enum CastCamp: String, CaseIterable, Identifiable, Sendable {
    case strongSupport = "strong_support"
    case leanSupport = "lean_support"
    case undecided
    case indifferent
    case leanCritical = "lean_critical"
    case strongCritical = "strong_critical"

    var id: String { rawValue }

    var title: String {
        switch self {
        case .strongSupport: "Strongly for"
        case .leanSupport: "Leans for"
        case .undecided: "Undecided"
        case .indifferent: "Indifferent"
        case .leanCritical: "Leans against"
        case .strongCritical: "Strongly against"
        }
    }

    /// The label for a persona's camp: the planned camp, or what the person is when the engine planned none.
    static func label(for persona: Persona) -> String {
        if let camp = persona.camp.flatMap(CastCamp.init(rawValue:)) { return camp.title }
        return persona.entityID != nil ? "Organisation or public figure" : "Not planned"
    }
}

/// `GET`/`PUT /api/runs/{id}/personas`: the cast under review while the run waits, else the run's people.
struct CastResponse: Codable, Hashable, Sendable {
    var waiting: Bool
    var personas: [Persona]
}

/// A cast saved to reuse in later runs ("Save as audience"). Stored as JSON in
/// `~/Library/Application Support/Tellfold/audiences/<id>.json`; starting a run from it skips the graph and persona
/// stages, so it costs fewer model calls.
struct SavedAudience: Codable, Hashable, Sendable, Identifiable {
    var id: String
    var name: String
    var createdAt: Date
    /// The question of the run the cast was made for.
    var question: String = ""
    var personas: [Persona]
    /// The topic map the people were cast from; the engine reuses it with the people.
    var graph: KnowledgeGraph?

    enum CodingKeys: String, CodingKey {
        case id, name, question, personas, graph
        case createdAt = "created_at"
    }

    /// The `audience` form field of `POST /api/runs`.
    func formJSON() throws -> String {
        struct Field: Encodable {
            let name: String
            let personas: [Persona]
            let graph: KnowledgeGraph?
        }
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        return String(decoding: try encoder.encode(Field(name: name, personas: personas, graph: graph)), as: UTF8.self)
    }
}
