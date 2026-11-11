import Foundation
import Observation
import os

/// Saved audiences: casts kept as JSON files, one per audience, in `~/Library/Application Support/Tellfold/audiences`.
/// The New Simulation sheet starts a run from one (skipping casting); the cast editor saves one.
@MainActor @Observable
final class AudienceStore {
    let directory: URL
    private(set) var audiences: [SavedAudience] = []

    private static let log = Logger(subsystem: "dev.tellfold.mac", category: "audiences")

    nonisolated static var defaultDirectory: URL {
        URL.applicationSupportDirectory.appending(path: "Tellfold/audiences", directoryHint: .isDirectory)
    }

    init(directory: URL = AudienceStore.defaultDirectory) {
        self.directory = directory
        reload()
    }

    private static let encoder: JSONEncoder = {
        let e = JSONEncoder()
        e.outputFormatting = [.prettyPrinted, .sortedKeys]
        e.dateEncodingStrategy = .iso8601
        return e
    }()

    private static let decoder: JSONDecoder = {
        let d = JSONDecoder()
        d.dateDecodingStrategy = .iso8601
        return d
    }()

    /// Reads every audience file, newest first. Unreadable files are skipped (and logged), never deleted.
    func reload() {
        let files = (try? FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)) ?? []
        audiences = files.filter { $0.pathExtension == "json" }.compactMap { url in
            do {
                return try Self.decoder.decode(SavedAudience.self, from: Data(contentsOf: url))
            } catch {
                Self.log.error("Skipping \(url.lastPathComponent, privacy: .public): \(error.localizedDescription, privacy: .public)")
                return nil
            }
        }
        .sorted { ($0.createdAt, $0.id) > ($1.createdAt, $1.id) }
    }

    func audience(_ id: String) -> SavedAudience? { audiences.first { $0.id == id } }

    /// Saves `personas` (and the topic map they came from) as a new audience; returns it.
    @discardableResult
    func save(name: String, question: String, personas: [Persona], graph: KnowledgeGraph?,
              now: Date = .now) throws -> SavedAudience {
        let audience = SavedAudience(id: UUID().uuidString.lowercased(), name: Self.cleanName(name), createdAt: now,
                                     question: question, personas: personas, graph: graph)
        try write(audience)
        reload()
        return audience
    }

    func rename(_ id: String, to name: String) throws {
        guard var audience = audience(id) else { return }
        audience.name = Self.cleanName(name)
        try write(audience)
        reload()
    }

    /// Moves the audience's file to the Trash.
    func delete(_ id: String) throws {
        try FileManager.default.trashItem(at: fileURL(id), resultingItemURL: nil)
        reload()
    }

    /// A trimmed, single-line name; "Saved audience" when empty.
    static func cleanName(_ name: String) -> String {
        let words = name.split(whereSeparator: \.isWhitespace).joined(separator: " ")
        return words.isEmpty ? "Saved audience" : String(words.prefix(80))
    }

    private func fileURL(_ id: String) -> URL { directory.appending(path: "\(id).json") }

    private func write(_ audience: SavedAudience) throws {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        try Self.encoder.encode(audience).write(to: fileURL(audience.id), options: .atomic)
    }
}
