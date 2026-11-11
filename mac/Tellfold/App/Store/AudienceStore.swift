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
    /// Bumped by every write, so a read that a write overtook is thrown away and done again.
    @ObservationIgnored private var generation = 0

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
    /// deleted. A save, rename or delete while the read runs makes it read again, so the list never loses a write.
    func reload() async {
        let directory = directory
        while true {
            let started = generation
            let found = await Task.detached(priority: .utility) { Self.read(directory) }.value
            guard started == generation else { continue }
            files = Dictionary(uniqueKeysWithValues: found.map { ($0.audience.id, $0.file) })
            audiences = Self.sorted(found.map(\.audience))
            return
        }
    }

    /// Every readable audience file. An id already taken (a file copied in Finder keeps the id inside it) gets a
    /// fresh one in memory, so each file is its own audience and a rename writes the new id; the file named after
    /// the id keeps it, then file name order decides.
    nonisolated private static func read(_ directory: URL) -> [(audience: SavedAudience, file: URL)] {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        let urls = (try? FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)) ?? []
        let found: [(audience: SavedAudience, file: URL)] = urls.filter { $0.pathExtension == "json" }.compactMap { url in
            do {
                return (try decoder.decode(SavedAudience.self, from: Data(contentsOf: url)), url)
            } catch {
                log.error("Skipping \(url.lastPathComponent, privacy: .public): \(error.localizedDescription, privacy: .public)")
                return nil
            }
        }
        let owns = { (item: (audience: SavedAudience, file: URL)) in
            item.file.deletingPathExtension().lastPathComponent == item.audience.id
        }
        var seen: Set<String> = []
        return found
            .sorted { (owns($0) ? 0 : 1, $0.file.lastPathComponent) < (owns($1) ? 0 : 1, $1.file.lastPathComponent) }
            .map { item in
                var audience = item.audience
                if !seen.insert(audience.id).inserted {
                    audience.id = UUID().uuidString.lowercased()
                    seen.insert(audience.id)
                }
                return (audience, item.file)
            }
    }

    nonisolated private static func sorted(_ list: [SavedAudience]) -> [SavedAudience] {
        list.sorted { ($0.createdAt, $0.id) > ($1.createdAt, $1.id) }
    }

    func audience(_ id: String) -> SavedAudience? { audiences.first { $0.id == id } }

    /// Saves `personas` (and the topic map and audience material they came from) as a new audience; returns it.
    @discardableResult
    func save(name: String, question: String, personas: [Persona], graph: KnowledgeGraph?,
              material: MaterialSummary? = nil, now: Date = .now) throws -> SavedAudience {
        let audience = SavedAudience(id: UUID().uuidString.lowercased(), name: Self.cleanName(name), createdAt: now,
                                     question: question, personas: personas, graph: graph, material: material)
        let file = directory.appending(path: "\(audience.id).json")
        generation += 1
        try write(audience, to: file)
        files[audience.id] = file
        audiences = Self.sorted(audiences + [audience])
        return audience
    }

    func rename(_ id: String, to name: String) throws {
        guard let i = audiences.firstIndex(where: { $0.id == id }), let file = files[id] else { return }
        var audience = audiences[i]
        audience.name = Self.cleanName(name)
        generation += 1
        try write(audience, to: file)
        audiences[i] = audience
    }

    /// Moves the audience's file to the Trash.
    func delete(_ id: String) throws {
        guard let file = files[id] else { return }
        generation += 1
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
