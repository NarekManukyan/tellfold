import Foundation

// Canvas-side value types. The graph canvases never touch the API models directly: `RunStore+Canvas.swift` (knowledge
// graph) and `Crowd/RunStore+Crowd.swift` (crowd graph) are the only files that map RunStore / Codable model names
// onto these, so a rename there is a one-file change.

nonisolated struct CanvasFollow: Sendable, Hashable {
    var follower: String
    var followee: String
}

nonisolated struct CanvasEntity: Sendable, Hashable, Identifiable {
    var id: String
    var name: String
    var kind: String
    var summary: String
}

nonisolated struct CanvasRelation: Sendable, Hashable {
    var source: String
    var target: String
    var label: String
}

nonisolated struct KnowledgeGraphInput: Sendable, Equatable {
    var entities: [CanvasEntity] = []
    var relations: [CanvasRelation] = []
    /// entity id → persona (agent) ids that embody it.
    var embodiedBy: [String: [String]] = [:]
    var agentNames: [String: String] = [:]
    var selectedAgentID: String?
    var isLoaded = false

    var layoutKey: Int {
        var h = Hasher()
        for e in entities { h.combine(e.id) }
        for r in relations {
            h.combine(r.source)
            h.combine(r.target)
        }
        return h.finalize()
    }
}

/// Helpers that tolerate either optional or non-optional, String or String-backed enum fields in the API models.
nonisolated enum CanvasCoerce {
    static func string(_ value: Any) -> String {
        if let s = value as? String { return s }
        if let r = value as? any RawRepresentable, let s = r.rawValue as? String { return s }
        return String(describing: value)
    }

    static func double(_ value: Any) -> Double? {
        if let d = value as? Double { return d.isFinite ? d : nil }
        if let f = value as? Float { return Double(f) }
        if let c = value as? CGFloat { return Double(c) }
        if let i = value as? Int { return Double(i) }
        return nil
    }

    static func int(_ value: Any) -> Int? {
        if let i = value as? Int { return i }
        if let d = value as? Double, d.isFinite { return Int(d) }
        return nil
    }

    static func sentimentMap<V>(_ map: [String: V]) -> [String: Double] {
        map.compactMapValues { double($0 as Any) }
    }
}
