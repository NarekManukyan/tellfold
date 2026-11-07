import AppKit
import Foundation

/// A camp region's label: the name, the head count and the mean sentiment. The name is never cut and the number is
/// never dropped: a long label puts the figures on a second line, and in the compact card the name wraps too.
nonisolated struct CrowdLabel: Sendable, Equatable {
    var name: String
    var count: Int
    var sentiment: Double?
    /// The compact Overview card wraps names to keep regions small.
    var compact = false

    /// One printed line: the name (semibold) and what follows it in mono.
    struct Row: Sendable, Equatable {
        var name: String
        var tail: String
    }

    private static var nameFont: NSFont { NSFont.systemFont(ofSize: 11.5, weight: .semibold) }
    private static var tailFont: NSFont { NSFont.monospacedSystemFont(ofSize: 11, weight: .regular) }
    static let sidePadding = 20.0
    /// A one-line label wider than this wraps its figures onto a second line.
    static let singleLineLimit = 230.0
    /// Compact card: the widest a printed line of the name may be before it wraps.
    static let compactNameLimit = 120.0

    var figures: String {
        var t = "\(count)"
        t += " · " + (sentiment.map { RunUI.sentimentText($0) } ?? CampSentimentValue.noPostsText)
        return t
    }

    private static func width(_ text: String, _ font: NSFont) -> Double {
        Double((text as NSString).size(withAttributes: [.font: font]).width.rounded(.up))
    }

    private static func nameWidth(_ text: String) -> Double { width(text, nameFont) }

    var nameWidth: Double { Self.nameWidth(name) }
    var oneLineWidth: Double { nameWidth + Self.width(" · " + figures, Self.tailFont) + Self.sidePadding }

    /// The printed lines.
    var rows: [Row] {
        if compact {
            var lines: [String] = []
            var current = ""
            for word in name.split(separator: " ").map(String.init) {
                let trial = current.isEmpty ? word : current + " " + word
                if !current.isEmpty, Self.nameWidth(trial) > Self.compactNameLimit {
                    lines.append(current); current = word
                } else {
                    current = trial
                }
            }
            if !current.isEmpty { lines.append(current) }
            if lines.isEmpty { lines = [name] }
            let tail = " · " + figures
            let last = lines[lines.count - 1]
            if Self.nameWidth(last) + Self.width(tail, Self.tailFont) <= Self.compactNameLimit + 40 {
                return lines.dropLast().map { Row(name: $0, tail: "") } + [Row(name: last, tail: tail)]
            }
            return lines.map { Row(name: $0, tail: "") } + [Row(name: "", tail: figures)]
        }
        if oneLineWidth <= Self.singleLineLimit { return [Row(name: name, tail: " · " + figures)] }
        return [Row(name: name, tail: ""), Row(name: "", tail: figures)]
    }

    var twoLines: Bool { rows.count > 1 }
    var lines: Int { rows.count }
    /// Points of screen the label needs.
    var width: Double {
        (rows.map { Self.nameWidth($0.name) + Self.width($0.tail, Self.tailFont) }.max() ?? 0) + Self.sidePadding
    }
}

/// Who acted on whom, aggregated over the rounds up to the playhead (replies and reposts).
nonisolated struct CrowdEdge: Sendable, Hashable {
    var from: String
    var to: String
    var replies: Int
    var reposts: Int

    var count: Int { replies + reposts }
}

/// One interaction of the playhead round, drawn as a faint curve in the Camps view.
nonisolated struct CrowdArc: Sendable, Hashable {
    var from: String
    var to: String
    var kind: ArcKind
}

/// Everything the crowd graph needs for one playhead position. Built from the store on the main actor; a pure function
/// of the store's data, so scrubbing always gives the same picture.
nonisolated struct CrowdScene: Sendable {
    var nodes: [CrowdNode] = []
    var index: [String: Int] = [:]
    var groups: [CampGroup] = []
    var isFallbackGrouping = false
    /// Mean sentiment per agent up to the playhead; agents without a scored post are absent.
    var sentiment: [String: Double] = [:]
    /// Whether any post of the run carries a sentiment score.
    var runHasSentiment = false
    /// Camp mean sentiment at the playhead by group id (see `RunStore.campSentiment`).
    var groupSentiments: [String: Double] = [:]
    var follows: [CanvasFollow] = []
    var arcs: [CrowdArc] = []
    var edges: [CrowdEdge] = []
    var round = 0
    var selectedAgentID: String?
    var selectedGroupID: String?
    var isLoaded = false

    /// What a camp region's label says and how much room it needs.
    func label(_ group: CampGroup, compact: Bool = false) -> CrowdLabel {
        CrowdLabel(name: group.name, count: group.members.count, sentiment: groupSentiment(group), compact: compact)
    }

    func node(_ id: String) -> CrowdNode? { index[id].map { nodes[$0] } }

    func group(of node: CrowdNode) -> CampGroup? { groups.indices.contains(node.groupIndex) ? groups[node.groupIndex] : nil }

    /// What changes the positions in `mode`: the people and their camps, and for Spectrum their scores.
    func layoutSignature(mode: CrowdMode) -> Int {
        var h = Hasher()
        h.combine(mode)
        h.combine(groups.count)
        if mode == .camps { for g in groups { h.combine(Int(label(g).width)); h.combine(label(g).lines) } }
        for n in nodes {
            h.combine(n.id)
            h.combine(n.influence)
            h.combine(n.groupIndex)
            if mode == .spectrum { h.combine(Int(((sentiment[n.id] ?? 9) * 1000).rounded())) }
        }
        return h.finalize()
    }

    /// The group's mean sentiment as of the playhead, else the engine's final figure.
    func groupSentiment(_ group: CampGroup) -> Double? { groupSentiments[group.id] }
}
