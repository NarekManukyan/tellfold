import AppKit
import SwiftUI
import UniformTypeIdentifiers

/// "Share card…": a preview of the run's share card in either size and appearance, with Copy Image and Save PNG….
struct ShareCardSheet: View {
    let store: RunStore
    @Environment(\.dismiss) private var dismiss
    @Environment(\.colorScheme) private var systemScheme

    @State private var format: ShareCardFormat = .landscape
    @State private var dark = false
    @State private var png: Data?
    @State private var errorMessage: String?
    @State private var copied = false

    private var scheme: ColorScheme { dark ? .dark : .light }

    private var content: ShareCardContent? {
        guard let report = store.report else { return nil }
        return ShareCardContent(report: report, stats: store.stats, posts: store.posts, personas: store.personas,
                                quoteLength: format.quoteLength)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: Theme.space4) {
            VStack(alignment: .leading, spacing: 4) {
                Text("Share card")
                    .font(Theme.title)
                    .foregroundStyle(Theme.text)
                    .accessibilityAddTraits(.isHeader)
                Text("The headline, confidence and sentiment of this run, with two posts the report cites.")
                    .font(Theme.small)
                    .foregroundStyle(Theme.muted)
            }
            HStack(spacing: Theme.space4) {
                Picker("Size", selection: $format) {
                    ForEach(ShareCardFormat.allCases) { Text($0.title).tag($0) }
                }
                .fixedSize()
                Picker("Appearance", selection: $dark) {
                    Text("Light").tag(false)
                    Text("Dark").tag(true)
                }
                .pickerStyle(.segmented)
                .fixedSize()
                Spacer()
            }
            preview
                .frame(maxWidth: .infinity, minHeight: 380, maxHeight: 520)
            if let errorMessage {
                Text(errorMessage).font(Theme.small).foregroundStyle(Theme.negative)
            }
            if let content, content.quotes.count < 2 {
                Text("The report cites fewer than two posts that can be quoted, so the card has \(content.quotes.isEmpty ? "no quotes" : "one quote").")
                    .font(Theme.caption)
                    .foregroundStyle(Theme.muted)
            }
            HStack {
                if copied {
                    Label("Copied", systemImage: "checkmark")
                        .font(Theme.small)
                        .foregroundStyle(Theme.positive)
                }
                Spacer()
                Button("Close", role: .cancel) { dismiss() }
                    .buttonStyle(.console)
                    .keyboardShortcut(.cancelAction)
                Button("Copy Image") { copy() }
                    .buttonStyle(.console)
                    .disabled(png == nil)
                    .keyboardShortcut("c", modifiers: .command)
                Button("Save PNG…") { Task { await save() } }
                    .buttonStyle(.consolePrimary)
                    .disabled(png == nil)
                    .keyboardShortcut(.defaultAction)
            }
        }
        .padding(Theme.space5)
        .frame(width: 760)
        .background(Theme.ground)
        .onAppear {
            dark = systemScheme == .dark
            render()
        }
        .onChange(of: format) { render() }
        .onChange(of: dark) { render() }
    }

    @ViewBuilder private var preview: some View {
        if let png, let image = NSImage(data: png) {
            Image(nsImage: image)
                .resizable()
                .interpolation(.high)
                .aspectRatio(contentMode: .fit)
                .clipShape(RoundedRectangle(cornerRadius: Theme.radius))
                .overlay(RoundedRectangle(cornerRadius: Theme.radius).strokeBorder(Theme.seamStrong, lineWidth: 1))
                .accessibilityLabel("Share card preview: \(content?.headline ?? "")")
        } else {
            ProgressView()
        }
    }

    private func render() {
        copied = false
        guard let content else {
            errorMessage = "This run has no report yet."
            png = nil
            return
        }
        do {
            png = try ShareCardRenderer.png(content, format: format, scheme: scheme)
            errorMessage = nil
        } catch {
            png = nil
            errorMessage = error.localizedDescription
        }
    }

    private func copy() {
        guard let png else { return }
        ShareCardRenderer.copy(png)
        copied = true
    }

    private func save() async {
        guard let png else { return }
        let panel = NSSavePanel()
        panel.allowedContentTypes = [.png]
        panel.canCreateDirectories = true
        panel.isExtensionHidden = false
        panel.nameFieldStringValue = ShareCardRenderer.suggestedFileName(
            title: store.report?.title ?? store.runID, format: format, scheme: scheme)
        let response = if let window = NSApp.keyWindow { await panel.beginSheetModal(for: window) } else { panel.runModal() }
        guard response == .OK, let url = panel.url else { return }
        do {
            try png.write(to: url, options: .atomic)
            dismiss()
        } catch {
            errorMessage = "Could not save the image: \(error.localizedDescription)"
        }
    }
}
