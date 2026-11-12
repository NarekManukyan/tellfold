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

/// Where a persona came from (`SOURCES` in engine-rs/crates/tellfold-engine/src/personas.rs); the engine drops any
/// other value.
enum PersonaSource: String, Sendable {
    /// Planned and written by the engine.
    case cast
    /// Written from a group the user described in words.
    case group
    /// Changed by the user in the cast editor.
    case edited
    /// Written from the user's own audience material (reviews, survey answers, ...).
    case material

    init?(_ persona: Persona) {
        guard let source = persona.source.flatMap(PersonaSource.init(rawValue:)) else { return nil }
        self = source
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
    /// The question of the run the cast was made for; "" in files written without one.
    var question: String
    var personas: [Persona]
    /// The topic map the people were cast from; the engine reuses it with the people.
    var graph: KnowledgeGraph?
    /// The audience material the people were built from, so a run with them can say so in its report.
    var material: MaterialSummary?

    enum CodingKeys: String, CodingKey {
        case id, name, question, personas, graph, material
        case createdAt = "created_at"
    }

    init(id: String, name: String, createdAt: Date, question: String = "", personas: [Persona],
         graph: KnowledgeGraph?, material: MaterialSummary? = nil) {
        self.id = id
        self.name = name
        self.createdAt = createdAt
        self.question = question
        self.personas = personas
        self.graph = graph
        self.material = material
    }

    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        name = try c.decode(String.self, forKey: .name)
        createdAt = try c.decode(Date.self, forKey: .createdAt)
        question = c.value(.question, default: "")
        personas = try c.decode([Persona].self, forKey: .personas)
        graph = c.optional(.graph)
        material = c.optional(.material)
    }

    /// The `audience` form field of `POST /api/runs`.
    func formJSON() throws -> String {
        struct Field: Encodable {
            let name: String
            let personas: [Persona]
            let graph: KnowledgeGraph?
            let material: MaterialSummary?
        }
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        let field = Field(name: name, personas: personas, graph: graph, material: material)
        return String(decoding: try encoder.encode(field), as: UTF8.self)
    }
}
