import Foundation

// Codable mirrors of `RewriteSuggestion` (tellfold/models.py): `rewrite.json` and `POST /api/runs/{id}/rewrite`.

struct RewriteChange: Codable, Hashable, Sendable, Identifiable {
    var what: String
    /// The crowd's reaction behind the change, with `#id` and `@handle` citations.
    var why: String = ""
    /// Ids of the posts that motivated it (checked by the engine).
    var cites: [Int] = []

    var id: String { what + why }
}

extension RewriteChange {
    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        what = c.value(.what, default: "")
        why = c.value(.why, default: "")
        cites = c.value(.cites, default: [])
    }
}

/// A better version of the run's text, written from the report and the posts it cites.
struct RewriteSuggestion: Codable, Hashable, Sendable {
    var revisedText: String
    var changes: [RewriteChange] = []
    var keptOnPurpose: [String] = []
    /// The seed document the revised text replaces.
    var sourceName: String = ""
    var createdAt: String = ""
    var citationAudit: CitationAudit?

    enum CodingKeys: String, CodingKey {
        case changes
        case revisedText = "revised_text"
        case keptOnPurpose = "kept_on_purpose"
        case sourceName = "source_name"
        case createdAt = "created_at"
        case citationAudit = "citation_audit"
    }
}

extension RewriteSuggestion {
    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        revisedText = c.value(.revisedText, default: "")
        changes = c.value(.changes, default: [])
        keptOnPurpose = c.value(.keptOnPurpose, default: [])
        sourceName = c.value(.sourceName, default: "")
        createdAt = c.value(.createdAt, default: "")
        citationAudit = c.optional(.citationAudit)
    }
}
