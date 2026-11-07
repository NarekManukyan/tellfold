import Foundation
import Testing
@testable import Tellfold

@Suite("Camp sentiment wording and the Overview crowd size")
struct CrowdCardTests {
    @Test func aCampWithoutPostsSaysNoPostsInsteadOfNA() {
        let silent = CrowdLabel(name: "Quiet supportive lurkers", count: 3, sentiment: nil)
        #expect(silent.figures == "3 · no posts")
        #expect(silent.rows.map(\.tail).joined().contains("no posts"))
        #expect(!silent.rows.map(\.tail).joined().contains("n/a"))
        let loud = CrowdLabel(name: "Advocates", count: 4, sentiment: 0.67)
        #expect(loud.figures == "4 · +0.67")
        #expect(CampSentimentValue.noPostsText == "no posts")
        #expect(CampSentimentValue.noPostsHelp == "None of these people posted, so there is no sentiment to average.")
    }

    private static let names = ["Reliability-proof advocates", "Converted skeptics now recommending", "Persistent pricing critics",
                                "Indifferent or unresolved", "Quiet supportive lurkers"]

    @Test(arguments: [400.0, 443, 500, 539, 540, 600, 639, 640, 700, 779, 780, 900, 1000])
    func theOverviewCardKeepsEvenTheSmallestAvatarsAboutTwentyPointsWithoutOverlap(width: Double) {
        // 20 people with the smallest avatars (influence under 0.2: 22 pt at scale 1), five camps with real-length names.
        let nodes = (0..<20).map { i in
            CrowdNode(id: "a\(i)", handle: "h\(i)", name: "Agent \(i)", influence: 0.1, isOrganisation: i % 7 == 0, groupIndex: i % 5)
        }
        let labels = Self.names.enumerated().map { i, name in
            CrowdLabel(name: name, count: 4, sentiment: i == 4 ? nil : 0.2, compact: true)
        }
        let height = OverviewView.crowdHeight(forWidth: width)
        let layout = CrowdLayout.compute(nodes: nodes, groupCount: 5, sentiment: [:], size: CGSize(width: width, height: height),
                                         mode: .camps, labels: labels)
        let diameter = 22 * CrowdModel.nodeScale(layout.fitScale)
        #expect(diameter >= 20, "width \(width): avatars are \(diameter) pt")
        for (i, a) in nodes.enumerated() {
            for b in nodes[(i + 1)...] {
                let p = layout.positions[a.id]!, q = layout.positions[b.id]!
                #expect(hypot(p.x - q.x, p.y - q.y) >= a.radius + b.radius - 1)
            }
        }
    }
}
