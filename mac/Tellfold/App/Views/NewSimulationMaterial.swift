import SwiftUI
import UniformTypeIdentifiers

/// "Who is the audience?": the user's own reviews, survey answers, support emails, social comments or meeting notes
/// to build the crowd from. The engine reads and redacts them on this Mac; the preview shows exactly what is sent.
struct MaterialStep: View {
    @Binding var inputs: [MaterialInput]
    @Binding var hideText: String
    let preview: MaterialPreview?
    let isReading: Bool
    let error: String?
    /// Files that were skipped; says so without blocking anything.
    var notice: String?
    /// Why material does not apply (a saved audience is chosen); nil when it does.
    let unavailableNote: String?
    /// The planner tool the cleaned text goes to, by name.
    let plannerName: String
    /// The crowd size, for the note when the material has fewer items than people.
    let agents: Int
    /// A note above the drop zone ("Run again…" of a grounded run).
    var note: String?
    let add: ([URL]) -> Void
    let showPreview: () -> Void

    @State private var showsImporter = false
    @State private var isDropTargeted = false

    static let contentTypes: [UTType] = {
        var types: [UTType] = [.plainText, .commaSeparatedText, .tabSeparatedText, .emailMessage]
        for ext in ["md", "markdown"] {
            if let type = UTType(filenameExtension: ext) { types.append(type) }
        }
        return types
    }()

    var body: some View {
        FormStep(title: "Who is the audience?",
                 helper: "Optional. Add what real people said, and the crowd is built from it. On this Mac first, "
                     + "\(MaterialText.redactedCategories) are removed; only the cleaned text goes to \(plannerName). "
                     + MaterialText.namesMayRemain) {
            if let unavailableNote {
                Text(unavailableNote).font(Theme.small).foregroundStyle(Theme.muted)
                    .fixedSize(horizontal: false, vertical: true)
            } else {
                if let note {
                    Text(note).font(Theme.small).foregroundStyle(Theme.muted)
                        .fixedSize(horizontal: false, vertical: true)
                }
                zone
                if let notice {
                    Text(notice).font(Theme.small).foregroundStyle(Theme.muted)
                        .fixedSize(horizontal: false, vertical: true)
                }
                if !inputs.isEmpty {
                    TextField("Other names to hide, separated by commas", text: $hideText)
                        .textFieldStyle(.roundedBorder)
                        .font(Theme.small)
                        .accessibilityLabel("Other names to hide")
                        .help("Staff, pupils or customers the automatic rules might miss. Each is hidden wherever it appears.")
                    status
                }
            }
        }
        .fileImporter(isPresented: $showsImporter, allowedContentTypes: Self.contentTypes,
                      allowsMultipleSelection: true) { result in
            if case .success(let urls) = result { add(urls) }
        }
    }

    private var zone: some View {
        VStack(alignment: .leading, spacing: Theme.space2) {
            if inputs.isEmpty {
                VStack(spacing: 6) {
                    Image(systemName: "person.3.sequence")
                        .font(.system(size: 20))
                        .foregroundStyle(Theme.muted)
                    Text("Drop reviews, survey answers, emails, comments or notes here")
                        .font(Theme.body).foregroundStyle(Theme.text)
                    Text("\(MaterialKind.extensionHint), up to 10 MB each. Choose a spreadsheet's column after adding it.")
                        .font(Theme.small).foregroundStyle(Theme.muted)
                }
                .frame(maxWidth: .infinity, minHeight: 72)
            } else {
                // The engine renames files, so a report belongs to a row by position, and only while the preview is current.
                ForEach($inputs) { $input in
                    let index = inputs.firstIndex { $0.id == input.id }
                    MaterialRow(input: $input, report: index.flatMap { preview?.report(at: $0, of: inputs) },
                                columns: index.flatMap { preview?.columnsReport(at: $0, of: inputs) }?.columns ?? []) {
                        inputs.removeAll { $0.id == input.id }
                    }
                }
            }
            Button(inputs.isEmpty ? "Choose material…" : "Add more…") { showsImporter = true }
                .buttonStyle(.console(.secondary, compact: true))
                .disabled(inputs.count >= MaterialLimits.maxFiles)
                .frame(maxWidth: .infinity)
        }
        .padding(Theme.space3)
        .frame(maxWidth: .infinity)
        .background(isDropTargeted ? Theme.accentSoft : Theme.card, in: RoundedRectangle(cornerRadius: Theme.radius))
        .overlay {
            RoundedRectangle(cornerRadius: Theme.radius)
                .strokeBorder(isDropTargeted ? Theme.accent : Theme.seamStrong,
                              style: StrokeStyle(lineWidth: 1, dash: inputs.isEmpty ? [5, 4] : []))
        }
        .dropDestination(for: URL.self) { urls, _ in
            add(urls)
            return true
        } isTargeted: { isDropTargeted = $0 }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Audience material drop zone")
    }

