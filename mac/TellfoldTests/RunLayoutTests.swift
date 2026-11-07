import AppKit
import SwiftUI
import Testing
@testable import Tellfold

// RunFlowLayout (trait / voice chips) must stay inside the proposed width, wrapping long chips instead of letting
// them overflow the inspector column, and its size must be a pure function of the proposal.
@MainActor
@Suite("Run layout")
struct RunLayoutTests {
    private func chips() -> some View {
        RunFlowLayout {
            ForEach(0..<6, id: \.self) { i in
                Text(i == 2 ? "A very long trait chip that is wider than any narrow column could ever be" : "trait \(i)")
                    .padding(.horizontal, 8)
            }
        }
    }

    @Test(arguments: [40.0, 120.0, 280.0, 900.0])
    func flowLayoutNeverExceedsTheProposedWidth(width: Double) {
        let host = NSHostingController(rootView: chips())
        let size = host.sizeThatFits(in: CGSize(width: width, height: 10_000))
        #expect(size.width <= width + 0.5)
        #expect(size.height > 0)
    }

    @Test func flowLayoutIsStableForTheSameProposal() {
        let host = NSHostingController(rootView: chips())
        let a = host.sizeThatFits(in: CGSize(width: 200, height: 10_000))
        let b = host.sizeThatFits(in: CGSize(width: 200, height: 10_000))
        #expect(a == b)
    }

    @Test func narrowerProposalsWrapIntoMoreRows() {
        let host = NSHostingController(rootView: chips())
        let wide = host.sizeThatFits(in: CGSize(width: 2_000, height: 10_000))
        let narrow = host.sizeThatFits(in: CGSize(width: 150, height: 10_000))
        #expect(narrow.height > wide.height)
    }
}
