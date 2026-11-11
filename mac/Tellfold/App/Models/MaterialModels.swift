import Foundation

/// The kinds of audience material the crowd can be built from (`MATERIAL_KINDS` in
/// engine-rs/crates/tellfold-models/src/lib.rs), in the order they are listed everywhere.
nonisolated enum MaterialKind: String, CaseIterable, Identifiable, Sendable {
    case reviews, survey, emails, comments, notes

    var id: String { rawValue }

    var title: String {
        switch self {
        case .reviews: "Customer reviews"
        case .survey: "Survey answers"
        case .emails: "Support emails"
        case .comments: "Social comments"
        case .notes: "Meeting notes"
        }
    }

    /// "1 review", "14 reviews".
    func noun(_ n: Int) -> String {
        let (one, many) = switch self {
        case .reviews: ("review", "reviews")
        case .survey: ("survey answer", "survey answers")
        case .emails: ("support email", "support emails")
        case .comments: ("social comment", "social comments")
        case .notes: ("meeting note", "meeting notes")
        }
        return "\(n) \(n == 1 ? one : many)"
    }

    /// File types material is read from (`MATERIAL_SUFFIXES` in tellfold-engine/src/material.rs).
    static let extensions: Set<String> = ["txt", "md", "markdown", "csv", "tsv", "eml"]

    static func isSpreadsheet(_ url: URL) -> Bool { ["csv", "tsv"].contains(url.pathExtension.lowercased()) }

    /// What a file most likely holds by its type: .eml files are emails, spreadsheets survey answers, anything
    /// else reviews until the user says otherwise.
    static func guess(for url: URL) -> MaterialKind {
        switch url.pathExtension.lowercased() {
        case "eml": .emails
        case "csv", "tsv": .survey
        default: .reviews
        }
    }
}

/// The engine's limits for audience material (tellfold-engine/src/material.rs).
nonisolated enum MaterialLimits {
    static let maxFiles = 20
    static let maxFileBytes = 10 * 1024 * 1024
    /// Items the planner reads per call (`MATERIAL_ITEMS_PER_CALL` in tellfold-store/src/estimate.rs).
    static let itemsPerCall = 40
}

/// The wording for audience material, kept free of views so it can be tested.
nonisolated enum MaterialText {
    /// "14 reviews and 3 survey answers", kinds in their usual order, kinds with none left out.
    static func counts(_ counts: [String: Int]) -> String {
        var parts = MaterialKind.allCases.compactMap { kind -> String? in
            guard let n = counts[kind.rawValue], n > 0 else { return nil }
            return kind.noun(n)
        }
        let other = counts.filter { MaterialKind(rawValue: $0.key) == nil }.values.reduce(0, +)
        if other > 0 { parts.append("\(other) \(other == 1 ? "item" : "items")") }
        switch parts.count {
        case 0: return "no material"
        case 1: return parts[0]
        default: return parts.dropLast().joined(separator: ", ") + " and " + parts[parts.count - 1]
        }
    }

    /// "4 names, 2 email addresses"; nil when nothing needed hiding.
    static func hidden(_ redacted: [String: Int]) -> String? {
        let order: [(String, String, String)] = [
            ("names", "name", "names"), ("emails", "email address", "email addresses"),
            ("phones", "phone number", "phone numbers"), ("addresses", "street address", "street addresses"),
            ("numbers", "order or account number", "order or account numbers"), ("links", "link", "links"),
        ]
        let parts = order.compactMap { key, one, many -> String? in
            guard let n = redacted[key], n > 0 else { return nil }
            return "\(n) \(n == 1 ? one : many)"
        }
        return parts.isEmpty ? nil : parts.joined(separator: ", ")
    }

    /// Extra words to hide, from what the user typed: one per comma or line, trimmed, empty ones dropped.
    static func hideWords(_ text: String) -> [String] {
        text.split(whereSeparator: { $0 == "," || $0.isNewline })
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
    }

    /// Planner calls that read `items` items of material.
    static func calls(items: Int) -> Int {
        items <= 0 ? 0 : (items + MaterialLimits.itemsPerCall - 1) / MaterialLimits.itemsPerCall
    }
}

/// What a run's crowd was built from (`SimConfig.material`); the redacted items stay in the run folder.
nonisolated struct MaterialSummary: Codable, Hashable, Sendable {
    var items: Int = 0
    var counts: [String: Int] = [:]
    var dropped: Int = 0
    var redacted: [String: Int] = [:]
    var files: [String] = []

    /// "Based on 14 reviews and 3 survey answers".
    var basedOn: String { "Based on \(MaterialText.counts(counts))" }

    init(items: Int = 0, counts: [String: Int] = [:], dropped: Int = 0, redacted: [String: Int] = [:],
         files: [String] = []) {
        self.items = items
        self.counts = counts
        self.dropped = dropped
        self.redacted = redacted
        self.files = files
    }

    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        items = c.value(.items, default: 0)
        counts = c.value(.counts, default: [:])
        dropped = c.value(.dropped, default: 0)
        redacted = c.value(.redacted, default: [:])
        files = c.value(.files, default: [])
    }
}

