import SwiftUI

/// Lays its children out left to right and wraps to a new line when a row is full.
struct FlowLayout: Layout {
    var spacing: CGFloat = 8
    var lineSpacing: CGFloat = 8

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        arrange(width: proposal.width ?? .infinity, subviews: subviews).size
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        let result = arrange(width: bounds.width, subviews: subviews)
        for (index, origin) in result.origins.enumerated() {
            subviews[index].place(at: CGPoint(x: bounds.minX + origin.x, y: bounds.minY + origin.y), proposal: .unspecified)
        }
    }

    private func arrange(width: CGFloat, subviews: Subviews) -> (size: CGSize, origins: [CGPoint]) {
        var origins: [CGPoint] = []
        var x: CGFloat = 0, y: CGFloat = 0, rowHeight: CGFloat = 0, maxWidth: CGFloat = 0
        for subview in subviews {
            let size = subview.sizeThatFits(.unspecified)
            if x > 0, x + size.width > width {
                x = 0
                y += rowHeight + lineSpacing
                rowHeight = 0
            }
            origins.append(CGPoint(x: x, y: y))
            x += size.width + spacing
            rowHeight = max(rowHeight, size.height)
            maxWidth = max(maxWidth, x - spacing)
        }
        return (CGSize(width: maxWidth, height: y + rowHeight), origins)
    }
}

/// A multi-line text area on the field surface: card fill, 1px strong hairline, 6px radius, with a placeholder.
struct PlainTextArea: View {
    @Binding var text: String
    let placeholder: String
    var minHeight: CGFloat = 96
    /// A text editor takes all the height it is offered; give it a ceiling where the card must keep its own size.
    var maxHeight: CGFloat?
    var fontSize: CGFloat = 14
    let label: String

    var body: some View {
        TextEditor(text: $text)
            .font(Theme.ui(fontSize))
            .foregroundStyle(Theme.text)
            .scrollContentBackground(.hidden)
            .padding(.horizontal, 6)
            .padding(.vertical, 6)
            .frame(minHeight: minHeight, maxHeight: maxHeight)
            .background(Theme.card, in: RoundedRectangle(cornerRadius: Theme.radius))
            .overlay(alignment: .topLeading) {
                if text.isEmpty {
                    Text(placeholder)
                        .font(Theme.ui(fontSize))
                        .foregroundStyle(Theme.faint)
                        .padding(.leading, 11)
                        .padding(.top, 6)
                        .allowsHitTesting(false)
                        .accessibilityHidden(true)
                }
            }
            .overlay(RoundedRectangle(cornerRadius: Theme.radius).strokeBorder(Theme.seamStrong, lineWidth: 1))
            .accessibilityLabel(label)
    }
}

/// The three example questions; choosing one fills the question field.
struct ExampleChips: View {
    let choose: (String) -> Void

    var body: some View {
        FlowLayout(spacing: Theme.space2, lineSpacing: 6) {
            ForEach(NewSimulationRules.exampleQuestions, id: \.self) { question in
                Button(question) { choose(question) }
                    .buttonStyle(ExampleChipStyle())
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Example questions")
    }
}

private struct ExampleChipStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        Chip(configuration: configuration)
    }

    private struct Chip: View {
        let configuration: Configuration
        @State private var hovering = false

        var body: some View {
            configuration.label
                .font(Theme.small)
                .foregroundStyle(Theme.secondary)
                .lineLimit(1)
                .padding(.horizontal, 10)
                .frame(height: 26)
                .background(configuration.isPressed ? Theme.seam : (hovering ? Theme.hover : Theme.card), in: Capsule())
                .overlay(Capsule().strokeBorder(Theme.seamStrong, lineWidth: 1))
                .contentShape(Capsule())
                .onHover { hovering = $0 }
        }
    }
}

/// A step of the form: a 13/600 title, an optional helper line, then its content.
struct FormStep<Content: View>: View {
    let title: String
    var helper: String?
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: Theme.space2) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(Theme.sectionTitle)
                    .foregroundStyle(Theme.text)
                    .accessibilityAddTraits(.isHeader)
                if let helper {
                    Text(helper).font(Theme.small).foregroundStyle(Theme.muted)
                }
            }
            content
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// Three size cards in a row, behaving as one radio group. Selected: 1.5px accent border on the accent wash.
struct SizePresetPicker: View {
    let selection: SizePreset?
    let choose: (SizePreset) -> Void
    /// Shown under the cards when the exact numbers match no preset.
    var customDetail: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: Theme.space3) {
                ForEach(SizePreset.allCases) { preset in
                    card(preset)
                }
            }
            .accessibilityElement(children: .contain)
            .accessibilityLabel("Size")
            if let customDetail {
                Text("Custom: \(customDetail)")
                    .font(Theme.small)
                    .foregroundStyle(Theme.muted)
            }
        }
    }

    private func card(_ preset: SizePreset) -> some View {
        let selected = selection == preset
        return Button { choose(preset) } label: {
            VStack(alignment: .leading, spacing: 3) {
                Text(preset.title).font(Theme.sectionTitle).foregroundStyle(Theme.text)
                Text(preset.detail).font(Theme.small).foregroundStyle(Theme.secondary)
                Text(preset.estimate).font(Theme.small).foregroundStyle(Theme.muted)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(Theme.space3)
            .background(selected ? Theme.accentSoft : Theme.card, in: RoundedRectangle(cornerRadius: Theme.cardRadius))
            .overlay {
                RoundedRectangle(cornerRadius: Theme.cardRadius)
                    .strokeBorder(selected ? Theme.accent : Theme.seam, lineWidth: selected ? 1.5 : 1)
            }
            .contentShape(RoundedRectangle(cornerRadius: Theme.cardRadius))
        }
        .buttonStyle(.plain)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(preset.title), \(preset.agents) people, \(preset.rounds) rounds, \(preset.estimate)")
        .accessibilityAddTraits(selected ? [.isButton, .isSelected] : .isButton)
    }
}

/// The empty state: a calm card that asks the first question and hands it to the New simulation sheet.
struct FirstQuestionCard: View {
    @Environment(AppModel.self) private var model
    @State private var question = ""

    var body: some View {
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: Theme.space4) {
                VStack(alignment: .leading, spacing: 4) {
                    Text("Ask your first question")
                        .font(Theme.title)
                        .foregroundStyle(Theme.text)
                        .accessibilityAddTraits(.isHeader)
                    Text("Shoal simulates a crowd reacting to your question and reads back what it found.")
                        .font(Theme.body)
                        .foregroundStyle(Theme.muted)
                }
                PlainTextArea(text: $question, placeholder: "How will people react to …?", minHeight: 84,
                              maxHeight: 84, label: "Your question")
                // Picking an example opens the sheet straight away with that question.
                ExampleChips { chosen in
                    question = chosen
                    open()
                }
                HStack {
                    Spacer()
                    Button("Continue") { open() }
                        .buttonStyle(.consolePrimary)
                        .disabled(!model.engine.state.isRunning)
                        .help(model.engine.state.isRunning ? "Continue with this question" : "Waiting for the engine to start")
                }
            }
            .padding(Theme.space5)
            .frame(maxWidth: 560)
            .card(padding: 0)
            .padding(Theme.space5)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func open() {
        model.newSimulation(prefill: NewSimulationPrefill(question: NewSimulationRules.trimmed(question)))
    }
}