    @ViewBuilder private var status: some View {
        if let error {
            Text(error).font(Theme.small).foregroundStyle(Theme.negative)
                .fixedSize(horizontal: false, vertical: true)
        } else if isReading && preview == nil {
            HStack(spacing: 6) {
                ProgressView().controlSize(.small)
                Text("Reading and cleaning the material…").font(Theme.small).foregroundStyle(Theme.muted)
            }
        } else if let preview {
            VStack(alignment: .leading, spacing: 4) {
                HStack(alignment: .firstTextBaseline, spacing: Theme.space2) {
                    Text(MaterialStep.summary(preview))
                        .font(Theme.small)
                        .foregroundStyle(Theme.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                    Spacer(minLength: Theme.space2)
                    if isReading { ProgressView().controlSize(.mini) }
                    Button("Preview what is sent…", action: showPreview)
                        .buttonStyle(.console(.ghost, compact: true))
                }
                ForEach(notes(preview), id: \.self) { note in
                    Label(note, systemImage: "info.circle")
                        .font(Theme.small)
                        .foregroundStyle(Theme.muted)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
    }

    /// The engine's warnings, then the note that some people share quotes when the items are fewer than the people.
    private func notes(_ preview: MaterialPreview) -> [String] {
        preview.warnings + [MaterialText.fewerItemsNote(items: preview.summary.items, agents: agents, thin: preview.thin)]
            .compactMap { $0 }
    }

    /// "17 items: 14 reviews and 3 survey answers. Hidden: 4 names, 2 email addresses."
    static func summary(_ preview: MaterialPreview) -> String {
        let n = preview.summary.items
        var text = "\(n) \(n == 1 ? "item" : "items"): \(MaterialText.counts(preview.summary.counts))."
        if let hidden = MaterialText.hidden(preview.summary.redacted) { text += " Hidden: \(hidden)." }
        return text
    }
}

/// One file of material: its name, what it holds, and a spreadsheet's column.
private struct MaterialRow: View {
    @Binding var input: MaterialInput
    /// What the engine found in this file; nil until the preview matches the files as they are now.
    let report: MaterialPreview.File?
    /// A spreadsheet's headers, kept while the preview is read again after a change of kind or column.
    let columns: [String]
    let remove: () -> Void

    var body: some View {
        HStack(spacing: Theme.space2) {
            Image(systemName: icon)
                .foregroundStyle(Theme.muted)
                .accessibilityHidden(true)
            Text(input.url.lastPathComponent)
                .font(Theme.small)
                .foregroundStyle(Theme.text)
                .lineLimit(1)
                .truncationMode(.middle)
            Spacer(minLength: Theme.space2)
            Picker("Holds", selection: $input.kind) {
                ForEach(kinds) { Text($0.title).tag($0) }
            }
            .labelsHidden()
            .fixedSize()
            .accessibilityLabel("What \(input.url.lastPathComponent) holds")
            if MaterialKind.isSpreadsheet(input.url), !columns.isEmpty {
                Picker("Column", selection: columnBinding) {
                    ForEach(columns, id: \.self) { Text($0).tag(Optional($0)) }
                }
                .labelsHidden()
                .frame(maxWidth: 180)
                .accessibilityLabel("Column of \(input.url.lastPathComponent) to read")
                .help("The column with the answers. Other columns, such as names or emails, are never read.")
            }
            if let report {
                Text("\(report.items)").font(Theme.mono(11)).foregroundStyle(Theme.muted)
                    .help("\(report.items) items found")
            }
            Button(action: remove) {
                Image(systemName: "xmark")
                    .font(.system(size: 10, weight: .bold))
                    .foregroundStyle(Theme.muted)
                    .frame(width: 22, height: 22)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Remove \(input.url.lastPathComponent)")
        }
        .frame(minHeight: 26)
    }

    private var icon: String {
        if input.kind == .emails { return "envelope" }
        return MaterialKind.isSpreadsheet(input.url) ? "tablecells" : "doc.text"
    }

    /// Survey answers need a spreadsheet; other kinds read text or a spreadsheet's column.
    private var kinds: [MaterialKind] {
        MaterialKind.isSpreadsheet(input.url) ? MaterialKind.allCases : MaterialKind.allCases.filter { $0 != .survey }
    }

    /// The chosen column, or the one the engine picked when none was chosen yet.
    private var columnBinding: Binding<String?> {
        Binding(get: { input.column ?? report?.column }, set: { input.column = $0 })
    }
}

/// Every item of the material exactly as it will be sent, with the hidden details marked.
struct MaterialPreviewSheet: View {
    let preview: MaterialPreview
    let plannerName: String
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: Theme.space2) {
                Text("What is sent").font(Theme.title).foregroundStyle(Theme.text)
                    .accessibilityAddTraits(.isHeader)
                Text(MaterialStep.summary(preview)).font(Theme.body).foregroundStyle(Theme.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                Text("Only the text below goes to \(plannerName), to sort it into camps and write people from it "
                     + "(\(preview.estimatedCalls) extra \(preview.estimatedCalls == 1 ? "call" : "calls")). "
                     + "Your files stay on this Mac, and the run keeps only this cleaned text. "
                     + "Removed: \(MaterialText.redactedCategories). A name the rules don't know can remain: "
                     + "add it under Other names to hide.")
                    .font(Theme.small).foregroundStyle(Theme.muted)
                    .fixedSize(horizontal: false, vertical: true)
                ForEach(preview.warnings, id: \.self) { warning in
                    Label(warning, systemImage: "info.circle").font(Theme.small).foregroundStyle(Theme.muted)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            .padding(Theme.space5)
            Seam()
            List(preview.items) { item in
                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 6) {
                        Text(item.id).font(Theme.mono(11)).foregroundStyle(Theme.muted)
                        Text(MaterialKind(rawValue: item.kind)?.title ?? item.kind).font(Theme.caption)
                            .foregroundStyle(Theme.muted)
                        Text(preview.fileName(forLabel: item.file)).font(Theme.caption).foregroundStyle(Theme.faint).lineLimit(1)
                    }
                    Self.marked(item.text).font(Theme.body).foregroundStyle(Theme.text)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .padding(.vertical, 2)
                .accessibilityElement(children: .combine)
            }
            .listStyle(.inset)
            Seam()
            HStack {
                Spacer()
                Button("Done") { dismiss() }
                    .buttonStyle(.consolePrimary)
                    .keyboardShortcut(.defaultAction)
            }
            .padding(Theme.space4)
        }
        .background(Theme.ground)
        .frame(minWidth: 560, idealWidth: 640, minHeight: 480, idealHeight: 620)
    }

    /// The item with each placeholder ("[name]") in mono and a stronger colour, so what was hidden stands out.
    static func marked(_ text: String) -> Text {
        RedactedText.pieces(text).reduce(Text("")) { result, piece in
            piece.1
                ? result + Text(piece.0).font(Theme.mono(11)).foregroundStyle(Theme.secondary)
                : result + Text(piece.0)
        }
    }
}
