import Foundation

// The only file in Canvas/ that reads RunStore and the Codable API models (MAC_CONTRACT.md names).
// Everything else works on the Canvas value types in CanvasData.swift.

extension KnowledgeGraphInput {
    @MainActor
    init(store: RunStore) {
        self.init()
        guard let detail = store.detail else { return }
        isLoaded = true
        let graph = detail.graph
        entities = (graph?.entities ?? []).map {
            CanvasEntity(id: $0.id, name: $0.name, kind: $0.kind, summary: $0.summary)
        }
        relations = (graph?.relations ?? []).map {
            CanvasRelation(source: $0.source, target: $0.target, label: $0.label)
        }
        for p in detail.personas {
            agentNames[p.id] = p.name
            if let e = p.entityID { embodiedBy[e, default: []].append(p.id) }
        }
        selectedAgentID = store.selectedAgentID
    }
}

@MainActor
func canvasSelectAgent(_ store: RunStore, _ id: String?) {
    if store.selectedAgentID != id { store.selectedAgentID = id }
}
