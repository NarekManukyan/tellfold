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

    /// File types material is read from (`MATERIAL_SUFFIXES` in tellfold-engine/src/material.rs), as listed in hints.
    static let orderedExtensions = ["txt", "md", "markdown", "csv", "tsv", "eml"]
    static let extensions = Set(orderedExtensions)

    /// ".txt, .md, .markdown, .csv, .tsv or .eml".
    static let extensionHint: String = {
        let dotted = orderedExtensions.map { "." + $0 }
        return dotted.dropLast().joined(separator: ", ") + " or " + dotted[dotted.count - 1]
    }()

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
    /// What the engine's redaction removes, in the engine's wording. It is not every name: outside greetings, sign-offs
    /// and introductions it knows only a list of common first names, so every claim about it uses this one list.
    static let redactedCategories = "common first names, names in greetings, sign-offs and introductions, @handles, "
        + "email addresses, phone numbers, street addresses, order or account numbers and links"

    /// What the engine cannot promise, said wherever the cleaned text is shown.
    static let namesMayRemain = "Names the rules don't know can remain: check the preview and add them below."

    /// The note under the crowd size when the material has fewer items than people (the engine words it the same).
    /// Nil when the material is thin (its own warning says so), or has enough items.
    static func fewerItemsNote(items: Int, agents: Int, thin: Bool) -> String? {
        guard !thin, items > 0, items < agents else { return nil }
        return "\(items) items for \(agents) people: some people will be written from the same quotes."
    }

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
    /// What was sent stays as it is; this is only present when the planner could read fewer items than were sent, and
    /// then holds the items by kind the people were actually built from.
    var read: [String: Int]?

    /// The items by kind the crowd was built from: what the planner read when that was less than what was sent.
    var readCounts: [String: Int] { read ?? counts }

    /// "Based on 14 reviews and 3 survey answers".
    var basedOn: String { "Based on \(MaterialText.counts(readCounts))" }

    init(items: Int = 0, counts: [String: Int] = [:], dropped: Int = 0, redacted: [String: Int] = [:],
         files: [String] = [], read: [String: Int]? = nil) {
        self.items = items
        self.counts = counts
        self.dropped = dropped
        self.redacted = redacted
        self.files = files
        self.read = read
    }

    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        items = c.value(.items, default: 0)
        counts = c.value(.counts, default: [:])
        dropped = c.value(.dropped, default: 0)
        redacted = c.value(.redacted, default: [:])
        files = c.value(.files, default: [])
        read = c.optional(.read)
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
        /// The neutral label the run keeps instead of the file's name ("reviews 1"); nil in older replies.
        var label: String?

        init(from decoder: any Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            name = try c.decode(String.self, forKey: .name)
            label = c.optional(.label)
            kind = c.value(.kind, default: "")
            items = c.value(.items, default: 0)
            columns = c.value(.columns, default: [])
            column = c.optional(.column)
        }

        enum CodingKeys: String, CodingKey { case name, kind, items, columns, column, label }
    }

    struct Item: Decodable, Hashable, Sendable, Identifiable {
        var id: String
        var kind: String
        /// The neutral label of the file it came from ("reviews 1"), not the file's name.
        var file: String
        var text: String
    }

    var summary: MaterialSummary
    /// The reports of the files, in the order the form sent them; the engine renames each file, so match them by
    /// position, never by name.
    var files: [File]
    /// The files this preview was read for, set by the sheet after decoding; empty when unknown.
    var inputs: [MaterialInput] = []
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

    /// The report of `current[index]`, only when this preview was read for exactly these files, kinds and columns
    /// (same count, same order); otherwise nil, since the report would be another file's or out of date.
    func report(at index: Int, of current: [MaterialInput]) -> File? {
        guard inputs == current, files.count == current.count, current.indices.contains(index) else { return nil }
        return files[index]
    }

    /// The report of `current[index]` while only its kind or column changed: the same files in the same order. Its
    /// column list still holds, so the picker stays put while the preview is read again.
    func columnsReport(at index: Int, of current: [MaterialInput]) -> File? {
        guard inputs.map(\.url) == current.map(\.url), files.count == current.count,
              current.indices.contains(index) else { return nil }
        return files[index]
    }

    /// What to show for an item's `file`: the local name of the file with that label, else the label itself.
    func fileName(forLabel label: String) -> String {
        guard files.count == inputs.count, let index = files.firstIndex(where: { $0.label == label }) else { return label }
        return inputs[index].url.lastPathComponent
    }
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

extension RunDetail {
    /// Built from the user's audience material: a person carries the quotes they were written from (an edited person
    /// keeps them, and an audience from the API can have such people without a `config.material`).
    var isGrounded: Bool {
        personas.contains { !($0.grounding?.quotes.isEmpty ?? true) }
    }
}