/// The slice of the user's material a person was written from (`Persona.grounding`).
nonisolated struct Grounding: Codable, Hashable, Sendable {
    /// The worry or hope that slice shares, in a few words.
    var theme: String = ""
    /// Its items by kind.
    var counts: [String: Int] = [:]
    /// The redacted quotes the person was written from.
    var quotes: [String] = []

    /// "Based on 14 reviews and 3 survey answers".
    var basedOn: String { "Based on \(MaterialText.counts(counts))" }

    init(theme: String = "", counts: [String: Int] = [:], quotes: [String] = []) {
        self.theme = theme
        self.counts = counts
        self.quotes = quotes
    }

    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        theme = c.value(.theme, default: "")
        counts = c.value(.counts, default: [:])
        quotes = c.value(.quotes, default: [])
    }
}

/// One file of audience material added on the New simulation sheet.
nonisolated struct MaterialInput: Identifiable, Hashable, Sendable {
    var url: URL
    var kind: MaterialKind
    /// A spreadsheet's column, by header; nil lets the engine take the one with the most text.
    var column: String?

    var id: URL { url }

    init(url: URL, kind: MaterialKind? = nil, column: String? = nil) {
        self.url = url
        self.kind = kind ?? MaterialKind.guess(for: url)
        self.column = column
    }
}

/// `POST /api/material/preview`: exactly what a run with this material would send.
nonisolated struct MaterialPreview: Decodable, Hashable, Sendable {
    struct File: Decodable, Hashable, Sendable {
        var name: String
        var kind: String
        var items: Int
        var columns: [String] = []
        var column: String?

        init(from decoder: any Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            name = try c.decode(String.self, forKey: .name)
            kind = c.value(.kind, default: "")
            items = c.value(.items, default: 0)
            columns = c.value(.columns, default: [])
            column = c.optional(.column)
        }

        enum CodingKeys: String, CodingKey { case name, kind, items, columns, column }
    }

    struct Item: Decodable, Hashable, Sendable, Identifiable {
        var id: String
        var kind: String
        var file: String
        var text: String
    }

    var summary: MaterialSummary
    var files: [File]
    var items: [Item]
    var warnings: [String]
    var thin: Bool
    var estimatedCalls: Int

    enum CodingKeys: String, CodingKey {
        case summary, files, items, warnings, thin
        case estimatedCalls = "estimated_calls"
    }

    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        summary = try c.decode(MaterialSummary.self, forKey: .summary)
        files = c.value(.files, default: [])
        items = c.value(.items, default: [])
        warnings = c.value(.warnings, default: [])
        thin = c.value(.thin, default: false)
        estimatedCalls = c.value(.estimatedCalls, default: MaterialText.calls(items: summary.items))
    }

    /// The report of the file at `url`, by the name it was sent under.
    func file(named name: String) -> File? { files.first { $0.name == name } }
}

/// Splits a redacted item into plain runs and the `[name]`-style placeholders, so a view can mark what was hidden.
nonisolated enum RedactedText {
    static let placeholders: Set<String> = ["[name]", "[email]", "[phone]", "[address]", "[number]", "[link]"]

    /// The text in order: (piece, isPlaceholder).
    static func pieces(_ text: String) -> [(String, Bool)] {
        var out: [(String, Bool)] = []
        var rest = Substring(text)
        while let open = rest.firstIndex(of: "[") {
            if let close = rest[open...].firstIndex(of: "]"),
               placeholders.contains(String(rest[open...close])) {
                if open > rest.startIndex { out.append((String(rest[..<open]), false)) }
                out.append((String(rest[open...close]), true))
                rest = rest[rest.index(after: close)...]
            } else {
                let next = rest.index(after: open)
                out.append((String(rest[..<next]), false))
                rest = rest[next...]
            }
        }
        if !rest.isEmpty { out.append((String(rest), false)) }
        // merge neighbouring plain runs
        var merged: [(String, Bool)] = []
        for piece in out {
            if let last = merged.last, !last.1, !piece.1 {
                merged[merged.count - 1].0 += piece.0
            } else {
                merged.append(piece)
            }
        }
        return merged
    }
}

/// How a grounded person's source reads in the cast editor and the inspector.
nonisolated enum GroundingText {
    /// "Based on 14 reviews and 3 survey answers · price keeps going up".
    static func line(_ grounding: Grounding) -> String {
        let theme = grounding.theme.trimmingCharacters(in: .whitespacesAndNewlines)
        return theme.isEmpty ? grounding.basedOn : "\(grounding.basedOn) · \(theme)"
    }
}
