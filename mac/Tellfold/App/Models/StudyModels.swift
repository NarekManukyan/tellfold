import Foundation

// Codable mirrors of the study API (`/api/studies`, tellfold/server/app.py) and of `compare.json`
// (`CompareResult` in tellfold/models.py). A study groups runs of one crowd that are compared with each other; every
// run has a letter (A, B, ...), and the comparison cites posts as `A#12`, post 12 of run A.

/// One run of a study, as `GET /api/studies` lists it.
struct StudyMember: Codable, Hashable, Sendable, Identifiable {
    var id: String
    var variant: String
    var letter: String
}

/// `GET /api/studies`: one row per study.
struct StudySummary: Codable, Hashable, Sendable, Identifiable {
    var id: String
    var kind: String = "compare"
    /// ISO 8601 UTC.
    var createdAt: String = ""
    var question: String = ""
    var runs: [StudyMember] = []
    var hasCompare: Bool = false
    var comparing: Bool = false

    enum CodingKeys: String, CodingKey {
        case id, kind, question, runs, comparing
        case createdAt = "created_at"
        case hasCompare = "has_compare"
    }
}

extension StudySummary {
    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        kind = c.value(.kind, default: "compare")
        createdAt = c.value(.createdAt, default: "")
        question = c.value(.question, default: "")
        runs = c.value(.runs, default: [])
        hasCompare = c.value(.hasCompare, default: false)
        comparing = c.value(.comparing, default: false)
    }
}

struct RoundPoint: Codable, Hashable, Sendable {
    var round: Int
    var sentiment: Double?
    var posts: Int = 0
}

struct CampDigest: Codable, Hashable, Sendable, Identifiable {
    var name: String
    var size: Int
    var sentiment: Double?
    var summary: String = ""

    var id: String { name }
}

/// What the engine measures about one run of a study before any model call.
struct RunDigest: Codable, Hashable, Sendable {
    var letter: String
    var runID: String
    var variant: String
    var state: String = ""
    var headline: String?
    var rounds: [RoundPoint] = []
    var finalSentiment: Double?
    var camps: [CampDigest] = []
    var objections: [String] = []
    var posts: Int = 0

    enum CodingKeys: String, CodingKey {
        case letter, variant, state, headline, rounds, camps, objections, posts
        case runID = "run_id"
        case finalSentiment = "final_sentiment"
    }
}

extension RunDigest {
    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        letter = c.value(.letter, default: "?")
        runID = c.value(.runID, default: "")
        variant = c.value(.variant, default: "")
        state = c.value(.state, default: "")
        headline = c.optional(.headline)
        rounds = c.value(.rounds, default: [])
        finalSentiment = c.optional(.finalSentiment)
        camps = c.value(.camps, default: [])
        objections = c.value(.objections, default: [])
        posts = c.value(.posts, default: 0)
    }
}

struct VariantSummary: Codable, Hashable, Sendable, Identifiable {
    var variant: String
    var summary: String = ""
    var objections: [String] = []

    var id: String { variant }
}

struct VariantObjections: Codable, Hashable, Sendable, Identifiable {
    var variant: String
    var objections: [String] = []

    var id: String { variant }
}

/// The cited comparison written by one `compare.write` call.
struct CompareReport: Codable, Hashable, Sendable {
    var headline: String
    /// The label of the version the crowd took better; nil for a tie.
    var betterVariant: String?
    var why: String = ""
    var perVariant: [VariantSummary] = []
    var objectionsOnlyIn: [VariantObjections] = []
    var caveats: [String] = []
    var citationAudit: CitationAudit?

    enum CodingKeys: String, CodingKey {
        case headline, why, caveats
        case betterVariant = "better_variant"
        case perVariant = "per_variant"
        case objectionsOnlyIn = "objections_only_in"
        case citationAudit = "citation_audit"
    }
}

extension CompareReport {
    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        headline = c.value(.headline, default: "Comparison")
        betterVariant = c.optional(.betterVariant)
        why = c.value(.why, default: "")
        perVariant = c.value(.perVariant, default: [])
        objectionsOnlyIn = c.value(.objectionsOnlyIn, default: [])
        caveats = c.value(.caveats, default: [])
        citationAudit = c.optional(.citationAudit)
    }
}

/// `studies/<id>/compare.json`.
struct CompareResult: Codable, Hashable, Sendable {
    var studyID: String
    var createdAt: String = ""
    var runs: [RunDigest] = []
    var report: CompareReport

    enum CodingKeys: String, CodingKey {
        case runs, report
        case studyID = "study_id"
        case createdAt = "created_at"
    }
}

/// One run in `GET /api/studies/{id}`: its summary and its digest.
struct StudyRun: Codable, Hashable, Sendable, Identifiable {
    var id: String
    var variant: String
    var letter: String
    var question: String = ""
    var state: RunState = .unknown
    var hasReport: Bool = false
    var example: Bool = false
    var digest: RunDigest

    enum CodingKeys: String, CodingKey {
        case id, variant, letter, question, state, example, digest
        case hasReport = "has_report"
    }
}

extension StudyRun {
    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        variant = c.value(.variant, default: "")
        letter = c.value(.letter, default: "?")
        question = c.value(.question, default: "")
        state = c.value(.state, default: .unknown)
        hasReport = c.value(.hasReport, default: false)
        example = c.value(.example, default: false)
        digest = c.value(.digest, default: RunDigest(letter: letter, runID: id, variant: variant))
    }
}

struct CompareState: Codable, Hashable, Sendable {
    var running: Bool = false
    /// Why the last comparison in this engine failed.
    var error: String?
}

/// `GET /api/studies/{id}`.
struct StudyDetail: Codable, Hashable, Sendable, Identifiable {
    var id: String
    var kind: String = "compare"
    var createdAt: String = ""
    var runs: [StudyRun] = []
    var compare: CompareResult?
    var compareState: CompareState = .init()

    enum CodingKeys: String, CodingKey {
        case id, kind, runs, compare
        case createdAt = "created_at"
        case compareState = "compare_state"
    }
}

extension StudyDetail {
    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        kind = c.value(.kind, default: "compare")
        createdAt = c.value(.createdAt, default: "")
        runs = c.value(.runs, default: [])
        compare = c.optional(.compare)
        compareState = c.value(.compareState, default: CompareState())
    }
}
