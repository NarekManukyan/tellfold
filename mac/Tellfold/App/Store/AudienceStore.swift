import Foundation
import Observation
import os

/// Saved audiences: casts kept as JSON files, one per audience, in `~/Library/Application Support/Tellfold/audiences`.
/// The New Simulation sheet starts a run from one (skipping casting); the cast editor saves one.
@MainActor @Observable
final class AudienceStore {
    let directory: URL
    private(set) var audiences: [SavedAudience] = []

    nonisolated private static let log = Logger(subsystem: "dev.tellfold.mac", category: "audiences")

    nonisolated static var defaultDirectory: URL {
        URL.applicationSupportDirectory.appending(path: "Tellfold/audiences", directoryHint: .isDirectory)
    }

    /// The file each audience was read from or written to: rename and delete act on that file, never on a path built
    /// from the id inside it.
    @ObservationIgnored private var files: [String: URL] = [:]

    /// The longest name kept; longer names are cut.
    static let maxNameLength = 80

    /// Starts empty; `reload()` reads the folder.
    init(directory: URL = AudienceStore.defaultDirectory) {
        self.directory = directory
    }

    private static let encoder: JSONEncoder = {
        let e = JSONEncoder()
        e.outputFormatting = [.prettyPrinted, .sortedKeys]
        e.dateEncodingStrategy = .iso8601
        return e
    }()

    /// Reads every audience file off the main actor, newest first. Unreadable files are skipped (and logged), never
    /// deleted.
    func reload() async {
        let directory = directory
        let found = await Task.detached(priority: .utility) { Self.read(directory) }.value
        files = Dictionary(found.map { ($0.audience.id, $0.file) }, uniquingKeysWith: { _, last in last })
        audiences = Self.sorted(found.map(\.audience))
    }

    nonisolated private static func read(_ directory: URL) -> [(audience: SavedAudience, file: URL)] {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        let urls = (try? FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)) ?? []
        return urls.filter { $0.pathExtension == "json" }.compactMap { url in
            do {
                return (try decoder.decode(SavedAudience.self, from: Data(contentsOf: url)), url)
            } catch {
                log.error("Skipping \(url.lastPathComponent, privacy: .public): \(error.localizedDescription, privacy: .public)")
                return nil
            }
        }
    }

    nonisolated private static func sorted(_ list: [SavedAudience]) -> [SavedAudience] {
        list.sorted { ($0.createdAt, $0.id) > ($1.createdAt, $1.id) }
    }

    func audience(_ id: String) -> SavedAudience? { audiences.first { $0.id == id } }

    /// Saves `personas` (and the topic map they came from) as a new audience; returns it.
    @discardableResult
    func save(name: String, question: String, personas: [Persona], graph: KnowledgeGraph?,
              now: Date = .now) throws -> SavedAudience {
        let audience = SavedAudience(id: UUID().uuidString.lowercased(), name: Self.cleanName(name), createdAt: now,
                                     question: question, personas: personas, graph: graph)
        let file = directory.appending(path: "\(audience.id).json")
        try write(audience, to: file)
        files[audience.id] = file
        audiences = Self.sorted(audiences + [audience])
        return audience
    }

    func rename(_ id: String, to name: String) throws {
        guard let i = audiences.firstIndex(where: { $0.id == id }), let file = files[id] else { return }
        var audience = audiences[i]
        audience.name = Self.cleanName(name)
        try write(audience, to: file)
        audiences[i] = audience
    }

    /// Moves the audience's file to the Trash.
    func delete(_ id: String) throws {
        guard let file = files[id] else { return }
        try FileManager.default.trashItem(at: file, resultingItemURL: nil)
        files[id] = nil
        audiences.removeAll { $0.id == id }
    }

    /// A trimmed, single-line name; "Saved audience" when empty.
    static func cleanName(_ name: String) -> String {
        let words = name.split(whereSeparator: \.isWhitespace).joined(separator: " ")
        return words.isEmpty ? "Saved audience" : String(words.prefix(maxNameLength))
    }

    private func write(_ audience: SavedAudience, to file: URL) throws {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        try Self.encoder.encode(audience).write(to: file, options: .atomic)
    }
}
